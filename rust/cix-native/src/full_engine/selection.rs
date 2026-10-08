//! Bounded whole-input archive selection.
//!
//! This is deliberately a complete-input planner (maximum 128 MiB), not a
//! streaming selector.  It never derives a route from a filename, environment,
//! benchmark identity, or cache.  Reservations are declared buffers/model
//! state, not measured process RSS; an outer supervisor remains responsible
//! for hard RSS and filesystem enforcement.

use super::{
    backend_provider::{
        BackendProvider, BackendRequest, BackendVariant, PaqLevel, TrustedBackendPaths,
    },
    catalogue::{self, Candidate, Recipe},
    containers,
    dispatch::{FullEngine, OperationLimits},
    jxl_provider::JxlProvider,
    mixed_selection, native_portfolio,
    spatial_provider::SpatialProvider,
};
use crate::paq_bridge::PaqVariant;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

pub const MAX_WHOLE_INPUT: usize = 128 << 20;
const MIB: usize = 1 << 20;
const PAQ_BASE: usize = 4_500_000_000;
const PAQ_9L_BASE: usize = 5_000_000_000;
const NATIVE_BASE: usize = 8_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionProfile {
    Fast,
    Default,
    Best,
}

#[derive(Clone, Debug)]
pub struct SelectionLimits {
    pub memory_bytes: usize,
    pub workers: usize,
    pub temporary_bytes: usize,
    pub output_bytes: usize,
    pub intermediate_bytes: usize,
    pub deadline: Option<Instant>,
    pub cancellation: Option<Arc<AtomicBool>>,
}
#[derive(Clone, Debug, Default)]
pub struct WholeConstraints {
    pub native: native_portfolio::NativePortfolioConstraints,
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub backends: TrustedBackendPaths,
    /// An absolute installed bridge path.  It is loaded afresh per trial;
    /// `JxlProvider` itself is intentionally not shared across threads.
    pub jxl_bridge: Option<PathBuf>,
    pub spatial_bridge: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrialStatus {
    Completed,
    BudgetOmitted,
    ProfileOmitted,
    Failed,
    Selected,
}
#[derive(Clone, Debug)]
pub struct Trial {
    pub id: String,
    pub priority: usize,
    pub status: TrialStatus,
    pub reason: Option<String>,
    pub archive_bytes: Option<usize>,
    /// Inner searches retain their own units: native complete archives and
    /// mixed-region payloads must not be confused in reporting.
    pub details: TrialDetails,
}
#[derive(Clone, Debug, Default)]
pub enum TrialDetails {
    #[default]
    None,
    Native(Vec<native_portfolio::NativePortfolioCandidate>),
    Mixed(mixed_selection::MixedDiagnostics),
}
#[derive(Clone, Debug)]
pub struct CandidateExecution {
    pub archive: Vec<u8>,
    pub details: TrialDetails,
}
impl CandidateExecution {
    fn bare(archive: Vec<u8>) -> Self {
        Self {
            archive,
            details: TrialDetails::None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct SelectionReport {
    pub profile: SelectionProfile,
    pub input_bytes: usize,
    /// Content-only recognizer outcomes. These are distinct from candidate
    /// trials: a non-admitted format is a safe discovery result, not a failed
    /// codec attempt.
    pub discovery: Vec<catalogue::Diagnostic>,
    pub trials: Vec<Trial>,
    pub peak_reserved_memory: usize,
    pub peak_reserved_workers: usize,
    pub peak_reserved_temporary: usize,
    pub selected: String,
}
#[derive(Clone, Debug)]
pub struct SelectionResult {
    pub archive: Vec<u8>,
    pub report: SelectionReport,
}

#[derive(Clone, Debug)]
enum PlanKind {
    Raw,
    Native {
        profile: SelectionProfile,
        constraints: native_portfolio::NativePortfolioConstraints,
    },
    Paq {
        variant: PaqVariant,
        level: u8,
        lstm: bool,
    },
    Specialist(Candidate),
    Mixed {
        profile: SelectionProfile,
    },
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub id: String,
    priority: usize,
    kind: PlanKind,
    memory: usize,
    workers: usize,
    temporary: usize,
}

fn checked_add(a: usize, b: usize) -> Result<usize, String> {
    a.checked_add(b)
        .ok_or_else(|| "selection reservation overflow".into())
}
fn scaled(base: usize, n: usize) -> Result<usize, String> {
    checked_add(
        base,
        n.checked_mul(8).ok_or("selection reservation overflow")?,
    )
}
/// Minimum reservation for the stored baseline before whole-input selection.
/// This does not guarantee admission of every later model or parallel trial.
pub fn minimum_whole_input_memory(
    input_bytes: usize,
    output_bytes: usize,
) -> Result<usize, String> {
    checked_add(
        checked_add(input_bytes, raw_plan_memory(input_bytes)?)?,
        output_bytes,
    )
}
fn raw_plan_memory(input_bytes: usize) -> Result<usize, String> {
    checked_add(
        input_bytes
            .checked_mul(3)
            .ok_or("raw reservation overflow")?,
        32 * MIB,
    )
}
fn temporary(n: usize, output: usize) -> Result<usize, String> {
    checked_add(n, output)
}
fn cancelled(limits: &SelectionLimits) -> bool {
    limits
        .cancellation
        .as_ref()
        .is_some_and(|v| v.load(Ordering::Acquire))
        || limits.deadline.is_some_and(|d| Instant::now() >= d)
}

fn specialist_is_paq(candidate: &Candidate) -> bool {
    match candidate.recipe {
        Recipe::Geometry { .. }
        | Recipe::Hydrogen
        | Recipe::GroupedGrid
        | Recipe::BoundedGrid { .. } => true,
        Recipe::RecordConstraints { paq, .. } | Recipe::Address { paq, .. } => paq,
        _ => false,
    }
}

fn affordable_mixed_memory(input_bytes: usize, limits: &SelectionLimits) -> Option<usize> {
    // The scheduler retains the caller source, an incumbent reservation and a
    // trial-result reservation while the mixed carrier is live. Give CIXH1
    // precisely the remainder; it will then omit nested specialists that do
    // not fit this actual operation budget.
    limits
        .memory_bytes
        .checked_sub(input_bytes)?
        .checked_sub(limits.output_bytes)?
        .checked_sub(limits.output_bytes)
}

fn plans(
    data: &[u8],
    profile: SelectionProfile,
    limits: &SelectionLimits,
    discovery: &mut Vec<catalogue::Diagnostic>,
    trials: &mut Vec<Trial>,
    constraints: &WholeConstraints,
) -> Result<Vec<Plan>, String> {
    let n = data.len();
    let temp = temporary(n, limits.output_bytes)?;
    let mut out = Vec::new();
    let mut next_priority = 0usize;
    if !constraints.native.forces_native_container() {
        let raw_memory = raw_plan_memory(n)?;
        out.push(Plan {
            id: "raw".into(),
            priority: next_priority,
            kind: PlanKind::Raw,
            memory: raw_memory,
            workers: 1,
            temporary: temp,
        });
        next_priority += 1;
    }
    let native_budget = limits
        .memory_bytes
        .saturating_sub(checked_add(
            n.checked_mul(12).ok_or("native reservation overflow")?,
            256 * MIB,
        )?)
        .clamp(64 * MIB, NATIVE_BASE);
    out.push(Plan {
        id: "native".into(),
        priority: next_priority,
        kind: PlanKind::Native {
            profile,
            constraints: constraints.native.clone(),
        },
        memory: checked_add(
            native_budget,
            checked_add(
                n.checked_mul(12).ok_or("native reservation overflow")?,
                128 * MIB,
            )?,
        )?,
        workers: limits.workers,
        temporary: temp,
    });
    next_priority += 1;
    // A forced native container has one faithful whole-input producer: the
    // constrained native portfolio.  Raw CIXR1, CIXB1, PAQ, specialist and
    // mixed plans cannot satisfy this wire request.
    if constraints.native.forces_native_container() {
        for id in [
            "raw",
            "paq-v216-8",
            "paq-v215-8",
            "paq-v215-9L",
            "heterogeneous-cixh1-v1",
        ] {
            trials.push(Trial {
                id: id.into(),
                priority: next_priority,
                status: TrialStatus::ProfileOmitted,
                reason: Some(
                    "explicit native container constraint excludes incompatible wire".into(),
                ),
                archive_bytes: None,
                details: TrialDetails::None,
            });
            next_priority += 1;
        }
        return Ok(out);
    }
    if profile == SelectionProfile::Best {
        for (id, variant, level, lstm, base) in [
            ("paq-v216-8", PaqVariant::V216, 8, false, PAQ_BASE),
            ("paq-v215-8", PaqVariant::V215, 8, false, PAQ_BASE),
            ("paq-v215-9L", PaqVariant::V215, 9, true, PAQ_9L_BASE),
        ] {
            out.push(Plan {
                id: id.into(),
                priority: next_priority,
                kind: PlanKind::Paq {
                    variant,
                    level,
                    lstm,
                },
                memory: scaled(base, n)?,
                workers: 1,
                temporary: checked_add(
                    n,
                    limits
                        .output_bytes
                        .checked_mul(2)
                        .ok_or("selection reservation overflow")?,
                )?,
            });
            next_priority += 1;
        }
    }
    if profile == SelectionProfile::Fast {
        return Ok(out);
    }
    let catalogue = catalogue::catalog(data)?;
    discovery.extend(catalogue.diagnostics);
    for candidate in catalogue.candidates {
        let priority = next_priority;
        next_priority += 1;
        if profile == SelectionProfile::Default && specialist_is_paq(&candidate) {
            trials.push(Trial {
                id: candidate.id,
                priority,
                status: TrialStatus::ProfileOmitted,
                reason: Some("DEFAULT omits PAQ-backed specialist trials".into()),
                archive_bytes: None,
                details: TrialDetails::None,
            });
            continue;
        }
        out.push(Plan {
            id: candidate.id.clone(),
            priority,
            memory: candidate.memory_bytes,
            workers: 1,
            temporary: checked_add(limits.intermediate_bytes, limits.output_bytes)?,
            kind: PlanKind::Specialist(candidate),
        });
    }
    let mixed_temporary = checked_add(limits.intermediate_bytes, limits.output_bytes)?;
    // BEST retains the established 5 GiB nested-search allowance where the
    // caller can afford it. Lower budgets still admit a generic carrier with
    // the exact remainder, whose nested policy records each resource omission.
    let mixed_memory = affordable_mixed_memory(n, limits).map(|affordable| {
        if profile == SelectionProfile::Best {
            affordable.min(5 << 30)
        } else {
            affordable
        }
    });
    match mixed_memory {
        Some(memory) if memory != 0 => out.push(Plan {
            id: "heterogeneous-cixh1-v1".into(),
            priority: next_priority,
            kind: PlanKind::Mixed { profile },
            memory,
            workers: limits.workers,
            temporary: mixed_temporary,
        }),
        _ => trials.push(Trial {
            id: "heterogeneous-cixh1-v1".into(),
            priority: next_priority,
            status: TrialStatus::BudgetOmitted,
            reason: Some(
                "source, retained archive and CIXH1 result reservations exhaust memory".into(),
            ),
            archive_bytes: None,
            details: TrialDetails::None,
        }),
    }
    Ok(out)
}

pub trait CandidateExecutor: Clone + Send + Sync + 'static {
    fn run(&self, plan: &Plan, data: &[u8], limits: &SelectionLimits) -> Result<Vec<u8>, String>;
    fn run_detailed(
        &self,
        plan: &Plan,
        data: &[u8],
        limits: &SelectionLimits,
    ) -> Result<CandidateExecution, String> {
        self.run(plan, data, limits).map(CandidateExecution::bare)
    }
}
#[derive(Clone, Debug)]
pub struct NativeExecutor {
    pub providers: ProviderConfig,
}
impl NativeExecutor {
    fn provider_detail(name: &str, error: impl AsRef<str>) -> String {
        const MAX: usize = 160;
        let error = error.as_ref();
        let mut detail = format!("{name} provider unavailable: {error}");
        if detail.len() > MAX {
            let mut end = MAX;
            while !detail.is_char_boundary(end) {
                end -= 1;
            }
            detail.truncate(end);
        }
        detail
    }

    /// A mixed carrier has valid generic backends without image/spatial
    /// providers. Failed optional loads are diagnostic omissions rather than a
    /// reason to suppress that carrier.
    fn mixed_engine(&self, output: usize) -> (FullEngine, Vec<String>) {
        let mut omissions = Vec::new();
        let images = match &self.providers.jxl_bridge {
            Some(path) => match JxlProvider::load_package_bridge(path, output) {
                Ok(provider) => Some(provider),
                Err(error) => {
                    omissions.push(Self::provider_detail("JXL", error.to_string()));
                    None
                }
            },
            None => {
                omissions.push("JXL provider unavailable: not configured".into());
                None
            }
        };
        let spatial = match &self.providers.spatial_bridge {
            Some(path) => match SpatialProvider::load_package_bridge(path, output) {
                Ok(provider) => Some(provider),
                Err(error) => {
                    omissions.push(Self::provider_detail("spatial", error.to_string()));
                    None
                }
            },
            None => {
                omissions.push("spatial provider unavailable: not configured".into());
                None
            }
        };
        (
            FullEngine {
                backends: BackendProvider {
                    paths: self.providers.backends.clone(),
                },
                images,
                spatial,
            },
            omissions,
        )
    }

    fn engine(
        &self,
        output: usize,
        need_jxl: bool,
        need_spatial: bool,
    ) -> Result<FullEngine, String> {
        let images = match self.providers.jxl_bridge.as_ref().filter(|_| need_jxl) {
            Some(path) => {
                Some(JxlProvider::load_package_bridge(path, output).map_err(|e| e.to_string())?)
            }
            None => None,
        };
        let spatial = match self
            .providers
            .spatial_bridge
            .as_ref()
            .filter(|_| need_spatial)
        {
            Some(path) => Some(
                SpatialProvider::load_package_bridge(path, output).map_err(|e| e.to_string())?,
            ),
            None => None,
        };
        Ok(FullEngine {
            backends: BackendProvider {
                paths: self.providers.backends.clone(),
            },
            images,
            spatial,
        })
    }
}
impl CandidateExecutor for NativeExecutor {
    fn run(&self, plan: &Plan, data: &[u8], limits: &SelectionLimits) -> Result<Vec<u8>, String> {
        self.run_detailed(plan, data, limits)
            .map(|result| result.archive)
    }
    fn run_detailed(
        &self,
        plan: &Plan,
        data: &[u8],
        limits: &SelectionLimits,
    ) -> Result<CandidateExecution, String> {
        if cancelled(limits) {
            return Err("selection cancelled or deadline exceeded".into());
        }
        match &plan.kind {
            PlanKind::Mixed { profile } => {
                let (engine, provider_omissions) = self.mixed_engine(limits.output_bytes);
                return mixed_selection::encode_with_provider_omissions_workers(
                    &engine,
                    data,
                    *profile,
                    &OperationLimits {
                        memory_bytes: plan.memory,
                        output_bytes: limits.output_bytes,
                        intermediate_bytes: limits.intermediate_bytes,
                        deadline: limits.deadline,
                        cancellation: limits.cancellation.clone(),
                    },
                    &provider_omissions,
                    plan.workers,
                )
                .map(|result| CandidateExecution {
                    archive: result.archive,
                    details: TrialDetails::Mixed(result.diagnostics),
                });
            }
            PlanKind::Raw => containers::encode_raw(data, limits.output_bytes),
            PlanKind::Native {
                profile,
                constraints,
            } => {
                return native_portfolio::select_constrained(
                    data,
                    &native_portfolio::NativePortfolioOptions {
                        profile: match profile {
                            SelectionProfile::Fast => {
                                native_portfolio::NativePortfolioProfile::Fast
                            }
                            SelectionProfile::Default => {
                                native_portfolio::NativePortfolioProfile::Default
                            }
                            SelectionProfile::Best => {
                                native_portfolio::NativePortfolioProfile::Best
                            }
                        },
                        output_bytes: limits.output_bytes,
                        memory_bytes: plan.memory,
                        workers: plan.workers,
                        deadline: limits.deadline,
                        cancellation: limits.cancellation.clone(),
                    },
                    constraints,
                )
                .map(|result| CandidateExecution {
                    archive: result.archive,
                    details: TrialDetails::Native(result.candidates),
                });
            }
            PlanKind::Paq {
                variant,
                level,
                lstm,
            } => {
                let provider = BackendProvider {
                    paths: self.providers.backends.clone(),
                };
                let member = provider.encode(
                    data,
                    &BackendRequest {
                        variant: BackendVariant::Paq {
                            variant: *variant,
                            level: PaqLevel {
                                number: *level,
                                lstm: *lstm,
                            },
                            lstm_layers: 1,
                            joint_discount_mode: None,
                        },
                        output_limit: limits.output_bytes,
                        memory_limit: plan.memory,
                        deadline: limits.deadline,
                        cancellation: limits.cancellation.clone(),
                    },
                )?;
                containers::wrap_paq(&member, *variant, limits.output_bytes)
            }
            PlanKind::Specialist(candidate) => self
                .engine(
                    limits.output_bytes,
                    matches!(
                        candidate.recipe,
                        Recipe::Spatial { .. }
                            | Recipe::Volume { .. }
                            | Recipe::VolumeSharedIdentity { .. }
                    ),
                    matches!(candidate.recipe, Recipe::SpatialAlternative { .. }),
                )?
                .encode_candidate(
                    candidate,
                    data,
                    &OperationLimits {
                        memory_bytes: plan.memory,
                        output_bytes: limits.output_bytes,
                        intermediate_bytes: limits.intermediate_bytes,
                        deadline: limits.deadline,
                        cancellation: limits.cancellation.clone(),
                    },
                ),
        }
        .map(CandidateExecution::bare)
    }
}

/// Run a deterministic first-fit schedule.  Equal-size frames retain their
/// original priority even if a later worker returns first.
pub fn select_whole_input<E: CandidateExecutor>(
    data: &[u8],
    profile: SelectionProfile,
    limits: SelectionLimits,
    executor: E,
) -> Result<SelectionResult, String> {
    select_whole_input_constrained(data, profile, limits, executor, WholeConstraints::default())
}

pub fn select_whole_input_constrained<E: CandidateExecutor>(
    data: &[u8],
    profile: SelectionProfile,
    limits: SelectionLimits,
    executor: E,
    constraints: WholeConstraints,
) -> Result<SelectionResult, String> {
    if data.len() > MAX_WHOLE_INPUT
        || limits.workers == 0
        || limits.memory_bytes == 0
        || limits.temporary_bytes == 0
    {
        return Err("whole-input selection exceeds declared admission".into());
    }
    if cancelled(&limits) {
        return Err("whole-input selection cancelled or deadline exceeded".into());
    }
    let mut discovery = Vec::new();
    let mut trials = Vec::new();
    let queue = plans(
        data,
        profile,
        &limits,
        &mut discovery,
        &mut trials,
        &constraints,
    )?;
    let mut pending: Vec<(usize, Plan)> = queue
        .into_iter()
        .map(|plan| (plan.priority, plan))
        .collect();
    let mut incumbent: Option<(usize, Vec<u8>)> = None;
    let mut peak = (data.len(), 0usize, 0usize);
    let mut aborted = false;
    let accept = |priority: usize,
                  plan: Plan,
                  result: Result<CandidateExecution, String>,
                  incumbent: &mut Option<(usize, Vec<u8>)>,
                  trials: &mut Vec<Trial>| {
        if cancelled(&limits) {
            trials.push(Trial {
                id: plan.id,
                priority,
                status: TrialStatus::BudgetOmitted,
                reason: Some("deadline or cancellation before result acceptance".into()),
                archive_bytes: None,
                details: TrialDetails::None,
            });
            return;
        }
        match result {
            Ok(CandidateExecution {
                archive: frame,
                details,
            }) if frame.len() <= limits.output_bytes && frame.capacity() <= limits.output_bytes => {
                let bytes = frame.len();
                if incumbent
                    .as_ref()
                    .is_none_or(|(old, prior)| (bytes, priority) < (prior.len(), *old))
                {
                    *incumbent = Some((priority, frame));
                }
                trials.push(Trial {
                    id: plan.id,
                    priority,
                    status: TrialStatus::Completed,
                    reason: None,
                    archive_bytes: Some(bytes),
                    details,
                });
            }
            Ok(_) => trials.push(Trial {
                id: plan.id,
                priority,
                status: TrialStatus::Failed,
                reason: Some("candidate returned frame beyond declared output reservation".into()),
                archive_bytes: None,
                details: TrialDetails::None,
            }),
            Err(reason) => trials.push(Trial {
                id: plan.id,
                priority,
                status: TrialStatus::Failed,
                reason: Some(reason),
                archive_bytes: None,
                details: TrialDetails::None,
            }),
        }
    };
    // Raw runs by itself before any concurrent work, matching the historical policy.
    if let Some((priority, raw)) = pending
        .first()
        .cloned()
        .filter(|(_, plan)| plan.id == "raw")
    {
        pending.remove(0);
        let raw_memory = checked_add(data.len(), checked_add(raw.memory, limits.output_bytes)?)?;
        if raw_memory > limits.memory_bytes
            || raw.workers > limits.workers
            || raw.temporary > limits.temporary_bytes
        {
            trials.push(Trial {
                id: raw.id,
                priority,
                status: TrialStatus::BudgetOmitted,
                reason: Some("raw memory/workers/temporary admission".into()),
                archive_bytes: None,
                details: TrialDetails::None,
            });
        } else {
            peak.0 = raw_memory;
            peak.1 = raw.workers;
            peak.2 = raw.temporary;
            let raw_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                executor.run_detailed(&raw, data, &limits)
            }))
            .unwrap_or_else(|_| Err("candidate worker panicked".into()));
            accept(
                priority,
                raw.clone(),
                raw_result,
                &mut incumbent,
                &mut trials,
            );
        }
    }
    std::thread::scope(|scope| -> Result<(), String> {
        use std::sync::mpsc;
        let (sender, receiver) = mpsc::channel();
        let mut running: Vec<(usize, Plan, std::thread::ScopedJoinHandle<'_, ()>)> = Vec::new();
        while !pending.is_empty() || !running.is_empty() {
            if cancelled(&limits) && !pending.is_empty() {
                aborted = true;
                for (priority, plan) in pending.drain(..) {
                    trials.push(Trial {
                        id: plan.id,
                        priority,
                        status: TrialStatus::BudgetOmitted,
                        reason: Some("deadline or cancellation".into()),
                        archive_bytes: None,
                        details: TrialDetails::None,
                    });
                }
            }
            if cancelled(&limits) && running.is_empty() {
                aborted = true;
                break;
            }
            // Admission reserves the fixed output cap for the retained winner,
            // rather than Vec capacity.  That makes future first-fit decisions
            // independent of allocator growth and completion order.
            let retained = checked_add(
                data.len(),
                if incumbent.is_some() {
                    limits.output_bytes
                } else {
                    0
                },
            )?;
            let used_memory = running.iter().try_fold(retained, |total, (_, plan, _)| {
                checked_add(total, checked_add(plan.memory, limits.output_bytes)?)
            })?;
            let used_workers = running.iter().try_fold(0usize, |total, (_, plan, _)| {
                checked_add(total, plan.workers)
            })?;
            let used_temp = running.iter().try_fold(0usize, |total, (_, plan, _)| {
                checked_add(total, plan.temporary)
            })?;
            let mut started = false;
            let mut index = 0;
            while index < pending.len() {
                let (_, plan) = &pending[index];
                let memory =
                    checked_add(used_memory, checked_add(plan.memory, limits.output_bytes)?)?;
                let workers = checked_add(used_workers, plan.workers)?;
                let temp = checked_add(used_temp, plan.temporary)?;
                if memory <= limits.memory_bytes
                    && workers <= limits.workers
                    && temp <= limits.temporary_bytes
                {
                    let (priority, plan) = pending.remove(index);
                    let worker = executor.clone();
                    let job_limits = limits.clone();
                    let tx = sender.clone();
                    peak.0 = peak.0.max(memory);
                    peak.1 = peak.1.max(workers);
                    peak.2 = peak.2.max(temp);
                    let running_plan = plan.clone();
                    let handle = scope.spawn(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            worker.run_detailed(&plan, data, &job_limits)
                        }))
                        .unwrap_or_else(|_| Err("candidate worker panicked".into()));
                        let _ = tx.send((priority, plan, result));
                    });
                    running.push((priority, running_plan, handle));
                    started = true;
                    break;
                }
                index += 1;
            }
            if started {
                continue;
            }
            if running.is_empty() {
                let (priority, plan) = pending.remove(0);
                trials.push(Trial {
                    id: plan.id,
                    priority,
                    status: TrialStatus::BudgetOmitted,
                    reason: Some("memory/workers/temporary first-fit admission".into()),
                    archive_bytes: None,
                    details: TrialDetails::None,
                });
                continue;
            }
            let (priority, plan, result) = receiver
                .recv()
                .map_err(|_| "candidate scheduler channel closed")?;
            if let Some(at) = running.iter().position(|(p, _, _)| *p == priority) {
                let (_, _, handle) = running.swap_remove(at);
                handle.join().map_err(|_| "candidate worker panicked")?;
            }
            accept(priority, plan, result, &mut incumbent, &mut trials);
        }
        Ok(())
    })?;
    if aborted || cancelled(&limits) {
        return Err("whole-input selection cancelled or deadline exceeded".into());
    }
    let (winner_priority, archive) = incumbent.ok_or("no complete candidate archive")?;
    let selected = trials
        .iter()
        .find(|trial| trial.priority == winner_priority)
        .ok_or("missing completed trial")?
        .id
        .clone();
    for trial in &mut trials {
        if trial.priority == winner_priority {
            trial.status = TrialStatus::Selected;
        }
    }
    trials.sort_by_key(|trial| trial.priority);
    Ok(SelectionResult {
        archive,
        report: SelectionReport {
            profile,
            input_bytes: data.len(),
            discovery,
            trials,
            peak_reserved_memory: peak.0,
            peak_reserved_workers: peak.1,
            peak_reserved_temporary: peak.2,
            selected,
        },
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct Fake {
        live: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }
    impl CandidateExecutor for Fake {
        fn run(&self, plan: &Plan, _: &[u8], _: &SelectionLimits) -> Result<Vec<u8>, String> {
            let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(live, Ordering::SeqCst);
            // Reverse generic completion pressure cannot alter priority ties.
            if matches!(plan.kind, PlanKind::Paq { .. }) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            self.live.fetch_sub(1, Ordering::SeqCst);
            Ok(vec![0; 7])
        }
    }
    fn limits() -> SelectionLimits {
        SelectionLimits {
            memory_bytes: 11_000_000_000,
            workers: 2,
            temporary_bytes: 512 << 20,
            output_bytes: 64 << 20,
            intermediate_bytes: 64 << 20,
            deadline: None,
            cancellation: None,
        }
    }
    #[test]
    fn first_priority_wins_equal_complete_frames_and_generic_trials_overlap() {
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let result = select_whole_input(
            b"selector",
            SelectionProfile::Best,
            limits(),
            Fake {
                live,
                peak: peak.clone(),
            },
        )
        .unwrap();
        assert_eq!(result.report.selected, "raw");
        assert!(peak.load(Ordering::SeqCst) >= 2);
        assert!(result.report.peak_reserved_workers <= 2);
        assert!(result.report.peak_reserved_memory <= 11_000_000_000);
        assert!(result.report.peak_reserved_temporary <= 512 << 20);
        // Frames are seven bytes here, yet admission reserves the fixed output
        // cap, so changing a worker's Vec allocation cannot change scheduling.
        assert!(result.report.peak_reserved_memory >= (64 << 20));
    }
    #[test]
    fn nested_diagnostics_survive_outer_selection() {
        #[derive(Clone)]
        struct Detailed;
        impl CandidateExecutor for Detailed {
            fn run(&self, _: &Plan, _: &[u8], _: &SelectionLimits) -> Result<Vec<u8>, String> {
                unreachable!("scheduler must preserve detailed results")
            }
            fn run_detailed(
                &self,
                plan: &Plan,
                _: &[u8],
                _: &SelectionLimits,
            ) -> Result<CandidateExecution, String> {
                Ok(CandidateExecution {
                    archive: vec![0; if plan.id == "native" { 1 } else { 7 }],
                    details: TrialDetails::Native(vec![
                        native_portfolio::NativePortfolioCandidate {
                            id: "inner-size-trial".into(),
                            status: native_portfolio::NativePortfolioStatus::Completed,
                            archive_bytes: Some(1),
                            reason: None,
                        },
                    ]),
                })
            }
        }
        let result =
            select_whole_input(b"diagnostics", SelectionProfile::Fast, limits(), Detailed).unwrap();
        assert_eq!(result.report.selected, "native");
        for trial in &result.report.trials {
            let TrialDetails::Native(inner) = &trial.details else {
                panic!("inner diagnostics lost")
            };
            assert_eq!(inner[0].id, "inner-size-trial");
        }
    }

    #[test]
    fn default_512m_admits_and_records_a_generic_mixed_trial() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/regions/mixed.bin"
        ));
        let limits = SelectionLimits {
            memory_bytes: 512 << 20,
            workers: 1,
            temporary_bytes: 128 << 20,
            output_bytes: 1 << 20,
            intermediate_bytes: 64 << 20,
            deadline: None,
            cancellation: None,
        };
        let executor = NativeExecutor {
            providers: ProviderConfig {
                backends: TrustedBackendPaths {
                    cix: "/not-used/cix".into(),
                    paq_libraries: "/not-used/lib".into(),
                    temporary_root: "/not-used/tmp".into(),
                },
                jxl_bridge: None,
                spatial_bridge: None,
            },
        };
        let result =
            select_whole_input(source, SelectionProfile::Default, limits, executor).unwrap();
        assert!(!result.report.discovery.is_empty());
        assert!(result.report.trials.iter().any(|trial| {
            trial.id == "heterogeneous-cixh1-v1"
                && trial.status == TrialStatus::Completed
                && matches!(trial.details, TrialDetails::Mixed(_))
        }));
    }

    #[test]
    fn malformed_optional_providers_do_not_suppress_generic_mixed_carrier() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/regions/mixed.bin"
        ));
        let executor = NativeExecutor {
            providers: ProviderConfig {
                backends: TrustedBackendPaths {
                    cix: "/not-used/cix".into(),
                    paq_libraries: "/not-used/lib".into(),
                    temporary_root: "/not-used/tmp".into(),
                },
                jxl_bridge: Some("/definitely/missing/cix-jxl-bridge.so".into()),
                spatial_bridge: Some("/definitely/missing/cix-spatial-bridge.so".into()),
            },
        };
        let plan = Plan {
            id: "heterogeneous-cixh1-v1".into(),
            priority: 0,
            kind: PlanKind::Mixed {
                profile: SelectionProfile::Default,
            },
            memory: 64 << 20,
            workers: 1,
            temporary: 2 << 20,
        };
        let limits = SelectionLimits {
            memory_bytes: 64 << 20,
            workers: 1,
            temporary_bytes: 2 << 20,
            output_bytes: 1 << 20,
            intermediate_bytes: 1 << 20,
            deadline: None,
            cancellation: None,
        };
        let result = executor.run_detailed(&plan, source, &limits).unwrap();
        assert!(result.archive.starts_with(b"CIXH1"));
        let TrialDetails::Mixed(diagnostics) = result.details else {
            panic!("mixed diagnostics missing");
        };
        assert_eq!(diagnostics.provider_omissions.len(), 2);
    }

    #[test]
    fn default_marks_paq_specialists_as_profile_omitted() {
        // A validated grid provides PAQ-only specialist recipes without using a codec.
        let mut grid = vec![0u8; 28 * 3];
        grid[8..12].copy_from_slice(&2i32.to_le_bytes());
        grid[12..16].copy_from_slice(&0i32.to_le_bytes());
        grid[16..20].copy_from_slice(&1i32.to_le_bytes());
        grid[20..24].copy_from_slice(&1i32.to_le_bytes());
        grid[24..28].copy_from_slice(&28i32.to_le_bytes());
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let result = select_whole_input(
            &grid,
            SelectionProfile::Default,
            limits(),
            Fake { live, peak },
        )
        .unwrap();
        assert!(result
            .report
            .trials
            .iter()
            .any(|trial| trial.status == TrialStatus::ProfileOmitted));
    }

    #[test]
    fn native_block_tuning_keeps_independent_automatic_plans() {
        let mut discovery = Vec::new();
        let mut trials = Vec::new();
        let constraints = WholeConstraints {
            native: native_portfolio::NativePortfolioConstraints {
                blocks: Some(vec![16 * 1024]),
                ..Default::default()
            },
        };
        let plans = plans(
            b"automatic candidates remain eligible",
            SelectionProfile::Default,
            &limits(),
            &mut discovery,
            &mut trials,
            &constraints,
        )
        .unwrap();
        assert!(plans.iter().any(|plan| plan.id == "raw"));
        assert!(plans.iter().any(|plan| plan.id == "heterogeneous-cixh1-v1"));
        assert!(!trials.iter().any(|trial| trial
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("native container constraint"))));
        let native = plans.iter().find(|plan| plan.id == "native").unwrap();
        let PlanKind::Native { constraints, .. } = &native.kind else {
            panic!("native plan lost its tuning constraints")
        };
        assert_eq!(constraints.blocks, Some(vec![16 * 1024]));
    }

    fn valid_grid_fixture() -> Vec<u8> {
        let mut grid = vec![0u8; 28 * 3];
        grid[8..12].copy_from_slice(&2i32.to_le_bytes());
        grid[12..16].copy_from_slice(&0i32.to_le_bytes());
        grid[16..20].copy_from_slice(&1i32.to_le_bytes());
        grid[20..24].copy_from_slice(&1i32.to_le_bytes());
        grid[24..28].copy_from_slice(&28i32.to_le_bytes());
        grid
    }

    #[test]
    fn backend_tuning_keeps_catalogue_and_mixed_discovery_for_supported_and_unsupported_routes() {
        for backend in ["deflate", "huffman"] {
            let mut discovery = Vec::new();
            let mut trials = Vec::new();
            let constraints = WholeConstraints {
                native: native_portfolio::NativePortfolioConstraints {
                    backend: Some(backend.into()),
                    ..Default::default()
                },
            };
            let plans = plans(
                &valid_grid_fixture(),
                SelectionProfile::Default,
                &limits(),
                &mut discovery,
                &mut trials,
                &constraints,
            )
            .unwrap();
            assert!(
                trials.iter().any(|trial| {
                    trial.status == TrialStatus::ProfileOmitted
                        && trial.id.starts_with("bounded-grid-mode")
                        && trial.reason.as_deref()
                            == Some("DEFAULT omits PAQ-backed specialist trials")
                }),
                "{backend} must retain the catalogue-backed PAQ specialist outcome"
            );
            assert!(plans.iter().any(|plan| plan.id == "heterogeneous-cixh1-v1"));
            let native = plans.iter().find(|plan| plan.id == "native").unwrap();
            let PlanKind::Native { constraints, .. } = &native.kind else {
                panic!("native plan missing")
            };
            assert_eq!(constraints.backend.as_deref(), Some(backend));
            assert!(!trials.iter().any(|trial| trial.reason.as_deref()
                == Some("explicit native container constraint excludes incompatible wire")));
        }
    }

    #[test]
    fn forced_native_format_excludes_independent_wires_with_reason() {
        let mut discovery = Vec::new();
        let mut trials = Vec::new();
        let constraints = WholeConstraints {
            native: native_portfolio::NativePortfolioConstraints {
                formats: Some(vec!["cixg1".into()]),
                ..Default::default()
            },
        };
        let plans = plans(
            b"forced native container",
            SelectionProfile::Default,
            &limits(),
            &mut discovery,
            &mut trials,
            &constraints,
        )
        .unwrap();
        assert!(plans.iter().all(|plan| plan.id == "native"));
        assert!(trials.iter().all(|trial| trial.reason.as_deref()
            == Some("explicit native container constraint excludes incompatible wire")));
    }
}

#[cfg(test)]
mod cancellation_regressions {
    use super::*;

    #[derive(Clone)]
    struct Panic;
    impl CandidateExecutor for Panic {
        fn run(&self, _: &Plan, _: &[u8], _: &SelectionLimits) -> Result<Vec<u8>, String> {
            panic!("test executor panic")
        }
    }
    #[derive(Clone)]
    struct CancelAfterRaw {
        token: Arc<AtomicBool>,
    }
    impl CandidateExecutor for CancelAfterRaw {
        fn run(&self, plan: &Plan, _: &[u8], _: &SelectionLimits) -> Result<Vec<u8>, String> {
            if plan.id == "raw" {
                self.token.store(true, Ordering::Release);
            }
            Ok(vec![1])
        }
    }
    fn limits(token: Option<Arc<AtomicBool>>) -> SelectionLimits {
        SelectionLimits {
            memory_bytes: 11_000_000_000,
            workers: 2,
            temporary_bytes: 512 << 20,
            output_bytes: 64 << 20,
            intermediate_bytes: 64 << 20,
            deadline: None,
            cancellation: token,
        }
    }
    #[test]
    fn pre_cancel_is_terminal() {
        let token = Arc::new(AtomicBool::new(true));
        assert!(
            select_whole_input(b"x", SelectionProfile::Fast, limits(Some(token)), Panic).is_err()
        );
    }
    #[test]
    fn cancellation_after_raw_never_returns_archive() {
        let token = Arc::new(AtomicBool::new(false));
        assert!(select_whole_input(
            b"x",
            SelectionProfile::Fast,
            limits(Some(token.clone())),
            CancelAfterRaw { token }
        )
        .is_err());
    }
    #[test]
    fn panicking_worker_is_recorded_without_scheduler_hang() {
        assert!(select_whole_input(b"x", SelectionProfile::Fast, limits(None), Panic).is_err());
    }
}
