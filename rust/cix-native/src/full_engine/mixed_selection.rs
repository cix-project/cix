//! Bounded automatic CIXH1 selection over input-derived regions.
//!
//! `mixed` owns the CIXH1 wire format and restoration checks.  This module
//! supplies the historical heterogeneous policy: every region first competes
//! stored, zlib-6, and xz-preset-3; a recognised region may additionally try
//! a bounded, family-round-robin slice of the normal specialist catalogue.
//! It never calls whole-input selection on a region.

use super::{
    backend_provider::{BackendRequest, BackendVariant},
    catalogue::{self, Candidate, Recipe},
    dispatch::{FullEngine, OperationLimits},
    mixed::{self, RegionBackend, RegionChoice},
    regions::{self, Region, RegionKind},
    selection::SelectionProfile,
};
use std::{
    sync::{atomic::Ordering, mpsc},
    time::Instant,
};

pub const MAX_NESTED_SPECIALISTS: usize = 8;
pub const MAX_NESTED_MEMORY: usize = 5 << 30;
pub const MAX_NESTED_TOTAL_MEMORY: usize = 10 << 30;
const HEADER_BYTES: usize = 50;
const REGION_HEADER_BYTES: usize = 43;
const MAX_DIAGNOSTICS: usize = 4096;
const DIAGNOSTIC_TEXT_BYTES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticStatus {
    Considered,
    Omitted,
    Failed,
    Selected,
}

/// A bounded record of an automatic decision.  `detail` is a fixed policy or
/// provider error string, truncated before retention so malformed providers
/// cannot make an archive request retain unbounded diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MixedDiagnostic {
    pub region: usize,
    pub region_kind: RegionKind,
    pub candidate: String,
    pub status: DiagnosticStatus,
    pub backend: Option<RegionBackend>,
    pub payload_bytes: Option<usize>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MixedDiagnostics {
    pub entries: Vec<MixedDiagnostic>,
    pub omitted_entries: usize,
    /// Optional provider-load failures retained separately from per-region
    /// codec decisions. Generic CIXH1 selection stays available in their
    /// presence.
    pub provider_omissions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct MixedSelectionResult {
    pub archive: Vec<u8>,
    pub diagnostics: MixedDiagnostics,
}

fn checked_add(left: usize, right: usize) -> Result<usize, String> {
    left.checked_add(right)
        .ok_or_else(|| "CIXH1 selection size overflow".into())
}

fn frame_cost(choice: &RegionChoice) -> Result<usize, String> {
    checked_add(
        REGION_HEADER_BYTES,
        checked_add(choice.parameters.len(), choice.payload.len())?,
    )
}

fn retained_choice_bytes(choice: &RegionChoice) -> Result<usize, String> {
    checked_add(choice.parameters.capacity(), choice.payload.capacity())
}

/// A strictly smaller payload is required to replace `incumbent`; this charges
/// both the new region header and its paid parameters before a provider runs.
fn improving_payload_cap(
    incumbent: &RegionChoice,
    parameter_bytes: usize,
    remaining_output: usize,
) -> Result<usize, String> {
    let paid = checked_add(REGION_HEADER_BYTES, parameter_bytes)?;
    let incumbent = frame_cost(incumbent)?;
    let strict = incumbent
        .checked_sub(paid)
        .and_then(|value| value.checked_sub(1))
        .ok_or("CIXH1 candidate cannot improve paid incumbent")?;
    let frame_remaining = remaining_output
        .checked_sub(paid)
        .ok_or("CIXH1 region frame exceeds remaining output")?;
    Ok(strict.min(frame_remaining))
}

fn owned_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(bytes.len())
        .map_err(|_| "CIXH1 selection allocation failed")?;
    output.extend_from_slice(bytes);
    Ok(output)
}

fn truncate_detail(error: impl AsRef<str>) -> String {
    const MAX: usize = 160;
    let value = error.as_ref();
    if value.len() <= MAX {
        value.to_owned()
    } else {
        let mut end = MAX;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &value[..end])
    }
}

fn push_diagnostic(diagnostics: &mut MixedDiagnostics, row: MixedDiagnostic) {
    if diagnostics.entries.len() < MAX_DIAGNOSTICS {
        diagnostics.entries.push(row);
    } else {
        diagnostics.omitted_entries = diagnostics.omitted_entries.saturating_add(1);
    }
}

fn diagnostic_buffer() -> Result<MixedDiagnostics, String> {
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(MAX_DIAGNOSTICS)
        .map_err(|_| "CIXH1 diagnostic allocation failed")?;
    Ok(MixedDiagnostics {
        entries,
        omitted_entries: 0,
        provider_omissions: Vec::new(),
    })
}

fn selector_overhead(region_count: usize) -> Result<usize, String> {
    let regions = region_count
        .checked_mul(std::mem::size_of::<Region>())
        .ok_or("CIXH1 region reservation overflow")?;
    let ranges = region_count
        .checked_mul(std::mem::size_of::<std::ops::Range<usize>>())
        .ok_or("CIXH1 range reservation overflow")?;
    let diagnostics = MAX_DIAGNOSTICS
        .checked_mul(std::mem::size_of::<MixedDiagnostic>())
        .and_then(|size| size.checked_add(MAX_DIAGNOSTICS * DIAGNOSTIC_TEXT_BYTES))
        .ok_or("CIXH1 diagnostic reservation overflow")?;
    checked_add(checked_add(regions, ranges)?, diagnostics)
}

fn is_paq(candidate: &Candidate) -> bool {
    match &candidate.recipe {
        Recipe::Geometry { .. }
        | Recipe::Hydrogen
        | Recipe::GroupedGrid
        | Recipe::BoundedGrid { .. } => true,
        Recipe::RecordConstraints { paq, .. } | Recipe::Address { paq, .. } => *paq,
        Recipe::FixedColumns
        | Recipe::Spatial { .. }
        | Recipe::SpatialAlternative { .. }
        | Recipe::Volume { .. }
        | Recipe::VolumeSharedIdentity { .. } => false,
    }
}

/// Preserve catalogue order within a family, but alternate families so one
/// large family cannot consume the eight nested attempts by itself.
fn round_robin(candidates: Vec<Candidate>) -> Vec<Candidate> {
    // Python's insertion-ordered dictionary keeps the first catalogue family
    // first.  A sorted map would silently change tie/attempt ordering.
    let mut families: Vec<(&'static str, Vec<Candidate>)> = Vec::new();
    for candidate in candidates {
        if let Some((_, values)) = families
            .iter_mut()
            .find(|(family, _)| *family == candidate.family)
        {
            values.push(candidate);
        } else {
            families.push((candidate.family, vec![candidate]));
        }
    }
    let mut positions = vec![0usize; families.len()];
    let mut ordered = Vec::new();
    loop {
        let mut advanced = false;
        for (index, (_, candidates)) in families.iter().enumerate() {
            if let Some(candidate) = candidates.get(positions[index]) {
                ordered.push(candidate.clone());
                positions[index] += 1;
                advanced = true;
            }
        }
        if !advanced {
            return ordered;
        }
    }
}

fn generic_choice(
    source: &[u8],
    workspace: usize,
    output_limit: usize,
    mut encode: impl FnMut(BackendVariant, usize, usize) -> Result<Vec<u8>, String>,
    region: usize,
    kind: RegionKind,
    diagnostics: &mut MixedDiagnostics,
) -> Result<RegionChoice, String> {
    if source.len() > workspace || source.len() > output_limit {
        return Err("CIXH1 generic stored region exceeds workspace/output limit".into());
    }
    let mut best = RegionChoice {
        backend: RegionBackend::Stored,
        parameters: Vec::new(),
        payload: owned_bytes(source)?,
    };
    // Tie order is the wire backend value. Stored therefore remains incumbent
    // on equal complete frame costs, matching the Python `min` tuple policy.
    for (backend, parameters, variant, name) in [
        (
            RegionBackend::Zlib,
            b"level=6".as_slice(),
            BackendVariant::Zlib { level: 6 },
            "zlib-6",
        ),
        (
            RegionBackend::Xz,
            b"preset=3".as_slice(),
            BackendVariant::Xz { preset: 3 },
            "xz-preset-3",
        ),
    ] {
        let parameters = owned_bytes(parameters)?;
        let retained = checked_add(retained_choice_bytes(&best)?, parameters.capacity())?;
        let provider_memory = workspace
            .checked_sub(retained)
            .ok_or("CIXH1 generic retained-buffer limit")?;
        let payload_cap = match improving_payload_cap(&best, parameters.len(), output_limit) {
            Ok(cap) => cap,
            Err(error) => {
                push_diagnostic(
                    diagnostics,
                    MixedDiagnostic {
                        region,
                        region_kind: kind,
                        candidate: name.into(),
                        status: DiagnosticStatus::Omitted,
                        backend: Some(backend),
                        payload_bytes: None,
                        detail: Some(truncate_detail(error)),
                    },
                );
                continue;
            }
        };
        match encode(variant, payload_cap, provider_memory) {
            Ok(payload) if payload.len() <= payload_cap && payload.capacity() <= payload_cap => {
                let payload_bytes = payload.len();
                let choice = RegionChoice {
                    backend,
                    parameters,
                    payload,
                };
                if frame_cost(&choice)? < frame_cost(&best)? {
                    best = choice;
                }
                push_diagnostic(
                    diagnostics,
                    MixedDiagnostic {
                        region,
                        region_kind: kind,
                        candidate: name.into(),
                        status: DiagnosticStatus::Considered,
                        backend: Some(backend),
                        payload_bytes: Some(payload_bytes),
                        detail: None,
                    },
                );
            }
            Ok(_) => push_diagnostic(
                diagnostics,
                MixedDiagnostic {
                    region,
                    region_kind: kind,
                    candidate: name.into(),
                    status: DiagnosticStatus::Failed,
                    backend: Some(backend),
                    payload_bytes: None,
                    detail: Some("provider exceeded declared output cap".into()),
                },
            ),
            Err(error) => push_diagnostic(
                diagnostics,
                MixedDiagnostic {
                    region,
                    region_kind: kind,
                    candidate: name.into(),
                    status: DiagnosticStatus::Failed,
                    backend: Some(backend),
                    payload_bytes: None,
                    detail: Some(truncate_detail(error)),
                },
            ),
        }
    }
    Ok(best)
}

/// Zlib and XZ both compare against the stored incumbent.  Their initial
/// encodes can therefore run together when the caller reserved two workers
/// and two complete candidate buffers.  If that conservative admission is
/// unavailable, callers retain the historical serial path.
fn run_parallel_generic_trials<T, F>(run: F) -> Result<Vec<Result<T, String>>, String>
where
    T: Send,
    F: Fn(usize) -> Result<T, String> + Sync,
{
    let context = crate::limits::capture_context();
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let mut handles = Vec::new();
        for index in 0..2 {
            let tx = sender.clone();
            let context = context.clone();
            let task = &run;
            handles.push(scope.spawn(move || {
                let _context = crate::limits::install_context(&context);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(index)))
                    .unwrap_or_else(|_| Err("CIXH1 generic trial worker panicked".into()));
                let _ = tx.send((index, result));
            }));
        }
        drop(sender);
        let mut results = vec![None, None];
        for _ in 0..2 {
            let (index, result) = receiver
                .recv()
                .map_err(|_| "CIXH1 generic scheduler channel closed")?;
            results[index] = Some(result);
        }
        for handle in handles {
            handle
                .join()
                .map_err(|_| "CIXH1 generic trial worker panicked")?;
        }
        results
            .into_iter()
            .map(|result| result.ok_or_else(|| "CIXH1 generic result missing".into()))
            .collect()
    })
}

fn speculative_failure_requires_serial_retry<T>(results: &[Result<T, String>]) -> bool {
    results.iter().any(Result::is_err)
}

// The independent limits and scheduling inputs mirror the selection contract.
#[allow(clippy::too_many_arguments)]
fn parallel_generic_choice(
    engine: &FullEngine,
    source: &[u8],
    workspace: usize,
    output_limit: usize,
    region: usize,
    kind: RegionKind,
    diagnostics: &mut MixedDiagnostics,
    limits: &OperationLimits,
    workers: usize,
) -> Option<Result<RegionChoice, String>> {
    if workers < 2 || source.len() > workspace || source.len() > output_limit {
        return None;
    }
    if let Err(error) = check_limits(limits) {
        return Some(Err(error));
    }
    let stored = RegionChoice {
        backend: RegionBackend::Stored,
        parameters: Vec::new(),
        payload: match owned_bytes(source) {
            Ok(payload) => payload,
            Err(error) => return Some(Err(error)),
        },
    };
    let attempts = [
        (
            RegionBackend::Zlib,
            b"level=6".as_slice(),
            BackendVariant::Zlib { level: 6 },
            "zlib-6",
        ),
        (
            RegionBackend::Xz,
            b"preset=3".as_slice(),
            BackendVariant::Xz { preset: 3 },
            "xz-preset-3",
        ),
    ];
    let mut parameters: Vec<Vec<u8>> = match attempts
        .iter()
        .map(|(_, parameters, _, _)| owned_bytes(parameters))
        .collect()
    {
        Ok(parameters) => parameters,
        Err(error) => return Some(Err(error)),
    };
    let mut caps = Vec::with_capacity(attempts.len());
    for parameters in &parameters {
        match improving_payload_cap(&stored, parameters.len(), output_limit) {
            Ok(cap) => caps.push(cap),
            Err(_) => return None,
        }
    }
    let retained = match checked_add(
        retained_choice_bytes(&stored).ok()?,
        parameters
            .iter()
            .try_fold(0usize, |total, value| {
                total
                    .checked_add(value.capacity())
                    .ok_or("CIXH1 generic parameter reservation")
            })
            .ok()?,
    ) {
        Ok(value) => value,
        Err(error) => return Some(Err(error)),
    };
    let payload_reservation = match caps.iter().try_fold(0usize, |total, cap| {
        total
            .checked_add(*cap)
            .ok_or("CIXH1 generic payload reservation")
    }) {
        Ok(value) => value,
        Err(error) => return Some(Err(error.into())),
    };
    let available = workspace
        .checked_sub(retained)
        .and_then(|value| value.checked_sub(payload_reservation))?;
    let per_trial_memory = available / attempts.len();
    if per_trial_memory == 0 {
        return None;
    }
    let backends = &engine.backends;
    let mut results = match run_parallel_generic_trials(|index| {
        let (_, _, variant, _) = attempts[index];
        let request = BackendRequest {
            variant,
            output_limit: caps[index],
            memory_limit: per_trial_memory,
            deadline: limits.deadline,
            cancellation: limits.cancellation.clone(),
        };
        backends.encode(source, &request)
    }) {
        Ok(results) => results,
        Err(error) => return Some(Err(error)),
    };
    if let Err(error) = check_limits(limits) {
        return Some(Err(error));
    }
    // Any speculative provider failure may be a consequence of the
    // split reservation. Retry serially rather than dropping a candidate
    // that the historical one-at-a-time path could admit.
    if speculative_failure_requires_serial_retry(&results) {
        return None;
    }
    // A successful Vec can have spare capacity beyond the canonical cap.
    // Keeping it would reject bytes that a serial, smaller allocation can
    // retain, so fall back before producing diagnostics.
    let stored_cost = frame_cost(&stored).ok()?;
    let zlib = results[0].as_ref().ok()?;
    let zlib_cap = improving_payload_cap(&stored, parameters[0].len(), output_limit).ok()?;
    if zlib.len() <= zlib_cap && zlib.capacity() > zlib_cap {
        return None;
    }
    let zlib_cost = checked_add(
        REGION_HEADER_BYTES,
        checked_add(parameters[0].len(), zlib.len()).ok()?,
    )
    .ok()?;
    let xz_incumbent_cost = if zlib.len() <= zlib_cap && zlib_cost < stored_cost {
        zlib_cost
    } else {
        stored_cost
    };
    let xz_paid = checked_add(REGION_HEADER_BYTES, parameters[1].len()).ok()?;
    let xz_cap = xz_incumbent_cost
        .checked_sub(xz_paid)
        .and_then(|value| value.checked_sub(1))?
        .min(output_limit.checked_sub(xz_paid)?);
    let xz = results[1].as_ref().ok()?;
    if xz.len() <= xz_cap && xz.capacity() > xz_cap {
        return None;
    }
    let mut best = stored;
    for (index, (backend, _, _, name)) in attempts.iter().enumerate() {
        let parameters = std::mem::take(&mut parameters[index]);
        let canonical_cap = match improving_payload_cap(&best, parameters.len(), output_limit) {
            Ok(cap) => cap,
            Err(error) => {
                push_diagnostic(
                    diagnostics,
                    MixedDiagnostic {
                        region,
                        region_kind: kind,
                        candidate: (*name).into(),
                        status: DiagnosticStatus::Omitted,
                        backend: Some(*backend),
                        payload_bytes: None,
                        detail: Some(truncate_detail(error)),
                    },
                );
                continue;
            }
        };
        match std::mem::replace(
            &mut results[index],
            Err("CIXH1 generic result already consumed".into()),
        ) {
            Ok(payload)
                if payload.len() <= canonical_cap && payload.capacity() <= canonical_cap =>
            {
                let bytes = payload.len();
                let choice = RegionChoice {
                    backend: *backend,
                    parameters,
                    payload,
                };
                if frame_cost(&choice).ok()? < frame_cost(&best).ok()? {
                    best = choice;
                }
                push_diagnostic(
                    diagnostics,
                    MixedDiagnostic {
                        region,
                        region_kind: kind,
                        candidate: (*name).into(),
                        status: DiagnosticStatus::Considered,
                        backend: Some(*backend),
                        payload_bytes: Some(bytes),
                        detail: None,
                    },
                );
            }
            Ok(_) => push_diagnostic(
                diagnostics,
                MixedDiagnostic {
                    region,
                    region_kind: kind,
                    candidate: (*name).into(),
                    status: DiagnosticStatus::Failed,
                    backend: Some(*backend),
                    payload_bytes: None,
                    detail: Some("provider exceeded canonical output cap".into()),
                },
            ),
            Err(error) => push_diagnostic(
                diagnostics,
                MixedDiagnostic {
                    region,
                    region_kind: kind,
                    candidate: (*name).into(),
                    status: DiagnosticStatus::Failed,
                    backend: Some(*backend),
                    payload_bytes: None,
                    detail: Some(truncate_detail(error)),
                },
            ),
        }
    }
    Some(Ok(best))
}

fn omission(
    candidate: &Candidate,
    profile: SelectionProfile,
    source_bytes: usize,
    parent_bytes: usize,
    retained_bytes: usize,
    accepted: usize,
) -> Option<&'static str> {
    if profile == SelectionProfile::Fast {
        return Some("fast profile uses generic region backends");
    }
    if profile == SelectionProfile::Default && is_paq(candidate) {
        return Some("default profile omits nested PAQ");
    }
    if candidate.memory_bytes > MAX_NESTED_MEMORY {
        return Some("nested resource policy");
    }
    if accepted >= MAX_NESTED_SPECIALISTS {
        return Some("nested candidate count bound");
    }
    parent_bytes
        .checked_add(source_bytes)
        .and_then(|total| total.checked_add(retained_bytes))
        .and_then(|total| total.checked_add(candidate.memory_bytes))
        .is_none_or(|total| total > MAX_NESTED_TOTAL_MEMORY)
        .then_some("nested aggregate memory policy")
}

fn check_limits(limits: &OperationLimits) -> Result<(), String> {
    if limits.memory_bytes == 0 || limits.intermediate_bytes == 0 {
        return Err("invalid CIXH1 selection limits".into());
    }
    if limits
        .cancellation
        .as_ref()
        .is_some_and(|token| token.load(Ordering::Acquire))
    {
        return Err("CIXH1 selection cancelled".into());
    }
    if limits
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        return Err("CIXH1 selection deadline exceeded".into());
    }
    Ok(())
}

/// Encode one automatic CIXH1 archive.  The caller passes its complete
/// operation limits; this function neither starts recursive whole-input
/// selection nor relaxes provider cancellation/deadline requests.
pub fn encode(
    engine: &FullEngine,
    source: &[u8],
    profile: SelectionProfile,
    limits: &OperationLimits,
) -> Result<MixedSelectionResult, String> {
    encode_with_provider_omissions(engine, source, profile, limits, &[])
}

/// As [`encode`], while retaining bounded optional-provider load omissions.
/// These omissions never make generic CIXH1 unavailable.
pub fn encode_with_provider_omissions(
    engine: &FullEngine,
    source: &[u8],
    profile: SelectionProfile,
    limits: &OperationLimits,
    provider_omissions: &[String],
) -> Result<MixedSelectionResult, String> {
    encode_with_provider_omissions_workers(engine, source, profile, limits, provider_omissions, 1)
}

/// As [`encode_with_provider_omissions`], with a worker reservation supplied
/// by the outer whole-input scheduler.
pub(crate) fn encode_with_provider_omissions_workers(
    engine: &FullEngine,
    source: &[u8],
    profile: SelectionProfile,
    limits: &OperationLimits,
    provider_omissions: &[String],
    workers: usize,
) -> Result<MixedSelectionResult, String> {
    check_limits(limits)?;
    if limits.output_bytes == 0 {
        return Err("invalid CIXH1 selection limits".into());
    }
    if source.len() > limits.intermediate_bytes {
        return Err("CIXH1 source exceeds intermediate admission".into());
    }
    let region_upper = source
        .len()
        .checked_add(regions::MAX_GENERIC_REGION - 1)
        .ok_or("CIXH1 region count overflow")?
        / regions::MAX_GENERIC_REGION;
    let region_upper = region_upper
        .checked_add(2 * regions::MAX_STRUCTURED_REGIONS)
        .ok_or("CIXH1 region count overflow")?;
    if region_upper > mixed::MAX_REGIONS {
        return Err("too many CIXH1 regions".into());
    }
    let overhead = selector_overhead(region_upper)?;
    let carrier_memory = limits
        .memory_bytes
        .checked_sub(overhead)
        .ok_or("CIXH1 selection workspace reservation")?;
    if checked_add(source.len(), HEADER_BYTES)? > carrier_memory {
        return Err("CIXH1 source/selection memory admission".into());
    }
    let partitions = regions::regions(source);
    if partitions.len() > mixed::MAX_REGIONS {
        return Err("too many CIXH1 regions".into());
    }
    let mut ranges = Vec::new();
    ranges
        .try_reserve_exact(partitions.len())
        .map_err(|_| "CIXH1 range allocation failed")?;
    ranges.extend(partitions.iter().map(|item| item.start..item.end));
    let mut diagnostics = diagnostic_buffer()?;
    diagnostics
        .provider_omissions
        .extend(provider_omissions.iter().take(4).map(truncate_detail));
    let mut parent_bytes = HEADER_BYTES;
    let mut region_number = 0usize;
    let archive = mixed::encode_with(
        source,
        &ranges,
        limits.output_bytes,
        carrier_memory,
        |region, workspace| {
            check_limits(limits)?;
            let descriptor = partitions
                .get(region_number)
                .copied()
                .ok_or("CIXH1 region selection position")?;
            let number = region_number;
            region_number = region_number
                .checked_add(1)
                .ok_or("CIXH1 region count overflow")?;
            let generic_limit = limits.output_bytes.saturating_sub(parent_bytes);
            let mut best = match parallel_generic_choice(
                engine,
                region,
                workspace,
                generic_limit,
                number,
                descriptor.kind,
                &mut diagnostics,
                limits,
                workers,
            ) {
                Some(result) => result?,
                None => generic_choice(
                    region,
                    workspace,
                    generic_limit,
                    |variant, output_limit, memory_limit| {
                        engine.backends.encode(
                            region,
                            &BackendRequest {
                                variant,
                                output_limit,
                                memory_limit,
                                deadline: limits.deadline,
                                cancellation: limits.cancellation.clone(),
                            },
                        )
                    },
                    number,
                    descriptor.kind,
                    &mut diagnostics,
                )?,
            };
            if descriptor.kind != RegionKind::Generic {
                // Catalogue failure is an observable specialist omission, not
                // permission to discard the already-valid generic incumbent.
                let candidates = match catalogue::catalog(region) {
                    Ok(catalogue) => catalogue.candidates,
                    Err(error) => {
                        push_diagnostic(
                            &mut diagnostics,
                            MixedDiagnostic {
                                region: number,
                                region_kind: descriptor.kind,
                                candidate: "specialist-catalogue".into(),
                                status: DiagnosticStatus::Failed,
                                backend: Some(RegionBackend::Specialist),
                                payload_bytes: None,
                                detail: Some(truncate_detail(error)),
                            },
                        );
                        Vec::new()
                    }
                };
                let mut accepted = 0usize;
                for candidate in round_robin(candidates) {
                    let retained = best.payload.capacity();
                    if let Some(reason) = omission(
                        &candidate,
                        profile,
                        region.len(),
                        parent_bytes,
                        retained,
                        accepted,
                    ) {
                        push_diagnostic(
                            &mut diagnostics,
                            MixedDiagnostic {
                                region: number,
                                region_kind: descriptor.kind,
                                candidate: candidate.id,
                                status: DiagnosticStatus::Omitted,
                                backend: Some(RegionBackend::Specialist),
                                payload_bytes: None,
                                detail: Some(reason.into()),
                            },
                        );
                        continue;
                    }
                    let parameters = owned_bytes(candidate.id.as_bytes())?;
                    let retained =
                        checked_add(retained_choice_bytes(&best)?, parameters.capacity())?;
                    let Some(available) = workspace.checked_sub(retained) else {
                        push_diagnostic(
                            &mut diagnostics,
                            MixedDiagnostic {
                                region: number,
                                region_kind: descriptor.kind,
                                candidate: candidate.id,
                                status: DiagnosticStatus::Omitted,
                                backend: Some(RegionBackend::Specialist),
                                payload_bytes: None,
                                detail: Some("nested operation workspace policy".into()),
                            },
                        );
                        continue;
                    };
                    if candidate.memory_bytes > available {
                        push_diagnostic(
                            &mut diagnostics,
                            MixedDiagnostic {
                                region: number,
                                region_kind: descriptor.kind,
                                candidate: candidate.id,
                                status: DiagnosticStatus::Omitted,
                                backend: Some(RegionBackend::Specialist),
                                payload_bytes: None,
                                detail: Some("nested operation workspace policy".into()),
                            },
                        );
                        continue;
                    }
                    let output_bytes =
                        match improving_payload_cap(&best, parameters.len(), generic_limit) {
                            Ok(cap) => cap,
                            Err(error) => {
                                push_diagnostic(
                                    &mut diagnostics,
                                    MixedDiagnostic {
                                        region: number,
                                        region_kind: descriptor.kind,
                                        candidate: candidate.id,
                                        status: DiagnosticStatus::Omitted,
                                        backend: Some(RegionBackend::Specialist),
                                        payload_bytes: None,
                                        detail: Some(truncate_detail(error)),
                                    },
                                );
                                continue;
                            }
                        };
                    let nested_limits = OperationLimits {
                        memory_bytes: candidate.memory_bytes,
                        output_bytes,
                        intermediate_bytes: limits.intermediate_bytes.min(available),
                        deadline: limits.deadline,
                        cancellation: limits.cancellation.clone(),
                    };
                    match engine.encode_candidate(&candidate, region, &nested_limits) {
                        Ok(payload) => {
                            accepted = accepted
                                .checked_add(1)
                                .ok_or("CIXH1 accepted count overflow")?;
                            let choice = RegionChoice {
                                backend: RegionBackend::Specialist,
                                parameters,
                                payload,
                            };
                            let cost = frame_cost(&choice)?;
                            push_diagnostic(
                                &mut diagnostics,
                                MixedDiagnostic {
                                    region: number,
                                    region_kind: descriptor.kind,
                                    candidate: candidate.id,
                                    status: DiagnosticStatus::Considered,
                                    backend: Some(RegionBackend::Specialist),
                                    payload_bytes: Some(choice.payload.len()),
                                    detail: None,
                                },
                            );
                            if cost < frame_cost(&best)? {
                                best = choice;
                            }
                        }
                        Err(error) => push_diagnostic(
                            &mut diagnostics,
                            MixedDiagnostic {
                                region: number,
                                region_kind: descriptor.kind,
                                candidate: candidate.id,
                                status: DiagnosticStatus::Failed,
                                backend: Some(RegionBackend::Specialist),
                                payload_bytes: None,
                                detail: Some(truncate_detail(error)),
                            },
                        ),
                    }
                }
            } else {
                push_diagnostic(
                    &mut diagnostics,
                    MixedDiagnostic {
                        region: number,
                        region_kind: descriptor.kind,
                        candidate: "specialists".into(),
                        status: DiagnosticStatus::Omitted,
                        backend: Some(RegionBackend::Specialist),
                        payload_bytes: None,
                        detail: Some("unknown generic region".into()),
                    },
                );
            }
            parent_bytes = checked_add(parent_bytes, frame_cost(&best)?)?;
            push_diagnostic(
                &mut diagnostics,
                MixedDiagnostic {
                    region: number,
                    region_kind: descriptor.kind,
                    candidate: "region".into(),
                    status: DiagnosticStatus::Selected,
                    backend: Some(best.backend),
                    payload_bytes: Some(best.payload.len()),
                    detail: None,
                },
            );
            Ok(best)
        },
    )?;
    check_limits(limits)?;
    Ok(MixedSelectionResult {
        archive,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::full_engine::backend_provider::{BackendProvider, TrustedBackendPaths};
    use std::sync::{atomic::AtomicUsize, atomic::Ordering, Arc, Barrier};

    #[test]
    fn generic_competition_preserves_stored_bytes_when_backends_fail() {
        let mut diagnostics = MixedDiagnostics::default();
        let choice = generic_choice(
            b"abcdefghijklmnopqrstuvwxyz",
            128,
            128,
            |variant, _, _| match variant {
                BackendVariant::Zlib { .. } => Err("unavailable".into()),
                BackendVariant::Xz { .. } => Err("unavailable".into()),
                _ => unreachable!(),
            },
            0,
            RegionKind::Generic,
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(choice.backend, RegionBackend::Stored);
        assert!(diagnostics
            .entries
            .iter()
            .any(|row| row.status == DiagnosticStatus::Failed));
    }

    #[test]
    fn round_robin_alternates_catalogue_families() {
        let make = |id: &str, family: &'static str| Candidate {
            id: id.into(),
            family,
            recipe: Recipe::FixedColumns,
            memory_bytes: 1,
        };
        let ids = round_robin(vec![make("b0", "b"), make("b1", "b"), make("a0", "a")])
            .into_iter()
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["b0", "a0", "b1"]);
    }

    #[test]
    fn generic_parallel_runner_overlaps_two_trials_and_returns_index_order() {
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let results = run_parallel_generic_trials({
            let live = live.clone();
            let peak = peak.clone();
            let barrier = barrier.clone();
            move |index| {
                let current = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                barrier.wait();
                live.fetch_sub(1, Ordering::SeqCst);
                Ok(index)
            }
        })
        .unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(results, vec![Ok(0), Ok(1)]);
    }

    #[test]
    fn generic_parallel_backend_failure_requires_serial_retry() {
        let results = vec![Ok::<_, String>(vec![1]), Err("provider failed".into())];
        assert!(speculative_failure_requires_serial_retry(&results));
    }

    #[test]
    fn worker_reserved_generic_selection_matches_serial_archive_and_decode() {
        let engine = FullEngine {
            backends: BackendProvider {
                paths: TrustedBackendPaths {
                    cix: "/not-used/cix".into(),
                    paq_libraries: "/not-used/lib".into(),
                    temporary_root: "/not-used/tmp".into(),
                },
            },
            images: None,
            spatial: None,
        };
        let source = b"parallel generic region ".repeat(4096);
        let limits = OperationLimits {
            memory_bytes: 1 << 30,
            output_bytes: 1 << 20,
            intermediate_bytes: 1 << 20,
            deadline: None,
            cancellation: None,
        };
        let serial = encode_with_provider_omissions_workers(
            &engine,
            &source,
            SelectionProfile::Default,
            &limits,
            &[],
            1,
        )
        .unwrap();
        let parallel = encode_with_provider_omissions_workers(
            &engine,
            &source,
            SelectionProfile::Default,
            &limits,
            &[],
            2,
        )
        .unwrap();
        assert_eq!(parallel.archive, serial.archive);
        assert_eq!(engine.decode(&parallel.archive, &limits).unwrap(), source);
    }

    #[test]
    fn parallel_generic_trials_fall_back_when_two_full_candidates_do_not_fit() {
        let engine = FullEngine {
            backends: BackendProvider {
                paths: TrustedBackendPaths {
                    cix: "/not-used/cix".into(),
                    paq_libraries: "/not-used/lib".into(),
                    temporary_root: "/not-used/tmp".into(),
                },
            },
            images: None,
            spatial: None,
        };
        let limits = OperationLimits {
            memory_bytes: 1 << 20,
            output_bytes: 1 << 20,
            intermediate_bytes: 1 << 20,
            deadline: None,
            cancellation: None,
        };
        let mut diagnostics = MixedDiagnostics::default();
        assert!(parallel_generic_choice(
            &engine,
            b"short region",
            16,
            16,
            0,
            RegionKind::Generic,
            &mut diagnostics,
            &limits,
            2,
        )
        .is_none());
    }

    #[test]
    fn late_and_interleaved_regions_remain_complete_partitions() {
        let data = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/regions/mixed.bin"
        ));
        let spans = regions::regions(data);
        assert_eq!(spans.first().map(|span| span.start), Some(0));
        assert_eq!(spans.last().map(|span| span.end), Some(data.len()));
        assert!(spans.windows(2).all(|pair| pair[0].end == pair[1].start));
        assert!(spans.iter().any(|span| span.kind == RegionKind::Generic));
        assert!(spans.iter().any(|span| span.kind != RegionKind::Generic));
    }

    #[test]
    fn default_profile_round_trips_interleaved_regions_through_native_providers() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/regions/mixed.bin"
        ));
        let engine = FullEngine {
            backends: BackendProvider {
                paths: TrustedBackendPaths {
                    cix: "/not-used/cix".into(),
                    paq_libraries: "/not-used/lib".into(),
                    temporary_root: "/not-used/tmp".into(),
                },
            },
            images: None,
            spatial: None,
        };
        let limits = OperationLimits {
            memory_bytes: 64 << 20,
            output_bytes: 1 << 20,
            intermediate_bytes: 1 << 20,
            deadline: None,
            cancellation: None,
        };
        let selected = encode(&engine, source, SelectionProfile::Default, &limits).unwrap();
        assert_eq!(engine.decode(&selected.archive, &limits).unwrap(), source);
        assert!(selected.diagnostics.entries.iter().any(|row| {
            row.status == DiagnosticStatus::Omitted
                && row.detail.as_deref() == Some("default profile omits nested PAQ")
        }));
    }
}
