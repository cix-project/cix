//! CLI-only full-engine adapter.
//!
//! This keeps executable discovery, private PAQ-worker preparation and
//! human-readable list output outside the library surfaces.  Provider DSOs
//! are loaded only for archive signatures that can require them, so a bad
//! optional bridge cannot break ordinary CIXG/CIXB1 decoding.

use crate::{
    cli::{cancellation_token, Opt},
    full_engine::{
        backend_provider::{BackendProvider, TrustedBackendPaths},
        dispatch::{FullEngine, OperationLimits},
        installed,
        io::{self, DecodePath},
        jxl_provider::JxlProvider,
        selection::{
            self, NativeExecutor, SelectionLimits, SelectionProfile, TrialDetails, WholeConstraints,
        },
        spatial_provider::SpatialProvider,
        window_stream::{self, EncodeLimits, StreamSummary},
    },
};
use std::{
    fs,
    io::{self as stdio, Read, Write},
    path::{Path, PathBuf},
};

struct PrefixReader<R> {
    prefix: stdio::Cursor<Vec<u8>>,
    source: R,
}
impl<R: Read> Read for PrefixReader<R> {
    fn read(&mut self, output: &mut [u8]) -> stdio::Result<usize> {
        let count = self.prefix.read(output)?;
        if count == 0 {
            self.source.read(output)
        } else {
            Ok(count)
        }
    }
}
struct CountingWriter {
    bytes: usize,
}
impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> stdio::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or_else(|| stdio::Error::other("decoded byte count overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> stdio::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DecodeReport {
    pub(crate) signature: Vec<u8>,
    pub(crate) path: DecodePath,
    pub(crate) restored_bytes: usize,
}

fn prefix<R: Read>(source: &mut R) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(5);
    let mut byte = [0];
    while bytes.len() < 6 {
        if source.read(&mut byte).map_err(|e| e.to_string())? == 0 {
            break;
        }
        bytes.push(byte[0]);
    }
    Ok(bytes)
}
fn magic(prefix: &[u8]) -> &[u8] {
    prefix.get(..5).unwrap_or(prefix)
}
fn needs_jxl(prefix: &[u8]) -> bool {
    magic(prefix) == b"CIXV\x01"
        || magic(prefix) == b"CIXI\x02"
        || (magic(prefix) == b"CIXI\x01" && !matches!(prefix.get(5), Some(2 | 3)))
}
fn needs_spatial(prefix: &[u8]) -> bool {
    magic(prefix) == b"CIXI\x01" && matches!(prefix.get(5), Some(2 | 3))
}
fn native_incremental(prefix: &[u8]) -> bool {
    let magic = magic(prefix);
    magic == b"CIXG1"
        || magic == b"CIXG2"
        || magic == b"CIXM5"
        || magic == b"CIXM6"
        || magic == b"CIXZ1"
        || magic == b"CIXB1"
}

#[cfg(unix)]
fn default_temporary_root() -> Result<PathBuf, String> {
    let root = match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home)
            .join(".cache")
            .join("cix")
            .join("paq-private"),
        None => std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(".cix-unused-private-root"),
    };
    if !root.is_absolute() {
        return Err("HOME does not provide an absolute private PAQ worker root".into());
    }
    Ok(root)
}
#[cfg(not(unix))]
fn default_temporary_root() -> Result<PathBuf, String> {
    std::env::current_dir()
        .map(|path| path.join(".cix-unused-private-root"))
        .map_err(|e| e.to_string())
}

#[cfg(unix)]
fn create_default_private_root(root: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    if root.exists() {
        return crate::paq_supervisor::validate_private_temporary_root(root)
            .map_err(|e| e.to_string());
    }
    let mut missing = Vec::new();
    let mut cursor = root;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor
            .parent()
            .ok_or("private PAQ worker root has no existing ancestor")?;
    }
    validate_existing_private_ancestors(cursor)?;
    // Do not chmod or otherwise mutate any existing directory. New elements
    // are created one at a time with 0700 before the supervisor validates the
    // complete real-directory ancestry.
    for directory in missing.into_iter().rev() {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&directory).map_err(|e| e.to_string())?;
    }
    crate::paq_supervisor::validate_private_temporary_root(root).map_err(|e| e.to_string())
}

#[cfg(unix)]
fn validate_existing_private_ancestors(start: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let uid = unsafe { libc::geteuid() };
    for ancestor in start.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("private PAQ worker root has a non-directory or symlink ancestor".into());
        }
        if metadata.uid() != uid && metadata.uid() != 0 {
            return Err("private PAQ worker root has an untrusted ancestor owner".into());
        }
        if metadata.mode() & 0o022 != 0 {
            return Err("private PAQ worker root has a group/other-writable ancestor".into());
        }
    }
    Ok(())
}

fn temporary_root(o: &Opt, need_paq: bool) -> Result<PathBuf, String> {
    let root = match &o.temporary_root {
        Some(path) => PathBuf::from(path),
        None => default_temporary_root()?,
    };
    if !root.is_absolute() {
        if need_paq {
            return Err("--temporary-root must be absolute".into());
        }
        // An invalid PAQ override never disables a non-PAQ CIX decode. Keep
        // a harmless absolute placeholder; PAQ admission will report the
        // override error only if that backend is actually requested.
        return default_temporary_root();
    }
    if need_paq {
        match &o.temporary_root {
            Some(_) => crate::paq_supervisor::validate_private_temporary_root(&root)
                .map_err(|e| e.to_string())?,
            None => {
                #[cfg(unix)]
                create_default_private_root(&root)?;
                #[cfg(not(unix))]
                return Err(
                    "PAQ worker capability is unavailable without a qualified private root".into(),
                );
            }
        }
    }
    Ok(root)
}

fn provider_config(o: &Opt, need_paq: bool) -> Result<selection::ProviderConfig, String> {
    let temporary_root = temporary_root(o, need_paq)?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let library_directory = o.private_library_directory.as_deref().map(Path::new);
    installed::discover(&executable, library_directory, &temporary_root)
        .map(|installed| installed.config)
        .map_err(|e| e.to_string())
}

/// PAQ isolation is only useful when an archive/search may reach a PAQ route.
/// Failure is advisory here: generic candidates still run and the eventual
/// PAQ trial records its own worker omission.
fn prepare_paq_root(o: &Opt) -> Result<(), String> {
    temporary_root(o, true).map(|_| ())
}

fn core_only_engine() -> FullEngine {
    // `io::decode` never dispatches these paths through a full-engine backend;
    // do not let ordinary CIX decoding depend on HOME, cwd, installed DSOs, or
    // a same-CIX worker path merely to satisfy the unused struct fields.
    FullEngine {
        backends: BackendProvider {
            paths: TrustedBackendPaths {
                cix: PathBuf::new(),
                paq_libraries: PathBuf::new(),
                temporary_root: PathBuf::new(),
            },
        },
        images: None,
        spatial: None,
    }
}

fn engine_for(o: &Opt, signature: &[u8]) -> Result<FullEngine, String> {
    if native_incremental(signature) || magic(signature) == b"CIXR1" {
        return Ok(core_only_engine());
    }
    let potential_paq = !native_incremental(signature) && magic(signature) != b"CIXR1";
    let config = provider_config(o, false)?;
    let mut omissions = Vec::new();
    if potential_paq {
        if let Err(error) = prepare_paq_root(o) {
            omissions.push(format!("PAQ worker root: {error}"));
        }
    }
    let images = if needs_jxl(signature) || magic(signature) == b"CIXH1" {
        match config.jxl_bridge.as_ref() {
            Some(path) => match JxlProvider::load_package_bridge(path, o.memory) {
                Ok(provider) => Some(provider),
                Err(error) => {
                    omissions.push(format!("JPEG XL bridge: {error}"));
                    None
                }
            },
            None => {
                omissions.push("JPEG XL bridge: installed CIX bridge file is missing".into());
                None
            }
        }
    } else {
        None
    };
    let spatial = if needs_spatial(signature) || magic(signature) == b"CIXH1" {
        match config.spatial_bridge.as_ref() {
            Some(path) => match SpatialProvider::load_package_bridge(path, o.memory) {
                Ok(provider) => Some(provider),
                Err(error) => {
                    omissions.push(format!("spatial bridge: {error}"));
                    None
                }
            },
            None => {
                omissions.push("spatial bridge: installed CIX bridge file is missing".into());
                None
            }
        }
    } else {
        None
    };
    let required_provider_failure = !omissions.is_empty()
        && magic(signature) != b"CIXH1"
        && omissions
            .iter()
            .any(|omission| !omission.starts_with("PAQ worker root:"));
    if required_provider_failure {
        return Err(format!(
            "selected archive cannot load required providers: {}",
            omissions.join("; ")
        ));
    }
    if !omissions.is_empty() && (o.explain || o.verbose) {
        for omission in &omissions {
            eprintln!("cix: mixed_provider_omission={omission}");
        }
    }
    Ok(FullEngine {
        backends: BackendProvider {
            paths: config.backends,
        },
        images,
        spatial,
    })
}

/// Decode any automatic CIX signature through the bounded full-engine I/O
/// bridge. A caller requesting list/test passes `verify_only`; list rendering
/// stays in the CLI while all restoration still uses the same archive path.
pub(crate) fn decode<R: Read, W: Write>(
    o: &Opt,
    mut source: R,
    destination: W,
    verify_only: bool,
) -> Result<DecodeReport, String> {
    let signature = prefix(&mut source)?;
    let engine = engine_for(o, &signature)?;
    let limits = OperationLimits {
        memory_bytes: o.memory,
        output_bytes: if native_incremental(&signature) {
            usize::MAX
        } else {
            o.memory
        },
        intermediate_bytes: (o.memory / 4).max(1),
        deadline: None,
        cancellation: Some(cancellation_token()),
    };
    let mut counted = CountingWriter { bytes: 0 };
    let path = if verify_only {
        io::decode(
            &engine,
            PrefixReader {
                prefix: stdio::Cursor::new(signature.clone()),
                source,
            },
            &mut counted,
            usize::MAX,
            &limits,
        )?
    } else {
        // The counting wrapper is not used on an ordinary decode; destination
        // receives bytes directly and `restored_bytes` remains observable only
        // for verification/list mode.
        io::decode(
            &engine,
            PrefixReader {
                prefix: stdio::Cursor::new(signature.clone()),
                source,
            },
            destination,
            usize::MAX,
            &limits,
        )?
    };
    Ok(DecodeReport {
        signature,
        path,
        restored_bytes: counted.bytes,
    })
}

pub(crate) fn list_line(report: &DecodeReport) -> String {
    let signature = String::from_utf8_lossy(&report.signature);
    format!("cix: signature={signature:?} path={:?} restored_bytes={} verified=true full_frame_buffered={}", report.path, report.restored_bytes, report.path == DecodePath::FullFrameBuffered)
}

fn whole_encode_ineligibility(o: &Opt) -> Option<&'static str> {
    if o.input == "-" {
        Some("stdin requires the bounded streaming encoder")
    } else if o.stream || o.flush_interval.is_some() {
        Some("stream/flush controls require the bounded streaming encoder")
    } else if o.history != 0 || o.independent_blocks {
        Some("history/reset controls require the bounded streaming encoder")
    } else if !matches!(o.format.as_str(), "auto" | "cixg1" | "cixg2" | "cixm6") {
        Some("explicit container format constrains the native encoder")
    } else if o.route.as_deref().is_some_and(|route| route != "auto") {
        Some("explicit route constrains the native encoder")
    } else if o.external_backend.is_some() || o.portfolio.is_some() {
        Some("explicit external/portfolio selection controls encoding")
    } else {
        None
    }
}

fn whole_constraints(o: &Opt) -> WholeConstraints {
    WholeConstraints {
        native: crate::full_engine::native_portfolio::NativePortfolioConstraints {
            formats: (!matches!(o.format.as_str(), "auto")).then(|| vec![o.format.clone()]),
            blocks: o
                .block
                .map(|block| vec![block.min(crate::core::engine::MAX_BLOCK)]),
            backend: o.backend_set.then(|| o.backend.clone()),
            effort: (o.level_set && !matches!(o.level, 1 | 6 | 9)).then_some(o.level),
            strategy: o.parallelism_set.then(|| o.parallelism.clone()),
        },
    }
}

fn selection_profile(level: u8) -> SelectionProfile {
    match level {
        0..=1 => SelectionProfile::Fast,
        2..=8 => SelectionProfile::Default,
        _ => SelectionProfile::Best,
    }
}

/// CIXW1 deliberately carries independent complete archives. Controls which
/// require the legacy encoder's continuous state or time-driven flush promise
/// are therefore not silently translated into a windowed archive.
pub(crate) fn window_encode_ineligibility(o: &Opt) -> Option<&'static str> {
    if o.stream {
        Some("--stream requires the legacy framed encoder")
    } else if o.flush_interval.is_some() {
        Some("--flush-interval requires the legacy idle-time flush contract")
    } else if o.history != 0 || o.independent_blocks {
        Some("history/reset controls require the legacy encoder")
    } else if !matches!(o.format.as_str(), "auto" | "cixg1" | "cixg2" | "cixm6") {
        Some("the selected container cannot be embedded in CIXW1")
    } else if o.route.as_deref().is_some_and(|route| route != "auto") {
        Some("explicit route requires the legacy encoder")
    } else if o.external_backend.is_some() || o.portfolio.is_some() {
        Some("explicit external/portfolio selection requires the legacy encoder")
    } else {
        None
    }
}

fn window_bytes(memory: usize) -> Option<usize> {
    // Keep one reasonably useful local-search window while leaving the rest of
    // the declared budget for source-aware selection and its bounded result.
    // Halving preserves a useful window under smaller explicit memory limits.
    let mut bytes = (16 << 20).min(selection::MAX_WHOLE_INPUT);
    while bytes >= 64 << 10 {
        let output = bytes.checked_add(2 << 20)?;
        let selection_memory = memory.checked_sub(bytes)?;
        if selection::minimum_whole_input_memory(bytes, output)
            .is_ok_and(|required| required <= selection_memory)
        {
            return Some(bytes);
        }
        bytes /= 2;
    }
    None
}

/// Encode an unbounded source as independently selected, bounded CIXW1
/// windows. The source is read once and only one source window plus its
/// selected archive are live at a time.
pub(crate) fn encode_windows<R: Read, W: Write>(
    o: &Opt,
    source: R,
    destination: &mut W,
) -> Result<Option<StreamSummary>, String> {
    if window_encode_ineligibility(o).is_some() {
        return Ok(None);
    }
    let Some(window_bytes) = window_bytes(o.memory) else {
        return Ok(None);
    };
    let profile = selection_profile(o.level);
    if profile == SelectionProfile::Best {
        if let Err(error) = prepare_paq_root(o) {
            if o.explain || o.verbose {
                eprintln!("cix: paq_worker_omission={error}");
            }
        }
    }
    let providers = provider_config(o, false)?;
    let constraints = whole_constraints(o);
    let show_reports = o.explain || o.verbose;
    let mut reported_windows = 0u64;
    const MAX_WINDOW_SELECTION_REPORTS: u64 = 8;
    let summary = window_stream::encode(
        source,
        destination,
        &EncodeLimits {
            window_bytes,
            memory_bytes: o.memory,
            // CIXW1 writes records directly. This cap controls serialized
            // accounting only, not a retained output allocation.
            output_bytes: usize::MAX,
            deadline: None,
            cancellation: Some(cancellation_token()),
        },
        |window, selection_memory| {
            let output_bytes = window
                .len()
                .checked_add(2 << 20)
                .ok_or("CIXW1 window output bound overflow")?;
            let required = selection::minimum_whole_input_memory(window.len(), output_bytes)?;
            if selection_memory < required {
                return Err("CIXW1 window cannot admit bounded local selection".into());
            }
            let result = selection::select_whole_input_constrained(
                window,
                profile,
                SelectionLimits {
                    memory_bytes: selection_memory,
                    workers: o.threads,
                    temporary_bytes: selection_memory,
                    output_bytes,
                    intermediate_bytes: (selection_memory / 4).max(1),
                    deadline: None,
                    cancellation: Some(cancellation_token()),
                },
                NativeExecutor {
                    providers: providers.clone(),
                },
                constraints.clone(),
            )?;
            if show_reports && reported_windows < MAX_WINDOW_SELECTION_REPORTS {
                eprintln!("cix: window_sequence={reported_windows} selection_report_begin");
                report_selection(&result.report);
                reported_windows += 1;
            }
            Ok(result.archive)
        },
    )?;
    if show_reports && summary.windows > reported_windows {
        eprintln!(
            "cix: window_selection_reports_omitted={} limit={MAX_WINDOW_SELECTION_REPORTS}",
            summary.windows - reported_windows,
        );
    }
    Ok(Some(summary))
}

fn whole_input_ineligibility(o: &Opt) -> Option<String> {
    let metadata = fs::metadata(&o.input).ok()?;
    if !metadata.is_file() {
        return Some("whole-engine selection requires a regular input file".into());
    }
    let length = match usize::try_from(metadata.len()) {
        Ok(length) => length,
        Err(_) => return Some("input length exceeds platform capacity".into()),
    };
    if length > selection::MAX_WHOLE_INPUT {
        return Some("input exceeds the 128 MiB whole-engine input limit".into());
    }
    let output_bytes = match length.checked_add(2 << 20) {
        Some(bytes) => bytes,
        None => return Some("whole-engine output bound overflow".into()),
    };
    match selection::minimum_whole_input_memory(length, output_bytes) {
        Ok(required) if o.memory < required => Some(format!(
            "memory budget {} is below the whole-engine baseline reservation {required}",
            o.memory
        )),
        Ok(_) => None,
        Err(error) => Some(error),
    }
}

/// Attempt automatic full-engine selection for a bounded regular input.
/// `Ok(None)` retains the legacy encoder for an explicitly unsupported
/// dimension and is paired with an explain diagnostic by the CLI caller.
pub(crate) fn encode_whole<W: Write>(
    o: &Opt,
    destination: &mut W,
) -> Result<Option<selection::SelectionReport>, String> {
    if whole_encode_ineligibility(o).is_some() {
        return Ok(None);
    }
    let metadata = fs::metadata(&o.input).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Ok(None);
    }
    let length =
        usize::try_from(metadata.len()).map_err(|_| "input length exceeds platform capacity")?;
    if length > selection::MAX_WHOLE_INPUT {
        return Ok(None);
    }
    let output_bytes = length
        .checked_add(2 << 20)
        .ok_or("whole-engine output bound overflow")?;
    let minimum_memory = selection::minimum_whole_input_memory(length, output_bytes)?;
    if o.memory < minimum_memory {
        return Ok(None);
    }
    let mut source = fs::File::open(&o.input).map_err(|e| e.to_string())?;
    let opened = source.metadata().map_err(|e| e.to_string())?;
    if !opened.is_file() || opened.len() != metadata.len() {
        return Err("input changed before full-engine selection could open it".into());
    }
    let mut input = Vec::new();
    input
        .try_reserve_exact(length)
        .map_err(|_| "whole-engine input allocation failed")?;
    Read::by_ref(&mut source)
        .take(length as u64)
        .read_to_end(&mut input)
        .map_err(|e| e.to_string())?;
    let after = source.metadata().map_err(|e| e.to_string())?;
    let current = fs::metadata(&o.input).map_err(|e| e.to_string())?;
    if input.len() != length
        || source.read(&mut [0]).map_err(|e| e.to_string())? != 0
        || after.len() != opened.len()
        || current.len() != opened.len()
    {
        return Err("input changed while full-engine selection was reading it".into());
    }
    let selection_memory = o
        .memory
        .checked_sub(input.capacity().saturating_sub(input.len()))
        .ok_or("whole-engine source allocation exceeds memory limit")?;
    if output_bytes > selection_memory {
        return Ok(None);
    }
    if selection_memory < minimum_memory {
        return Ok(None);
    }
    let profile = selection_profile(o.level);
    if profile == SelectionProfile::Best {
        if let Err(error) = prepare_paq_root(o) {
            if o.explain || o.verbose {
                eprintln!("cix: paq_worker_omission={error}");
            }
        }
    }
    // PAQ plans are present only in BEST. Default/FAST retain their normal
    // policy without creating a PAQ root merely because it may be configured.
    let providers = provider_config(o, false)?;
    let result = selection::select_whole_input_constrained(
        &input,
        profile,
        SelectionLimits {
            memory_bytes: selection_memory,
            workers: o.threads,
            temporary_bytes: o.memory,
            output_bytes,
            intermediate_bytes: (o.memory / 4).max(1),
            deadline: None,
            cancellation: Some(cancellation_token()),
        },
        NativeExecutor { providers },
        whole_constraints(o),
    )?;
    destination
        .write_all(&result.archive)
        .map_err(|e| e.to_string())?;
    Ok(Some(result.report))
}

pub(crate) fn encode_ineligibility(o: &Opt) -> Option<String> {
    whole_encode_ineligibility(o)
        .map(str::to_owned)
        .or_else(|| whole_input_ineligibility(o))
}

pub(crate) fn report_window_summary(summary: &StreamSummary) {
    eprintln!(
        "cix: full_engine windowed=true windows={} input_bytes={} archive_bytes={} independent_windows={} cross_window_history=none whole_archive_comparison=none",
        summary.windows,
        summary.input_bytes,
        summary.archive_bytes,
        summary.independent_windows,
    );
}

pub(crate) fn report_selection(report: &selection::SelectionReport) {
    eprintln!("cix: full_engine selected={} input_bytes={} peak_memory={} peak_workers={} peak_temporary={}", report.selected, report.input_bytes, report.peak_reserved_memory, report.peak_reserved_workers, report.peak_reserved_temporary);
    for discovery in &report.discovery {
        eprintln!("cix: discovery={discovery:?}");
    }
    for trial in &report.trials {
        eprintln!(
            "cix: candidate={} status={:?} archive_bytes={:?} reason={:?}",
            trial.id, trial.status, trial.archive_bytes, trial.reason
        );
        if let TrialDetails::Mixed(details) = &trial.details {
            for entry in &details.entries {
                eprintln!(
                    "cix: mixed_region={} kind={} candidate={} status={:?} backend={:?} payload_bytes={:?} detail={:?}",
                    entry.region,
                    entry.region_kind.name(),
                    entry.candidate,
                    entry.status,
                    entry.backend,
                    entry.payload_bytes,
                    entry.detail,
                );
            }
            if details.omitted_entries != 0 {
                eprintln!(
                    "cix: mixed_diagnostics_truncated={} (additional regional payload diagnostics)",
                    details.omitted_entries
                );
            }
            for omission in &details.provider_omissions {
                eprintln!("cix: mixed_provider_omission={omission}");
            }
        }
        if let TrialDetails::Native(details) = &trial.details {
            for detail in details {
                eprintln!(
                    "cix: native_candidate={} status={:?} archive_bytes={:?} reason={:?}",
                    detail.id, detail.status, detail.archive_bytes, detail.reason
                );
            }
        }
    }
}
