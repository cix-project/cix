//! Internal same-CIX PAQ worker entry. Never called by the process-free SDK.
//!
//! Only `main_entry` dispatches this module, before CLI signal handlers or
//! threads exist. The full-engine supervisor and public job protocol are separate.
#[cfg(target_os = "linux")]
use crate::full_engine::dynamic_library::{DynamicLibrary, DynamicLibraryError};
use crate::full_engine::installed::shared_library_filename;
use crate::paq_bridge::{
    PaqBridge, PaqExecutionScope, PaqInvocation, PaqVariant, PaqWorkerEnvironment,
};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const SWITCH: &str = "--internal-paq-worker-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Encode,
    Decode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Level {
    number: u8,
    lstm: bool,
}
impl Level {
    fn parse(text: &str) -> Result<Self, String> {
        let value = text
            .strip_prefix('-')
            .ok_or("PAQ level must begin with '-' ")?;
        let (number, lstm) = value
            .strip_suffix('L')
            .map_or((value, false), |v| (v, true));
        let number: u8 = number.parse().map_err(|_| "invalid PAQ level")?;
        let result = Self { number, lstm };
        if number > 12 || result.argument() != text {
            return Err("PAQ level must be -0 through -12, optionally followed by L".into());
        }
        Ok(result)
    }
    fn argument(self) -> String {
        format!("-{}{}", self.number, if self.lstm { "L" } else { "" })
    }
}

#[derive(Debug)]
struct Request {
    variant: PaqVariant,
    operation: Operation,
    directory: PathBuf,
    input: PathBuf,
    output: PathBuf,
    memory: u64,
    output_limit: u64,
    level: Level,
    layers: u8,
    joint_discount: Option<u8>,
}

fn text(value: &OsString) -> Result<&str, String> {
    value
        .to_str()
        .ok_or_else(|| "non-UTF-8 worker control value".into())
}
fn variant(value: &str) -> Result<PaqVariant, String> {
    match value {
        "v215" => Ok(PaqVariant::V215),
        "v216" => Ok(PaqVariant::V216),
        "joint-discount" => Ok(PaqVariant::JointDiscount),
        "store-state" => Ok(PaqVariant::StoreState),
        _ => Err("unknown PAQ variant".into()),
    }
}
fn identity(variant: PaqVariant) -> (&'static str, &'static [u8]) {
    match variant {
        PaqVariant::V215 => ("cix_paq_v215_bridge_version", b"cix-paq-v215-bridge/1"),
        PaqVariant::V216 => ("cix_paq_v216_bridge_version", b"cix-paq-v216-bridge/1"),
        PaqVariant::JointDiscount => (
            "cix_paq_joint_discount_bridge_version",
            b"cix-paq-joint_discount-bridge/1",
        ),
        PaqVariant::StoreState => (
            "cix_paq_store_state_bridge_version",
            b"cix-paq-store_state-bridge/1",
        ),
    }
}
impl Request {
    fn parse(args: &[OsString]) -> Result<Self, String> {
        if args.len() != 10 {
            return Err("internal worker v1 needs variant, operation, library directory, input, output, memory bytes, output bytes, level, LSTM layers, joint mode".into());
        }
        let variant = variant(text(&args[0])?)?;
        let operation = match text(&args[1])? {
            "encode" => Operation::Encode,
            "decode" => Operation::Decode,
            _ => return Err("invalid PAQ operation".into()),
        };
        let memory = text(&args[5])?
            .parse::<u64>()
            .map_err(|_| "invalid memory limit")?;
        let output_limit = text(&args[6])?
            .parse::<u64>()
            .map_err(|_| "invalid output limit")?;
        if memory == 0
            || output_limit == 0
            || memory > i64::MAX as u64
            || output_limit > i64::MAX as u64
        {
            return Err("worker limits must be finite positive signed-64-bit byte counts".into());
        }
        let level = Level::parse(text(&args[7])?)?;
        let layers = text(&args[8])?
            .parse::<u8>()
            .map_err(|_| "invalid LSTM layer count")?;
        if !(1..=5).contains(&layers) {
            return Err("LSTM layers must be 1 through 5".into());
        }
        let joint_discount = match text(&args[9])? {
            "none" => None,
            "0" => Some(0),
            "1" => Some(1),
            "2" => Some(2),
            "3" => Some(3),
            _ => return Err("invalid joint-discount mode".into()),
        };
        if variant != PaqVariant::JointDiscount && joint_discount.is_some() {
            return Err("joint mode requires the joint-discount variant".into());
        }
        let request = Self {
            variant,
            operation,
            directory: (&args[2]).into(),
            input: (&args[3]).into(),
            output: (&args[4]).into(),
            memory,
            output_limit,
            level,
            layers,
            joint_discount,
        };
        if request.operation == Operation::Encode && request.level.number == 0 {
            return Err(
                "PAQ level -0 is unsupported: pinned level-0 archives cannot be safely decoded"
                    .into(),
            );
        }
        Ok(request)
    }

    /// Every packaged PAQ variant has the same pinned level-zero null path:
    /// its raw-byte decoder leaves `predictorMain` null, while the retained
    /// block transform dispatcher unconditionally dereferences it. Inspect no
    /// more than the stable seven-byte `paq8px` header before the FFI call.
    fn reject_level_zero_archive(&self) -> Result<(), String> {
        if self.operation != Operation::Decode {
            return Ok(());
        }
        let mut header = [0_u8; 7];
        let read = std::fs::File::open(&self.input)
            .map_err(|error| format!("cannot read PAQ archive header: {error}"))?
            .read(&mut header)
            .map_err(|error| format!("cannot read PAQ archive header: {error}"))?;
        if read == header.len() && &header[..6] == b"paq8px" && header[6] == 0 {
            return Err("PAQ level-0 archive is unsupported: pinned decoder dereferences an absent predictor".into());
        }
        Ok(())
    }
    fn library(&self) -> Result<PathBuf, String> {
        for path in [&self.directory, &self.input, &self.output] {
            if !path.is_absolute() || path.as_os_str().as_encoded_bytes().contains(&0) {
                return Err("worker paths must be absolute and contain no NUL".into());
            }
        }
        for path in [&self.input, &self.output] {
            let bytes = path.as_os_str().as_encoded_bytes();
            if bytes.contains(&b'\\') || bytes.ends_with(b"/") {
                return Err(
                    "PAQ worker file paths must not contain backslashes or end in a separator"
                        .into(),
                );
            }
        }
        if !self.input.is_file() {
            return Err("worker input must be a regular file".into());
        }
        match std::fs::symlink_metadata(&self.output) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Ok(_) => return Err("worker output already exists, including a symlink".into()),
            Err(e) => return Err(format!("cannot inspect worker output: {e}")),
        }
        if !self.output.parent().is_some_and(Path::is_dir) {
            return Err("worker output directory is missing".into());
        }
        let directory = self
            .directory
            .canonicalize()
            .map_err(|e| format!("cannot resolve package library directory: {e}"))?;
        let descriptor = self.variant.descriptor().ok_or("unknown PAQ descriptor")?;
        let library = directory.join(shared_library_filename(descriptor.library_basename));
        let metadata = std::fs::symlink_metadata(&library)
            .map_err(|e| format!("missing packaged bridge: {e}"))?;
        if !metadata.file_type().is_file() {
            return Err("packaged bridge must be an ordinary file".into());
        }
        Ok(library)
    }
    fn argv(&self) -> Vec<OsString> {
        let mut args = vec![
            "cix-internal-paq".into(),
            match self.operation {
                Operation::Encode => self.level.argument().into(),
                Operation::Decode => "-d".into(),
            },
        ];
        args.extend(["-simd".into(), "AVX2".into()]);
        if self.operation == Operation::Encode && self.level.lstm {
            args.push(format!("-lstmlayers={}", self.layers).into());
        }
        args.extend([
            self.input.clone().into_os_string(),
            self.output.clone().into_os_string(),
        ]);
        args
    }
}

/// This is a CLI-only entry; its process is dedicated to exactly one codec call.
pub(crate) fn main(args: &[OsString]) -> i32 {
    let result = Request::parse(args).and_then(|request| run(&request));
    match result {
        Ok(()) => {
            eprintln!("CIX_PAQ_WORKER_V1 status=ok");
            0
        }
        Err(error) => {
            eprintln!("CIX_PAQ_WORKER_V1 status=error\n{error}");
            1
        }
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn run(_: &Request) -> Result<(), String> {
    Err("native PAQ worker currently requires Linux x86-64 qualification".into())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn run(request: &Request) -> Result<(), String> {
    let library_path = request.library()?;
    request.reject_level_zero_archive()?;
    if !std::is_x86_feature_detected!("avx2") {
        return Err("this PAQ worker profile requires AVX2".into());
    }
    // This module is entered before the CLI creates threads. Scrub inherited
    // research/model-path settings before dlopen can run upstream constructors.
    for (key, _) in std::env::vars_os() {
        std::env::remove_var(key);
    }
    std::env::set_var("LC_ALL", "C");
    std::env::set_var("OMP_NUM_THREADS", "1");
    let mut environment = PaqWorkerEnvironment {
        entries: Vec::new(),
    };
    if let Some(mode) = request.joint_discount {
        std::env::set_var("CIX_JOINT_DISCOUNT", mode.to_string());
        environment
            .entries
            .push(("CIX_JOINT_DISCOUNT".into(), mode.to_string().into()));
    }
    apply_limits(request.memory, request.output_limit)?;
    let library = load_library(&library_path)?;
    let descriptor = request
        .variant
        .descriptor()
        .ok_or("unknown PAQ descriptor")?;
    let (symbol, expected) = identity(request.variant);
    // Both signatures are defined by CIX-owned headers. The private DSO
    // handle outlives every call and returned function pointer.
    let version: unsafe extern "C" fn() -> *const libc::c_char =
        unsafe { library.symbol(symbol) }.map_err(map_symbol_error)?;
    let actual = unsafe { version() };
    if !unsafe { exact_version(actual, expected) } {
        return Err("packaged PAQ bridge ABI/variant mismatch".into());
    }
    let process: crate::paq_bridge::PaqProcess =
        unsafe { library.symbol(descriptor.process_symbol) }.map_err(map_symbol_error)?;
    let invocation = PaqInvocation {
        scope: PaqExecutionScope::IsolatedWorker,
        variant: request.variant,
        argv: request.argv(),
        input: request.input.clone(),
        output: request.output.clone(),
        worker_environment: environment,
        memory_limit_bytes: request.memory,
    };
    unsafe { PaqBridge::from_symbol(request.variant, process) }
        .invoke(&invocation)
        .map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&request.output)
        .map_err(|e| format!("worker output unavailable: {e}"))?
        .len();
    if size > request.output_limit {
        return Err("worker output limit exceeded".into());
    }
    Ok(())
}

// ABI contract: provider returns either null or a valid NUL-terminated C string.
// Read one byte at a time and stop at its NUL, never form a larger borrowed slice.
#[cfg(target_os = "linux")]
unsafe fn exact_version(actual: *const libc::c_char, expected: &[u8]) -> bool {
    if actual.is_null() {
        return false;
    }
    for (index, &byte) in expected.iter().enumerate() {
        let value = *actual.add(index) as u8;
        if value == 0 || value != byte {
            return false;
        }
    }
    *actual.add(expected.len()) == 0
}

#[cfg(target_os = "linux")]
fn apply_limits(memory: u64, output: u64) -> Result<(), String> {
    let mut limits = Vec::new();
    for (kind, requested) in [(libc::RLIMIT_AS, memory), (libc::RLIMIT_FSIZE, output)] {
        let mut old: libc::rlimit = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrlimit(kind, &mut old) } != 0 {
            return Err(format!(
                "cannot read worker limit: {}",
                std::io::Error::last_os_error()
            ));
        }
        let requested =
            libc::rlim_t::try_from(requested).map_err(|_| "worker limit out of platform range")?;
        let hard = old.rlim_max.min(requested);
        let soft = old.rlim_cur.min(hard);
        limits.push((
            kind,
            libc::rlimit {
                rlim_cur: soft,
                rlim_max: hard,
            },
        ));
    }
    for (kind, limit) in limits {
        if unsafe { libc::setrlimit(kind, &limit) } != 0 {
            return Err(format!(
                "cannot apply worker limit: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn load_library(path: &Path) -> Result<DynamicLibrary, String> {
    DynamicLibrary::open(path).map_err(map_load_error)
}

#[cfg(target_os = "linux")]
fn map_load_error(error: DynamicLibraryError) -> String {
    match error {
        DynamicLibraryError::InvalidPath => "NUL library path".into(),
        DynamicLibraryError::LoadFailed => {
            "cannot load packaged PAQ bridge or its installed dependencies".into()
        }
        DynamicLibraryError::MissingSymbol(_) => unreachable!("open cannot resolve a symbol"),
    }
}

#[cfg(target_os = "linux")]
fn map_symbol_error(error: DynamicLibraryError) -> String {
    match error {
        DynamicLibraryError::MissingSymbol(name) => {
            format!("packaged PAQ symbol is unavailable: {name}")
        }
        DynamicLibraryError::InvalidPath | DynamicLibraryError::LoadFailed => {
            unreachable!("symbol lookup cannot open a library")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args() -> Vec<OsString> {
        [
            "v215",
            "encode",
            "/package/lib",
            "/input",
            "/output",
            "1000000000",
            "1000000",
            "-1",
            "2",
            "none",
        ]
        .map(OsString::from)
        .into()
    }
    #[test]
    fn request_rejects_ambiguous_levels_limits_and_modes() {
        for bad in ["1", "-01", "-1LL", "-13", "-1A", "-L", "--1"] {
            assert!(Level::parse(bad).is_err(), "{bad}");
        }
        for level in 0..=12 {
            for suffix in ["", "L"] {
                let value = format!("-{level}{suffix}");
                assert_eq!(Level::parse(&value).unwrap().argument(), value);
            }
        }
        for (index, value) in [
            (0, "unknown"),
            (1, "test"),
            (5, "0"),
            (6, "18446744073709551615"),
            (8, "6"),
            (9, "1"),
        ] {
            let mut a = args();
            a[index] = value.into();
            assert!(Request::parse(&a).is_err());
        }
    }
    #[test]
    fn argv_keeps_paths_separate_and_never_loads_external_models() {
        let mut a = args();
        a[3] = "/input with spaces".into();
        a[7] = "-4L".into();
        a[8] = "3".into();
        let request = Request::parse(&a).unwrap();
        assert_eq!(
            request.argv(),
            [
                "cix-internal-paq",
                "-4L",
                "-simd",
                "AVX2",
                "-lstmlayers=3",
                "/input with spaces",
                "/output"
            ]
            .map(OsString::from)
        );
        let mut request = request;
        request.operation = Operation::Decode;
        assert!(!request
            .argv()
            .iter()
            .any(|value| value.to_string_lossy().starts_with("-lstmlayers=")));
        request.operation = Operation::Encode;
        request.level.lstm = false;
        assert!(!request
            .argv()
            .iter()
            .any(|value| value.to_string_lossy().starts_with("-lstmlayers=")));
    }
    #[test]
    fn version_identity_matches_each_cix_header() {
        for v in [
            PaqVariant::V215,
            PaqVariant::V216,
            PaqVariant::JointDiscount,
            PaqVariant::StoreState,
        ] {
            let d = v.descriptor().unwrap();
            let (symbol, version) = identity(v);
            assert_eq!(symbol, format!("{}_version", d.library_basename));
            assert!(version.ends_with(b"-bridge/1"));
        }
    }
    #[test]
    fn library_name_uses_the_installed_platform_rule() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "cix-paq-library-name-{}-{nonce}",
            std::process::id()
        ));
        let library_directory = root.join("library");
        let input = root.join("input");
        let output = root.join("output");
        std::fs::create_dir_all(&library_directory).unwrap();
        std::fs::write(&input, b"input").unwrap();
        let expected = library_directory.join(shared_library_filename(
            PaqVariant::V215.descriptor().unwrap().library_basename,
        ));
        std::fs::write(&expected, b"bridge").unwrap();

        let mut arguments = args();
        arguments[2] = library_directory.into_os_string();
        arguments[3] = input.into_os_string();
        arguments[4] = output.into_os_string();
        let request = Request::parse(&arguments).unwrap();
        assert_eq!(request.library().unwrap(), expected);

        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn all_pinned_variants_reject_level_zero_before_native_invocation() {
        for variant in [
            ("v215", "none"),
            ("v216", "none"),
            ("joint-discount", "0"),
            ("store-state", "none"),
        ] {
            let mut request = args();
            request[0] = variant.0.into();
            request[7] = "-0".into();
            request[9] = variant.1.into();
            let error = Request::parse(&request).unwrap_err().to_string();
            assert!(error.contains("level -0 is unsupported"), "{error}");
        }
    }
    #[test]
    fn exact_level_zero_header_rejects_before_dlopen() {
        let path = std::env::temp_dir().join(format!("cix-paq-level-zero-{}", std::process::id()));
        std::fs::write(&path, b"paq8px\0trailing archive bytes").unwrap();
        let mut request = Request::parse(&args()).unwrap();
        request.operation = Operation::Decode;
        request.input = path.clone();
        let error = request.reject_level_zero_archive().unwrap_err();
        assert!(error.contains("level-0 archive is unsupported"));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn nonzero_or_nonpaq_header_stays_admitted() {
        let path = std::env::temp_dir().join(format!("cix-paq-level-one-{}", std::process::id()));
        std::fs::write(&path, b"paq8px\x01").unwrap();
        let mut request = Request::parse(&args()).unwrap();
        request.operation = Operation::Decode;
        request.input = path.clone();
        assert!(request.reject_level_zero_archive().is_ok());
        std::fs::remove_file(path).unwrap();
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn version_check_stops_at_short_string_and_rejects_mismatch() {
        let good = std::ffi::CString::new("cix-paq-v215-bridge/1").unwrap();
        let short = std::ffi::CString::new("cix").unwrap();
        let expected = identity(PaqVariant::V215).1;
        unsafe {
            assert!(exact_version(good.as_ptr(), expected));
            assert!(!exact_version(short.as_ptr(), expected));
            assert!(!exact_version(std::ptr::null(), expected));
            assert!(!exact_version(good.as_ptr(), identity(PaqVariant::V216).1));
        }
    }
}
