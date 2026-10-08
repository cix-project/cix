//! Bounded decoder bridge. Full-engine frames and ASCII CIXB1 are buffered;
//! other supported ASCII native frames retain core's incremental decoder path.
use super::dispatch::{FullEngine, OperationLimits};
use crate::core::{self, NativeOptions};
use std::io::{self, Read, Write};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodePath {
    NativeIncremental,
    NativeCoreBuffered,
    FullFrameBuffered,
    /// CIXW1 restores independent embedded archives record by record.
    WindowIncremental,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeCapabilityReport {
    pub native_incremental: bool,
    pub native_core_buffered: bool,
    pub full_engine_frame_buffered: bool,
    pub window_incremental: bool,
}
pub fn capability_report() -> DecodeCapabilityReport {
    DecodeCapabilityReport {
        native_incremental: true,
        native_core_buffered: true,
        full_engine_frame_buffered: true,
        window_incremental: true,
    }
}
fn check(l: &OperationLimits) -> Result<(), String> {
    if l.memory_bytes == 0 || l.intermediate_bytes == 0 {
        return Err("full-engine decoder limits must be non-zero".into());
    }
    if l.cancellation
        .as_ref()
        .is_some_and(|v| v.load(std::sync::atomic::Ordering::Acquire))
    {
        return Err("full-engine decoder cancelled".into());
    }
    if l.deadline.is_some_and(|d| Instant::now() >= d) {
        return Err("full-engine decoder deadline exceeded".into());
    }
    Ok(())
}
fn classify(prefix: &[u8]) -> Result<DecodePath, String> {
    const NATIVE: &[&[u8]] = &[b"CIXG1", b"CIXG2", b"CIXM5", b"CIXM6", b"CIXZ1"];
    const FULL: &[&[u8]] = &[
        b"CIXR1",
        b"CIXP\x01",
        b"CIXH1",
        b"CIXG\x01",
        b"CIXQ\x01",
        b"CIXD\x01",
        b"CIXD\x02",
        b"CIXS\x01",
        b"CIXY\x01",
        b"CIXZ\x01",
        b"CIXI\x01",
        b"CIXI\x02",
        b"CIXV\x01",
        b"CIXB\x03",
        super::address_relations::RELOCATED_MAGIC,
        b"CIXB\x0b",
        b"CIXB\x0c",
        b"CIXB\x13",
        b"CIXB\x1d",
        b"CIXB\x1e",
        b"CIXB\x1f",
        b"CIXB\x20",
        b"CIXB\x21",
    ];
    if prefix == b"CIXW1" {
        Ok(DecodePath::WindowIncremental)
    } else if prefix == b"CIXB1" {
        Ok(DecodePath::NativeCoreBuffered)
    } else if NATIVE.contains(&prefix) {
        Ok(DecodePath::NativeIncremental)
    } else if FULL.contains(&prefix) {
        Ok(DecodePath::FullFrameBuffered)
    } else {
        Err("unsupported or truncated CIX archive signature".into())
    }
}

struct PrefixReader<R> {
    prefix: [u8; 5],
    used: usize,
    at: usize,
    source: R,
}
struct CountingReader<R> {
    inner: R,
    remaining: usize,
}
impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::other("native archive exceeds declared cap")),
            };
        }
        let count = output.len().min(self.remaining);
        let n = self.inner.read(&mut output[..count])?;
        self.remaining -= n;
        Ok(n)
    }
}
impl<R: Read> Read for PrefixReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.at < self.used {
            let count = out.len().min(self.used - self.at);
            out[..count].copy_from_slice(&self.prefix[self.at..self.at + count]);
            self.at += count;
            Ok(count)
        } else {
            self.source.read(out)
        }
    }
}
fn prefix<R: Read>(source: &mut R, limits: &OperationLimits) -> Result<([u8; 5], usize), String> {
    let mut out = [0u8; 5];
    let mut used = 0;
    while used < out.len() {
        check(limits)?;
        if source
            .read(&mut out[used..used + 1])
            .map_err(|e| e.to_string())?
            == 0
        {
            break;
        }
        used += 1;
    }
    Ok((out, used))
}
fn frame<R: Read>(
    mut source: R,
    prefix: &[u8],
    cap: usize,
    limits: &OperationLimits,
) -> Result<Vec<u8>, String> {
    if cap == 0 || prefix.len() > cap {
        return Err("full-engine archive exceeds declared cap".into());
    }
    // No decoded output is live during input collection. The decoder admits
    // its actual output and model allocations after this scratch buffer drops.
    if prefix
        .len()
        .checked_mul(2)
        .and_then(|n| n.checked_add(64 << 10))
        .is_none_or(|n| n > limits.memory_bytes)
    {
        return Err("full-engine prefix and read scratch exceed memory limit".into());
    }
    let mut out = Vec::new();
    out.try_reserve_exact(prefix.len())
        .map_err(|_| "full-engine archive allocation failed")?;
    out.extend_from_slice(prefix);
    let mut chunk = [0; 64 << 10];
    loop {
        check(limits)?;
        let n = source.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let next = out
            .len()
            .checked_add(n)
            .ok_or("full-engine archive size overflow")?;
        if next > cap
            || out
                .capacity()
                .checked_add(next)
                .and_then(|n| n.checked_add(prefix.len()))
                .and_then(|n| n.checked_add(chunk.len()))
                .is_none_or(|n| n > limits.memory_bytes)
        {
            return Err("full-engine archive exceeds declared resource limit".into());
        }
        out.try_reserve_exact(n)
            .map_err(|_| "full-engine archive allocation failed")?;
        if out
            .capacity()
            .checked_add(prefix.len())
            .and_then(|n| n.checked_add(chunk.len()))
            .is_none_or(|n| n > limits.memory_bytes)
        {
            return Err("full-engine archive buffer exceeds memory limit".into());
        }
        out.extend_from_slice(&chunk[..n]);
    }
    Ok(out)
}
/// Full-engine decoding is bounded but not incremental; see capability_report.
pub fn decode<R: Read, W: Write>(
    engine: &FullEngine,
    mut source: R,
    mut destination: W,
    max_archive_bytes: usize,
    limits: &OperationLimits,
) -> Result<DecodePath, String> {
    check(limits)?;
    // Every supported archive needs a five-byte signature.
    if max_archive_bytes < 5 {
        return Err("archive exceeds declared cap".into());
    }
    let (first, first_len) = prefix(&mut source, limits)?;
    match classify(&first[..first_len])? {
        DecodePath::WindowIncremental => {
            let mut reader = CountingReader {
                inner: PrefixReader {
                    prefix: first,
                    used: first_len,
                    at: 0,
                    source,
                },
                // The replayed signature is part of the archive limit.
                remaining: max_archive_bytes,
            };
            super::window_stream::decode(
                &mut reader,
                &mut destination,
                limits,
                |payload, output, nested_limits| {
                    // CIXW1 v1 has no recursive carrier semantics. The window
                    // parser rejects this too; preserve the guard at the
                    // dispatch boundary so the callback remains explicit.
                    if payload.starts_with(super::window_stream::MAGIC) {
                        return Err("nested CIXW1 archives are not supported".into());
                    }
                    decode(
                        engine,
                        io::Cursor::new(payload),
                        output,
                        payload.len(),
                        nested_limits,
                    )
                    .map(|_| ())
                },
            )?;
            Ok(DecodePath::WindowIncremental)
        }
        DecodePath::NativeIncremental => {
            let deadline = limits
                .deadline
                .map(|d| d.saturating_duration_since(Instant::now()));
            core::decode(
                CountingReader {
                    inner: PrefixReader {
                        prefix: first,
                        used: first_len,
                        at: 0,
                        source,
                    },
                    // The prefix is replayed through this same counter.
                    remaining: max_archive_bytes,
                },
                &mut destination,
                &NativeOptions {
                    output_limit: limits.output_bytes,
                    memory_limit: limits.memory_bytes,
                    workers: 1,
                    deadline,
                    cancellation: limits.cancellation.clone(),
                    ..NativeOptions::default()
                },
            )
            .map_err(|e| e.to_string())?;
            Ok(DecodePath::NativeIncremental)
        }
        DecodePath::NativeCoreBuffered => {
            let archive = frame(source, &first[..first_len], max_archive_bytes, limits)?;
            let memory = limits
                .memory_bytes
                // decode_cixb1 materializes its own archive buffer, so charge
                // the complete outer frame while both buffers coexist.
                .checked_sub(archive.capacity())
                .ok_or("native archive buffer exceeds memory limit")?;
            core::decode(
                io::Cursor::new(archive),
                &mut destination,
                &NativeOptions {
                    output_limit: limits.output_bytes,
                    memory_limit: memory,
                    workers: 1,
                    deadline: limits
                        .deadline
                        .map(|d| d.saturating_duration_since(Instant::now())),
                    cancellation: limits.cancellation.clone(),
                    ..NativeOptions::default()
                },
            )
            .map_err(|e| e.to_string())?;
            Ok(DecodePath::NativeCoreBuffered)
        }
        DecodePath::FullFrameBuffered => {
            let archive = frame(source, &first[..first_len], max_archive_bytes, limits)?;
            check(limits)?;
            let adjusted = OperationLimits {
                memory_bytes: limits
                    .memory_bytes
                    .checked_sub(archive.capacity().saturating_sub(archive.len()))
                    .ok_or("full-engine archive buffer exceeds memory limit")?,
                ..limits.clone()
            };
            let output = engine.decode(&archive, &adjusted)?;
            check(&adjusted)?;
            if output.len() > adjusted.output_bytes || output.capacity() > adjusted.output_bytes {
                return Err("full-engine decoder returned output beyond reservation".into());
            }
            destination.write_all(&output).map_err(|e| e.to_string())?;
            Ok(DecodePath::FullFrameBuffered)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::full_engine::{
        backend_provider::{BackendProvider, TrustedBackendPaths},
        containers,
    };
    use std::{
        io::{Cursor, Read},
        path::PathBuf,
    };
    fn limits() -> OperationLimits {
        OperationLimits {
            memory_bytes: 16 << 20,
            output_bytes: 4 << 20,
            intermediate_bytes: 4 << 20,
            deadline: None,
            cancellation: None,
        }
    }
    fn engine() -> FullEngine {
        FullEngine {
            backends: BackendProvider {
                paths: TrustedBackendPaths {
                    cix: PathBuf::from("/none"),
                    paq_libraries: PathBuf::from("/none"),
                    temporary_root: std::env::temp_dir(),
                },
            },
            images: None,
            spatial: None,
        }
    }
    struct Fragmented(Cursor<Vec<u8>>);
    impl Read for Fragmented {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = buffer.len().min(2);
            self.0.read(&mut buffer[..count])
        }
    }
    #[test]
    fn prefixes_are_exact() {
        assert_eq!(classify(b"CIXG1").unwrap(), DecodePath::NativeIncremental);
        assert_eq!(classify(b"CIXM5").unwrap(), DecodePath::NativeIncremental);
        assert_eq!(classify(b"CIXW1").unwrap(), DecodePath::WindowIncremental);
        assert_eq!(classify(b"CIXB1").unwrap(), DecodePath::NativeCoreBuffered);
        assert_eq!(
            classify(b"CIXG\x01").unwrap(),
            DecodePath::FullFrameBuffered
        );
        let relocated = super::super::address_relations::RELOCATED_MAGIC;
        assert_eq!(classify(relocated).unwrap(), DecodePath::FullFrameBuffered);
        for length in 0..relocated.len() {
            assert!(classify(&relocated[..length]).is_err());
        }
        assert!(classify(b"CIXB\xff").is_err());
        assert!(classify(b"CIXB\x0a\x00").is_err());
        assert!(classify(b"CIXG").is_err());
    }
    #[test]
    fn impossible_archive_caps_do_not_read_signature() {
        struct GuardedSignature {
            calls: usize,
            consumed: usize,
        }
        impl Read for GuardedSignature {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                self.calls += 1;
                let signature = b"CIXR1";
                if self.consumed == signature.len() {
                    return Err(io::Error::other("read after supported signature"));
                }
                let count = buffer.len().min(signature.len() - self.consumed);
                buffer[..count]
                    .copy_from_slice(&signature[self.consumed..self.consumed + count]);
                self.consumed += count;
                Ok(count)
            }
        }
        for cap in 0..5 {
            let mut source = GuardedSignature {
                calls: 0,
                consumed: 0,
            };
            let mut output = b"unchanged".to_vec();
            let error = decode(&engine(), &mut source, &mut output, cap, &limits())
                .unwrap_err();
            assert_eq!(error, "archive exceeds declared cap", "cap {cap}");
            assert_eq!(source.calls, 0, "cap {cap}");
            assert_eq!(source.consumed, 0, "cap {cap}");
            assert_eq!(output, b"unchanged", "cap {cap}");
        }
        let mut source = GuardedSignature {
            calls: 0,
            consumed: 0,
        };
        let mut output = b"unchanged".to_vec();
        let error = decode(&engine(), &mut source, &mut output, 5, &limits()).unwrap_err();
        assert_eq!(error, "read after supported signature");
        assert_eq!(source.calls, 6);
        assert_eq!(source.consumed, 5);
        assert_eq!(output, b"unchanged");
    }
    #[test]
    fn generic_and_raw_decode() {
        let input = b"io adapter".repeat(30);
        let native = core::encode_buffer(
            &input,
            &NativeOptions {
                output_limit: 1 << 20,
                memory_limit: 8 << 20,
                workers: 1,
                ..NativeOptions::default()
            },
        )
        .unwrap();
        let mut out = Vec::new();
        assert_eq!(
            decode(
                &engine(),
                Fragmented(Cursor::new(native)),
                &mut out,
                1 << 20,
                &limits()
            )
            .unwrap(),
            DecodePath::NativeIncremental
        );
        assert_eq!(out, input);
        let native = core::encode_buffer(
            &input,
            &NativeOptions {
                output_limit: 1 << 20,
                memory_limit: 8 << 20,
                workers: 1,
                ..NativeOptions::default()
            },
        )
        .unwrap();
        let archive_len = native.len();
        let mut exact = Vec::new();
        assert!(decode(
            &engine(),
            Cursor::new(native.clone()),
            &mut exact,
            archive_len,
            &limits()
        )
        .is_ok());
        assert!(decode(
            &engine(),
            Cursor::new(native.clone()),
            Vec::new(),
            archive_len - 1,
            &limits()
        )
        .is_err());
        let mut trailing = native;
        trailing.push(0);
        assert!(decode(
            &engine(),
            Cursor::new(trailing),
            Vec::new(),
            archive_len,
            &limits()
        )
        .is_err());
        let heterogeneous = crate::full_engine::mixed::encode_with(
            &input,
            std::slice::from_ref(&(0..input.len())),
            1 << 20,
            8 << 20,
            |region, _| {
                Ok(crate::full_engine::mixed::RegionChoice {
                    backend: crate::full_engine::mixed::RegionBackend::Specialist,
                    parameters: b"nested-raw".to_vec(),
                    payload: containers::encode_raw(region, 1 << 20).unwrap(),
                })
            },
        )
        .unwrap();
        let mut out = Vec::new();
        assert_eq!(
            decode(
                &engine(),
                Fragmented(Cursor::new(heterogeneous)),
                &mut out,
                1 << 20,
                &limits()
            )
            .unwrap(),
            DecodePath::FullFrameBuffered
        );
        assert_eq!(out, input);
        let raw = containers::encode_raw(&input, 1 << 20).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            decode(
                &engine(),
                Fragmented(Cursor::new(raw)),
                &mut out,
                1 << 20,
                &limits()
            )
            .unwrap(),
            DecodePath::FullFrameBuffered
        );
        assert_eq!(out, input);
    }
    #[test]
    fn cap_and_truncation_fail() {
        assert!(decode(
            &engine(),
            Cursor::new(b"CIXP\x01".to_vec()),
            Vec::new(),
            64,
            &limits()
        )
        .is_err());
        let raw = containers::encode_raw(&[0; 100], 1024).unwrap();
        assert!(decode(&engine(), Cursor::new(raw), Vec::new(), 5, &limits()).is_err());
    }
    #[test]
    fn cancellation_and_short_prefix_do_not_publish() {
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let limits = OperationLimits {
            cancellation: Some(cancelled),
            ..limits()
        };
        let mut output = b"unchanged".to_vec();
        assert!(decode(
            &engine(),
            Cursor::new(b"CIXG1".to_vec()),
            &mut output,
            64,
            &limits
        )
        .is_err());
        assert_eq!(output, b"unchanged");
        let tiny = OperationLimits {
            memory_bytes: 1,
            output_bytes: 1,
            intermediate_bytes: 1,
            deadline: None,
            cancellation: None,
        };
        assert!(decode(&engine(), Cursor::new(b"C".to_vec()), Vec::new(), 1, &tiny).is_err());
    }
}
