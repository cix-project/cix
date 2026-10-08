//! Process-free whole-archive native portfolio.
//!
//! This is the library counterpart of the historical BEST archive search.  It
//! accepts bytes already owned by its caller: it neither opens paths nor
//! consults environment state, installs handlers, starts processes, or writes
//! diagnostics.  The result carries every attempted family and every bounded
//! omission so an embedding can make an honest policy decision.

use crate::core::{
    self,
    engine::{
        check_interrupted, decode, encode_m6, encode_with_strategy, EncodeOptions, MAX_BLOCK,
    },
    external,
};
use std::{
    io::{self, Write},
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

const MIB: usize = 1 << 20;
const OUTPUT_SLACK: usize = 2 * MIB;
const MIN_REMAINDER: usize = 16 * MIB;
const MAX_INPUT: usize = external::MAX_INPUT;
const MAX_DIAGNOSTICS: usize = 64;
// A BEST result retains its baseline plus fourteen CIXB1 diagnostics.  Keep
// explicit block constraints bounded so the native family records remain
// complete rather than silently falling off the diagnostics cap.
const MAX_EXPLICIT_BLOCKS: usize = 16;
const PREFIX_PRUNED: &str = "whole-archive candidate prefix cannot improve the incumbent";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativePortfolioProfile {
    Fast,
    Default,
    Best,
}

#[derive(Clone, Debug)]
pub struct NativePortfolioOptions {
    pub profile: NativePortfolioProfile,
    /// Aggregate budget.  The selector charges caller source bytes, retained
    /// winner capacity, trial output capacity and codec/backend reservations.
    pub memory_bytes: usize,
    /// Maximum serialized archive capacity for any retained or trial result.
    pub output_bytes: usize,
    pub workers: usize,
    /// Absolute caller deadline. Native one-shot CIXB1 backends observe it
    /// before and after their call, so expiry cannot preempt a backend ABI.
    pub deadline: Option<Instant>,
    pub cancellation: Option<Arc<AtomicBool>>,
}

/// Explicit whole-input constraints. Empty fields retain the historical
/// automatic portfolio; the CLI maps only controls with a native wire meaning.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativePortfolioConstraints {
    pub formats: Option<Vec<String>>,
    pub blocks: Option<Vec<u32>>,
    pub backend: Option<String>,
    pub effort: Option<u8>,
    pub strategy: Option<String>,
}

impl NativePortfolioConstraints {
    /// Only an explicit container format excludes other whole-input wires.
    /// Block and backend values tune native candidates without constraining
    /// independent automatic candidates.
    pub fn forces_native_container(&self) -> bool {
        self.formats.is_some()
    }
    fn allows(&self, format: &str) -> bool {
        if self.backend.as_ref().is_some_and(|backend| {
            format == "cixm6"
                || (format == "cixg1"
                    && (backend == "huffman" || backend.starts_with("context-range-")))
        }) {
            return false;
        }
        self.formats
            .as_ref()
            .is_none_or(|formats| formats.iter().any(|value| value == format))
    }
    fn block_list(&self, input_len: usize) -> Vec<u32> {
        self.blocks.clone().unwrap_or_else(|| {
            if input_len <= 16 * MIB {
                vec![16 * 1024, 32 * 1024, MAX_BLOCK]
            } else {
                vec![MAX_BLOCK]
            }
        })
    }
    fn backend(&self) -> &str {
        self.backend.as_deref().unwrap_or("hybrid")
    }
    fn effort(&self, fallback: u8) -> Result<u8, String> {
        let effort = self.effort.unwrap_or(fallback);
        if (1..=9).contains(&effort) {
            Ok(effort)
        } else {
            Err("native effort must be 1 through 9".into())
        }
    }
    fn strategy(&self) -> Result<&str, String> {
        match self.strategy.as_deref().unwrap_or("blocks") {
            "blocks" | "candidates" => Ok(self.strategy.as_deref().unwrap_or("blocks")),
            _ => Err("native strategy must be blocks or candidates".into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativePortfolioStatus {
    Completed,
    Omitted,
    Selected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePortfolioCandidate {
    pub id: String,
    pub status: NativePortfolioStatus,
    pub archive_bytes: Option<usize>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NativePortfolioResult {
    pub archive: Vec<u8>,
    pub selected: String,
    pub candidates: Vec<NativePortfolioCandidate>,
    pub source_bytes_charged: usize,
    pub retained_output_capacity: usize,
    pub search_budget: Duration,
    pub elapsed: Duration,
}

impl NativePortfolioOptions {
    fn validate(&self, input_len: usize) -> Result<(), String> {
        if input_len > MAX_INPUT {
            return Err("native whole-archive input exceeds 128 MiB admission".into());
        }
        if self.memory_bytes == 0 || self.output_bytes == 0 || self.workers == 0 {
            return Err(
                "native portfolio requires non-zero memory, output and worker limits".into(),
            );
        }
        Ok(())
    }
}

fn archive_cap(n: usize) -> usize {
    n.saturating_add(OUTPUT_SLACK)
        .min(external::MAX_PAYLOAD.saturating_add(47))
}
fn best_fallback_cap(options: &NativePortfolioOptions, input_len: usize) -> Result<usize, String> {
    let cap = archive_cap(input_len);
    if options.output_bytes < cap {
        Err("BEST output limit cannot admit the raw CIXG2 fallback".into())
    } else {
        Ok(cap)
    }
}
fn whole_budget(n: usize) -> Duration {
    // The historical level-9 policy: 15 minutes plus up to 45 minutes scaled
    // by input.  The two native families each retain half of this allocation.
    Duration::from_secs(900 + (n / MIB).saturating_mul(30).min(2700) as u64)
}
fn stopped(o: &NativePortfolioOptions) -> bool {
    o.cancellation
        .as_ref()
        .is_some_and(|v| v.load(std::sync::atomic::Ordering::Acquire))
        || o.deadline.is_some_and(|d| Instant::now() >= d)
}
fn remaining(
    o: &NativePortfolioOptions,
    family: Duration,
    started: Instant,
    slots: usize,
) -> Option<Duration> {
    let local = family.saturating_sub(started.elapsed());
    let global = o
        .deadline
        .map(|d| d.saturating_duration_since(Instant::now()))
        .unwrap_or(local);
    local
        .min(global)
        .checked_div(slots as u32)
        .filter(|v| !v.is_zero())
}

#[derive(Clone, Debug)]
enum NativeTrialKind {
    Cixg {
        format: &'static str,
        block: u32,
        backend: String,
        backend_set: bool,
        effort: u8,
        strategy: String,
    },
    M6 {
        block: usize,
        effort: u8,
    },
}

#[derive(Clone, Debug)]
struct NativeTrialSpec {
    order: usize,
    id: String,
    budget: Duration,
    kind: NativeTrialKind,
}

/// Return a fixed per-wave budget.  Unlike completion-driven allocations,
/// this is independent of which worker returns first.  A family can consume
/// at most its historical half-budget when every wave reaches its deadline.
fn family_trial_budget(
    options: &NativePortfolioOptions,
    family: Duration,
    started: Instant,
    trial_count: usize,
    workers: usize,
) -> Option<Duration> {
    if trial_count == 0 {
        return None;
    }
    let waves = trial_count.checked_add(workers.checked_sub(1)?)? / workers;
    remaining(options, family, started, waves)
}

/// The outer selector already reserves `options.workers` for its single
/// native plan.  This local admission divides that reservation among native
/// trials: source and incumbent are retained once; every concurrent trial
/// reserves its full writer cap and a disjoint codec workspace.
fn family_admission(
    options: &NativePortfolioOptions,
    source: usize,
    winner: usize,
    trial_count: usize,
) -> Option<(usize, usize)> {
    let maximum = options.workers.min(trial_count);
    for slots in (1..=maximum).rev() {
        let Some(outputs) = options.output_bytes.checked_mul(slots) else {
            continue;
        };
        let Some(remaining) = options
            .memory_bytes
            .checked_sub(source)
            .and_then(|value| value.checked_sub(winner))
            .and_then(|value| value.checked_sub(outputs))
        else {
            continue;
        };
        let memory = remaining / slots;
        if memory >= MIN_REMAINDER {
            return Some((slots, memory));
        }
    }
    None
}

/// Run fixed native trial specifications in bounded waves.  Results are
/// returned in canonical specification order, not worker completion order.
/// The closure receives a disjoint per-trial codec budget and must not start
/// further worker pools.
fn schedule_native_family<T, F, A>(
    specs: Vec<NativeTrialSpec>,
    options: &NativePortfolioOptions,
    source: usize,
    winner: usize,
    run: F,
    mut accept: A,
) -> Result<(), String>
where
    T: Send,
    F: Fn(&NativeTrialSpec, usize) -> Result<T, String> + Sync,
    A: FnMut(Vec<(NativeTrialSpec, Result<T, String>)>) -> Result<(), String>,
{
    if specs.is_empty() {
        return Ok(());
    }
    let Some((slots, memory)) = family_admission(options, source, winner, specs.len()) else {
        return accept(
            specs
                .into_iter()
                .map(|spec| {
                    (
                        spec,
                        Err(
                            "source, retained output and trial output exceed memory admission"
                                .into(),
                        ),
                    )
                })
                .collect(),
        );
    };
    let context = crate::limits::capture_context();
    std::thread::scope(|scope| -> Result<(), String> {
        use std::sync::mpsc;

        let (sender, receiver) = mpsc::channel();
        let mut pending = specs.into_iter();
        loop {
            if stopped(options) {
                return Err("native portfolio cancelled or deadline exceeded".into());
            }
            let mut handles = Vec::new();
            for _ in 0..slots {
                let Some(spec) = pending.next() else {
                    break;
                };
                let tx = sender.clone();
                let child_context = context.clone();
                let task = &run;
                handles.push(scope.spawn(move || {
                    let _context = crate::limits::install_context(&child_context);
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        task(&spec, memory)
                    }))
                    .unwrap_or_else(|_| Err("native trial worker panicked".into()));
                    let _ = tx.send((spec, result));
                }));
            }
            if handles.is_empty() {
                break;
            }
            let mut outcomes = Vec::with_capacity(handles.len());
            for _ in 0..handles.len() {
                outcomes.push(
                    receiver
                        .recv()
                        .map_err(|_| "native trial scheduler channel closed")?,
                );
            }
            for handle in handles {
                handle.join().map_err(|_| "native trial worker panicked")?;
            }
            outcomes.sort_by_key(|(spec, _)| spec.order);
            // `accept` consumes this wave before another one launches, so
            // successful Vec results cannot remain retained across waves.
            accept(outcomes)?;
            // Do not launch another wave after an already-running trial has
            // observed caller cancellation or the absolute deadline.
            if stopped(options) {
                return Err("native portfolio cancelled or deadline exceeded".into());
            }
        }
        Ok(())
    })
}
fn omission(
    out: &mut Vec<NativePortfolioCandidate>,
    id: impl Into<String>,
    reason: impl Into<String>,
) {
    if out.len() < MAX_DIAGNOSTICS {
        out.push(NativePortfolioCandidate {
            id: id.into(),
            status: NativePortfolioStatus::Omitted,
            archive_bytes: None,
            reason: Some(reason.into()),
        });
    }
}
fn completed(out: &mut Vec<NativePortfolioCandidate>, id: impl Into<String>, bytes: usize) {
    if out.len() < MAX_DIAGNOSTICS {
        out.push(NativePortfolioCandidate {
            id: id.into(),
            status: NativePortfolioStatus::Completed,
            archive_bytes: Some(bytes),
            reason: None,
        });
    }
}
fn resource_error(error: &str) -> bool {
    error.contains("deadline")
        || error.contains("memory")
        || error.contains("output")
        || error.contains(PREFIX_PRUNED)
}
fn candidate_rejection(error: &str) -> bool {
    error.starts_with("BSC candidate rejected:") || error.starts_with("ZPAQ candidate rejected:")
}

struct CappedWriter {
    bytes: Vec<u8>,
    cap: usize,
    incumbent: Option<usize>,
}
impl CappedWriter {
    fn new(cap: usize, incumbent: Option<usize>) -> Result<Self, String> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(cap)
            .map_err(|_| "whole-archive output reservation allocation failed".to_string())?;
        Ok(Self {
            bytes,
            cap,
            incumbent,
        })
    }
}
impl Write for CappedWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.len() > self.cap.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("whole-archive output limit exceeded"));
        }
        if self
            .incumbent
            .is_some_and(|n| input.len() > n.saturating_sub(self.bytes.len()))
        {
            return Err(io::Error::other(PREFIX_PRUNED));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct CompareWriter<'a> {
    expected: &'a [u8],
    offset: usize,
    exact: bool,
}
impl Write for CompareWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self.offset.saturating_add(bytes.len());
        self.exact &=
            end <= self.expected.len() && self.expected.get(self.offset..end) == Some(bytes);
        self.offset = end;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn available(
    o: &NativePortfolioOptions,
    source: usize,
    winner: usize,
    trial: usize,
) -> Option<usize> {
    o.memory_bytes
        .checked_sub(source)?
        .checked_sub(winner)?
        .checked_sub(trial)
        .filter(|n| *n >= MIN_REMAINDER)
}
fn validate(
    archive: &[u8],
    archive_capacity: usize,
    input: &[u8],
    o: &NativePortfolioOptions,
    retained: usize,
) -> Result<(), String> {
    let memory = available(o, input.len(), retained, archive_capacity)
        .ok_or("validation memory admission")?;
    validate_with_memory(archive, input, memory)
}

fn validate_with_memory(archive: &[u8], input: &[u8], memory: usize) -> Result<(), String> {
    let mut restored = CompareWriter {
        expected: input,
        offset: 0,
        exact: true,
    };
    decode(
        io::Cursor::new(archive),
        &mut restored,
        false,
        false,
        -1,
        memory,
    )?;
    if restored.exact && restored.offset == input.len() {
        check_interrupted()?;
        Ok(())
    } else {
        Err("whole-archive candidate failed exact reconstruction".into())
    }
}
fn consider(
    id: String,
    candidate: Vec<u8>,
    input: &[u8],
    o: &NativePortfolioOptions,
    winner: &mut Vec<u8>,
    name: &mut String,
    diagnostics: &mut Vec<NativePortfolioCandidate>,
) -> Result<(), String> {
    if candidate.len() > o.output_bytes || candidate.capacity() > o.output_bytes {
        omission(diagnostics, id, "candidate exceeded output reservation");
        return Ok(());
    }
    validate(
        &candidate,
        candidate.capacity(),
        input,
        o,
        winner.capacity(),
    )?;
    completed(diagnostics, id.clone(), candidate.len());
    if candidate.len() < winner.len() {
        *winner = candidate;
        *name = id;
    }
    Ok(())
}
struct NativeTrialOutput {
    candidate: Vec<u8>,
}

fn run_native_trial(
    input: &[u8],
    o: &NativePortfolioOptions,
    winner_len: usize,
    memory: usize,
    spec: &NativeTrialSpec,
) -> Result<NativeTrialOutput, String> {
    check_interrupted()?;
    let mut writer = CappedWriter::new(o.output_bytes, Some(winner_len))?;
    let guard = crate::limits::DeadlineGuard::new(spec.budget);
    let result = match &spec.kind {
        NativeTrialKind::Cixg {
            format,
            block,
            backend,
            backend_set,
            effort,
            strategy,
        } => encode_with_strategy(
            io::Cursor::new(input),
            &mut writer,
            EncodeOptions {
                block: *block,
                level: *effort,
                forced: None,
                backend,
                backend_set: *backend_set,
                format,
                input_fd: -1,
                flush_interval: None,
                memory,
                // The family scheduler owns parallelism.  Starting a block
                // worker pool here would exceed its aggregate reservation.
                workers: 1,
                explain: false,
                verbose: false,
                strategy,
            },
        ),
        NativeTrialKind::M6 { block, effort } => encode_m6(
            io::Cursor::new(input),
            &mut writer,
            *block,
            None,
            *effort,
            memory,
        ),
    };
    let terminal = check_interrupted();
    drop(guard);
    if let Err(error) = terminal {
        if stopped(o) {
            return Err("native portfolio cancelled or deadline exceeded".into());
        }
        return Err(error);
    }
    match result {
        Ok(()) => {
            // Validation reuses this worker's disjoint codec workspace while
            // other running trials retain their own reservations.  That lets
            // the scheduler drop a fully checked result before the next wave.
            validate_with_memory(&writer.bytes, input, memory)?;
            Ok(NativeTrialOutput {
                candidate: writer.bytes,
            })
        }
        Err(e) => Err(e),
    }
}

fn run_native_family(
    input: &[u8],
    o: &NativePortfolioOptions,
    winner: &mut Vec<u8>,
    selected: &mut String,
    diagnostics: &mut Vec<NativePortfolioCandidate>,
    specs: Vec<NativeTrialSpec>,
) -> Result<(), String> {
    // Preserve serial incumbent eligibility when the caller has one worker:
    // later encodes see an earlier improved winner and may be prefix-pruned.
    if o.workers == 1 {
        for spec in specs {
            let winner_len = winner.len();
            let winner_capacity = winner.capacity();
            schedule_native_family(
                vec![spec],
                o,
                input.len(),
                winner_capacity,
                |spec, memory| run_native_trial(input, o, winner_len, memory, spec),
                |outcomes| accept_native_outcomes(outcomes, o, winner, selected, diagnostics),
            )?;
        }
        return Ok(());
    }
    let winner_len = winner.len();
    // A completed CappedWriter may become the winner with capacity equal to
    // the full declared output cap. Reserve that worst case for every wave;
    // otherwise a result accepted after wave one could enlarge the retained
    // winner beyond the admission calculated for wave two.
    let winner_capacity = o.output_bytes;
    schedule_native_family(
        specs,
        o,
        input.len(),
        winner_capacity,
        |spec, memory| run_native_trial(input, o, winner_len, memory, spec),
        |outcomes| accept_native_outcomes(outcomes, o, winner, selected, diagnostics),
    )
}

fn accept_native_outcomes(
    outcomes: Vec<(NativeTrialSpec, Result<NativeTrialOutput, String>)>,
    o: &NativePortfolioOptions,
    winner: &mut Vec<u8>,
    selected: &mut String,
    diagnostics: &mut Vec<NativePortfolioCandidate>,
) -> Result<(), String> {
    if stopped(o) {
        return Err("native portfolio cancelled or deadline exceeded".into());
    }
    for (spec, result) in outcomes {
        match result {
            Ok(output) => {
                consider_validated(spec.id, output.candidate, o, winner, selected, diagnostics)
            }
            Err(e) if resource_error(&e) => omission(diagnostics, spec.id, e),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn consider_validated(
    id: String,
    candidate: Vec<u8>,
    o: &NativePortfolioOptions,
    winner: &mut Vec<u8>,
    name: &mut String,
    diagnostics: &mut Vec<NativePortfolioCandidate>,
) {
    if candidate.len() > o.output_bytes || candidate.capacity() > o.output_bytes {
        omission(diagnostics, id, "candidate exceeded output reservation");
        return;
    }
    completed(diagnostics, id.clone(), candidate.len());
    if candidate.len() < winner.len() {
        *winner = candidate;
        *name = id;
    }
}

/// Encode one buffer. FAST and DEFAULT deliberately retain the existing
/// direct core path. BEST additionally evaluates CIXG1/CIXG2, CIXM6 and all
/// registered complete CIXB1 descriptors. Native families use bounded parallel
/// trials with canonical acceptance; external descriptors remain serial.
pub fn select(
    input: &[u8],
    options: &NativePortfolioOptions,
) -> Result<NativePortfolioResult, String> {
    select_constrained(input, options, &NativePortfolioConstraints::default())
}

pub fn select_constrained(
    input: &[u8],
    options: &NativePortfolioOptions,
    constraints: &NativePortfolioConstraints,
) -> Result<NativePortfolioResult, String> {
    options.validate(input.len())?;
    if constraints.formats.as_ref().is_some_and(|formats| {
        formats.is_empty()
            || formats
                .iter()
                .any(|format| !matches!(format.as_str(), "cixg1" | "cixg2" | "cixm6"))
    }) || constraints.blocks.as_ref().is_some_and(|blocks| {
        blocks.is_empty()
            || blocks.len() > MAX_EXPLICIT_BLOCKS
            || blocks.iter().any(|block| *block == 0 || *block > MAX_BLOCK)
    }) {
        return Err("invalid native whole-container constraint".into());
    }
    constraints.effort(6)?;
    constraints.strategy()?;
    if constraints.backend.as_deref().is_some_and(|backend| {
        !matches!(
            backend,
            "cix"
                | "raw"
                | "runs"
                | "ppm"
                | "mixture"
                | "deflate"
                | "hybrid"
                | "range"
                | "count-range"
                | "context-range-o1-b4"
                | "context-range-o1-b8"
                | "context-range-o2-b8"
                | "huffman"
        )
    }) {
        return Err("unsupported native backend constraint".into());
    }
    if stopped(options) {
        return Err("native portfolio cancelled or deadline exceeded".into());
    }
    let started = Instant::now();
    let _library = crate::limits::LibraryGuard::new();
    let _cancel = options
        .cancellation
        .clone()
        .map(crate::limits::CancellationGuard::new);
    // This is the caller's absolute cancellation contract.  The historical
    // CIXG/CIXM6 effort budget below is deliberately narrower and does not
    // impose a synthetic timeout on retained one-shot CIXB1 backends.
    let _caller_deadline = options.deadline.map(|deadline| {
        crate::limits::DeadlineGuard::new(deadline.saturating_duration_since(Instant::now()))
    });
    if options.profile != NativePortfolioProfile::Best {
        let memory_limit = available(options, input.len(), 0, options.output_bytes)
            .ok_or("native profile cannot admit source, output reservation and codec remainder")?;
        // `encode_buffer` owns an unconstrained Vec whose allocator capacity
        // can exceed its logical writer limit. Reserve the caller's complete
        // output bound first and pass that bounded writer to the core path.
        let mut writer = CappedWriter::new(options.output_bytes, None)?;
        if constraints == &NativePortfolioConstraints::default() {
            core::encode(
                input,
                &mut writer,
                &core::NativeOptions {
                    profile: match options.profile {
                        NativePortfolioProfile::Fast => core::NativeProfile::Fast,
                        _ => core::NativeProfile::Default,
                    },
                    output_limit: options.output_bytes,
                    memory_limit,
                    workers: options.workers,
                    deadline: options
                        .deadline
                        .map(|d| d.saturating_duration_since(Instant::now())),
                    cancellation: options.cancellation.clone(),
                },
            )
            .map_err(|e| e.to_string())?;
        } else {
            let format = ["cixg2", "cixg1", "cixm6"]
                .into_iter()
                .find(|format| constraints.allows(format))
                .ok_or("no native whole-container satisfies the explicit format constraint")?;
            let block = constraints
                .blocks
                .as_ref()
                .and_then(|blocks| blocks.first())
                .copied()
                .unwrap_or(MAX_BLOCK);
            if format == "cixm6" {
                encode_m6(
                    io::Cursor::new(input),
                    &mut writer,
                    block as usize,
                    None,
                    constraints.effort(if options.profile == NativePortfolioProfile::Fast {
                        1
                    } else {
                        6
                    })?,
                    memory_limit,
                )?;
            } else {
                encode_with_strategy(
                    io::Cursor::new(input),
                    &mut writer,
                    EncodeOptions {
                        block,
                        level: constraints.effort(
                            if options.profile == NativePortfolioProfile::Fast {
                                1
                            } else {
                                6
                            },
                        )?,
                        forced: None,
                        backend: constraints.backend(),
                        backend_set: constraints.backend.is_some(),
                        format,
                        input_fd: -1,
                        flush_interval: None,
                        memory: memory_limit,
                        workers: options.workers,
                        explain: false,
                        verbose: false,
                        strategy: constraints.strategy()?,
                    },
                )?;
            }
        }
        check_interrupted()?;
        let archive = writer.bytes;
        let archive_bytes = archive.len();
        return Ok(NativePortfolioResult {
            selected: "core-native".into(),
            retained_output_capacity: archive.capacity(),
            archive,
            candidates: vec![NativePortfolioCandidate {
                id: "core-native".into(),
                status: NativePortfolioStatus::Selected,
                archive_bytes: Some(archive_bytes),
                reason: None,
            }],
            source_bytes_charged: input.len(),
            search_budget: Duration::ZERO,
            elapsed: started.elapsed(),
        });
    }
    let cap = best_fallback_cap(options, input.len())?;
    let baseline_format = ["cixg2", "cixg1", "cixm6"]
        .into_iter()
        .find(|format| constraints.allows(format))
        .ok_or("no native whole-container satisfies the explicit format constraint")?;
    let Some(memory) = available(options, input.len(), 0, cap) else {
        return Err("BEST cannot admit source, raw fallback and codec remainder".into());
    };
    let mut raw = CappedWriter::new(cap, None)?;
    let baseline_block = constraints
        .blocks
        .as_ref()
        .and_then(|blocks| blocks.first().copied())
        .unwrap_or(MAX_BLOCK);
    if baseline_format == "cixm6" {
        encode_m6(
            io::Cursor::new(input),
            &mut raw,
            baseline_block as usize,
            None,
            constraints.effort(9)?,
            memory,
        )?;
    } else {
        encode_with_strategy(
            io::Cursor::new(input),
            &mut raw,
            EncodeOptions {
                // Preserve the historical unconstrained raw baseline. An explicit
                // block is a real wire constraint and is therefore authoritative.
                block: baseline_block,
                level: constraints.effort(9)?,
                forced: (baseline_format != "cixm6").then_some("raw"),
                backend: constraints.backend(),
                backend_set: constraints.backend.is_some(),
                format: baseline_format,
                input_fd: -1,
                flush_interval: None,
                memory,
                workers: options.workers,
                explain: false,
                verbose: false,
                strategy: constraints.strategy()?,
            },
        )?;
    }
    let mut winner = raw.bytes;
    check_interrupted()?;
    validate(&winner, winner.capacity(), input, options, 0)?;
    let mut selected = if baseline_format == "cixm6" {
        format!("cixm6:native-auto:block={baseline_block}")
    } else {
        format!("{baseline_format}:raw-baseline")
    };
    let mut diagnostics = vec![NativePortfolioCandidate {
        id: selected.clone(),
        status: NativePortfolioStatus::Completed,
        archive_bytes: Some(winner.len()),
        reason: None,
    }];
    let budget = whole_budget(input.len());
    let cixg_budget = budget / 2;
    let m6_budget = budget.saturating_sub(cixg_budget);
    let blocks = constraints.block_list(input.len());
    let cixg_started = Instant::now();
    let cixg_pairs: Vec<(&'static str, u32)> = ["cixg1", "cixg2"]
        .into_iter()
        .filter(|format| constraints.allows(format))
        .flat_map(|f| blocks.iter().copied().map(move |b| (f, b)))
        .collect();
    let cixg_winner_reservation = if options.workers == 1 {
        winner.capacity()
    } else {
        options.output_bytes
    };
    let cixg_workers = family_admission(
        options,
        input.len(),
        cixg_winner_reservation,
        cixg_pairs.len(),
    )
    .map(|(workers, _)| workers)
    .unwrap_or(1);
    if let Some(slice) = family_trial_budget(
        options,
        cixg_budget,
        cixg_started,
        cixg_pairs.len(),
        cixg_workers,
    ) {
        let effort = constraints.effort(9)?;
        let strategy = constraints.strategy()?.to_owned();
        let backend = constraints.backend().to_owned();
        run_native_family(
            input,
            options,
            &mut winner,
            &mut selected,
            &mut diagnostics,
            cixg_pairs
                .into_iter()
                .enumerate()
                .map(|(order, (format, block))| NativeTrialSpec {
                    order,
                    id: format!("{format}:native-auto:block={block}"),
                    budget: slice,
                    kind: NativeTrialKind::Cixg {
                        format,
                        block,
                        backend: backend.clone(),
                        backend_set: constraints.backend.is_some(),
                        effort,
                        strategy: strategy.clone(),
                    },
                })
                .collect(),
        )?;
    } else if !cixg_pairs.is_empty() {
        for (format, block) in cixg_pairs {
            omission(
                &mut diagnostics,
                format!("{format}:native-auto:block={block}"),
                "CIXG family deadline allocation exhausted",
            );
        }
    }
    let m6_started = Instant::now();
    if constraints.allows("cixm6") && constraints.backend.is_none() {
        let m6_winner_reservation = if options.workers == 1 {
            winner.capacity()
        } else {
            options.output_bytes
        };
        let m6_workers =
            family_admission(options, input.len(), m6_winner_reservation, blocks.len())
                .map(|(workers, _)| workers)
                .unwrap_or(1);
        if let Some(slice) =
            family_trial_budget(options, m6_budget, m6_started, blocks.len(), m6_workers)
        {
            let effort = constraints.effort(9)?;
            run_native_family(
                input,
                options,
                &mut winner,
                &mut selected,
                &mut diagnostics,
                blocks
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(order, block)| NativeTrialSpec {
                        order,
                        id: format!("cixm6:native-auto:block={block}"),
                        budget: slice,
                        kind: NativeTrialKind::M6 {
                            block: block as usize,
                            effort,
                        },
                    })
                    .collect(),
            )?;
        } else {
            for block in &blocks {
                omission(
                    &mut diagnostics,
                    format!("cixm6:native-auto:block={block}"),
                    "CIXM6 family deadline allocation exhausted",
                );
            }
        }
    } else if constraints.backend.is_some() {
        for block in &blocks {
            omission(
                &mut diagnostics,
                format!("cixm6:native-auto:block={block}"),
                "explicit backend does not apply to CIXM6",
            );
        }
    }
    for descriptor in external::best_candidates() {
        if constraints.forces_native_container() {
            omission(
                &mut diagnostics,
                descriptor.id,
                "explicit native container constraint excludes CIXB1 candidate",
            );
            continue;
        }
        check_interrupted()?;
        if stopped(options) {
            return Err("native portfolio cancelled or deadline exceeded".into());
        }
        let available = options
            .memory_bytes
            .saturating_sub(input.len())
            .saturating_sub(winner.capacity());
        if let Err(reason) = descriptor.admit(input.len(), available) {
            omission(&mut diagnostics, descriptor.id, reason.reason);
            continue;
        }
        let archive = match descriptor.full_archive_encode(input) {
            Ok(v) => v,
            Err(e) if resource_error(&e) || candidate_rejection(&e) => {
                if stopped(options) {
                    return Err("native portfolio cancelled or deadline exceeded".into());
                }
                omission(&mut diagnostics, descriptor.id, e);
                continue;
            }
            Err(e) => return Err(e),
        };
        // A one-shot backend can only observe cancellation before/after its
        // native call.  Do not publish or validate its archive after a caller
        // cancellation/deadline raced the call.
        check_interrupted()?;
        // `encoder_peak_bytes` above reserves the descriptor's payload/wrapper
        // copies; this check makes the caller-visible retained-output bound
        // explicit even when a backend produced an unexpectedly large frame.
        let decoder_available = options
            .memory_bytes
            .saturating_sub(input.len())
            .saturating_sub(winner.capacity())
            .saturating_sub(archive.capacity());
        match descriptor.decoder_peak_bytes(input.len()) {
            Ok(needed) if needed <= decoder_available => consider(
                descriptor.id.into(),
                archive,
                input,
                options,
                &mut winner,
                &mut selected,
                &mut diagnostics,
            )?,
            Ok(needed) => omission(
                &mut diagnostics,
                descriptor.id,
                format!("validation needs {needed} bytes, available {decoder_available}"),
            ),
            Err(reason) => omission(&mut diagnostics, descriptor.id, reason),
        }
    }
    validate(&winner, winner.capacity(), input, options, 0)?;
    if let Some(entry) = diagnostics.iter_mut().find(|x| x.id == selected) {
        entry.status = NativePortfolioStatus::Selected;
    }
    Ok(NativePortfolioResult {
        retained_output_capacity: winner.capacity(),
        archive: winner,
        selected,
        candidates: diagnostics,
        source_bytes_charged: input.len(),
        search_budget: budget,
        elapsed: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Condvar, Mutex,
    };

    fn scheduler_options(
        workers: usize,
        cancellation: Option<Arc<AtomicBool>>,
    ) -> NativePortfolioOptions {
        NativePortfolioOptions {
            profile: NativePortfolioProfile::Best,
            memory_bytes: 64 * MIB,
            output_bytes: 1024,
            workers,
            deadline: None,
            cancellation,
        }
    }

    fn scheduler_specs(count: usize) -> Vec<NativeTrialSpec> {
        (0..count)
            .map(|order| NativeTrialSpec {
                order,
                id: format!("trial-{order}"),
                budget: Duration::from_secs(1),
                kind: NativeTrialKind::M6 {
                    block: 1024,
                    effort: 1,
                },
            })
            .collect()
    }

    #[test]
    fn best_admission_includes_a_raw_fallback() {
        let n = 4096;
        let options = NativePortfolioOptions {
            profile: NativePortfolioProfile::Best,
            memory_bytes: 128 * MIB,
            output_bytes: archive_cap(n).saturating_sub(1),
            workers: 1,
            deadline: None,
            cancellation: None,
        };
        assert!(options.validate(n).is_ok());
        assert!(best_fallback_cap(&options, n).is_err());
    }

    #[test]
    fn direct_profile_allows_a_smaller_legal_output_cap() {
        let options = NativePortfolioOptions {
            profile: NativePortfolioProfile::Fast,
            memory_bytes: 128 * MIB,
            output_bytes: 1,
            workers: 1,
            deadline: None,
            cancellation: None,
        };
        assert!(options.validate(4096).is_ok());
    }

    #[test]
    fn output_reservation_failure_is_fallible() {
        assert!(CappedWriter::new(usize::MAX, None).is_err());
    }

    #[test]
    fn family_budget_is_split_without_losing_total_time() {
        let total = whole_budget(8 * MIB);
        let cixg = total / 2;
        assert_eq!(cixg + total.saturating_sub(cixg), total);
    }

    #[test]
    fn capped_writer_keeps_an_equal_candidate_materializable() {
        let mut writer = CappedWriter::new(8, Some(3)).unwrap();
        writer.write_all(b"abc").unwrap();
        assert_eq!(writer.bytes, b"abc");
        assert_eq!(
            writer.write_all(b"d").unwrap_err().to_string(),
            PREFIX_PRUNED
        );
    }

    #[test]
    fn family_scheduler_bounds_overlap_memory_and_orders_completion() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let budgets = Arc::new(Mutex::new(Vec::new()));
        let completed = Arc::new(Mutex::new(Vec::new()));
        let accepted = Arc::new(Mutex::new(Vec::new()));
        // Both first-wave trials must be live before either is allowed to
        // release the ordering gate. Unlike Barrier, a broken serialized
        // scheduler fails promptly instead of hanging the complete test run.
        let overlap = Arc::new((Mutex::new((0usize, false)), Condvar::new()));
        let (second_done, first_release) = mpsc::channel();
        let first_release = Arc::new(Mutex::new(first_release));
        let options = scheduler_options(2, None);
        schedule_native_family(
            scheduler_specs(4),
            &options,
            4096,
            2048,
            {
                let active = active.clone();
                let peak = peak.clone();
                let budgets = budgets.clone();
                let completed = completed.clone();
                let first_release = first_release.clone();
                let overlap = overlap.clone();
                move |spec, memory| {
                    budgets.lock().unwrap().push(memory);
                    let live = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(live, Ordering::SeqCst);
                    if spec.order < 2 {
                        let (state, ready) = &*overlap;
                        let mut gate = state.lock().unwrap();
                        gate.0 += 1;
                        if gate.0 == 2 {
                            gate.1 = true;
                            ready.notify_all();
                        } else {
                            let (updated, timeout) = ready
                                .wait_timeout_while(gate, Duration::from_secs(5), |gate| !gate.1)
                                .unwrap();
                            gate = updated;
                            if timeout.timed_out() && !gate.1 {
                                active.fetch_sub(1, Ordering::SeqCst);
                                return Err("native scheduler did not overlap first wave".into());
                            }
                        }
                    }
                    if spec.order == 0 {
                        first_release.lock().unwrap().recv().unwrap();
                    } else if spec.order == 1 {
                        // Record completion before releasing trial 0, making
                        // the asserted completion order an actual happens-before relation.
                        completed.lock().unwrap().push(spec.order);
                        second_done.send(()).unwrap();
                        active.fetch_sub(1, Ordering::SeqCst);
                        return Ok(3usize.saturating_sub(spec.order));
                    }
                    completed.lock().unwrap().push(spec.order);
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(3usize.saturating_sub(spec.order))
                }
            },
            {
                let accepted = accepted.clone();
                move |wave| {
                    accepted.lock().unwrap().extend(
                        wave.iter()
                            .map(|(spec, value)| (spec.order, *value.as_ref().unwrap())),
                    );
                    Ok(())
                }
            },
        )
        .unwrap();
        let (slots, per_trial) = family_admission(&options, 4096, 2048, 4).unwrap();
        assert_eq!(slots, 2);
        assert!(peak.load(Ordering::SeqCst) >= 2);
        assert!(peak.load(Ordering::SeqCst) <= options.workers);
        assert!(4096 + 2048 + slots * (options.output_bytes + per_trial) <= options.memory_bytes);
        assert!(budgets
            .lock()
            .unwrap()
            .iter()
            .all(|value| *value == per_trial));
        assert_eq!(&completed.lock().unwrap()[..2], &[1, 0]);
        assert_eq!(
            &*accepted.lock().unwrap(),
            &[(0, 3), (1, 2), (2, 1), (3, 0)]
        );
    }

    #[test]
    fn family_scheduler_stops_before_a_later_cancelled_wave() {
        let cancellation = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicUsize::new(0));
        let options = scheduler_options(2, Some(cancellation.clone()));
        let result = schedule_native_family(
            scheduler_specs(4),
            &options,
            4096,
            2048,
            {
                let cancellation = cancellation.clone();
                let started = started.clone();
                move |spec, _| {
                    started.fetch_add(1, Ordering::SeqCst);
                    if spec.order == 0 {
                        cancellation.store(true, Ordering::Release);
                    }
                    Ok(())
                }
            },
            |_| Ok(()),
        );
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(started.load(Ordering::SeqCst) <= 2);
    }

    #[test]
    fn family_admission_retries_smaller_parallelism() {
        let mut options = scheduler_options(4, None);
        options.memory_bytes = 2 * (options.output_bytes + MIN_REMAINDER);
        assert_eq!(
            family_admission(&options, 0, 0, 4).map(|value| value.0),
            Some(2)
        );
    }

    #[test]
    fn family_scheduler_drops_successful_results_between_waves() {
        struct Held(Arc<AtomicUsize>);
        impl Drop for Held {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let options = scheduler_options(2, None);
        schedule_native_family(
            scheduler_specs(4),
            &options,
            4096,
            2048,
            {
                let active = active.clone();
                let peak = peak.clone();
                move |_, _| {
                    let live = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(live, Ordering::SeqCst);
                    Ok(Held(active.clone()))
                }
            },
            |wave| {
                assert!(wave.len() <= 2);
                assert!(active.load(Ordering::SeqCst) <= 2);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn constrained_m6_best_is_equivalent_with_one_or_two_family_workers() {
        let input: Vec<u8> = (0..4096)
            .map(|index| ((index * 17 + index / 11) & 0xff) as u8)
            .collect();
        let constraints = NativePortfolioConstraints {
            formats: Some(vec!["cixm6".into()]),
            ..NativePortfolioConstraints::default()
        };
        let make_options = |workers| NativePortfolioOptions {
            profile: NativePortfolioProfile::Best,
            memory_bytes: 2usize << 30,
            output_bytes: archive_cap(input.len()),
            workers,
            deadline: None,
            cancellation: None,
        };
        let serial = select_constrained(&input, &make_options(1), &constraints).unwrap();
        let parallel = select_constrained(&input, &make_options(2), &constraints).unwrap();
        assert_eq!(serial.selected, parallel.selected);
        assert_eq!(serial.archive, parallel.archive);
        assert_eq!(
            core::decode_buffer(
                &parallel.archive,
                &core::NativeOptions {
                    profile: core::NativeProfile::Best,
                    output_limit: input.len(),
                    memory_limit: 2usize << 30,
                    workers: 1,
                    deadline: None,
                    cancellation: None,
                },
            )
            .unwrap(),
            input
        );
    }

    #[test]
    fn backend_constraint_reports_supported_and_unsupported_native_routes() {
        let deflate = NativePortfolioConstraints {
            backend: Some("deflate".into()),
            ..NativePortfolioConstraints::default()
        };
        assert!(deflate.allows("cixg1"));
        assert!(deflate.allows("cixg2"));
        assert!(!deflate.allows("cixm6"));
        let huffman = NativePortfolioConstraints {
            backend: Some("huffman".into()),
            ..NativePortfolioConstraints::default()
        };
        assert!(!huffman.allows("cixg1"));
        assert!(huffman.allows("cixg2"));
        assert!(!huffman.allows("cixm6"));
        assert!(!huffman.forces_native_container());
    }

    #[test]
    fn best_tiny_input_keeps_every_whole_archive_family_observable() {
        // Deliberately generated bytes: this exercises selection without a
        // corpus identity, file name, or pre-existing archive.
        let input: Vec<u8> = (0..191)
            .map(|index| ((index * 37 + index / 7) & 0xff) as u8)
            .collect();
        let output_bytes = archive_cap(input.len());
        let options = NativePortfolioOptions {
            profile: NativePortfolioProfile::Best,
            memory_bytes: 2usize << 30,
            output_bytes,
            workers: 1,
            deadline: None,
            cancellation: None,
        };

        let selected = select(&input, &options).expect("tiny BEST selection");
        assert!(selected.retained_output_capacity <= options.output_bytes);
        assert!(selected.archive.len() <= options.output_bytes);

        let cixg_ids: Vec<String> = ["cixg1", "cixg2"]
            .into_iter()
            .flat_map(|format| {
                [16 * 1024, 32 * 1024, MAX_BLOCK]
                    .into_iter()
                    .map(move |block| format!("{format}:native-auto:block={block}"))
            })
            .collect();
        let m6_ids: Vec<String> = [16 * 1024, 32 * 1024, MAX_BLOCK]
            .into_iter()
            .map(|block| format!("cixm6:native-auto:block={block}"))
            .collect();
        assert_eq!(external::best_candidates().len(), 14);
        let external_ids: Vec<String> = external::best_candidates()
            .iter()
            .map(|candidate| candidate.id.to_string())
            .collect();
        for id in cixg_ids
            .iter()
            .chain(m6_ids.iter())
            .chain(external_ids.iter())
        {
            let trial = selected
                .candidates
                .iter()
                .find(|trial| trial.id == id.as_str())
                .unwrap_or_else(|| panic!("missing BEST diagnostic for {id}"));
            assert!(matches!(
                trial.status,
                NativePortfolioStatus::Completed
                    | NativePortfolioStatus::Omitted
                    | NativePortfolioStatus::Selected
            ));
        }

        let minimum_completed = selected
            .candidates
            .iter()
            .filter_map(|trial| match trial.status {
                NativePortfolioStatus::Completed | NativePortfolioStatus::Selected => {
                    trial.archive_bytes
                }
                NativePortfolioStatus::Omitted => None,
            })
            .min()
            .expect("raw baseline is complete");
        assert_eq!(selected.archive.len(), minimum_completed);

        // This is intentionally archive-only: the decoder receives only the
        // selected bytes, with no selector state or original input supplied.
        let restored = core::decode_buffer(
            &selected.archive,
            &core::NativeOptions {
                profile: core::NativeProfile::Best,
                output_limit: input.len(),
                memory_limit: options.memory_bytes,
                workers: 1,
                deadline: None,
                cancellation: None,
            },
        )
        .expect("selected archive restores independently");
        assert_eq!(restored, input);
    }
}
