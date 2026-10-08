//! Bounded whole-archive BEST selection for ordinary named inputs.
//!
//! CIXG frames remain the streaming encoder.  For a regular, named file that
//! fits the declared memory limit, BEST can also compare a complete native
//! CIXG2 archive with complete CIXB1 backend envelopes.  This module never
//! changes the streaming path: it reads the source once, rejects a metadata
//! race, and owns every whole-input allocation explicitly.

use super::Opt;
use crate::core::engine::{
    check_interrupted, decode, encode_m6, encode_with_strategy, external, ioerr, EncodeOptions,
    MAX_BLOCK,
};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

/// Whole-package search is deliberately budgeted separately from the old
/// per-route portfolio deadline.  The budget is an effort policy, not a
/// container property: a future effort level can extend the search without
/// changing the archive format.  It scales modestly with input so that BEST
/// does not give a tiny file and a near-limit input the same opportunity.
/// Native-library calls remain non-preemptible; their memory admission is
/// therefore part of the candidate result and is reported to the caller.
fn native_whole_archive_budget(input_len: usize, level: u8) -> Duration {
    let base = match level {
        0..=1 => 60,
        2..=5 => 300,
        6..=8 => 600,
        _ => 900,
    };
    // BEST earns up to 45 extra minutes for a legal whole-file trial.
    // This is deliberately tied to search effort and input scale, rather
    // than being a universal hard-coded "native auto" timeout.
    let size_allowance = (input_len / (1024 * 1024)).saturating_mul(30).min(2700) as u64;
    Duration::from_secs(base + size_allowance)
}
const OUTPUT_SLACK: usize = 2 * 1024 * 1024;
const NATIVE_MINIMUM: usize = 16 * 1024 * 1024;
const COMPETITIVE_PREFIX_PRUNED: &str =
    "whole-archive candidate prefix cannot improve the incumbent";

#[derive(Debug)]
pub(super) struct SelectedArchive {
    pub archive: Vec<u8>,
    pub selected: String,
    pub evaluated: usize,
    pub omitted: Vec<String>,
    pub elapsed: Duration,
    pub search_budget: Duration,
}

/// Whole-archive candidates are legal for bounded named sources.  Expert
/// controls narrow the candidates below; they do not turn off BEST as a
/// whole.  In particular, `--route predictor` still gets a complete archive
/// trial and `--backend` still permits native framing selection.
pub(crate) fn eligible(o: &Opt) -> bool {
    let file_fits = fs::metadata(&o.input).is_ok_and(|metadata| {
        metadata.is_file()
            && metadata.len() <= external::MAX_INPUT as u64
            && usize::try_from(metadata.len()).is_ok_and(|length| {
                let cap = archive_cap(length);
                o.memory
                    >= length
                        .saturating_add(1)
                        .saturating_add(cap.saturating_mul(2))
                        .saturating_add(NATIVE_MINIMUM)
            })
    });
    o.level >= 9
        && !o.decode
        && !o.test
        && !o.list
        && o.input != "-"
        && !o.stream
        && o.flush_interval.is_none()
        && o.history == 0
        && o.portfolio.is_none()
        && o.external_backend.is_none()
        && matches!(o.format.as_str(), "auto" | "cixg1" | "cixg2")
        && file_fits
}

pub(crate) fn exclusion_reason(o: &Opt) -> Option<&'static str> {
    if o.level < 9 || o.decode || o.test || o.list {
        return None;
    }
    if o.input == "-" {
        Some("stdin/non-seekable input uses bounded CIXG streaming")
    } else if o.stream || o.flush_interval.is_some() {
        Some("--stream/--flush-interval requires bounded CIXG streaming")
    } else if fs::metadata(&o.input).is_err() || !fs::metadata(&o.input).is_ok_and(|m| m.is_file())
    {
        Some("whole-archive BEST requires a readable regular named file")
    } else if fs::metadata(&o.input).is_ok_and(|m| m.len() > external::MAX_INPUT as u64) {
        Some("input exceeds the bounded whole-archive BEST limit; using CIXG streaming")
    } else if fs::metadata(&o.input).is_ok_and(|m| {
        usize::try_from(m.len()).is_ok_and(|length| {
            o.memory
                < length
                    .saturating_add(1)
                    .saturating_add(archive_cap(length).saturating_mul(2))
                    .saturating_add(NATIVE_MINIMUM)
        })
    }) {
        Some("--memory cannot admit source and complete archive trials; using CIXG streaming")
    } else if o.history != 0 {
        Some("history requires the bounded streaming path")
    } else if o.portfolio.is_some() || o.external_backend.is_some() {
        Some("explicit portfolio/external selection already controls whole-archive candidates")
    } else if !matches!(o.format.as_str(), "auto" | "cixg1" | "cixg2") {
        Some("the selected container has its own encoder")
    } else {
        None
    }
}

fn same_identity(before: &Metadata, after: &Metadata) -> bool {
    if before.len() != after.len() {
        return false;
    }
    // A changed modification time catches replacements on all supported
    // platforms.  Unix inode/device identity catches replacement preserving
    // length and timestamp resolution.
    if before.modified().ok() != after.modified().ok() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn read_regular_once(path: &str) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(ioerr)?;
    let before = file.metadata().map_err(ioerr)?;
    if !before.is_file() {
        return Err("whole-archive BEST requires a regular named input file".into());
    }
    let declared = usize::try_from(before.len()).map_err(|_| "input is too large")?;
    if declared > external::MAX_INPUT {
        return Err(format!(
            "whole-archive BEST excludes input above {} MiB; using bounded CIXG streaming",
            external::MAX_INPUT / (1024 * 1024)
        ));
    }
    let cap = declared.checked_add(1).ok_or("input length overflow")?;
    // Reserve the single overflow-probe byte too.  `read_to_end` therefore
    // cannot geometrically grow the source while checking declared_len + 1.
    let mut data = Vec::with_capacity(cap);
    std::io::Read::by_ref(&mut file)
        .take(cap as u64)
        .read_to_end(&mut data)
        .map_err(ioerr)?;
    // Compare the opened descriptor as well as the path.  The descriptor
    // catches writes to the file while it is open; comparing the path catches
    // an atomic replacement between the read and the check.
    let after_descriptor = file.metadata().map_err(ioerr)?;
    let after_path = fs::metadata(path).map_err(ioerr)?;
    if data.len() != declared
        || !same_identity(&before, &after_descriptor)
        || !same_identity(&after_descriptor, &after_path)
    {
        return Err("input changed while BEST was reading it; refusing a raced archive".into());
    }
    Ok(data)
}

/// A strict output writer means a candidate cannot allocate or emit beyond its
/// admitted complete archive capacity.  `WriteZero` is classified as a normal
/// candidate omission by the caller, never as a corrupt archive.
struct CappedWriter {
    bytes: Vec<u8>,
    cap: usize,
    competitive_cap: Option<usize>,
}

impl CappedWriter {
    fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(cap),
            cap,
            competitive_cap: None,
        }
    }

    /// Keep the existing absolute admission cap separate from the exact
    /// competitive cap. A serialized archive only grows, so once its prefix
    /// is longer than a snapshotted incumbent it cannot win this stable tie
    /// policy. Equality remains materializable and is compared normally.
    fn with_competitive_cap(mut self, incumbent_len: usize) -> Self {
        self.competitive_cap = Some(incumbent_len);
        self
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for CappedWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > self.cap.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "whole-archive candidate output exceeded admission",
            ));
        }
        if self.competitive_cap.is_some_and(|incumbent_len| {
            data.len() > incumbent_len.saturating_sub(self.bytes.len())
        }) {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                COMPETITIVE_PREFIX_PRUNED,
            ));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn archive_cap(input_len: usize) -> usize {
    input_len
        .saturating_add(OUTPUT_SLACK)
        .min(external::MAX_PAYLOAD.saturating_add(47))
}

fn is_interruption(error: &str) -> bool {
    error.contains("interrupted") || error == crate::limits::INTERRUPTED_ERROR
}

fn is_resource_omission(error: &str) -> bool {
    error == crate::limits::DEADLINE_ERROR
        || error.contains(crate::limits::DEADLINE_ERROR)
        || error.contains("memory budget")
        || error.starts_with("whole-archive validation memory/resource limit")
        || error.starts_with("memory reservation")
        || error.starts_with("input/profiler buffers exceed --memory")
        || error.starts_with("CIXM6 history, source and candidate buffers exceed --memory")
        || error.starts_with("CIXM6 encoder needs --memory")
        || error.starts_with("mixture route needs")
        || error.starts_with("block needs about")
        || error == "whole-archive candidate output exceeded admission"
}

fn is_competitive_prefix_prune(error: &str) -> bool {
    error == COMPETITIVE_PREFIX_PRUNED || error.contains(COMPETITIVE_PREFIX_PRUNED)
}

// A direct BSC block has no stored fallback in its payload: libbsc returns
// NOT_COMPRESSIBLE when the transformed representation would lose. This is a
// normal candidate outcome; the always-present CIX raw candidate is the
// lossless fallback. Treat it as an observed rejection, not a BEST failure.
fn is_candidate_rejection(error: &str) -> bool {
    error.starts_with("BSC candidate rejected:") || error.starts_with("ZPAQ candidate rejected:")
}

struct CompareWriter<'a> {
    expected: &'a [u8],
    offset: usize,
    exact: bool,
}

impl<'a> CompareWriter<'a> {
    fn new(expected: &'a [u8]) -> Self {
        Self {
            expected,
            offset: 0,
            exact: true,
        }
    }
}

impl Write for CompareWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self.offset.saturating_add(bytes.len());
        if end > self.expected.len() || self.expected.get(self.offset..end) != Some(bytes) {
            self.exact = false;
        }
        self.offset = end;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn decode_budget(
    memory: usize,
    source_capacity: usize,
    incumbent_capacity: usize,
    archive_capacity: usize,
) -> Result<usize, String> {
    memory
        .checked_sub(source_capacity)
        .and_then(|rest| rest.checked_sub(incumbent_capacity))
        .and_then(|rest| rest.checked_sub(archive_capacity))
        // Both CIXG and CIXB1 decoding now charge their retained buffers and
        // native state against this remainder. A second arbitrary halving
        // would exclude valid model archives that fit the aggregate budget.
        .filter(|rest| *rest >= 128 * 1024)
        .ok_or_else(|| "whole-archive validation memory/resource limit exceeded".into())
}

fn validate(
    archive: &[u8],
    archive_capacity: usize,
    input: &Vec<u8>,
    incumbent_capacity: usize,
    memory: usize,
) -> Result<(), String> {
    let decoder_memory = decode_budget(
        memory,
        input.capacity(),
        incumbent_capacity,
        archive_capacity,
    )?;
    let mut restored = CompareWriter::new(input);
    decode(
        io::Cursor::new(archive),
        &mut restored,
        false,
        false,
        -1,
        decoder_memory,
    )?;
    if !restored.exact || restored.offset != input.len() {
        return Err("whole-archive candidate failed exact reconstruction".into());
    }
    Ok(())
}

fn consider(
    winner: &mut Vec<u8>,
    winner_name: &mut String,
    name: &str,
    archive: Vec<u8>,
    input: &Vec<u8>,
    memory: usize,
) -> Result<bool, String> {
    validate(
        &archive,
        archive.capacity(),
        input,
        winner.capacity(),
        memory,
    )?;
    if archive.len() < winner.len() {
        *winner = archive;
        *winner_name = name.into();
        Ok(true)
    } else {
        Ok(false)
    }
}

fn external_validation_admitted(
    descriptor: &external::CandidateDescriptor,
    source_capacity: usize,
    incumbent_capacity: usize,
    archive_capacity: usize,
    input_len: usize,
    memory: usize,
) -> Result<(), String> {
    let available = memory
        .checked_sub(source_capacity)
        .and_then(|rest| rest.checked_sub(incumbent_capacity))
        .and_then(|rest| rest.checked_sub(archive_capacity))
        .ok_or("whole-archive validation memory/resource limit exceeded")?;
    let required = descriptor.decoder_peak_bytes(input_len)?;
    if required > available {
        return Err(format!(
            "whole-archive validation memory/resource limit exceeded: {} needs {required} bytes, available {available}",
            descriptor.id
        ));
    }
    Ok(())
}

struct NativeSearch<'a> {
    data: &'a Vec<u8>,
    forced_route: Option<&'a str>,
    cixg_format: &'a str,
    cixg_block: u32,
    cixg_backend: &'a str,
    cap: usize,
    cixg_budget: Duration,
    m6_budget: Duration,
}

fn available_for_trial(o: &Opt, data: &Vec<u8>, winner: &Vec<u8>, cap: usize) -> Option<usize> {
    o.memory
        .checked_sub(data.capacity())
        .and_then(|rest| rest.checked_sub(winner.capacity()))
        .and_then(|rest| rest.checked_sub(cap))
        .filter(|available| *available >= NATIVE_MINIMUM)
}

struct TrialState<'a> {
    o: &'a Opt,
    winner: &'a mut Vec<u8>,
    winner_name: &'a mut String,
    evaluated: &'a mut usize,
    omitted: &'a mut Vec<String>,
    data: &'a Vec<u8>,
}

fn record_native_trial(
    state: &mut TrialState<'_>,
    name: String,
    output: CappedWriter,
    encoded: Result<(), String>,
) -> Result<(), String> {
    match encoded {
        Ok(()) => {
            *state.evaluated += 1;
            if state.o.explain || state.o.verbose {
                eprintln!(
                    "cix: packaging_candidate={name} archive_bytes={}",
                    output.bytes.len()
                );
            }
            match consider(
                state.winner,
                state.winner_name,
                &name,
                output.into_inner(),
                state.data,
                state.o.memory,
            ) {
                Ok(_) => Ok(()),
                Err(error) if is_resource_omission(&error) => {
                    state.omitted.push(format!("{name} ({error})"));
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        Err(error) if is_interruption(&error) => Err(error),
        Err(error) if is_competitive_prefix_prune(&error) => {
            state.omitted.push(format!(
                "{name} (exact archive prefix exceeded incumbent; cannot improve)"
            ));
            Ok(())
        }
        Err(error) if is_resource_omission(&error) || is_candidate_rejection(&error) => {
            state.omitted.push(format!("{name} ({error})"));
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn run_cixg_trials(
    o: &Opt,
    search: &NativeSearch<'_>,
    winner: &mut Vec<u8>,
    winner_name: &mut String,
    evaluated: &mut usize,
    omitted: &mut Vec<String>,
) -> Result<(), String> {
    let formats: Vec<&str> = if o.format == "auto" {
        vec!["cixg1", "cixg2"]
    } else {
        vec![o.format.as_str()]
    };
    let blocks: Vec<u32> = match o.block {
        Some(block) => vec![block],
        None if search.data.len() <= 16 * 1024 * 1024 => vec![16 * 1024, 32 * 1024, MAX_BLOCK],
        None => vec![MAX_BLOCK],
    };
    let trials: Vec<(&str, u32)> = formats
        .into_iter()
        .flat_map(|format| {
            blocks.iter().copied().filter_map(move |block| {
                (search.forced_route.is_none()
                    || format != search.cixg_format
                    || block != search.cixg_block)
                    .then_some((format, block))
            })
        })
        .collect();
    let started = Instant::now();
    for (index, (format, block)) in trials.iter().copied().enumerate() {
        check_interrupted()?;
        let left = trials_len_remaining(
            index,
            &trials,
            search.cixg_budget,
            started,
            format,
            block,
            omitted,
        );
        let Some(budget) = left else { continue };
        let Some(available) = available_for_trial(o, search.data, winner, search.cap) else {
            omitted.push(format!(
                "{format}:native-auto:block={block} (insufficient aggregate memory)"
            ));
            continue;
        };
        // Snapshot before starting the candidate. A deadline may make later
        // candidate availability differ after this exact prune, but this
        // candidate itself cannot beat this stable incumbent once its emitted
        // prefix exceeds it.
        let mut output = CappedWriter::new(search.cap).with_competitive_cap(winner.len());
        let guard = crate::limits::DeadlineGuard::new(budget);
        let encoded = encode_with_strategy(
            io::Cursor::new(search.data.as_slice()),
            &mut output,
            EncodeOptions {
                block,
                level: o.level,
                forced: search.forced_route,
                backend: search.cixg_backend,
                backend_set: o.backend_set,
                format,
                input_fd: -1,
                flush_interval: None,
                memory: available,
                workers: o.threads,
                explain: o.explain,
                verbose: o.verbose,
                strategy: &o.parallelism,
            },
        );
        drop(guard);
        let name = format!(
            "{format}:{}:block={block}",
            search.forced_route.unwrap_or("native-auto")
        );
        record_native_trial(
            &mut TrialState {
                o,
                winner,
                winner_name,
                evaluated,
                omitted,
                data: search.data,
            },
            name,
            output,
            encoded,
        )?;
    }
    Ok(())
}

fn trials_len_remaining(
    index: usize,
    trials: &[(&str, u32)],
    budget: Duration,
    started: Instant,
    format: &str,
    block: u32,
    omitted: &mut Vec<String>,
) -> Option<Duration> {
    let remaining = budget.saturating_sub(started.elapsed());
    let per_trial = remaining.checked_div((trials.len() - index) as u32);
    match per_trial.filter(|duration| !duration.is_zero()) {
        Some(duration) => Some(duration),
        None => {
            omitted.push(format!(
                "{format}:native-auto:block={block} (CIXG family deadline allocation exhausted)"
            ));
            None
        }
    }
}

fn run_m6_trials(
    o: &Opt,
    search: &NativeSearch<'_>,
    winner: &mut Vec<u8>,
    winner_name: &mut String,
    evaluated: &mut usize,
    omitted: &mut Vec<String>,
) -> Result<(), String> {
    let blocks: Vec<usize> = if o.format != "auto"
        || search.forced_route.is_some()
        || o.backend_set
        || o.independent_blocks
    {
        vec![]
    } else if let Some(block) = o.block {
        vec![block as usize]
    } else if search.data.len() <= 16 * 1024 * 1024 {
        vec![16 * 1024, 32 * 1024, MAX_BLOCK as usize]
    } else {
        vec![MAX_BLOCK as usize]
    };
    if blocks.is_empty() && o.format == "auto" {
        omitted.push(
            "cixm6:native-auto (incompatible expert container/route/backend/block constraint)"
                .into(),
        );
    }
    let started = Instant::now();
    for (index, block_size) in blocks.iter().copied().enumerate() {
        check_interrupted()?;
        let remaining = search.m6_budget.saturating_sub(started.elapsed());
        let Some(budget) = remaining
            .checked_div((blocks.len() - index) as u32)
            .filter(|duration| !duration.is_zero())
        else {
            omitted.push(format!(
                "cixm6:native-auto:block={block_size} (CIXM6 family deadline allocation exhausted)"
            ));
            continue;
        };
        let Some(available) = available_for_trial(o, search.data, winner, search.cap) else {
            omitted.push(format!(
                "cixm6:native-auto:block={block_size} (insufficient aggregate memory)"
            ));
            continue;
        };
        let mut output = CappedWriter::new(search.cap).with_competitive_cap(winner.len());
        let guard = crate::limits::DeadlineGuard::new(budget);
        let encoded = encode_m6(
            io::Cursor::new(search.data.as_slice()),
            &mut output,
            block_size,
            None,
            o.level,
            available,
        );
        drop(guard);
        let name = format!("cixm6:native-auto:block={block_size}");
        record_native_trial(
            &mut TrialState {
                o,
                winner,
                winner_name,
                evaluated,
                omitted,
                data: search.data,
            },
            name,
            output,
            encoded,
        )?;
    }
    Ok(())
}

fn run_external_trials(
    o: &Opt,
    data: &Vec<u8>,
    winner: &mut Vec<u8>,
    winner_name: &mut String,
    evaluated: &mut usize,
    omitted: &mut Vec<String>,
    allowed: bool,
) -> Result<(), String> {
    if !allowed {
        omitted.push("cixb1:external candidates (incompatible expert native constraint)".into());
        return Ok(());
    }
    for descriptor in external::best_candidates() {
        run_external_trial(o, descriptor, data, winner, winner_name, evaluated, omitted)?;
    }
    Ok(())
}

fn run_external_trial(
    o: &Opt,
    descriptor: &external::CandidateDescriptor,
    data: &Vec<u8>,
    winner: &mut Vec<u8>,
    winner_name: &mut String,
    evaluated: &mut usize,
    omitted: &mut Vec<String>,
) -> Result<(), String> {
    check_interrupted()?;
    let available = o
        .memory
        .saturating_sub(data.capacity())
        .saturating_sub(winner.capacity());
    if let Err(omission) = descriptor.admit(data.len(), available) {
        omitted.push(format!("{} ({})", descriptor.id, omission.reason));
        return Ok(());
    }
    let archive = match descriptor.full_archive_encode(data) {
        Ok(archive) => archive,
        Err(error) if is_interruption(&error) => return Err(error),
        Err(error) if is_resource_omission(&error) || is_candidate_rejection(&error) => {
            omitted.push(format!("{} ({error})", descriptor.id));
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    *evaluated += 1;
    report_external_archive(o, descriptor, archive.len())?;
    match validate_and_consider_external(o, descriptor, data, winner, winner_name, archive) {
        Ok(()) => Ok(()),
        Err(error) if is_resource_omission(&error) => {
            omitted.push(format!("{} ({error})", descriptor.id));
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn report_external_archive(
    o: &Opt,
    descriptor: &external::CandidateDescriptor,
    archive_len: usize,
) -> Result<(), String> {
    if o.explain || o.verbose {
        eprintln!(
            "cix: packaging_candidate={} backend={} version={} archive_bytes={archive_len}",
            descriptor.id,
            descriptor.backend,
            external::backend_version(descriptor.backend)?.replace(char::is_whitespace, "_"),
        );
    }
    Ok(())
}

fn validate_and_consider_external(
    o: &Opt,
    descriptor: &external::CandidateDescriptor,
    data: &Vec<u8>,
    winner: &mut Vec<u8>,
    winner_name: &mut String,
    archive: Vec<u8>,
) -> Result<(), String> {
    external_validation_admitted(
        descriptor,
        data.capacity(),
        winner.capacity(),
        archive.capacity(),
        data.len(),
        o.memory,
    )?;
    consider(winner, winner_name, descriptor.id, archive, data, o.memory)?;
    Ok(())
}

/// Select a complete archive.  The source is read once before candidate work;
/// candidate execution is serial because native CIXG2 may use `--threads` and
/// backend libraries have coarse cancellation points.
pub(super) fn select(o: &Opt) -> Result<SelectedArchive, String> {
    let started = Instant::now();
    let data = read_regular_once(&o.input)?;
    let search_budget = native_whole_archive_budget(data.len(), o.level);
    // Reserve time for both native container families.  CIXG cannot consume
    // CIXM6's allocation merely because it happens to run first.
    let cixg_budget = search_budget / 2;
    let m6_budget = search_budget.saturating_sub(cixg_budget);
    check_interrupted()?;
    let cap = archive_cap(data.len());
    let fallback_available = o
        .memory
        .checked_sub(data.capacity())
        .and_then(|rest| rest.checked_sub(cap))
        .ok_or("whole-archive BEST cannot admit source and raw fallback")?;
    if fallback_available < NATIVE_MINIMUM {
        return Err(format!(
            "whole-archive BEST requires at least {} bytes of --memory for this input; using bounded CIXG streaming",
            data.capacity().saturating_add(cap).saturating_add(NATIVE_MINIMUM)
        ));
    }

    let forced_route = o.route.as_deref().filter(|route| *route != "auto");
    let cixg_format = if o.format == "auto" {
        "cixg2"
    } else {
        o.format.as_str()
    };
    let cixg_block = o.block.unwrap_or(MAX_BLOCK);
    let cixg_backend = if o.backend_set {
        o.backend.as_str()
    } else {
        "hybrid"
    };

    // Establish a legal incumbent through the same bounded writer used for
    // every native trial.  A route override constrains even this initial
    // candidate, so BEST never silently replaces `--route predictor` with
    // raw bytes.
    let mut raw_output = CappedWriter::new(cap);
    encode_with_strategy(
        io::Cursor::new(data.as_slice()),
        &mut raw_output,
        EncodeOptions {
            block: cixg_block,
            level: o.level,
            forced: forced_route.or(Some("raw")),
            backend: cixg_backend,
            backend_set: o.backend_set,
            format: cixg_format,
            input_fd: -1,
            flush_interval: None,
            memory: fallback_available,
            workers: o.threads,
            explain: false,
            verbose: false,
            strategy: &o.parallelism,
        },
    )?;
    let mut winner = raw_output.into_inner();
    validate(&winner, winner.capacity(), &data, 0, o.memory)?;
    let mut winner_name = format!("{cixg_format}:{}-baseline", forced_route.unwrap_or("raw"));
    if o.explain || o.verbose {
        eprintln!(
            "cix: packaging_candidate={winner_name} archive_bytes={}",
            winner.len()
        );
    }
    let mut evaluated = 1usize;
    let mut omitted = Vec::new();

    let search = NativeSearch {
        data: &data,
        forced_route,
        cixg_format,
        cixg_block,
        cixg_backend,
        cap,
        cixg_budget,
        m6_budget,
    };
    run_cixg_trials(
        o,
        &search,
        &mut winner,
        &mut winner_name,
        &mut evaluated,
        &mut omitted,
    )?;
    run_m6_trials(
        o,
        &search,
        &mut winner,
        &mut winner_name,
        &mut evaluated,
        &mut omitted,
    )?;
    let external_allowed = o.format == "auto"
        && forced_route.is_none()
        && !o.backend_set
        && o.block.is_none()
        && !o.independent_blocks;
    run_external_trials(
        o,
        &data,
        &mut winner,
        &mut winner_name,
        &mut evaluated,
        &mut omitted,
        external_allowed,
    )?;
    validate(&winner, winner.capacity(), &data, 0, o.memory)?;
    Ok(SelectedArchive {
        archive: winner,
        selected: winner_name,
        evaluated,
        omitted,
        elapsed: started.elapsed(),
        search_budget,
    })
}

pub(super) fn encode<W: Write>(o: &Opt, mut dst: W) -> Result<(), String> {
    let selected = select(o)?;
    dst.write_all(&selected.archive).map_err(ioerr)?;
    dst.flush().map_err(ioerr)?;
    if o.explain || o.verbose || !selected.omitted.is_empty() {
        eprintln!(
            "cix: effort=best whole_archive=yes selected={} archive_bytes={} candidates_evaluated={} search_seconds={:.3} native_search_budget_seconds={} external_one_shot_deadline=not_preemptible omitted={}",
            selected.selected,
            selected.archive.len(),
            selected.evaluated,
            selected.elapsed.as_secs_f64(),
            selected.search_budget.as_secs(),
            if selected.omitted.is_empty() { "none".to_string() } else { selected.omitted.join(",") },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn competitive_cap_prunes_only_after_the_incumbent_boundary() {
        let mut output = CappedWriter::new(32).with_competitive_cap(3);
        output.write_all(b"abc").unwrap();
        let error = output.write_all(b"d").unwrap_err();
        assert_eq!(error.to_string(), COMPETITIVE_PREFIX_PRUNED);
        assert_eq!(output.bytes, b"abc");
    }

    #[test]
    fn competitive_cap_allows_an_exact_tie_to_materialize() {
        let mut output = CappedWriter::new(32).with_competitive_cap(3);
        output.write_all(b"abc").unwrap();
        assert_eq!(output.into_inner(), b"abc");
    }

    #[test]
    fn native_cixg_losing_prefix_is_omitted_without_replacing_the_winner() {
        let data = b"native archive prefix prune\n".repeat(16);
        let mut winner = vec![0x5a];
        let original_winner = winner.clone();
        let mut winner_name = "incumbent".to_string();
        let mut evaluated = 0;
        let mut omitted = Vec::new();
        let mut output =
            CappedWriter::new(archive_cap(data.len())).with_competitive_cap(winner.len());
        let encoded = encode_with_strategy(
            io::Cursor::new(data.as_slice()),
            &mut output,
            EncodeOptions {
                block: 16 * 1024,
                level: 1,
                forced: Some("raw"),
                backend: "hybrid",
                backend_set: false,
                format: "cixg1",
                input_fd: -1,
                flush_interval: None,
                memory: 64 << 20,
                workers: 1,
                explain: false,
                verbose: false,
                strategy: "blocks",
            },
        );
        assert!(is_competitive_prefix_prune(encoded.as_ref().unwrap_err()));
        let o = crate::cli::Opt {
            memory: 64 << 20,
            ..Default::default()
        };
        record_native_trial(
            &mut TrialState {
                o: &o,
                winner: &mut winner,
                winner_name: &mut winner_name,
                evaluated: &mut evaluated,
                omitted: &mut omitted,
                data: &data,
            },
            "cixg1:raw:block=16384".into(),
            output,
            encoded,
        )
        .unwrap();
        assert_eq!(winner, original_winner);
        assert_eq!(winner_name, "incumbent");
        assert_eq!(evaluated, 0);
        assert_eq!(
            omitted,
            vec!["cixg1:raw:block=16384 (exact archive prefix exceeded incumbent; cannot improve)"]
        );
    }

    #[test]
    fn validation_uses_the_charged_aggregate_remainder() {
        assert_eq!(
            decode_budget(512 << 20, 1 << 20, 2 << 20, 3 << 20).unwrap(),
            506 << 20
        );
        assert!(decode_budget(5 << 20, 1 << 20, 2 << 20, 3 << 20).is_err());
    }

    #[test]
    fn validation_accepts_a_large_native_mixer_that_fits_the_budget() {
        use sha2::{Digest, Sha256};
        let data = vec![b'x'; MAX_BLOCK as usize];
        let cap = archive_cap(data.len());
        let mut output = CappedWriter::new(cap);
        // Materialize this bounded model directly: a debug selector can
        // legitimately time out the trial and choose raw, which would not
        // exercise the decoder's large-model admission being tested.
        let payload =
            crate::core::engine::fixedmix::encode_with_history_eta(&data, &[], 6).unwrap();
        let hash: [u8; 32] = Sha256::digest(&data).into();
        output.write_all(crate::core::engine::MAGIC_V2).unwrap();
        output.write_all(&MAX_BLOCK.to_le_bytes()).unwrap();
        output.write_all(&0u32.to_le_bytes()).unwrap();
        crate::core::engine::put_frame(
            &mut output,
            9,
            data.len() as u32,
            payload.len() as u32,
            &hash,
        )
        .unwrap();
        output.write_all(&payload).unwrap();
        crate::core::engine::put_frame(&mut output, 255, 0, 0, &hash).unwrap();
        let archive = output.into_inner();
        assert_eq!(archive[13], 9, "fixture actually uses the mixer");
        validate(&archive, archive.capacity(), &data, 1 << 20, 512 << 20).unwrap();
    }

    #[test]
    fn cixm6_complete_candidate_exactly_decodes_before_promotion() {
        // This is deliberately a complete CIXM6 stream (header, frames and
        // footer), rather than a route-payload test.  BEST calls the same
        // encoder and the same archive decoder before retaining the candidate.
        let data = b"CIXM6 packaging competition\n".repeat(192);
        let cap = archive_cap(data.len());
        let mut output = CappedWriter::new(cap);
        crate::core::engine::encode_m6(
            io::Cursor::new(data.as_slice()),
            &mut output,
            16 * 1024,
            None,
            9,
            128 << 20,
        )
        .unwrap();
        let archive = output.into_inner();
        assert!(archive.starts_with(b"CIXM6"));
        validate(&archive, archive.capacity(), &data, 1 << 20, 256 << 20).unwrap();
    }

    #[test]
    fn predictor_override_remains_a_complete_native_best_candidate() {
        // A route override constrains the route dimension only.  It must not
        // bypass whole-archive validation or silently substitute raw/CIXB1.
        let path = std::env::temp_dir().join(format!(
            "cix-portfolio-auto-predictor-{}-{}.bin",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(
            &path,
            b"predictable record\npredictable record\n".repeat(192),
        )
        .unwrap();
        let selected = select(&crate::cli::Opt {
            level: 9,
            threads: 1,
            memory: 512 << 20,
            input: path.to_string_lossy().into_owned(),
            route: Some("predictor".into()),
            backend: "hybrid".into(),
            format: "auto".into(),
            parallelism: "blocks".into(),
            ..Default::default()
        })
        .unwrap();
        let _ = std::fs::remove_file(path);
        assert!(selected.selected.starts_with("cixg2:predictor"));
        assert!(selected
            .omitted
            .iter()
            .any(|entry| entry.starts_with("cixb1:")));
    }
}
