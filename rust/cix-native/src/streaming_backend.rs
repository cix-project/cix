//! Versioned persistent native-backend streaming.
//!
//! `CIXZ1` is deliberately separate from CIXG1/CIXG2.  A CIXG frame is an
//! independently decoded route payload, whereas this format carries pieces of
//! one zstd frame and its decoder state across blocks.  Each source block is
//! still bounded, flushed as a complete CIX frame before EOF, and protected by
//! a per-block hash.  The final zstd epilogue is stored in the CIX footer, so
//! the stream remains incrementally decodable and its complete integrity is
//! only accepted after that footer is consumed.

use super::*;
use sha2::{Digest, Sha256};
use std::ffi::{c_char, c_int, c_void, CStr};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub(super) const MAGIC: &[u8; 5] = b"CIXZ1";
const ROUTE_ZSTD: u8 = 11;
const ROUTE_RAW: u8 = 0;
const ROUTE_ZSTD_RESET: u8 = 12;
const ROUTE_CIXG: u8 = 13;
// A zstd streaming flush can contain framing and a partly buffered block in
// addition to the 64 KiB application block.  This is an explicit allocation
// cap, not an estimate of compressed size.
const STREAM_SLACK: usize = 256 * 1024;
// After an explicit zstd flush, ending the active frame only needs its small
// frame epilogue. Reserve this fixed charge while comparing an open persistent
// frame with closed raw/CIXG candidates; the actual footer remains checked.
// With checksums disabled and every data block flushed, Zstd finishes with
// one empty last-block header (3 bytes). Enforce this native-format invariant
// when materializing tails so selection charges the exact closing cost.
const FINALIZATION_COST: usize = 3;
const FRAME_PREFIX_COST: usize = 41;
const ZSTD_MAX_WINDOW: usize = 1 << 27;

/// A decoder must never let a nested archive turn its advertised outer block
/// length into an allocation.  This writer retains at most the descriptor's
/// promised bytes and makes `write_all` fail before growing its backing Vec.
/// It is deliberately local to CIXZ1: CIXG's ordinary decoder remains a
/// general stream decoder, while this embedding has a strict outer bound.
struct CappedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl CappedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for CappedWriter {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if input.len() > remaining {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "nested CIXG output exceeds CIXZ1 source descriptor",
            ));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[repr(C)]
struct ZstdInBuffer {
    src: *const c_void,
    size: usize,
    pos: usize,
}

#[repr(C)]
struct ZstdOutBuffer {
    dst: *mut c_void,
    size: usize,
    pos: usize,
}

enum ZstdCStream {}
enum ZstdDStream {}

#[link(name = "zstd")]
unsafe extern "C" {
    fn ZSTD_createCStream() -> *mut ZstdCStream;
    fn ZSTD_freeCStream(stream: *mut ZstdCStream) -> usize;
    fn ZSTD_initCStream(stream: *mut ZstdCStream, level: c_int) -> usize;
    fn ZSTD_CCtx_setParameter(stream: *mut ZstdCStream, parameter: c_int, value: c_int) -> usize;
    fn ZSTD_compressStream2(
        stream: *mut ZstdCStream,
        output: *mut ZstdOutBuffer,
        input: *mut ZstdInBuffer,
        directive: c_int,
    ) -> usize;
    fn ZSTD_createDStream() -> *mut ZstdDStream;
    fn ZSTD_freeDStream(stream: *mut ZstdDStream) -> usize;
    fn ZSTD_initDStream(stream: *mut ZstdDStream) -> usize;
    fn ZSTD_DCtx_setParameter(stream: *mut ZstdDStream, parameter: c_int, value: c_int) -> usize;
    fn ZSTD_decompressStream(
        stream: *mut ZstdDStream,
        output: *mut ZstdOutBuffer,
        input: *mut ZstdInBuffer,
    ) -> usize;
    fn ZSTD_isError(code: usize) -> u32;
    fn ZSTD_getErrorName(code: usize) -> *const c_char;
    fn ZSTD_estimateCStreamSize(level: c_int) -> usize;
    fn ZSTD_estimateDStreamSize(window_size: usize) -> usize;
}

fn zstd_error(code: usize) -> String {
    unsafe {
        CStr::from_ptr(ZSTD_getErrorName(code))
            .to_string_lossy()
            .into_owned()
    }
}

fn checked(code: usize, operation: &str) -> Result<usize, String> {
    if unsafe { ZSTD_isError(code) } != 0 {
        Err(format!("zstd {operation}: {}", zstd_error(code)))
    } else {
        Ok(code)
    }
}

struct Compressor(*mut ZstdCStream);
impl Compressor {
    fn new(level: c_int) -> Result<Self, String> {
        let stream = unsafe { ZSTD_createCStream() };
        if stream.is_null() {
            return Err("zstd could not allocate a streaming compressor".into());
        }
        if let Err(error) = checked(
            unsafe { ZSTD_initCStream(stream, level) },
            "stream initialization",
        ) {
            unsafe { ZSTD_freeCStream(stream) };
            return Err(error);
        }
        // ZSTD_c_checksumFlag: CIX owns frame and whole-stream SHA-256.
        if let Err(error) = checked(
            unsafe { ZSTD_CCtx_setParameter(stream, 201, 0) },
            "checksum policy",
        ) {
            unsafe { ZSTD_freeCStream(stream) };
            return Err(error);
        }
        Ok(Self(stream))
    }

    fn flush(&mut self, input: &[u8]) -> Result<Vec<u8>, String> {
        // ZSTD_e_flush.  The state remains live after every CIX frame.
        self.process(input, 1)
    }

    fn finish(&mut self) -> Result<Vec<u8>, String> {
        // ZSTD_e_end writes the frame epilogue. It belongs in the CIX footer.
        let tail = self.process(&[], 2)?;
        if tail.len() != FINALIZATION_COST {
            return Err("zstd closing bytes differ from exact selector accounting".into());
        }
        Ok(tail)
    }

    fn process(&mut self, input: &[u8], directive: c_int) -> Result<Vec<u8>, String> {
        let cap = input
            .len()
            .checked_add(STREAM_SLACK)
            .ok_or("zstd frame output capacity overflow")?;
        let mut collected = Vec::with_capacity(cap);
        let mut source = ZstdInBuffer {
            src: input.as_ptr().cast(),
            size: input.len(),
            pos: 0,
        };
        loop {
            check_interrupted()?;
            let mut chunk = [0u8; 64 * 1024];
            let mut output = ZstdOutBuffer {
                dst: chunk.as_mut_ptr().cast(),
                size: chunk.len(),
                pos: 0,
            };
            let remaining = checked(
                unsafe { ZSTD_compressStream2(self.0, &mut output, &mut source, directive) },
                "stream compression",
            )?;
            collected.extend_from_slice(&chunk[..output.pos]);
            if collected.len() > cap {
                return Err("zstd streaming frame exceeds bounded output allowance".into());
            }
            if source.pos == source.size && remaining == 0 {
                return Ok(collected);
            }
            if output.pos == 0 && source.pos == source.size && remaining != 0 {
                return Err("zstd streaming flush made no progress".into());
            }
        }
    }
}
impl Drop for Compressor {
    fn drop(&mut self) {
        unsafe { ZSTD_freeCStream(self.0) };
    }
}

struct Decompressor(*mut ZstdDStream);
impl Decompressor {
    fn new() -> Result<Self, String> {
        let stream = unsafe { ZSTD_createDStream() };
        if stream.is_null() {
            return Err("zstd could not allocate a streaming decoder".into());
        }
        // ZSTD_d_windowLogMax = 100. Refuse frames whose window would exceed
        // the 128 MiB admission used by `required_memory`.
        if let Err(error) = checked(
            unsafe { ZSTD_DCtx_setParameter(stream, 100, 27) },
            "decoder window limit",
        ) {
            unsafe { ZSTD_freeDStream(stream) };
            return Err(error);
        }
        if let Err(error) = checked(
            unsafe { ZSTD_initDStream(stream) },
            "stream decoder initialization",
        ) {
            unsafe { ZSTD_freeDStream(stream) };
            return Err(error);
        }
        Ok(Self(stream))
    }

    fn decode_block(&mut self, payload: &[u8], expected: usize) -> Result<Vec<u8>, String> {
        let mut source = ZstdInBuffer {
            src: payload.as_ptr().cast(),
            size: payload.len(),
            pos: 0,
        };
        let mut restored = vec![0u8; expected];
        let mut produced = 0usize;
        let mut stream_ended = false;
        while source.pos < source.size {
            check_interrupted()?;
            if produced == restored.len() {
                return Err("zstd frame produced more bytes than its CIX descriptor".into());
            }
            let mut output = ZstdOutBuffer {
                dst: unsafe { restored.as_mut_ptr().add(produced) }.cast(),
                size: restored.len() - produced,
                pos: 0,
            };
            let before = source.pos;
            let remaining = checked(
                unsafe { ZSTD_decompressStream(self.0, &mut output, &mut source) },
                "stream decompression",
            )?;
            stream_ended |= remaining == 0;
            produced += output.pos;
            if before == source.pos && output.pos == 0 {
                return Err("zstd decoder made no progress in a frame".into());
            }
        }
        if produced != expected {
            return Err(format!(
                "zstd frame decoded {produced} bytes but CIX descriptor requires {expected}"
            ));
        }
        if stream_ended {
            return Err("zstd stream ended before the CIXZ1 footer".into());
        }
        Ok(restored)
    }

    fn finish(&mut self, payload: &[u8]) -> Result<(), String> {
        let mut source = ZstdInBuffer {
            src: payload.as_ptr().cast(),
            size: payload.len(),
            pos: 0,
        };
        loop {
            check_interrupted()?;
            let mut scratch = [0u8; 1];
            let mut output = ZstdOutBuffer {
                dst: scratch.as_mut_ptr().cast(),
                size: scratch.len(),
                pos: 0,
            };
            let before = source.pos;
            let remaining = checked(
                unsafe { ZSTD_decompressStream(self.0, &mut output, &mut source) },
                "stream footer decompression",
            )?;
            if output.pos != 0 {
                return Err("zstd footer unexpectedly decoded source bytes".into());
            }
            if source.pos == source.size && remaining == 0 {
                return Ok(());
            }
            if before == source.pos {
                return Err("truncated zstd stream footer".into());
            }
        }
    }
}
impl Drop for Decompressor {
    fn drop(&mut self) {
        unsafe { ZSTD_freeDStream(self.0) };
    }
}

fn level_for(effort: u8) -> c_int {
    if effort >= 9 {
        22
    } else {
        3
    }
}

pub(super) fn required_memory(block: u32) -> usize {
    let native = native_memory().unwrap_or(usize::MAX);
    (block as usize)
        // Input, bounded nested CIXG archive, current zstd payload and
        // confirmation payload. Twelve blocks leaves room for CIXG's framed
        // 8*n maximum plus all live application buffers.
        .saturating_mul(12)
        .saturating_add(STREAM_SLACK.saturating_mul(2))
        // A persistent candidate is represented by two independently advanced
        // contexts. This avoids unsupported copying/snapshotting of a live
        // zstd stream while permitting current-state comparisons.
        .saturating_add(native.saturating_mul(2))
}

fn decoder_memory() -> Result<usize, String> {
    checked(
        unsafe { ZSTD_estimateDStreamSize(ZSTD_MAX_WINDOW) },
        "stream decoder memory estimate",
    )
}

fn native_memory() -> Result<usize, String> {
    let encoder = checked(
        unsafe { ZSTD_estimateCStreamSize(level_for(9)) },
        "stream memory estimate",
    )?;
    let decoder = checked(
        unsafe { ZSTD_estimateDStreamSize(ZSTD_MAX_WINDOW) },
        "stream decoder memory estimate",
    )?;
    // Encoder and decoder run in separate processes, but making the public
    // stream admission cover the larger one keeps `--memory` authoritative
    // for either operation. Application buffers are charged by the caller.
    Ok(encoder.max(decoder))
}

pub(super) fn read_probe<R: Read>(
    src: &mut R,
    block: u32,
    input_fd: i32,
    flush_interval: Option<Duration>,
) -> Result<(Vec<u8>, bool), String> {
    read_block(src, block as usize, input_fd, flush_interval)
}

fn native_cixg_candidate(data: &[u8], memory: usize, workers: usize) -> Result<Vec<u8>, String> {
    // This is a complete bounded CIXG2 subarchive, not an estimator. Its
    // header, frame and footer are retained inside the outer route payload so
    // a later CIXZ1 decoder has every native route decision available.
    let mut archive = Vec::new();
    super::encode_with_strategy(
        std::io::Cursor::new(data),
        &mut archive,
        super::EncodeOptions {
            block: u32::try_from(data.len()).map_err(|_| "CIXG nested source overflow")?,
            level: 9,
            forced: None,
            backend: "hybrid",
            backend_set: false,
            format: "cixg2",
            input_fd: -1,
            flush_interval: None,
            memory,
            workers: workers.max(1),
            explain: false,
            verbose: false,
            strategy: "candidates",
        },
    )?;
    if !matches!(archive.get(..5), Some(b"CIXG1") | Some(b"CIXG2")) {
        return Err("nested native candidate did not produce CIXG".into());
    }
    Ok(archive)
}

fn resource_omission(error: &str) -> bool {
    error == crate::limits::DEADLINE_ERROR
        || error.contains(crate::limits::DEADLINE_ERROR)
        || error.contains("memory budget")
        || error.contains("memory reservation")
        || error.contains("exceed --memory")
        || error.contains("input/profiler buffers exceed --memory")
        || error.contains("block needs about")
}

fn write_reset<W: Write>(
    dst: &mut W,
    compressors: &mut Option<(Compressor, Compressor)>,
    active_segment: &mut bool,
) -> Result<(), String> {
    let Some((_, mut compressor)) = compressors.take() else {
        return Ok(());
    };
    if !*active_segment {
        return Ok(());
    }
    let tail = compressor.finish()?;
    if tail.len() > STREAM_SLACK {
        return Err("zstd reset footer exceeds bounded allowance".into());
    }
    put_frame(
        dst,
        ROUTE_ZSTD_RESET,
        0,
        u32::try_from(tail.len()).map_err(|_| "zstd reset footer overflow")?,
        &[0u8; 32],
    )
    .map_err(ioerr)?;
    dst.write_all(&tail).map_err(ioerr)?;
    dst.flush().map_err(ioerr)?;
    *active_segment = false;
    Ok(())
}

struct StreamTotals {
    whole: Sha256,
    total: u64,
    frames: u64,
}

impl StreamTotals {
    fn new() -> Self {
        Self {
            whole: Sha256::new(),
            total: 0,
            frames: 0,
        }
    }

    fn record(&mut self, data: &[u8]) -> Result<(), String> {
        self.whole.update(data);
        self.total = self
            .total
            .checked_add(data.len() as u64)
            .ok_or("input length overflow")?;
        self.frames += 1;
        Ok(())
    }
}

struct RegionCandidates {
    native: Option<Vec<u8>>,
    native_omission: Option<String>,
    zstd_payload: Vec<u8>,
}

fn prepare_region_candidates(
    data: &[u8],
    block: u32,
    level: u8,
    memory: usize,
    workers: usize,
    compressors: &mut Option<(Compressor, Compressor)>,
) -> Result<RegionCandidates, String> {
    let native_budget = memory.checked_sub(
        native_memory()?
            .saturating_mul(2)
            .saturating_add((block as usize).saturating_mul(12))
            .saturating_add(STREAM_SLACK.saturating_mul(2)),
    );
    let (native, native_omission) = match native_budget {
        Some(budget) => match native_cixg_candidate(data, budget, workers) {
            Ok(candidate) => (Some(candidate), None),
            Err(error) if resource_omission(&error) => (None, Some(error)),
            Err(error) => return Err(error),
        },
        None => (
            None,
            Some("CIXG candidate omitted: stream reservation exhausts --memory".into()),
        ),
    };
    if compressors.is_none() {
        *compressors = Some((
            Compressor::new(level_for(level))?,
            Compressor::new(level_for(level))?,
        ));
    }
    Ok(RegionCandidates {
        native,
        native_omission,
        zstd_payload: compressors
            .as_mut()
            .expect("stream contexts initialized")
            .0
            .flush(data)?,
    })
}

fn select_region_route(data_len: usize, candidates: &RegionCandidates, active: bool) -> u8 {
    let reset = if active {
        FRAME_PREFIX_COST.saturating_add(FINALIZATION_COST)
    } else {
        0
    };
    let native_len = candidates
        .native
        .as_ref()
        .map_or(usize::MAX, |payload| payload.len().saturating_add(reset));
    let raw_len = data_len.saturating_add(reset);
    let zstd_len = candidates
        .zstd_payload
        .len()
        .saturating_add(FINALIZATION_COST);
    if zstd_len <= raw_len && zstd_len <= native_len {
        ROUTE_ZSTD
    } else if native_len <= raw_len {
        ROUTE_CIXG
    } else {
        ROUTE_RAW
    }
}

fn explain_region(data_len: usize, candidates: &RegionCandidates, active: bool, selected: u8) {
    let reset = if active {
        FRAME_PREFIX_COST.saturating_add(FINALIZATION_COST)
    } else {
        0
    };
    let native = candidates.native.as_ref().map_or_else(
        || {
            format!(
                "omitted:{}",
                candidates.native_omission.as_deref().unwrap_or("unknown")
            )
        },
        |payload| format!("{}B", payload.len().saturating_add(reset)),
    );
    eprintln!(
        "cix: CIXZ1 region source={} candidates=[raw:{}B,zstd-persistent:{}B,cixg:{}] transition_allowance={}B selected={}",
        data_len, data_len.saturating_add(reset), candidates.zstd_payload.len().saturating_add(FINALIZATION_COST), native, reset,
        match selected { ROUTE_ZSTD => "zstd-persistent", ROUTE_CIXG => "cixg2", _ => "raw" },
    );
}

fn emit_frame<W: Write>(
    dst: &mut W,
    route: u8,
    source: &[u8],
    payload: &[u8],
) -> Result<(), String> {
    let hash: [u8; 32] = Sha256::digest(source).into();
    put_frame(
        dst,
        route,
        u32::try_from(source.len()).map_err(|_| "source block overflow")?,
        u32::try_from(payload.len()).map_err(|_| "payload block overflow")?,
        &hash,
    )
    .map_err(ioerr)?;
    dst.write_all(payload).map_err(ioerr)?;
    dst.flush().map_err(ioerr)
}

fn emit_selected_region<W: Write>(
    dst: &mut W,
    data: &[u8],
    candidates: RegionCandidates,
    selected: u8,
    compressors: &mut Option<(Compressor, Compressor)>,
    active: &mut bool,
) -> Result<(), String> {
    match selected {
        ROUTE_RAW => {
            write_reset(dst, compressors, active)?;
            emit_frame(dst, ROUTE_RAW, data, data)
        }
        ROUTE_CIXG => {
            write_reset(dst, compressors, active)?;
            let payload = candidates
                .native
                .expect("selected native candidate is present");
            if payload.len() > data.len().saturating_mul(8).saturating_add(8192) {
                return Err("nested CIXG candidate exceeds bounded frame allowance".into());
            }
            emit_frame(dst, ROUTE_CIXG, data, &payload)
        }
        ROUTE_ZSTD => {
            if candidates.zstd_payload.len() > data.len().saturating_add(STREAM_SLACK) {
                return Err("zstd streaming payload exceeds bounded frame allowance".into());
            }
            // Release the losing nested archive before duplicating the zstd payload.
            drop(candidates.native);
            let confirmation = compressors
                .as_mut()
                .expect("stream contexts initialized")
                .1
                .flush(data)?;
            if confirmation != candidates.zstd_payload {
                return Err("zstd duplicate persistent contexts diverged".into());
            }
            *active = true;
            emit_frame(dst, ROUTE_ZSTD, data, &candidates.zstd_payload)
        }
        _ => Err("invalid selected CIXZ1 route".into()),
    }
}

#[derive(Clone, Copy)]
struct RegionConfig {
    block: u32,
    level: u8,
    memory: usize,
    workers: usize,
    explain: bool,
}

/// Inputs fixed by the caller's admitted CIXZ1 stream decision.
///
/// Keeping these values together makes the boundary between the outer CLI
/// selection and this stateful encoder explicit. The first block was already
/// read for bounded probing and therefore must be consumed before `src`.
pub(super) struct EncodeRequest {
    pub(super) first: Vec<u8>,
    pub(super) first_eof: bool,
    pub(super) block: u32,
    pub(super) level: u8,
    pub(super) input_fd: i32,
    pub(super) flush_interval: Option<Duration>,
    pub(super) memory: usize,
    pub(super) workers: usize,
    pub(super) explain: bool,
    pub(super) verbose: bool,
}

fn encode_region<W: Write>(
    dst: &mut W,
    data: &[u8],
    config: RegionConfig,
    compressors: &mut Option<(Compressor, Compressor)>,
    active: &mut bool,
    totals: &mut StreamTotals,
) -> Result<(), String> {
    let was_active = *active;
    let candidates = prepare_region_candidates(
        data,
        config.block,
        config.level,
        config.memory,
        config.workers,
        compressors,
    )?;
    let selected = select_region_route(data.len(), &candidates, was_active);
    if config.explain {
        explain_region(data.len(), &candidates, was_active, selected);
    }
    emit_selected_region(dst, data, candidates, selected, compressors, active)?;
    totals.record(data)
}

/// Encode a selected persistent zstd CIXZ1 stream. This function does not
/// choose the route: its caller must have made a bounded probe first.
pub(super) fn encode<R: Read, W: Write>(
    mut src: R,
    mut dst: W,
    request: EncodeRequest,
) -> Result<(), String> {
    let EncodeRequest {
        first,
        first_eof,
        block,
        level,
        input_fd,
        flush_interval,
        memory,
        workers,
        explain,
        verbose,
    } = request;
    if required_memory(block) > memory {
        return Err("persistent zstd stream exceeds --memory".into());
    }
    dst.write_all(MAGIC).map_err(ioerr)?;
    dst.write_all(&block.to_le_bytes()).map_err(ioerr)?;
    dst.write_all(&0u32.to_le_bytes()).map_err(ioerr)?;
    let mut compressors = None;
    let mut active_zstd_segment = false;
    let mut totals = StreamTotals::new();
    let mut next = Some((first, first_eof));
    loop {
        let (data, eof) = match next.take() {
            Some(value) => value,
            None => read_block(&mut src, block as usize, input_fd, flush_interval)?,
        };
        if !data.is_empty() {
            encode_region(
                &mut dst,
                &data,
                RegionConfig {
                    block,
                    level,
                    memory,
                    workers,
                    explain,
                },
                &mut compressors,
                &mut active_zstd_segment,
                &mut totals,
            )?;
        }
        if eof {
            break;
        }
    }
    let tail = match compressors {
        Some((mut compressor, _)) if active_zstd_segment => compressor.finish()?,
        _ => Vec::new(),
    };
    if tail.len() > STREAM_SLACK {
        return Err("zstd stream footer exceeds bounded allowance".into());
    }
    let hash: [u8; 32] = totals.whole.finalize().into();
    put_frame(
        &mut dst,
        END,
        0,
        u32::try_from(tail.len()).map_err(|_| "zstd footer overflow")?,
        &hash,
    )
    .map_err(ioerr)?;
    dst.write_all(&tail).map_err(ioerr)?;
    dst.flush().map_err(ioerr)?;
    if verbose {
        eprintln!("cix: effort={} format=CIXZ1 backend=zstd-persistent frames={} source_bytes={} zstd_workers=1 native_candidate_workers={} memory_reservation={}",
            effort_display_name(level), totals.frames, totals.total, workers, required_memory(block));
    }
    Ok(())
}

fn read_block<R: Read>(
    src: &mut R,
    block: usize,
    input_fd: i32,
    flush_interval: Option<Duration>,
) -> Result<(Vec<u8>, bool), String> {
    let mut bytes = vec![0u8; block];
    let mut filled = 0usize;
    let mut since: Option<Instant> = None;
    loop {
        let timeout = since.zip(flush_interval).and_then(|(start, interval)| {
            let left = interval.saturating_sub(start.elapsed());
            (left > Duration::ZERO).then_some(left)
        });
        if since
            .zip(flush_interval)
            .is_some_and(|(start, interval)| start.elapsed() >= interval)
        {
            bytes.truncate(filled);
            return Ok((bytes, false));
        }
        match read_poll(src, &mut bytes[filled..], input_fd, timeout)? {
            Some(0) => {
                bytes.truncate(filled);
                return Ok((bytes, true));
            }
            Some(n) => {
                filled += n;
                since.get_or_insert_with(Instant::now);
                if filled == block {
                    return Ok((bytes, false));
                }
            }
            None if filled != 0 => {
                bytes.truncate(filled);
                return Ok((bytes, false));
            }
            None => {}
        }
    }
}

/// Decode the remainder after `main` has already consumed the five-byte
/// CIXZ1 magic.  Data reaches `dst` frame-by-frame; an atomic file caller
/// therefore only replaces its destination after the footer is verified.
struct FrameHeader {
    route: u8,
    source_len: usize,
    payload_len: usize,
    hash: [u8; 32],
}

fn read_frame_header<R: Read>(src: &mut R, input_fd: i32) -> Result<FrameHeader, String> {
    let mut route = [0u8; 1];
    read_exact_poll(src, &mut route, input_fd)?;
    let source_len = read_u32(src, input_fd)? as usize;
    let payload_len = read_u32(src, input_fd)? as usize;
    let mut hash = [0u8; 32];
    read_exact_poll(src, &mut hash, input_fd)?;
    Ok(FrameHeader {
        route: route[0],
        source_len,
        payload_len,
        hash,
    })
}

fn validate_data_frame(frame: &FrameHeader, block: u32, memory: usize) -> Result<(), String> {
    if !matches!(frame.route, ROUTE_ZSTD | ROUTE_RAW | ROUTE_CIXG)
        || frame.source_len == 0
        || frame.source_len > block as usize
    {
        return Err("invalid CIXZ1 frame descriptor".into());
    }
    let allowed = match frame.route {
        ROUTE_ZSTD => frame.payload_len <= frame.source_len.saturating_add(STREAM_SLACK),
        ROUTE_RAW => frame.payload_len == frame.source_len,
        ROUTE_CIXG => frame.payload_len <= frame.source_len.saturating_mul(8).saturating_add(8192),
        _ => false,
    };
    if !allowed {
        return Err("CIXZ1 payload exceeds bounded frame allowance".into());
    }
    let live = frame
        .source_len
        .checked_add(frame.payload_len)
        .and_then(|size| size.checked_add(decoder_memory().unwrap_or(usize::MAX)))
        .ok_or("CIXZ1 frame memory overflow")?;
    if live > memory {
        return Err("CIXZ1 frame exceeds --memory".into());
    }
    Ok(())
}

fn decode_data_payload(
    frame: &FrameHeader,
    payload: Vec<u8>,
    decoder: &mut Option<Decompressor>,
    memory: usize,
) -> Result<Vec<u8>, String> {
    match frame.route {
        ROUTE_RAW => {
            if decoder.is_some() {
                return Err("CIXZ1 raw frame requires an explicit zstd reset boundary".into());
            }
            Ok(payload)
        }
        ROUTE_CIXG => decode_nested_payload(frame, payload, decoder, memory),
        ROUTE_ZSTD => decoder
            .get_or_insert(Decompressor::new()?)
            .decode_block(&payload, frame.source_len),
        _ => Err("invalid CIXZ1 route".into()),
    }
}

fn decode_nested_payload(
    frame: &FrameHeader,
    payload: Vec<u8>,
    decoder: &Option<Decompressor>,
    memory: usize,
) -> Result<Vec<u8>, String> {
    if decoder.is_some() {
        return Err("CIXZ1 CIXG frame requires an explicit zstd reset boundary".into());
    }
    if !matches!(payload.get(..5), Some(b"CIXG1") | Some(b"CIXG2")) {
        return Err("CIXZ1 nested frame is not a CIXG1/CIXG2 archive".into());
    }
    let nested_memory = memory
        .checked_sub(frame.payload_len)
        .and_then(|left| left.checked_sub(frame.source_len))
        .ok_or("CIXZ1 nested CIXG exceeds --memory")?;
    let mut nested = CappedWriter::new(frame.source_len);
    super::decode(
        std::io::Cursor::new(payload),
        &mut nested,
        false,
        false,
        -1,
        nested_memory,
    )?;
    let restored = nested.into_inner();
    if restored.len() != frame.source_len {
        return Err("CIXZ1 nested CIXG decoded length mismatch".into());
    }
    Ok(restored)
}

fn decode_reset<R: Read>(
    src: &mut R,
    input_fd: i32,
    frame: &FrameHeader,
    decoder: &mut Option<Decompressor>,
) -> Result<(), String> {
    if frame.source_len != 0 || frame.payload_len != FINALIZATION_COST || frame.hash != [0u8; 32] {
        return Err("invalid CIXZ1 zstd reset frame".into());
    }
    let mut tail = vec![0u8; frame.payload_len];
    read_exact_poll(src, &mut tail, input_fd)?;
    decoder
        .take()
        .ok_or("CIXZ1 zstd reset without active stream")?
        .finish(&tail)
}

struct FinishRequest<'a> {
    input_fd: i32,
    frame: &'a FrameHeader,
    decoder: &'a mut Option<Decompressor>,
    whole: Sha256,
    verify_only: bool,
    list: bool,
    total: u64,
    frames: u64,
}

fn finish_stream<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    request: FinishRequest<'_>,
) -> Result<(), String> {
    let FinishRequest {
        input_fd,
        frame,
        decoder,
        whole,
        verify_only,
        list,
        total,
        frames,
    } = request;
    let expected_tail = if decoder.is_some() {
        FINALIZATION_COST
    } else {
        0
    };
    if frame.source_len != 0 || frame.payload_len != expected_tail {
        return Err("invalid CIXZ1 footer descriptor".into());
    }
    let mut tail = vec![0u8; frame.payload_len];
    read_exact_poll(src, &mut tail, input_fd)?;
    match decoder.take() {
        Some(mut state) => state.finish(&tail)?,
        None if tail.is_empty() => {}
        None => return Err("CIXZ1 footer has a zstd epilogue without an active stream".into()),
    }
    if frame.hash != <[u8; 32]>::from(whole.finalize()) {
        return Err("CIXZ1 whole-stream checksum mismatch".into());
    }
    let mut extra = [0u8; 1];
    loop {
        match read_poll(src, &mut extra, input_fd, None)? {
            Some(0) => break,
            Some(_) => return Err("trailing bytes after CIXZ1 footer".into()),
            None => continue,
        }
    }
    if !verify_only {
        dst.flush().map_err(ioerr)?;
    }
    if list {
        eprintln!("CIXZ1 stream: backend=zstd-persistent frames={frames} decoded {total} bytes");
    }
    Ok(())
}

pub(super) fn decode<R: Read, W: Write>(
    mut src: R,
    mut dst: W,
    verify_only: bool,
    list: bool,
    input_fd: i32,
    memory: usize,
) -> Result<(), String> {
    let mut header = [0u8; 8];
    read_exact_poll(&mut src, &mut header, input_fd)?;
    let block = u32::from_le_bytes(header[..4].try_into().unwrap());
    let reserved = u32::from_le_bytes(header[4..].try_into().unwrap());
    if block == 0 || block > MAX_BLOCK || reserved != 0 {
        return Err("invalid CIXZ1 header".into());
    }
    if decoder_memory()?
        .saturating_add((block as usize).saturating_mul(12))
        .saturating_add(STREAM_SLACK)
        > memory
    {
        return Err("persistent zstd decoder exceeds --memory".into());
    }
    let mut decoder = None;
    let mut whole = Sha256::new();
    let mut total = 0u64;
    let mut frames = 0u64;
    loop {
        let frame = read_frame_header(&mut src, input_fd)?;
        if frame.route == END {
            return finish_stream(
                &mut src,
                &mut dst,
                FinishRequest {
                    input_fd,
                    frame: &frame,
                    decoder: &mut decoder,
                    whole,
                    verify_only,
                    list,
                    total,
                    frames,
                },
            );
        }
        if frame.route == ROUTE_ZSTD_RESET {
            decode_reset(&mut src, input_fd, &frame, &mut decoder)?;
            continue;
        }
        validate_data_frame(&frame, block, memory)?;
        let mut payload = vec![0u8; frame.payload_len];
        read_exact_poll(&mut src, &mut payload, input_fd)?;
        let restored = decode_data_payload(&frame, payload, &mut decoder, memory)?;
        if <[u8; 32]>::from(Sha256::digest(&restored)) != frame.hash {
            return Err("CIXZ1 block checksum mismatch".into());
        }
        whole.update(&restored);
        total = total
            .checked_add(restored.len() as u64)
            .ok_or("decoded length overflow")?;
        frames += 1;
        if list {
            eprintln!(
                "CIXZ1 block: route={} source={} bytes payload={} bytes",
                match frame.route {
                    ROUTE_ZSTD => "zstd-persistent",
                    ROUTE_CIXG => "cixg2",
                    _ => "raw",
                },
                frame.source_len,
                frame.payload_len
            );
        }
        if !verify_only {
            dst.write_all(&restored).map_err(ioerr)?;
            dst.flush().map_err(ioerr)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn decode_archive(archive: Vec<u8>, memory: usize) -> Result<Vec<u8>, String> {
        let mut restored = Vec::new();
        // Production `super::decode` consumes the magic before dispatching
        // here. Unit tests call this module directly, so mirror that boundary.
        decode(
            Cursor::new(archive[5..].to_vec()),
            &mut restored,
            false,
            false,
            -1,
            memory,
        )?;
        Ok(restored)
    }

    fn round_trip(data: Vec<u8>) {
        let block = 64 * 1024;
        let memory = required_memory(block).saturating_add(4 * 1024 * 1024);
        let mut archive = Vec::new();
        let split = data.len().min(block as usize);
        let first = data[..split].to_vec();
        let rest = data[split..].to_vec();
        encode(
            Cursor::new(rest),
            &mut archive,
            EncodeRequest {
                first,
                first_eof: split == data.len(),
                block,
                level: 9,
                input_fd: -1,
                flush_interval: None,
                memory,
                workers: 1,
                explain: false,
                verbose: false,
            },
        )
        .unwrap();
        assert_eq!(&archive[..5], MAGIC);
        let restored = decode_archive(archive, memory).unwrap();
        assert_eq!(restored, data);
    }

    #[test]
    fn persistent_backend_round_trips_strong_weak_and_mixed_regions() {
        let mut mixed = b"repeated text channel ".repeat(8192);
        mixed.extend((0..(64 * 1024)).map(|n| (n as u8).wrapping_mul(73)));
        mixed.extend(b"repeated text channel ".repeat(8192));
        round_trip(mixed);
        round_trip(Vec::new());
    }

    #[test]
    fn persistent_backend_rejects_corrupt_frame() {
        let input = b"corrupt persistent zstd CIX frame".repeat(1024);
        let block = 64 * 1024;
        let memory = required_memory(block).saturating_add(4 * 1024 * 1024);
        let mut archive = Vec::new();
        encode(
            Cursor::new(Vec::<u8>::new()),
            &mut archive,
            EncodeRequest {
                first: input,
                first_eof: true,
                block,
                level: 9,
                input_fd: -1,
                flush_interval: None,
                memory,
                workers: 1,
                explain: false,
                verbose: false,
            },
        )
        .unwrap();
        // Header then first CIX frame header.  Mutate payload only when the
        // selected first route is zstd; otherwise corrupt its hash field.
        let first = 13usize;
        let source = u32::from_le_bytes(archive[first + 1..first + 5].try_into().unwrap()) as usize;
        let payload =
            u32::from_le_bytes(archive[first + 5..first + 9].try_into().unwrap()) as usize;
        let payload_start = first + 41;
        assert!(source != 0 && payload != 0);
        archive[payload_start] ^= 0x80;
        assert!(decode_archive(archive, memory).is_err());
    }

    #[test]
    fn nested_cixg_frame_is_bounded_and_round_trips() {
        let input = b"nested CIXG selection fixture ".repeat(2048);
        let block = 64 * 1024;
        let memory = required_memory(block).saturating_add(8 * 1024 * 1024);
        let payload = native_cixg_candidate(&input, memory / 2, 1).unwrap();
        assert!(matches!(payload.get(..5), Some(b"CIXG1") | Some(b"CIXG2")));
        let mut archive = Vec::new();
        archive.extend_from_slice(MAGIC);
        archive.extend_from_slice(&block.to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        let frame_hash: [u8; 32] = Sha256::digest(&input).into();
        put_frame(
            &mut archive,
            ROUTE_CIXG,
            input.len() as u32,
            payload.len() as u32,
            &frame_hash,
        )
        .unwrap();
        archive.extend_from_slice(&payload);
        let whole_hash: [u8; 32] = Sha256::digest(&input).into();
        put_frame(&mut archive, END, 0, 0, &whole_hash).unwrap();
        let restored = decode_archive(archive, memory).unwrap();
        assert_eq!(restored, input);
    }

    #[test]
    fn fresh_raw_segment_has_no_invalid_zstd_reset() {
        let mut input = vec![0u8; 64 * 1024];
        let mut state = 0x6d2b_79f5u32;
        for byte in &mut input {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *byte = (state >> 24) as u8;
        }
        let block = 64 * 1024;
        let memory = required_memory(block).saturating_add(8 * 1024 * 1024);
        let mut archive = Vec::new();
        encode(
            Cursor::new(Vec::<u8>::new()),
            &mut archive,
            EncodeRequest {
                first: input.clone(),
                first_eof: true,
                block,
                level: 9,
                input_fd: -1,
                flush_interval: None,
                memory,
                workers: 1,
                explain: false,
                verbose: false,
            },
        )
        .unwrap();
        assert_eq!(archive[13], ROUTE_RAW);
        let restored = decode_archive(archive, memory).unwrap();
        assert_eq!(restored, input);
    }
    #[test]
    fn nested_archive_cannot_expand_past_outer_length() {
        let input = b"bounded inner output".repeat(32);
        let memory = required_memory(1024).saturating_add(8 * 1024 * 1024);
        let payload = native_cixg_candidate(&input, memory, 1).unwrap();
        assert!(payload.len() < 8192);
        let mut archive = Vec::new();
        archive.extend_from_slice(MAGIC);
        archive.extend_from_slice(&1024u32.to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        put_frame(&mut archive, ROUTE_CIXG, 1, payload.len() as u32, &[0; 32]).unwrap();
        archive.extend_from_slice(&payload);
        let error = decode_archive(archive, memory).unwrap_err();
        assert!(error.contains("exceeds CIXZ1 source descriptor"), "{error}");
    }

    #[test]
    fn inactive_footer_rejects_unaccounted_zstd_tail() {
        let mut archive = Vec::new();
        archive.extend_from_slice(MAGIC);
        archive.extend_from_slice(&1024u32.to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        put_frame(&mut archive, END, 0, 3, &Sha256::digest([]).into()).unwrap();
        archive.extend_from_slice(&[1, 0, 0]);
        let error = decode_archive(archive, required_memory(1024)).unwrap_err();
        assert!(error.contains("footer"), "{error}");
    }
}
