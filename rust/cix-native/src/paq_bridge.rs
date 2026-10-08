//! Worker-only PAQ library bridge contract.
//!
//! PAQ v215 retains process-lifetime static model state. This module therefore
//! makes its isolation requirement explicit: the full engine may invoke a
//! packaged PAQ bridge only from a fresh, same-CIX worker process; SDK callers
//! receive an error instead of an implicit process-dependent codec call.

use std::ffi::{CString, OsStr, OsString};
use std::fmt;
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};

const ERROR_CAPACITY: usize = 512;

/// PAQ source variant selected by a full-engine worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaqVariant {
    V215,
    V216,
    JointDiscount,
    StoreState,
}

/// Calling context required by a codec with process-lifetime upstream state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaqExecutionScope {
    IsolatedWorker,
    NativeSdk,
}

/// Environment to install in the fresh same-CIX worker before loading PAQ.
/// The bridge does not mutate the caller environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaqWorkerEnvironment {
    pub entries: Vec<(OsString, OsString)>,
}

/// Complete filesystem-facing request passed from the full-engine worker to a
/// PAQ bridge. `argv` begins with a display program name and is otherwise the
/// unchanged upstream command-line syntax.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaqInvocation {
    pub scope: PaqExecutionScope,
    pub variant: PaqVariant,
    pub argv: Vec<OsString>,
    pub input: PathBuf,
    pub output: PathBuf,
    pub worker_environment: PaqWorkerEnvironment,
    pub memory_limit_bytes: u64,
}

/// Build and loader identity for one unchanged PAQ source variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaqVariantDescriptor {
    pub variant: PaqVariant,
    pub source_subdirectory: &'static str,
    pub library_basename: &'static str,
    pub process_symbol: &'static str,
    pub worker_only: bool,
}

pub const PAQ_VARIANTS: [PaqVariantDescriptor; 4] = [
    PaqVariantDescriptor {
        variant: PaqVariant::V215,
        source_subdirectory: "vendor/paq/v215",
        library_basename: "cix_paq_v215_bridge",
        process_symbol: "cix_paq_v215_process",
        worker_only: true,
    },
    PaqVariantDescriptor {
        variant: PaqVariant::V216,
        source_subdirectory: "vendor/paq/v216",
        library_basename: "cix_paq_v216_bridge",
        process_symbol: "cix_paq_v216_process",
        worker_only: true,
    },
    PaqVariantDescriptor {
        variant: PaqVariant::JointDiscount,
        source_subdirectory: "vendor/paq/joint-discount",
        library_basename: "cix_paq_joint_discount_bridge",
        process_symbol: "cix_paq_joint_discount_process",
        worker_only: true,
    },
    PaqVariantDescriptor {
        variant: PaqVariant::StoreState,
        source_subdirectory: "vendor/paq/store-state",
        library_basename: "cix_paq_store_state_bridge",
        process_symbol: "cix_paq_store_state_process",
        worker_only: true,
    },
];

impl PaqVariant {
    pub fn descriptor(self) -> Option<&'static PaqVariantDescriptor> {
        PAQ_VARIANTS
            .iter()
            .find(|descriptor| descriptor.variant == self)
    }
}

/// The C bridge's stable status values. Values are part of the C ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum PaqBridgeStatus {
    Ok = 0,
    InvalidArgument = 1,
    AlreadyUsed = 2,
    UpstreamFailure = 3,
    Exception = 4,
}

impl PaqBridgeStatus {
    fn from_raw(value: c_int) -> Option<Self> {
        match value {
            0 => Some(Self::Ok),
            1 => Some(Self::InvalidArgument),
            2 => Some(Self::AlreadyUsed),
            3 => Some(Self::UpstreamFailure),
            4 => Some(Self::Exception),
            _ => None,
        }
    }
}

/// A typed failure returned by the bridge boundary. The CIX wrapper itself
/// does not terminate its host or change signal/resource state; upstream PAQ
/// LSTM paths can call `exit(1)`, which is why invocation is worker-only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaqBridgeError {
    NotSdkSafe,
    UnsupportedVariant(PaqVariant),
    VariantMismatch {
        loaded: PaqVariant,
        requested: PaqVariant,
    },
    MissingInput(PathBuf),
    ExistingOutput(PathBuf),
    InvalidArgument(String),
    Status {
        status: Option<PaqBridgeStatus>,
        message: String,
    },
}

impl fmt::Display for PaqBridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSdkSafe => {
                write!(formatter, "PAQ bridge requires an isolated same-CIX worker")
            }
            Self::UnsupportedVariant(variant) => {
                write!(formatter, "PAQ bridge has no packaged variant: {variant:?}")
            }
            Self::VariantMismatch { loaded, requested } => write!(
                formatter,
                "PAQ bridge loaded {loaded:?}, requested {requested:?}"
            ),
            Self::MissingInput(path) => write!(
                formatter,
                "PAQ input is not a regular file: {}",
                path.display()
            ),
            Self::ExistingOutput(path) => {
                write!(formatter, "PAQ output already exists: {}", path.display())
            }
            Self::InvalidArgument(message) => {
                write!(formatter, "invalid PAQ bridge request: {message}")
            }
            Self::Status { status, message } => {
                write!(formatter, "PAQ bridge {status:?}: {message}")
            }
        }
    }
}

impl std::error::Error for PaqBridgeError {}

pub type PaqProcess =
    unsafe extern "C" fn(c_int, *const *const c_char, *const c_char, *mut c_char, usize) -> c_int;

/// One packaged PAQ bridge supplied by the full engine's platform loader.
/// Keeping the loader outside this type avoids a hidden `dlopen`, child
/// process, or library-path policy in the SDK-facing API.
#[derive(Clone, Copy)]
pub struct PaqBridge {
    variant: PaqVariant,
    process: PaqProcess,
}

impl PaqBridge {
    /// # Safety
    ///
    /// `process` must resolve to the descriptor's `process_symbol` from the
    /// matching CIX-built bridge DSO.
    pub const unsafe fn from_symbol(variant: PaqVariant, process: PaqProcess) -> Self {
        Self { variant, process }
    }

    /// Validate the worker-owned filesystem/environment plan, then issue one
    /// call into the loaded bridge. The caller installs `worker_environment`
    /// and rlimits before this call; neither is silently inherited or changed
    /// by this API.
    pub fn invoke(&self, request: &PaqInvocation) -> Result<(), PaqBridgeError> {
        validate(request)?;
        if request.variant != self.variant {
            return Err(PaqBridgeError::VariantMismatch {
                loaded: self.variant,
                requested: request.variant,
            });
        }
        let argv = c_strings(&request.argv)?;
        let pointers: Vec<*const c_char> = argv.iter().map(|value| value.as_ptr()).collect();
        let output = c_path(&request.output)?;
        let mut error = [0 as c_char; ERROR_CAPACITY];
        let raw = unsafe {
            (self.process)(
                pointers.len() as c_int,
                pointers.as_ptr(),
                output.as_ptr(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        // Bound the read even if a failed provider fills the entire buffer
        // without a NUL terminator.
        let message = error_message(&error);
        match PaqBridgeStatus::from_raw(raw) {
            Some(PaqBridgeStatus::Ok) => Ok(()),
            status => Err(PaqBridgeError::Status { status, message }),
        }
    }
}

/// Reject requests that would accidentally make the native SDK depend on PAQ
/// process state. The worker launcher owns environment application, rlimits,
/// cancellation and dynamic library loading.
pub fn validate(request: &PaqInvocation) -> Result<(), PaqBridgeError> {
    if request.scope != PaqExecutionScope::IsolatedWorker {
        return Err(PaqBridgeError::NotSdkSafe);
    }
    if request.variant.descriptor().is_none() {
        return Err(PaqBridgeError::UnsupportedVariant(request.variant));
    }
    if request.argv.len() < 2 || request.argv.len() > c_int::MAX as usize {
        return Err(PaqBridgeError::InvalidArgument(
            "PAQ argv requires a representable program and command list".into(),
        ));
    }
    if request.memory_limit_bytes == 0 {
        return Err(PaqBridgeError::InvalidArgument(
            "worker memory limit must be non-zero".into(),
        ));
    }
    if !request.input.is_file() {
        return Err(PaqBridgeError::MissingInput(request.input.clone()));
    }
    if request.output.exists() {
        return Err(PaqBridgeError::ExistingOutput(request.output.clone()));
    }
    for value in request
        .worker_environment
        .entries
        .iter()
        .flat_map(|pair| [&pair.0, &pair.1])
    {
        if value.as_encoded_bytes().contains(&0) {
            return Err(PaqBridgeError::InvalidArgument(
                "environment contains NUL".into(),
            ));
        }
    }
    Ok(())
}

fn error_message(error: &[c_char]) -> String {
    let bytes: Vec<u8> = error
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn native_c_string(value: &OsStr) -> Result<CString, PaqBridgeError> {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes()
    };
    // The unchanged upstream narrow-char Windows API has no qualified UTF-8
    // contract. The worker must use representable staging paths until its
    // platform adapter is qualified, never silently replace path characters.
    #[cfg(not(unix))]
    let bytes = value
        .to_str()
        .filter(|text| text.is_ascii())
        .ok_or_else(|| {
            PaqBridgeError::InvalidArgument(
                "non-ASCII PAQ arguments require a qualified platform adapter".into(),
            )
        })?
        .as_bytes();
    CString::new(bytes)
        .map_err(|_| PaqBridgeError::InvalidArgument("PAQ argument contains NUL".into()))
}

fn c_strings(values: &[OsString]) -> Result<Vec<CString>, PaqBridgeError> {
    values.iter().map(|value| native_c_string(value)).collect()
}

fn c_path(path: &Path) -> Result<CString, PaqBridgeError> {
    native_c_string(path.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_buffer_read_is_bounded_without_terminator() {
        assert_eq!(
            error_message(&[b'x' as c_char; ERROR_CAPACITY]),
            "x".repeat(ERROR_CAPACITY)
        );
        assert_eq!(error_message(&[b'x' as c_char, 0, b'y' as c_char]), "x");
        assert!(native_c_string(OsStr::new("embedded\0nul")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unix_arguments_and_paths_preserve_arbitrary_filename_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let raw = b"fixture-\xff";
        let value = OsStr::from_bytes(raw);
        assert_eq!(native_c_string(value).unwrap().as_bytes(), raw);
        assert_eq!(c_path(Path::new(value)).unwrap().as_bytes(), raw);
        assert_eq!(
            c_strings(&[value.to_os_string()]).unwrap()[0].as_bytes(),
            raw
        );
    }
}
