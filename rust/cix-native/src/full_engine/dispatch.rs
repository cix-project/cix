//! Native provider binding for retained complete specialist frames.
//!
//! This is a library qualification surface, not an automatic CLI policy. The
//! caller supplies installed providers and an explicit operation budget. Native
//! calls observe cancellation between calls; isolated PAQ workers are terminable.
use super::{
    address_relations as address,
    arithmetic::vdecode,
    backend_provider::*,
    bounded_values,
    catalogue::{Candidate, Recipe},
    conditional_values, containers, graph_residuals, grid_context, grid_counts, interval_context,
    jxl_provider::JxlProvider,
    mixed, record_ordering, spatial_frames,
    spatial_provider::SpatialProvider,
    strided_values, structured_frames, volume_lifting,
};
use crate::{core, paq_bridge::PaqVariant};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

#[derive(Clone, Debug)]
pub struct OperationLimits {
    pub memory_bytes: usize,
    pub output_bytes: usize,
    /// Independent ceiling for intermediate transformed streams, whose length
    /// is not stored in several historical envelopes.
    pub intermediate_bytes: usize,
    pub deadline: Option<Instant>,
    pub cancellation: Option<Arc<AtomicBool>>,
}
impl OperationLimits {
    fn check(&self) -> Result<(), String> {
        if self.memory_bytes == 0 || self.intermediate_bytes > isize::MAX as usize {
            return Err("invalid full-engine resource limits".into());
        }
        if self
            .cancellation
            .as_ref()
            .is_some_and(|token| token.load(Ordering::Acquire))
        {
            return Err("full-engine operation cancelled".into());
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err("full-engine deadline exceeded".into());
        }
        Ok(())
    }
    fn remaining(&self, retained: usize) -> Result<usize, String> {
        self.memory_bytes
            .checked_sub(retained)
            .ok_or_else(|| "full-engine retained-buffer limit".into())
    }
    fn request(&self, variant: BackendVariant, memory: usize, output: usize) -> BackendRequest {
        BackendRequest {
            variant,
            memory_limit: memory,
            output_limit: output,
            deadline: self.deadline,
            cancellation: self.cancellation.clone(),
        }
    }
}

pub struct FullEngine {
    pub backends: BackendProvider,
    pub images: Option<JxlProvider>,
    pub spatial: Option<SpatialProvider>,
}

struct SpatialMetadata<'a> {
    engine: &'a FullEngine,
    limits: &'a OperationLimits,
}
impl spatial_frames::SpatialMetadataBackend for SpatialMetadata<'_> {
    fn compress(&self, source: &[u8], maximum: usize, memory: usize) -> Result<Vec<u8>, String> {
        self.engine.backends.encode(
            source,
            &self
                .limits
                .request(BackendVariant::Xz { preset: 9 }, memory, maximum),
        )
    }
    fn decompress(&self, source: &[u8], expected: usize, memory: usize) -> Result<Vec<u8>, String> {
        self.engine.backends.decode(
            source,
            expected,
            &self
                .limits
                .request(BackendVariant::Xz { preset: 9 }, memory, expected),
        )
    }
}
fn brotli(window: u32) -> BackendVariant {
    BackendVariant::Brotli {
        quality: 11,
        lgwin: window,
    }
}
fn paq(variant: PaqVariant, number: u8, joint: Option<u8>) -> BackendVariant {
    BackendVariant::Paq {
        variant,
        level: PaqLevel {
            number,
            lstm: false,
        },
        lstm_layers: 1,
        joint_discount_mode: joint,
    }
}
fn sum(a: usize, b: usize) -> Result<usize, String> {
    a.checked_add(b)
        .ok_or_else(|| "full-engine size overflow".into())
}
fn checked_source_size(frame: &[u8], offset: usize, limit: usize) -> Result<usize, String> {
    let (length, at) = vdecode(frame, offset)?;
    let length = usize::try_from(length).map_err(|_| "full-engine source length overflow")?;
    if length > limit
        || frame
            .get(at..at.checked_add(4).ok_or("frame overflow")?)
            .is_none()
    {
        return Err("full-engine paid source length or checksum exceeds bounds".into());
    }
    Ok(length)
}
impl FullEngine {
    fn image_provider(&self) -> Result<&JxlProvider, String> {
        self.images
            .as_ref()
            .ok_or_else(|| "packaged JXL provider is unavailable".into())
    }
    fn encode_payload(
        &self,
        raw: &[u8],
        variant: BackendVariant,
        wrapped: bool,
        source_size: usize,
        limits: &OperationLimits,
    ) -> Result<Vec<u8>, String> {
        if raw.len() > limits.intermediate_bytes {
            return Err("transformed intermediate exceeds limit".into());
        }
        // Reserve source and an additional payload-sized envelope allocation.
        let cap = limits.output_bytes.min(limits.intermediate_bytes);
        let memory = limits.remaining(sum(source_size, cap)?)?;
        let payload = self
            .backends
            .encode(raw, &limits.request(variant, memory, cap))?;
        if wrapped {
            let BackendVariant::Paq { variant, .. } = variant else {
                return Err("only PAQ has CIXP framing".into());
            };
            containers::wrap_paq(&payload, variant, cap)
        } else {
            Ok(payload)
        }
    }
    fn decode_payload(
        &self,
        payload: &[u8],
        variant: BackendVariant,
        wrapped: bool,
        workspace: usize,
        limits: &OperationLimits,
    ) -> Result<Vec<u8>, String> {
        let cap = limits.intermediate_bytes.min(workspace / 2);
        if wrapped {
            let (variant, member) = containers::unwrap_paq(payload, workspace)?;
            let memory = workspace
                .checked_sub(payload.len())
                .ok_or("wrapped PAQ memory limit")?;
            self.backends
                .decode_bounded(&member, &limits.request(paq(variant, 8, None), memory, cap))
        } else {
            self.backends
                .decode_bounded(payload, &limits.request(variant, workspace, cap))
        }
    }

    /// Encode one explicit catalogue recipe. Search, tie priority, scheduling
    /// and reporting remain caller-owned; failures are never silent fallback.
    pub fn encode_candidate(
        &self,
        candidate: &Candidate,
        source: &[u8],
        limits: &OperationLimits,
    ) -> Result<Vec<u8>, String> {
        limits.check()?;
        if candidate.memory_bytes > limits.memory_bytes || source.len() > limits.intermediate_bytes
        {
            return Err("candidate exceeds declared memory/intermediate admission".into());
        }
        let encode = |raw: &[u8], kind, wrapped| {
            self.encode_payload(raw, kind, wrapped, source.len(), limits)
        };
        let archive = match &candidate.recipe {
            Recipe::Geometry { representation } => {
                let raw = graph_residuals::encode(source, 2, *representation)?;
                structured_frames::encode_geometry(source, &raw, &|raw| {
                    encode(raw, paq(PaqVariant::V215, 8, None), false)
                })?
            }
            Recipe::Hydrogen => {
                let raw = graph_residuals::encode_hydrogen(source)?;
                structured_frames::encode_hydrogen(source, &raw, &|raw| {
                    encode(raw, paq(PaqVariant::V215, 8, None), false)
                })?
            }
            Recipe::RecordConstraints { mode, paq: use_paq } => {
                let raw = record_ordering::transform_mysql_record_constraints(source, *mode)?;
                let kind = if *use_paq {
                    paq(PaqVariant::V215, 8, None)
                } else {
                    brotli(22)
                };
                record_ordering::encode_mysql_record_constraints(
                    source,
                    &raw,
                    u8::from(*use_paq),
                    &|raw| encode(raw, kind, *use_paq),
                )?
            }
            Recipe::FixedColumns => {
                let raw = record_ordering::transform_mysql_fixed_columns(source, 1)?;
                record_ordering::encode_mysql_fixed_columns(source, &raw, &|raw| {
                    encode(raw, brotli(22), false)
                })?
            }
            Recipe::GroupedGrid => conditional_values::encode_grouped_frame(source, &|raw| {
                encode(raw, paq(PaqVariant::V215, 8, None), true)
            })?,
            Recipe::BoundedGrid { mode } => {
                bounded_values::encode_backend_frame(source, *mode, 1, &|raw| {
                    encode(raw, paq(PaqVariant::V215, 8, None), true)
                })?
            }
            Recipe::Address {
                family,
                paq: use_paq,
                mode,
            } => {
                use address::{HistoricalFrame as F, Transform as T};
                let request = address::BackendRequest {
                    variant: if *use_paq {
                        address::BackendVariant::PaqV215
                    } else {
                        address::BackendVariant::Brotli
                    },
                    options: "-8".into(),
                    stream: 0,
                };
                match family {
                    F::Got | F::Relocated | F::Pointer | F::Section => {
                        let transform = match family {
                            F::Got => T::Got { mode: *mode },
                            F::Relocated => T::RelocatedPointers {
                                mode: *mode,
                                shared: true,
                            },
                            F::Pointer => T::PointerDeltas { mode: *mode },
                            _ => T::SectionPointers { mode: *mode },
                        };
                        address::encode_transformed_frame(
                            *family,
                            transform,
                            source,
                            request,
                            &mut |_, raw| {
                                encode(
                                    raw,
                                    if *use_paq {
                                        paq(PaqVariant::V215, 8, None)
                                    } else {
                                        brotli(24)
                                    },
                                    *use_paq,
                                )
                            },
                        )?
                    }
                    F::JointDiscount => {
                        let variant = if *mode == 4 {
                            PaqVariant::V215
                        } else {
                            PaqVariant::JointDiscount
                        };
                        let request = address::BackendRequest {
                            variant: if *mode == 4 {
                                address::BackendVariant::PaqV215
                            } else {
                                address::BackendVariant::PaqJointDiscount
                            },
                            ..request
                        };
                        address::encode_checked_frame(
                            *family,
                            T::SectionPointers { mode: 4 },
                            source,
                            *mode,
                            request,
                            &mut |_, raw| {
                                encode(raw, paq(variant, 8, (*mode < 4).then_some(*mode)), false)
                            },
                        )?
                    }
                    F::Filtered => {
                        let request = address::BackendRequest {
                            variant: address::BackendVariant::Brotli,
                            ..request
                        };
                        let transform = if *mode == 2 {
                            T::Identity
                        } else {
                            T::PdataSymbols { mode: 1 }
                        };
                        address::encode_checked_frame(
                            *family,
                            transform,
                            source,
                            *mode,
                            request,
                            &mut |_, raw| {
                                if *mode == 0 {
                                    return encode(raw, brotli(24), false);
                                }
                                // The pinned v216 worker rejects -0 because that
                                // archive level is not safely decodable. Filtered
                                // fresh archives use the qualified v216 -8 stage;
                                // their inverse remains the state-only decoder.
                                let intermediate =
                                    encode(raw, paq(PaqVariant::V216, 8, None), false)?;
                                self.encode_payload(
                                    &intermediate,
                                    brotli(24),
                                    false,
                                    sum(source.len(), raw.len())?,
                                    limits,
                                )
                            },
                        )?
                    }
                }
            }
            Recipe::Spatial { region, effort } => volume_lifting::encode_spatial_with_effort(
                source,
                region,
                *effort,
                self.image_provider()?,
            )?,
            Recipe::SpatialAlternative { region, codec } => spatial_frames::encode_spatial_frame(
                source,
                region,
                *codec,
                self.spatial
                    .as_ref()
                    .ok_or("packaged spatial provider is unavailable")?,
                &SpatialMetadata {
                    engine: self,
                    limits,
                },
                limits.output_bytes,
                limits.memory_bytes,
            )?,
            Recipe::Volume {
                region,
                mode,
                effort,
            } => volume_lifting::encode_volume(
                source,
                region,
                *mode,
                *effort,
                self.image_provider()?,
            )?,
            Recipe::VolumeSharedIdentity { region } => {
                volume_lifting::encode_volume_shared_identity(
                    source,
                    region,
                    self.image_provider()?,
                )?
                .archive
            }
        };
        limits.check()?;
        if archive.len() > limits.output_bytes {
            return Err("complete specialist archive exceeds output limit".into());
        }
        Ok(archive)
    }

    /// Archive-only restoration with bounded recursive CIXH1 nesting. Binary
    /// legacy signatures are dispatched separately from ASCII CIXG1/CIXB1.
    pub fn decode(&self, archive: &[u8], limits: &OperationLimits) -> Result<Vec<u8>, String> {
        self.decode_at(archive, limits, 0)
    }
    fn decode_at(
        &self,
        archive: &[u8],
        limits: &OperationLimits,
        depth: usize,
    ) -> Result<Vec<u8>, String> {
        limits.check()?;
        if depth > 16 {
            return Err("CIXH1 nesting exceeds supported depth".into());
        }
        let workspace = limits.remaining(archive.len())?;
        let maximum = limits.output_bytes;
        let decode = |payload: &[u8], memory: usize, kind, wrapped| {
            self.decode_payload(payload, kind, wrapped, memory, limits)
        };
        let result = if archive.starts_with(containers::RAW_MAGIC) {
            containers::decode_raw(archive, maximum, workspace)
        } else if archive.starts_with(containers::PAQ_FRAME_MAGIC) {
            let (variant, member) = containers::unwrap_paq(archive, workspace)?;
            let memory = workspace
                .checked_sub(member.len())
                .ok_or("PAQ member memory limit")?;
            self.backends.decode_bounded(
                &member,
                &limits.request(
                    paq(variant, 8, None),
                    memory,
                    maximum.min(limits.intermediate_bytes),
                ),
            )
        } else if archive.starts_with(mixed::MAGIC) {
            mixed::decode_with(
                archive,
                maximum,
                limits.memory_bytes,
                |kind, _, payload, expected, memory| {
                    let nested = OperationLimits {
                        memory_bytes: memory,
                        output_bytes: expected,
                        ..limits.clone()
                    };
                    match kind {
                        mixed::RegionBackend::Stored => unreachable!("stored handled by carrier"),
                        mixed::RegionBackend::Zlib => self.backends.decode(
                            payload,
                            expected,
                            &nested.request(BackendVariant::Zlib { level: 6 }, memory, expected),
                        ),
                        mixed::RegionBackend::Xz => self.backends.decode(
                            payload,
                            expected,
                            &nested.request(BackendVariant::Xz { preset: 6 }, memory, expected),
                        ),
                        mixed::RegionBackend::Specialist => {
                            self.decode_at(payload, &nested, depth + 1)
                        }
                    }
                },
            )
        } else if archive.starts_with(structured_frames::GEOMETRY_MAGIC)
            || archive.starts_with(structured_frames::HYDROGEN_MAGIC)
        {
            let size = checked_source_size(archive, 5, maximum)?;
            let geometry = archive.starts_with(structured_frames::GEOMETRY_MAGIC);
            let callback = |payload: &[u8], memory: usize| {
                let workspace = memory
                    .checked_sub(size.checked_mul(8).ok_or("graph workspace overflow")?)
                    .ok_or("graph workspace limit")?;
                decode(payload, workspace, paq(PaqVariant::V215, 8, None), false)
            };
            if geometry {
                structured_frames::decode_geometry(archive, maximum, workspace, &callback, |raw| {
                    graph_residuals::decode_with_limit(raw, size)
                })
            } else {
                structured_frames::decode_hydrogen(archive, maximum, workspace, &callback, |raw| {
                    graph_residuals::decode_hydrogen_with_limit(raw, size)
                })
            }
        } else if archive.starts_with(record_ordering::RECORD_MAGIC) {
            let selector = *archive.get(5).ok_or("truncated CIXD backend")?;
            record_ordering::decode_mysql_record_constraints(
                archive,
                maximum,
                workspace,
                &|payload, memory| {
                    decode(
                        payload,
                        memory,
                        if selector == 1 {
                            paq(PaqVariant::V215, 8, None)
                        } else {
                            brotli(22)
                        },
                        selector == 1,
                    )
                },
                record_ordering::inverse_mysql_record_constraints_with_limit,
            )
        } else if archive.starts_with(record_ordering::FIXED_MAGIC) {
            record_ordering::decode_mysql_fixed_columns(
                archive,
                maximum,
                workspace,
                &|payload, memory| decode(payload, memory, brotli(22), false),
                record_ordering::inverse_mysql_fixed_columns_with_limit,
            )
        } else if archive.starts_with(conditional_values::GROUPED_MAGIC) {
            conditional_values::decode_grouped_frame(
                archive,
                maximum,
                workspace,
                &|payload, memory| decode(payload, memory, paq(PaqVariant::V215, 8, None), true),
            )
        } else if archive.starts_with(grid_context::MAGIC) {
            let backend = *archive.get(5).ok_or("truncated feedback backend")?;
            grid_context::decode_frame(
                archive,
                maximum,
                workspace,
                &|payload, memory| match backend {
                    0 => decode(payload, memory, BackendVariant::Xz { preset: 9 }, false),
                    1 => decode(payload, memory, brotli(22), false),
                    2 => crate::standard_formats::decode(
                        crate::standard_formats::StandardFormat::Zstd,
                        payload,
                        &crate::standard_formats::StandardOptions {
                            level: 9,
                            output_limit: limits.intermediate_bytes.min(memory / 2),
                            memory_limit: memory,
                        },
                    )
                    .map_err(|error| error.to_string()),
                    3 => decode(payload, memory, paq(PaqVariant::V215, 8, None), true),
                    _ => Err("invalid feedback backend".into()),
                },
            )
        } else if archive.starts_with(grid_counts::COUNT_MAGIC) {
            grid_counts::decode_counts_frame(archive, maximum, workspace, &|payload, memory| {
                decode(payload, memory, brotli(22), false)
            })
        } else if archive.starts_with(grid_counts::CONDITIONAL_MAGIC) {
            grid_counts::decode_conditional_frame(
                archive,
                maximum,
                workspace,
                &|payload, memory| decode(payload, memory, brotli(22), false),
            )
        } else if archive.starts_with(interval_context::MAGIC) {
            interval_context::decode_frame(archive, maximum, workspace, &|payload, memory| {
                decode(payload, memory, brotli(22), false)
            })
        } else if archive.starts_with(bounded_values::BACKEND_MAGIC) {
            let selector = *archive.get(6).ok_or("truncated grid backend")?;
            bounded_values::decode_backend_frame(archive, maximum, workspace, &|payload, memory| {
                decode(
                    payload,
                    memory,
                    if selector == 1 {
                        paq(PaqVariant::V215, 8, None)
                    } else {
                        brotli(22)
                    },
                    selector == 1,
                )
            })
        } else if archive.starts_with(strided_values::GRID_MAGIC) {
            strided_values::decode_wcs_grid(archive, maximum, workspace, &|payload, memory| {
                decode(payload, memory, BackendVariant::Xz { preset: 9 }, false)
            })
        } else if address::recognizes_magic(archive) {
            self.decode_address(archive, maximum, workspace, limits)
        } else if archive.starts_with(volume_lifting::SPATIAL_MAGIC) {
            if matches!(archive.get(5), Some(2 | 3)) {
                spatial_frames::decode_spatial_frame(
                    archive,
                    maximum,
                    limits.memory_bytes,
                    self.spatial
                        .as_ref()
                        .ok_or("packaged spatial provider is unavailable")?,
                    &SpatialMetadata {
                        engine: self,
                        limits,
                    },
                )
            } else {
                volume_lifting::decode_spatial(archive, maximum, workspace, self.image_provider()?)
            }
        } else if archive.starts_with(volume_lifting::VOLUME_MAGIC) {
            volume_lifting::decode_volume(archive, maximum, workspace, self.image_provider()?)
        } else if archive.starts_with(volume_lifting::MULTISCALE_MAGIC) {
            volume_lifting::decode_multiscale(archive, maximum, workspace, self.image_provider()?)
        } else {
            let options = core::NativeOptions {
                memory_limit: workspace,
                output_limit: maximum,
                deadline: limits
                    .deadline
                    .map(|d| d.saturating_duration_since(Instant::now())),
                cancellation: limits.cancellation.clone(),
                ..core::NativeOptions::default()
            };
            core::decode_buffer(archive, &options).map_err(|error| error.to_string())
        }?;
        limits.check()?;
        if result.len() > maximum {
            return Err("full-engine output exceeds limit".into());
        }
        Ok(result)
    }

    fn decode_address(
        &self,
        archive: &[u8],
        maximum: usize,
        memory: usize,
        limits: &OperationLimits,
    ) -> Result<Vec<u8>, String> {
        use address::{BackendVariant as V, HistoricalFrame as F};
        let callback = |request: &address::BackendRequest, payload: &[u8], workspace| {
            let wrapped = matches!(request.variant, V::PaqV215 | V::PaqV216);
            let kind = if wrapped {
                paq(PaqVariant::V215, 8, None)
            } else {
                brotli(24)
            };
            self.decode_payload(payload, kind, wrapped, workspace, limits)
        };
        if archive.starts_with(address::GOT_MAGIC)
            || archive.starts_with(address::RELOCATED_MAGIC)
            || archive.starts_with(address::POINTER_MAGIC)
            || archive.starts_with(address::SECTION_MAGIC)
        {
            let family = if archive.starts_with(address::GOT_MAGIC) {
                F::Got
            } else if archive.starts_with(address::RELOCATED_MAGIC) {
                F::Relocated
            } else if archive.starts_with(address::POINTER_MAGIC) {
                F::Pointer
            } else {
                F::Section
            };
            return address::decode_transformed_frame(
                family,
                archive,
                maximum,
                memory,
                &mut |request, payload, workspace| callback(request, payload, workspace),
            );
        }
        let mode = *archive.get(5).ok_or("truncated executable frame mode")?;
        let joint = archive.starts_with(address::JOINT_MAGIC);
        let family = if joint { F::JointDiscount } else { F::Filtered };
        let request = address::BackendRequest {
            variant: if joint {
                V::PaqJointDiscount
            } else {
                V::Brotli
            },
            options: String::new(),
            stream: 0,
        };
        address::decode_checked_frame(
            family,
            archive,
            maximum,
            memory,
            request,
            &mut |_, payload, workspace| {
                if joint {
                    self.decode_payload(
                        payload,
                        paq(
                            if mode == 4 {
                                PaqVariant::V215
                            } else {
                                PaqVariant::JointDiscount
                            },
                            8,
                            (mode < 4).then_some(mode),
                        ),
                        false,
                        workspace,
                        limits,
                    )
                } else {
                    let intermediate =
                        self.decode_payload(payload, brotli(24), false, workspace, limits)?;
                    if mode == 0 {
                        return Ok(intermediate);
                    }
                    let remaining = workspace
                        .checked_sub(intermediate.len())
                        .ok_or("filter intermediate memory limit")?;
                    self.decode_payload(
                        &intermediate,
                        paq(PaqVariant::StoreState, 0, None),
                        false,
                        remaining,
                        limits,
                    )
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn engine() -> FullEngine {
        FullEngine {
            backends: BackendProvider {
                paths: TrustedBackendPaths {
                    cix: "/not-used/cix".into(),
                    paq_libraries: "/not-used/lib".into(),
                    temporary_root: "/not-used/tmp".into(),
                },
            },
            images: None,
            spatial: None,
        }
    }
    fn limits() -> OperationLimits {
        OperationLimits {
            memory_bytes: 1 << 30,
            output_bytes: 1 << 20,
            intermediate_bytes: 1 << 20,
            deadline: None,
            cancellation: None,
        }
    }
    #[test]
    fn feedback_legacy_frames_dispatch_all_policies_and_native_backends() {
        use crate::standard_formats::{self, StandardFormat, StandardOptions};
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/grid_feedback/input.bin"
        ));
        let engine = engine();
        let limits = limits();
        for mode in 1..=3 {
            for model in 0..=1 {
                for backend in 0..=2 {
                    let frame = grid_context::encode_frame(source, mode, model, backend, &|raw| {
                        match backend {
                            0 => crate::external::raw_xz_encode_preset(raw, 9, 1 << 20),
                            1 => crate::external::raw_brotli_encode(raw, 11, 22, 1 << 20),
                            2 => standard_formats::encode(
                                StandardFormat::Zstd,
                                raw,
                                &StandardOptions {
                                    level: 6,
                                    output_limit: 4 << 20,
                                    memory_limit: 1 << 30,
                                },
                            )
                            .map_err(|error| error.to_string()),
                            _ => unreachable!(),
                        }
                    })
                    .unwrap();
                    assert_eq!(
                        engine.decode(&frame, &limits).unwrap(),
                        source,
                        "mode {mode} model {model} backend {backend}"
                    );
                    let mut trailing = frame;
                    trailing.push(0);
                    assert!(engine.decode(&trailing, &limits).is_err());
                }
            }
        }
    }

    #[test]
    fn interval_legacy_frames_dispatch_all_policies() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/grid_interval/boundary256-input.bin"
        ));
        let engine = engine();
        let limits = limits();
        for policy in 1..=3 {
            let frame = interval_context::encode_frame(source, policy, &|raw| {
                crate::external::raw_brotli_encode(raw, 11, 22, 1 << 20)
            })
            .unwrap();
            assert_eq!(engine.decode(&frame, &limits).unwrap(), source);
            let mut trailing = frame;
            trailing.push(0);
            assert!(engine.decode(&trailing, &limits).is_err());
        }
    }

    #[test]
    fn counts_legacy_frames_dispatch_all_modes_and_codings() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/grid_counts/input.bin"
        ));
        let engine = engine();
        let limits = limits();
        let backend = |raw: &[u8]| crate::external::raw_brotli_encode(raw, 11, 22, 1 << 20);
        for mode in 0..5 {
            let frame = grid_counts::encode_counts_frame(source, mode, &backend).unwrap();
            assert_eq!(engine.decode(&frame, &limits).unwrap(), source);
        }
        for context in 0..4 {
            for coding in 0..3 {
                let frame =
                    grid_counts::encode_conditional_frame(source, context, coding, &backend)
                        .unwrap();
                assert_eq!(engine.decode(&frame, &limits).unwrap(), source);
                let mut trailing = frame;
                trailing.push(0);
                assert!(engine.decode(&trailing, &limits).is_err());
            }
        }
    }

    #[test]
    fn installed_spatial_legacy_frames_and_native_candidates() {
        let Some(path) = std::env::var_os("CIX_TEST_SPATIAL_BRIDGE") else {
            return;
        };
        let mut engine = engine();
        engine.spatial = Some(
            SpatialProvider::load_package_bridge(std::path::Path::new(&path), 1 << 20).unwrap(),
        );
        let limits = limits();
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/spatial_frames/jls-input.bin"
        ));
        let frame = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/spatial_frames/jls.cix"
        ));
        assert_eq!(engine.decode(frame, &limits).unwrap(), source);
        let j2k_source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/spatial_frames/j2k-input.bin"
        ));
        let frame = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/spatial_frames/j2k.cix"
        ));
        assert_eq!(engine.decode(frame, &limits).unwrap(), j2k_source);
        let candidates = super::super::catalogue::catalog(source).unwrap();
        let mut encoded = 0;
        for candidate in candidates.candidates {
            if matches!(candidate.recipe, Recipe::SpatialAlternative { .. }) {
                let frame = engine
                    .encode_candidate(&candidate, source, &limits)
                    .unwrap();
                assert_eq!(engine.decode(&frame, &limits).unwrap(), source);
                encoded += 1;
            }
        }
        assert_eq!(
            encoded, 2,
            "both restored spatial candidates must be content-admitted"
        );
    }

    #[test]
    fn record_brotli_archive_dispatch_and_corruption() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/record_stride/record.input"
        ));
        let engine = engine();
        let limits = limits();
        for mode in 0..=4 {
            let candidate = Candidate {
                id: "test-record".into(),
                family: "database-constraints",
                recipe: Recipe::RecordConstraints { mode, paq: false },
                memory_bytes: 256 << 20,
            };
            let archive = engine
                .encode_candidate(&candidate, source, &limits)
                .unwrap();
            assert_eq!(engine.decode(&archive, &limits).unwrap(), source);
            assert!(engine
                .decode(
                    &archive,
                    &OperationLimits {
                        output_bytes: source.len() - 1,
                        ..limits.clone()
                    }
                )
                .is_err());
            let mut broken = archive.clone();
            broken.push(0);
            assert!(engine.decode(&broken, &limits).is_err());
            let mut broken = archive;
            let end = broken.len() - 1;
            broken[end] ^= 1;
            assert!(engine.decode(&broken, &limits).is_err());
        }
    }
    #[test]
    fn raw_native_and_mixed_archives_share_dispatch() {
        let engine = engine();
        let limits = limits();
        let source = b"unknown data\0mixed bytes";
        let stored = containers::encode_raw(source, 1024).unwrap();
        assert_eq!(engine.decode(&stored, &limits).unwrap(), source);
        let native = core::encode_buffer(source, &core::NativeOptions::default()).unwrap();
        assert_eq!(engine.decode(&native, &limits).unwrap(), source);
        let range = 0..source.len();
        let mixed = mixed::encode_with(
            source,
            std::slice::from_ref(&range),
            4096,
            1 << 20,
            |_, _| {
                Ok(mixed::RegionChoice {
                    backend: mixed::RegionBackend::Specialist,
                    parameters: b"stored-test".to_vec(),
                    payload: stored.clone(),
                })
            },
        )
        .unwrap();
        assert_eq!(engine.decode(&mixed, &limits).unwrap(), source);
        assert!(engine.decode(b"CIXB\x1d", &limits).is_err());
        assert!(engine.decode(b"CIXQ\x01", &limits).is_err());
        assert!(engine
            .decode(
                &mixed,
                &OperationLimits {
                    deadline: Some(Instant::now()),
                    ..limits
                }
            )
            .is_err());
    }
    #[test]
    fn installed_paq_and_jxl_complete_archive_dispatch() {
        let (Some(executable), Some(libraries), Some(temporary)) = (
            std::env::var_os("CIX_TEST_EXECUTABLE"),
            std::env::var_os("CIX_TEST_PAQ_LIBDIR"),
            std::env::var_os("CIX_TEST_PAQ_WORKROOT"),
        ) else {
            return;
        };
        let engine = FullEngine {
            backends: BackendProvider {
                paths: TrustedBackendPaths {
                    cix: executable.into(),
                    paq_libraries: libraries.into(),
                    temporary_root: temporary.into(),
                },
            },
            spatial: None,
            images: std::env::var_os("CIX_TEST_JXL_BRIDGE").map(|p| {
                JxlProvider::load_package_bridge(std::path::Path::new(&p), 1 << 20).unwrap()
            }),
        };
        let limits = OperationLimits {
            memory_bytes: 5_800_000_000,
            ..limits()
        };
        let record = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/record_stride/record.input"
        ));
        let grid = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bounded_values/input.bin"
        ));
        let address = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/address_relations/source.bin"
        ));
        let graph = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/native_ports/graph.input"
        ));
        let mut recipes = vec![
            (Recipe::Geometry { representation: 1 }, graph.as_slice()),
            (Recipe::Geometry { representation: 2 }, graph.as_slice()),
            (Recipe::Hydrogen, graph.as_slice()),
            (
                Recipe::RecordConstraints { mode: 3, paq: true },
                record.as_slice(),
            ),
            (Recipe::GroupedGrid, grid.as_slice()),
        ];
        for mode in 1..=4 {
            recipes.push((Recipe::BoundedGrid { mode }, grid.as_slice()));
        }
        for (family, mode) in [
            (address::HistoricalFrame::Got, 3),
            (address::HistoricalFrame::Relocated, 5),
            (address::HistoricalFrame::Pointer, 5),
            (address::HistoricalFrame::Section, 4),
            (address::HistoricalFrame::JointDiscount, 2),
            (address::HistoricalFrame::Filtered, 0),
            (address::HistoricalFrame::Filtered, 1),
            (address::HistoricalFrame::Filtered, 2),
        ] {
            recipes.push((
                Recipe::Address {
                    family,
                    paq: true,
                    mode,
                },
                address.as_slice(),
            ));
        }
        for (recipe, source) in recipes {
            let candidate = Candidate {
                id: format!("{:?}", recipe),
                family: "qualification",
                recipe,
                memory_bytes: 4_600_000_000,
            };
            let frame = engine
                .encode_candidate(&candidate, source, &limits)
                .unwrap_or_else(|e| panic!("{} encode: {e}", candidate.id));
            let restored = engine
                .decode(&frame, &limits)
                .unwrap_or_else(|e| panic!("{} decode: {e}", candidate.id));
            assert_eq!(restored, source, "{}", candidate.id);
            eprintln!("INSTALLED-ARCHIVE {} {} bytes", candidate.id, frame.len());
        }
        if engine.images.is_some() {
            let mut dicom = Vec::new();
            for (group, element, value) in
                [(0x28u16, 0x10u16, 2u16), (0x28, 0x11, 3), (0x28, 0x100, 16)]
            {
                dicom.extend(group.to_le_bytes());
                dicom.extend(element.to_le_bytes());
                dicom.extend(2u32.to_le_bytes());
                dicom.extend(value.to_le_bytes());
            }
            dicom.extend(0x7fe0u16.to_le_bytes());
            dicom.extend(0x10u16.to_le_bytes());
            dicom.extend(24u32.to_le_bytes());
            for n in 0..12u16 {
                dicom.extend(n.wrapping_mul(1007).to_le_bytes());
            }
            let region = volume_lifting::dicom_regions(&dicom).remove(0);
            for recipe in [
                Recipe::Spatial {
                    region: region.clone(),
                    effort: 10,
                },
                Recipe::VolumeSharedIdentity { region },
            ] {
                let candidate = Candidate {
                    id: "jxl-installed".into(),
                    family: "qualification",
                    recipe,
                    memory_bytes: 1536 << 20,
                };
                let frame = engine
                    .encode_candidate(&candidate, &dicom, &limits)
                    .unwrap();
                assert_eq!(engine.decode(&frame, &limits).unwrap(), dicom);
            }
        }
    }
}
