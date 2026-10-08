//! Product-private raw backend provider. CIXP framing belongs to containers.
//!
//! Native one-shot calls check cancellation and the shared absolute deadline
//! before and after execution; they cannot be interrupted in the middle. Memory
//! admission accounts for owned buffers and estimated native state, not process
//! RSS. The PAQ worker receives the remaining address-space allowance after
//! parent input, result and diagnostic reservations.
use crate::{external, paq_bridge::PaqVariant, paq_supervisor};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub struct TrustedBackendPaths {
    pub cix: PathBuf,
    pub paq_libraries: PathBuf,
    pub temporary_root: PathBuf,
}
#[derive(Clone, Copy, Debug)]
pub enum BackendVariant {
    Zlib {
        level: u32,
    },
    Xz {
        preset: u32,
    },
    Brotli {
        quality: u32,
        lgwin: u32,
    },
    Paq {
        variant: PaqVariant,
        level: PaqLevel,
        lstm_layers: u8,
        joint_discount_mode: Option<u8>,
    },
}
#[derive(Clone, Debug)]
pub struct BackendRequest {
    pub variant: BackendVariant,
    pub output_limit: usize,
    pub memory_limit: usize,
    pub deadline: Option<Instant>,
    pub cancellation: Option<Arc<AtomicBool>>,
}
pub use paq_supervisor::PaqLevel;
#[derive(Debug)]
pub struct BackendProvider {
    pub paths: TrustedBackendPaths,
}
#[derive(Clone, Copy, Debug)]
struct PaqOptions {
    variant: PaqVariant,
    operation: paq_supervisor::PaqOperation,
    level: PaqLevel,
    lstm_layers: u8,
    joint: Option<u8>,
}
impl BackendProvider {
    pub fn decode_bounded(&self, payload: &[u8], r: &BackendRequest) -> Result<Vec<u8>, String> {
        let _library = crate::limits::LibraryGuard::new();
        let _cancel = r
            .cancellation
            .clone()
            .map(crate::limits::CancellationGuard::new);
        let _deadline = r.deadline.map(|d| {
            crate::limits::DeadlineGuard::new(d.saturating_duration_since(Instant::now()))
        });
        self.native_admit(payload.len(), r, self.state_bytes(r.variant)?)?;
        let output = match r.variant {
            BackendVariant::Zlib { .. } => {
                external::raw_zlib_decode_bounded(payload, r.output_limit)
            }
            BackendVariant::Xz { .. } => {
                external::raw_xz_decode_bounded(payload, r.output_limit, r.memory_limit)
            }
            BackendVariant::Brotli { .. } => {
                external::raw_brotli_decode_bounded(payload, r.output_limit)
            }
            BackendVariant::Paq {
                variant,
                level,
                lstm_layers,
                joint_discount_mode,
            } => self.paq(
                payload,
                r,
                PaqOptions {
                    variant,
                    operation: paq_supervisor::PaqOperation::Decode,
                    level,
                    lstm_layers,
                    joint: joint_discount_mode,
                },
            ),
        }?;
        self.native_admit(payload.len(), r, self.state_bytes(r.variant)?)?;
        Ok(output)
    }
    pub fn encode(&self, input: &[u8], r: &BackendRequest) -> Result<Vec<u8>, String> {
        let _library = crate::limits::LibraryGuard::new();
        let _cancel = r
            .cancellation
            .clone()
            .map(crate::limits::CancellationGuard::new);
        let _deadline = r.deadline.map(|d| {
            crate::limits::DeadlineGuard::new(d.saturating_duration_since(Instant::now()))
        });
        self.native_admit(input.len(), r, self.state_bytes(r.variant)?)?;
        let output = match r.variant {
            BackendVariant::Zlib { level } => {
                external::raw_zlib_encode(input, level, r.output_limit)
            }
            BackendVariant::Xz { preset } => {
                external::raw_xz_encode_preset(input, preset, r.output_limit)
            }
            BackendVariant::Brotli { quality, lgwin } => {
                external::raw_brotli_encode(input, quality, lgwin, r.output_limit)
            }
            BackendVariant::Paq {
                variant,
                level,
                lstm_layers,
                joint_discount_mode,
            } => self.paq(
                input,
                r,
                PaqOptions {
                    variant,
                    operation: paq_supervisor::PaqOperation::Encode,
                    level,
                    lstm_layers,
                    joint: joint_discount_mode,
                },
            ),
        }?;
        self.native_admit(input.len(), r, self.state_bytes(r.variant)?)?;
        Ok(output)
    }
    pub fn decode(
        &self,
        payload: &[u8],
        expected: usize,
        r: &BackendRequest,
    ) -> Result<Vec<u8>, String> {
        if expected > r.output_limit {
            return Err("backend output exceeds limit".into());
        }
        let exact = BackendRequest {
            output_limit: expected,
            ..r.clone()
        };
        let output = self.decode_bounded(payload, &exact)?;
        if output.len() != expected {
            return Err("backend decoded length mismatch".into());
        }
        Ok(output)
    }
    fn native_admit(&self, input: usize, r: &BackendRequest, state: usize) -> Result<(), String> {
        if r.cancellation
            .as_ref()
            .is_some_and(|x| x.load(Ordering::Acquire))
        {
            return Err("backend cancelled".into());
        };
        if r.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err("backend deadline exceeded".into());
        };
        if input
            .checked_add(
                r.output_limit
                    .checked_mul(2)
                    .ok_or("backend output size overflow")?,
            )
            .and_then(|x| x.checked_add(state))
            .is_none_or(|x| x > r.memory_limit)
        {
            return Err(
                "backend request exceeds admitted input/output/native-state reservation".into(),
            );
        };
        Ok(())
    }
    fn state_bytes(&self, variant: BackendVariant) -> Result<usize, String> {
        match variant {
            BackendVariant::Xz { preset } if preset <= 9 => {
                Ok(external::xz_easy_memory_usage(preset))
            }
            BackendVariant::Xz { .. } => Err("unsupported XZ preset".into()),
            BackendVariant::Brotli { lgwin, .. } if (10..=24).contains(&lgwin) => {
                // Carry forward the canonical provider's conservative
                // high-quality estimate; a window alone is not all state.
                Ok(256 << 20)
            }
            BackendVariant::Brotli { .. } => Err("invalid Brotli window".into()),
            BackendVariant::Zlib { level } if level <= 9 => Ok(8 << 20),
            BackendVariant::Zlib { .. } => Err("invalid zlib level".into()),
            BackendVariant::Paq { .. } => Ok(64 << 20),
        }
    }
    fn paq(
        &self,
        input: &[u8],
        r: &BackendRequest,
        options: PaqOptions,
    ) -> Result<Vec<u8>, String> {
        let parent_bytes = input
            .len()
            .checked_add(r.output_limit)
            .and_then(|n| n.checked_add(2 * 65536 + 65536))
            .ok_or("PAQ parent reservation overflow")?;
        let worker_memory = r
            .memory_limit
            .checked_sub(parent_bytes)
            .ok_or("PAQ parent/worker memory limit")?;
        paq_supervisor::validate_private_temporary_root(&self.paths.temporary_root)
            .map_err(|e| e.to_string())?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let dir = self
            .paths
            .temporary_root
            .join(format!("cix-paq-input-{}-{nonce}", std::process::id()));
        paq_supervisor::create_private_directory(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("input");
        if let Err(error) = fs::write(&path, input) {
            let _ = fs::remove_dir_all(&dir);
            return Err(error.to_string());
        };
        let q = paq_supervisor::PaqWorkerRequest {
            paths: paq_supervisor::PaqWorkerPaths {
                cix_executable: self.paths.cix.clone(),
                private_library_directory: self.paths.paq_libraries.clone(),
                temporary_root: self.paths.temporary_root.clone(),
            },
            variant: options.variant,
            operation: options.operation,
            input: path,
            input_limit_bytes: input.len().max(1) as u64,
            output_limit_bytes: r.output_limit as u64,
            parent_result_limit_bytes: r.output_limit,
            memory_limit_bytes: worker_memory as u64,
            level: options.level,
            lstm_layers: options.lstm_layers,
            joint_discount_mode: options.joint,
            deadline: r.deadline,
            cancellation: r.cancellation.clone(),
            diagnostic_limit_bytes: 65536,
        };
        if r.cancellation
            .as_ref()
            .is_some_and(|x| x.load(Ordering::Acquire))
        {
            let _ = fs::remove_dir_all(&dir);
            return Err("backend cancelled".into());
        };
        if r.deadline.is_some_and(|d| Instant::now() >= d) {
            let _ = fs::remove_dir_all(&dir);
            return Err("backend deadline exceeded".into());
        };
        let result = paq_supervisor::run(&q)
            .map(|x| x.output)
            .map_err(|e| e.to_string());
        let cleanup = std::fs::remove_dir_all(&dir).map_err(|e| e.to_string());
        match (result, cleanup) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn provider() -> BackendProvider {
        BackendProvider {
            paths: TrustedBackendPaths {
                cix: "/unused/cix".into(),
                paq_libraries: "/unused/lib".into(),
                temporary_root: "/unused/tmp".into(),
            },
        }
    }
    fn request(variant: BackendVariant) -> BackendRequest {
        BackendRequest {
            variant,
            output_limit: 65536,
            memory_limit: 1 << 30,
            deadline: None,
            cancellation: None,
        }
    }
    #[test]
    fn native_payloads_are_exact_and_reject_trailing_truncated_and_capped_output() {
        let source = b"bounded raw provider\0abc123".repeat(127);
        for variant in [
            BackendVariant::Zlib { level: 6 },
            BackendVariant::Xz { preset: 0 },
            BackendVariant::Xz { preset: 3 },
            BackendVariant::Brotli {
                quality: 4,
                lgwin: 18,
            },
        ] {
            let provider = provider();
            let request = request(variant);
            let encoded = provider.encode(&source, &request).unwrap();
            assert_eq!(
                provider.decode(&encoded, source.len(), &request).unwrap(),
                source
            );
            assert_eq!(provider.decode_bounded(&encoded, &request).unwrap(), source);
            assert!(provider
                .decode(&encoded, source.len() - 1, &request)
                .is_err());
            assert!(provider
                .decode(&encoded, source.len() + 1, &request)
                .is_err());
            let mut trailing = encoded.clone();
            trailing.push(0);
            assert!(provider.decode_bounded(&trailing, &request).is_err());
            assert!(provider
                .decode_bounded(&encoded[..encoded.len() - 1], &request)
                .is_err());
            assert!(provider
                .encode(
                    &source,
                    &BackendRequest {
                        output_limit: 1,
                        ..request.clone()
                    }
                )
                .is_err());
            assert!(provider
                .decode_bounded(
                    &encoded,
                    &BackendRequest {
                        output_limit: source.len() - 1,
                        ..request.clone()
                    }
                )
                .is_err());
            let empty = provider.encode(&[], &request).unwrap();
            assert_eq!(provider.decode(&empty, 0, &request).unwrap(), b"");
        }
    }
    #[test]
    fn request_stops_and_memory_limits_precede_provider_work() {
        let provider = provider();
        let req = request(BackendVariant::Brotli {
            quality: 11,
            lgwin: 24,
        });
        assert!(provider
            .encode(
                b"x",
                &BackendRequest {
                    memory_limit: 128,
                    ..req.clone()
                }
            )
            .unwrap_err()
            .contains("reservation"));
        let flag = Arc::new(AtomicBool::new(true));
        assert!(provider
            .encode(
                b"x",
                &BackendRequest {
                    cancellation: Some(flag),
                    ..req.clone()
                }
            )
            .unwrap_err()
            .contains("cancelled"));
        assert!(provider
            .decode_bounded(
                b"broken",
                &BackendRequest {
                    deadline: Some(Instant::now()),
                    ..req.clone()
                }
            )
            .unwrap_err()
            .contains("deadline"));
        assert!(provider
            .encode(
                b"x",
                &BackendRequest {
                    output_limit: usize::MAX,
                    ..req
                }
            )
            .is_err());
    }
    #[test]
    fn direct_raw_decoders_reject_unrepresentable_output_caps() {
        assert!(external::raw_zlib_decode(&[], usize::MAX, usize::MAX).is_err());
        assert!(external::raw_xz_decode(&[], usize::MAX, usize::MAX, usize::MAX).is_err());
        assert!(external::raw_brotli_decode(&[], usize::MAX, usize::MAX).is_err());
    }
}
