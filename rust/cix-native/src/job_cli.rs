//! Same-executable RC23 job-mode entry point.
//!
//! Control uses stdin/stdout only; binary input and output are the
//! explicit files named by the fixed argument grammar. One process handles one
//! request. Cancellation is cooperative: a parent that needs to interrupt a
//! native call must terminate the process.

use crate::full_engine::{
    backend_provider::BackendProvider,
    dispatch::FullEngine,
    installed,
    job_protocol::{
        self, CancelToken, ControlRecord, JobError, JobErrorCode, JobOperation, JobProgress,
    },
    selection::NativeExecutor,
};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

struct Arguments {
    input: PathBuf,
    output: PathBuf,
    library: PathBuf,
    temporary: PathBuf,
}

fn absolute_regular(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} must be absolute"));
    }
    if !fs::metadata(&path)
        .map_err(|_| format!("{label} is unavailable"))?
        .is_file()
    {
        return Err(format!("{label} is not a regular file"));
    }
    fs::canonicalize(path).map_err(|error| error.to_string())
}

fn absolute_directory(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} must be absolute"));
    }
    if !fs::metadata(&path)
        .map_err(|_| format!("{label} is unavailable"))?
        .is_dir()
    {
        return Err(format!("{label} is not a directory"));
    }
    fs::canonicalize(path).map_err(|error| error.to_string())
}

fn parse(args: Vec<OsString>) -> Result<Arguments, String> {
    if args.len() != 9
        || args[0] != "--cix-job-v1"
        || args[1] != "--input"
        || args[3] != "--output"
        || args[5] != "--private-library-dir"
        || args[7] != "--temporary-root"
    {
        return Err("job mode requires --cix-job-v1 --input ABS --output ABS --private-library-dir ABS --temporary-root ABS".into());
    }
    let output = PathBuf::from(&args[4]);
    if !output.is_absolute() {
        return Err("output must be absolute".into());
    }
    if output.exists() {
        return Err("output already exists".into());
    }
    let parent = output.parent().ok_or("output has no parent")?;
    let _ = absolute_directory(parent.to_path_buf(), "output parent")?;
    Ok(Arguments {
        input: absolute_regular(PathBuf::from(&args[2]), "input")?,
        output,
        library: absolute_directory(PathBuf::from(&args[6]), "private library directory")?,
        temporary: absolute_directory(PathBuf::from(&args[8]), "temporary root")?,
    })
}

fn control_error<W: Write>(
    out: &mut W,
    request_id: u64,
    code: JobErrorCode,
    message: impl Into<String>,
) -> i32 {
    let message = bounded_error_message(message.into());
    let _ = write_record(
        out,
        &ControlRecord::Error(JobError {
            request_id,
            code,
            message,
        }),
    );
    1
}

fn bounded_error_message(mut message: String) -> String {
    const MAX_ERROR_MESSAGE: usize = job_protocol::MAX_CONTROL_TEXT_BYTES;
    if message.len() > MAX_ERROR_MESSAGE {
        let mut end = MAX_ERROR_MESSAGE.saturating_sub(" [truncated]".len());
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str(" [truncated]");
    }
    message
}

fn write_record<W: Write>(out: &mut W, record: &ControlRecord) -> Result<(), String> {
    job_protocol::write_control(out, record)?;
    out.flush().map_err(|error| error.to_string())
}

fn temporary_output(output: &Path) -> Result<(PathBuf, File), String> {
    let parent = output.parent().ok_or("output has no parent")?;
    for number in 0..64_u32 {
        let candidate = parent.join(format!(".cix-job-{}-{number}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("cannot create unique output temporary".into())
}

/// Atomically claim a previously absent destination. `rename` is unsuitable:
/// on Unix it can overwrite a concurrently-created destination.
fn publish(temp: &Path, output: &Path) -> Result<(), String> {
    fs::hard_link(temp, output).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            "output already exists".to_owned()
        } else {
            error.to_string()
        }
    })?;
    fs::remove_file(temp)
        .map_err(|error| format!("output committed but temporary cleanup failed: {error}"))
}

fn prefix<R: Read>(source: &mut R) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(6);
    let mut byte = [0_u8; 1];
    while bytes.len() < 6 {
        match source.read(&mut byte).map_err(|error| error.to_string())? {
            0 => break,
            _ => bytes.push(byte[0]),
        }
    }
    Ok(bytes)
}

fn native_without_worker_temporary(signature: &[u8]) -> bool {
    const NATIVE: &[&[u8]] = &[b"CIXG1", b"CIXG2", b"CIXM5", b"CIXM6", b"CIXZ1", b"CIXB1"];
    NATIVE.contains(&signature.get(..5).unwrap_or(signature))
}

fn staged_output_reservation(
    request: &job_protocol::JobRequest,
    signature: Option<&[u8]>,
) -> Result<usize, String> {
    let stage = match request.operation {
        // Selection writes an archive, constrained by both declared caps.
        JobOperation::SelectWholeInput | JobOperation::SelectIndependentWindows => request
            .limits
            .archive_bytes
            .min(request.limits.output_bytes),
        JobOperation::DecodeArchive => request.limits.output_bytes,
    };
    let worker = match signature {
        // PAQ owns an input and an output temporary simultaneously. All
        // non-native decode frames are conservatively admitted as potential
        // PAQ routes before dispatch decides their exact backend.
        Some(signature) if !native_without_worker_temporary(signature) => request
            .limits
            .archive_bytes
            .max(request.limits.intermediate_bytes)
            .checked_add(
                request
                    .limits
                    .output_bytes
                    .max(request.limits.intermediate_bytes),
            )
            .ok_or("decode temporary reservation overflow")?,
        _ => 0,
    };
    stage
        .checked_add(worker)
        .ok_or_else(|| "staged temporary reservation overflow".to_owned())
}

fn provider_engine(
    config: &crate::full_engine::selection::ProviderConfig,
    output_limit: usize,
    signature: &[u8],
) -> Result<FullEngine, String> {
    let magic = signature.get(..5).unwrap_or(signature);
    let direct_jxl = magic == b"CIXV\x01"
        || magic == b"CIXI\x02"
        || (magic == b"CIXI\x01" && !matches!(signature.get(5), Some(2 | 3)));
    let direct_spatial = magic == b"CIXI\x01" && matches!(signature.get(5), Some(2 | 3));
    let nested = magic == b"CIXH1";
    let images = match config.jxl_bridge.as_ref() {
        Some(path) if direct_jxl || nested => {
            match crate::full_engine::jxl_provider::JxlProvider::load_package_bridge(
                path,
                output_limit,
            ) {
                Ok(provider) => Some(provider),
                Err(_) if nested => None,
                Err(error) => return Err(format!("JPEG XL bridge: {error}")),
            }
        }
        None if direct_jxl => {
            return Err("JPEG XL bridge: installed CIX bridge file is missing".into())
        }
        _ => None,
    };
    let spatial = match config.spatial_bridge.as_ref() {
        Some(path) if direct_spatial || nested => {
            match crate::full_engine::spatial_provider::SpatialProvider::load_package_bridge(
                path,
                output_limit,
            ) {
                Ok(provider) => Some(provider),
                Err(_) if nested => None,
                Err(error) => return Err(format!("spatial bridge: {error}")),
            }
        }
        None if direct_spatial => {
            return Err("spatial bridge: installed CIX bridge file is missing".into())
        }
        _ => None,
    };
    Ok(FullEngine {
        backends: BackendProvider {
            paths: config.backends.clone(),
        },
        images,
        spatial,
    })
}

struct PrefixedReader<R> {
    first: Option<u8>,
    rest: R,
}

impl<R: Read> Read for PrefixedReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if let Some(first) = self.first.take() {
            output[0] = first;
            return Ok(1);
        }
        self.rest.read(output)
    }
}

fn listen_cancel(
    mut input: io::Stdin,
    request_id: u64,
    token: CancelToken,
    listener_error: Arc<AtomicBool>,
    commit_cutoff: Arc<Mutex<bool>>,
) {
    thread::spawn(move || {
        let mut first = [0_u8; 1];
        let first = match input.read(&mut first) {
            Ok(0) => return,
            Ok(_) => first[0],
            Err(_) => {
                if !*commit_cutoff.lock().expect("commit cutoff poisoned") {
                    listener_error.store(true, Ordering::Release);
                    token.cancel();
                }
                return;
            }
        };
        let mut input = PrefixedReader {
            first: Some(first),
            rest: input,
        };
        match job_protocol::read_control(&mut input) {
            Ok(ControlRecord::Cancel { request_id: id }) if id == request_id => {
                apply_cancel_before_cutoff(&commit_cutoff, &token);
            }
            Ok(ControlRecord::Cancel { .. }) | Ok(_) => {
                let cutoff = commit_cutoff.lock().expect("commit cutoff poisoned");
                if !*cutoff {
                    listener_error.store(true, Ordering::Release);
                    token.cancel();
                }
            }
            Err(_) => {
                let cutoff = commit_cutoff.lock().expect("commit cutoff poisoned");
                if !*cutoff {
                    listener_error.store(true, Ordering::Release);
                    token.cancel();
                }
            }
        }
    });
}

fn apply_cancel_before_cutoff(cutoff: &Mutex<bool>, token: &CancelToken) -> bool {
    if *cutoff.lock().expect("commit cutoff poisoned") {
        false
    } else {
        token.cancel();
        true
    }
}

fn cli_capabilities() -> job_protocol::JobCapability {
    let mut capabilities = job_protocol::capabilities();
    capabilities.data_transport =
        "separate explicit absolute input/output files; whole selection input <= 128 MiB; bounded independent CIXW1 windows with cumulative input/archive caps".into();
    capabilities.cancellation =
        "cooperative stage-boundary cancellation; parent terminates process for a hard stop".into();
    capabilities
}

/// Entry called by `main` after future registration. Returns process status.
pub fn main(args: Vec<OsString>) -> i32 {
    let mut stdout = io::stdout();
    if args.as_slice() == [OsString::from("--cix-job-capabilities-v1")] {
        return if write_record(&mut stdout, &ControlRecord::Capability(cli_capabilities())).is_ok()
        {
            0
        } else {
            1
        };
    }
    let parsed = match parse(args) {
        Ok(value) => value,
        Err(error) => return control_error(&mut stdout, 0, JobErrorCode::InvalidRequest, error),
    };
    let mut stdin = io::stdin();
    let request = match job_protocol::read_control(&mut stdin) {
        Ok(ControlRecord::Request(value)) => value,
        Ok(_) => {
            return control_error(
                &mut stdout,
                0,
                JobErrorCode::Protocol,
                "first control record must be a request",
            )
        }
        Err(error) => return control_error(&mut stdout, 0, JobErrorCode::Protocol, error),
    };

    let output = Arc::new(Mutex::new(stdout));
    let control_failed = Arc::new(AtomicBool::new(false));
    let listener_error = Arc::new(AtomicBool::new(false));
    let commit_cutoff = Arc::new(Mutex::new(false));
    let cancel = CancelToken::new();
    listen_cancel(
        stdin,
        request.request_id,
        cancel.clone(),
        listener_error.clone(),
        commit_cutoff.clone(),
    );
    let emit = {
        let output = output.clone();
        let control_failed = control_failed.clone();
        let cancel = cancel.clone();
        move |progress: JobProgress| {
            let written = output.lock().ok().and_then(|mut out| {
                write_record(&mut *out, &ControlRecord::Progress(progress)).ok()
            });
            if written.is_none() {
                control_failed.store(true, Ordering::Release);
                cancel.cancel();
            }
        }
    };

    let input = match File::open(&parsed.input) {
        Ok(value) => value,
        Err(error) => {
            let mut out = output.lock().expect("stdout mutex poisoned");
            return control_error(
                &mut *out,
                request.request_id,
                JobErrorCode::Io,
                error.to_string(),
            );
        }
    };
    let mut source = BufReader::new(input);
    let signature = if request.operation == JobOperation::DecodeArchive {
        match prefix(&mut source) {
            Ok(value) => Some(value),
            Err(error) => {
                let mut out = output.lock().expect("stdout mutex poisoned");
                return control_error(&mut *out, request.request_id, JobErrorCode::Io, error);
            }
        }
    } else {
        None
    };
    let staged = match staged_output_reservation(&request, signature.as_deref()) {
        Ok(value) if value <= request.limits.temporary_bytes => value,
        Ok(_) => {
            let mut out = output.lock().expect("stdout mutex poisoned");
            return control_error(
                &mut *out,
                request.request_id,
                JobErrorCode::ResourceLimit,
                "staged output and potential worker temporary exceed declared temporary limit",
            );
        }
        Err(error) => {
            let mut out = output.lock().expect("stdout mutex poisoned");
            return control_error(
                &mut *out,
                request.request_id,
                JobErrorCode::ResourceLimit,
                error,
            );
        }
    };
    let mut execution_request = request.clone();
    if matches!(
        request.operation,
        JobOperation::SelectWholeInput | JobOperation::SelectIndependentWindows
    ) {
        execution_request.limits.temporary_bytes = request.limits.temporary_bytes - staged;
        if execution_request.limits.temporary_bytes == 0 {
            let mut out = output.lock().expect("stdout mutex poisoned");
            return control_error(
                &mut *out,
                request.request_id,
                JobErrorCode::ResourceLimit,
                "no temporary budget remains for whole-input selection after staging output",
            );
        }
    }
    let (temporary, mut data_output) = match temporary_output(&parsed.output) {
        Ok(value) => value,
        Err(error) => {
            let mut out = output.lock().expect("stdout mutex poisoned");
            return control_error(&mut *out, request.request_id, JobErrorCode::Io, error);
        }
    };
    let executable = match std::env::current_exe() {
        Ok(value) => value,
        Err(error) => {
            drop(data_output);
            let _ = fs::remove_file(&temporary);
            let mut out = output.lock().expect("stdout mutex poisoned");
            return control_error(
                &mut *out,
                request.request_id,
                JobErrorCode::Io,
                error.to_string(),
            );
        }
    };
    let installed = match installed::discover(&executable, Some(&parsed.library), &parsed.temporary)
    {
        Ok(value) => value,
        Err(error) => {
            drop(data_output);
            let _ = fs::remove_file(&temporary);
            let mut out = output.lock().expect("stdout mutex poisoned");
            return control_error(
                &mut *out,
                request.request_id,
                JobErrorCode::InvalidRequest,
                error.to_string(),
            );
        }
    };
    let result = match request.operation {
        JobOperation::SelectWholeInput => job_protocol::execute_select_whole_input(
            &execution_request,
            &mut source,
            &mut data_output,
            NativeExecutor {
                providers: installed.config.clone(),
            },
            cancel.clone(),
            emit,
        ),
        JobOperation::SelectIndependentWindows => job_protocol::execute_select_independent_windows(
            &execution_request,
            &mut source,
            &mut data_output,
            NativeExecutor {
                providers: installed.config.clone(),
            },
            cancel.clone(),
            emit,
        ),
        JobOperation::DecodeArchive => {
            let signature = signature.expect("decode signature captured before staging");
            let engine =
                match provider_engine(&installed.config, request.limits.output_bytes, &signature) {
                    Ok(value) => value,
                    Err(error) => {
                        drop(data_output);
                        let _ = fs::remove_file(&temporary);
                        let mut out = output.lock().expect("stdout mutex poisoned");
                        return control_error(
                            &mut *out,
                            request.request_id,
                            JobErrorCode::Execution,
                            error,
                        );
                    }
                };
            job_protocol::execute_decode(
                &request,
                &engine,
                Cursor::new(signature).chain(source),
                &mut data_output,
                cancel.clone(),
                emit,
            )
        }
    };
    let flush = data_output.sync_all().map_err(|error| error.to_string());
    drop(data_output);

    if control_failed.load(Ordering::Acquire) {
        let _ = fs::remove_file(&temporary);
        return 1;
    }
    if listener_error.load(Ordering::Acquire) {
        let _ = fs::remove_file(&temporary);
        let mut out = output.lock().expect("stdout mutex poisoned");
        return control_error(
            &mut *out,
            request.request_id,
            JobErrorCode::Protocol,
            "invalid cancellation control record",
        );
    }
    if cancel.is_cancelled() {
        let _ = fs::remove_file(&temporary);
        let mut out = output.lock().expect("stdout mutex poisoned");
        return control_error(
            &mut *out,
            request.request_id,
            JobErrorCode::Cancelled,
            "job cancelled cooperatively",
        );
    }
    if let Err(error) = flush {
        let _ = fs::remove_file(&temporary);
        let mut out = output.lock().expect("stdout mutex poisoned");
        return control_error(&mut *out, request.request_id, JobErrorCode::Io, error);
    }
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            let mut out = output.lock().expect("stdout mutex poisoned");
            return control_error(
                &mut *out,
                request.request_id,
                JobErrorCode::Execution,
                error,
            );
        }
    };

    // Serialize under the protocol's bounded writer before committing the
    // independent data channel. This separates an invalid/oversized result
    // from the unavoidable post-commit stdout I/O failure case.
    let mut final_control = Vec::new();
    if let Err(error) =
        job_protocol::write_control(&mut final_control, &ControlRecord::Result(result))
    {
        let _ = fs::remove_file(&temporary);
        let mut out = output.lock().expect("stdout mutex poisoned");
        return control_error(&mut *out, request.request_id, JobErrorCode::Protocol, error);
    }

    // The mutex makes cancellation-before-commit deterministic: an optional
    // cancel record either takes effect before this cutoff, or is ignored once
    // publication has begun.
    {
        let mut cutoff = commit_cutoff.lock().expect("commit cutoff poisoned");
        if control_failed.load(Ordering::Acquire) || listener_error.load(Ordering::Acquire) {
            let _ = fs::remove_file(&temporary);
            return 1;
        }
        if cancel.is_cancelled() {
            let _ = fs::remove_file(&temporary);
            let mut out = output.lock().expect("stdout mutex poisoned");
            return control_error(
                &mut *out,
                request.request_id,
                JobErrorCode::Cancelled,
                "job cancelled cooperatively",
            );
        }
        *cutoff = true;
    }
    if let Err(error) = publish(&temporary, &parsed.output) {
        let _ = fs::remove_file(&temporary);
        let mut out = output.lock().expect("stdout mutex poisoned");
        return control_error(&mut *out, request.request_id, JobErrorCode::Io, error);
    }
    // Control and filesystem publication cannot be one transaction. A failed
    // final write returns nonzero even though the already-committed output may
    // exist; it never emits a false success result followed by an error.
    let mut out = output.lock().expect("stdout mutex poisoned");
    if out
        .write_all(&final_control)
        .and_then(|()| out.flush())
        .is_err()
    {
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_deterministically_rejected_after_commit_cutoff() {
        let cutoff = Mutex::new(false);
        let before = CancelToken::new();
        assert!(apply_cancel_before_cutoff(&cutoff, &before));
        assert!(before.is_cancelled());

        *cutoff.lock().expect("cutoff") = true;
        let after = CancelToken::new();
        assert!(!apply_cancel_before_cutoff(&cutoff, &after));
        assert!(!after.is_cancelled());
    }

    #[test]
    fn terminal_error_message_is_utf8_bounded() {
        let message = bounded_error_message("é".repeat(job_protocol::MAX_CONTROL_TEXT_BYTES));
        assert!(message.len() <= job_protocol::MAX_CONTROL_TEXT_BYTES);
        assert!(message.is_char_boundary(message.len()));
        assert!(message.ends_with(" [truncated]"));
    }

    #[test]
    fn paq_workspace_reservation_covers_intermediate_input_and_output() {
        let request = job_protocol::JobRequest {
            request_id: 1,
            operation: JobOperation::DecodeArchive,
            profile: job_protocol::JobProfile::Default,
            limits: job_protocol::JobLimits {
                input_bytes: 8,
                archive_bytes: 64,
                output_bytes: 32,
                memory_bytes: 1024,
                intermediate_bytes: 256,
                temporary_bytes: 1024,
                workers: 1,
                deadline_millis: None,
            },
            window_bytes: None,
            diagnostics: false,
        };
        // The CIXP prefix is non-native and may dispatch PAQ after an
        // intermediate transform. The staged final output adds 32 bytes.
        assert_eq!(
            staged_output_reservation(&request, Some(b"CIXP\x01")),
            Ok(32 + 256 + 256)
        );
    }
}
