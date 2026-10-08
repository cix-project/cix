//! Native one-shot standard-format byte streams, never CIX envelopes.
use crate::external;
use flate2::{
    bufread::GzDecoder,
    write::{DeflateEncoder, GzEncoder, ZlibEncoder},
    Compression, Decompress, FlushDecompress, Status,
};
use std::ffi::{c_char, c_void, CStr};
use std::io::{BufReader, Cursor, Read, Write};
use std::ptr;

pub const MAX_STANDARD_INPUT: usize = 128 << 20;
const SNAPPY_CHUNK: usize = 64 << 10;
const SNAPPY_IDENTIFIER: &[u8] = b"\xff\x06\0\0sNaPpY";
const FLATE_STATE_BYTES: usize = 1 << 20;
// A frame may use 4 MiB blocks and retain history; reserve substantially more
// than one block for the native decoder's block buffer and bookkeeping.
const LZ4_STATE_BYTES: usize = 16 << 20;
const EXTERNAL_EXPANSION_BOUND: usize = 2 << 20;
const BROTLI_FALLBACK_WORKSPACE_BYTES: usize = 512 << 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandardFormat {
    Gzip,
    Zlib,
    Deflate,
    Bzip2,
    Xz,
    Zstd,
    Brotli,
    Lz4Frame,
    SnappyFramed,
}
#[derive(Clone, Debug)]
pub struct StandardOptions {
    pub level: u8,
    pub output_limit: usize,
    pub memory_limit: usize,
}
impl Default for StandardOptions {
    fn default() -> Self {
        Self {
            level: 6,
            output_limit: MAX_STANDARD_INPUT,
            memory_limit: 256 << 20,
        }
    }
}
#[derive(Debug)]
pub enum StandardError {
    InvalidOptions(&'static str),
    OutputLimit,
    Unavailable(StandardFormat),
    Codec(String),
}
impl std::fmt::Display for StandardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOptions(x) => f.write_str(x),
            Self::OutputLimit => f.write_str("standard-format output exceeds limit"),
            Self::Unavailable(x) => write!(f, "{x:?} is unavailable in this native build"),
            Self::Codec(x) => f.write_str(x),
        }
    }
}
impl std::error::Error for StandardError {}

fn admit(input: usize, options: &StandardOptions, extra: usize) -> Result<(), StandardError> {
    if input > MAX_STANDARD_INPUT {
        return Err(StandardError::InvalidOptions(
            "standard-format input exceeds 128 MiB",
        ));
    }
    if options.output_limit == 0 || options.memory_limit == 0 {
        return Err(StandardError::InvalidOptions(
            "output and memory limits must be non-zero",
        ));
    }
    let live = input
        .checked_add(options.output_limit)
        .and_then(|n| n.checked_add(1))
        .and_then(|n| n.checked_add(extra))
        .ok_or(StandardError::InvalidOptions(
            "standard-format live-buffer size overflow",
        ))?;
    if live > options.memory_limit {
        return Err(StandardError::InvalidOptions(
            "standard-format input, bounded output, and codec state exceed memory limit",
        ));
    }
    Ok(())
}
fn check(input: usize, options: &StandardOptions) -> Result<(), StandardError> {
    admit(input, options, 0)
}
fn profile(level: u8) -> Result<&'static str, StandardError> {
    match level {
        0..=2 => Ok("fast"),
        3..=7 => Ok("default"),
        8..=9 => Ok("size"),
        _ => Err(StandardError::InvalidOptions("level must be 0 through 9")),
    }
}
fn xz_preset(level: u8) -> Result<u32, StandardError> {
    match profile(level)? {
        "fast" => Ok(0),
        "default" => Ok(6),
        "size" => Ok(9),
        _ => unreachable!(),
    }
}
fn brotli_workspace(_level: u8, _input_len: usize) -> usize {
    // The bundled Brotli ABI doesn't export the optional peak estimator.
    // Keep admission conservative rather than guessing below its window,
    // match finder, and allocator state.
    BROTLI_FALLBACK_WORKSPACE_BYTES
}
fn output_buffer(limit: usize) -> Result<Vec<u8>, StandardError> {
    let len = limit
        .checked_add(1)
        .ok_or(StandardError::InvalidOptions("output limit overflow"))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(len)
        .map_err(|_| StandardError::Codec("standard-format output allocation failed".into()))?;
    output.resize(len, 0);
    Ok(output)
}
struct CappedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl CappedWriter {
    fn new(limit: usize) -> Result<Self, StandardError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(limit)
            .map_err(|_| StandardError::Codec("standard-format output allocation failed".into()))?;
        Ok(Self { bytes, limit })
    }
}
impl Write for CappedWriter {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        let available = self.limit.saturating_sub(self.bytes.len());
        let take = available.min(input.len());
        self.bytes.extend_from_slice(&input[..take]);
        Ok(take)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn flate_encode<W: Write>(
    mut writer: impl FnMut(CappedWriter) -> W,
    input: &[u8],
    limit: usize,
    finish: impl FnOnce(W) -> std::io::Result<CappedWriter>,
) -> Result<Vec<u8>, StandardError> {
    let mut encoder = writer(CappedWriter::new(limit)?);
    if encoder.write_all(input).is_err() {
        return Err(StandardError::OutputLimit);
    }
    Ok(finish(encoder)
        .map_err(|_| StandardError::OutputLimit)?
        .bytes)
}

pub fn encode(
    format: StandardFormat,
    input: &[u8],
    options: &StandardOptions,
) -> Result<Vec<u8>, StandardError> {
    check(input.len(), options)?;
    profile(options.level)?;
    let level = Compression::new(options.level as u32);
    let out = match format {
        StandardFormat::Gzip => {
            admit(input.len(), options, FLATE_STATE_BYTES)?;
            flate_encode(
                |w| GzEncoder::new(w, level),
                input,
                options.output_limit,
                |e| e.finish(),
            )?
        }
        StandardFormat::Zlib => {
            admit(input.len(), options, FLATE_STATE_BYTES)?;
            flate_encode(
                |w| ZlibEncoder::new(w, level),
                input,
                options.output_limit,
                |e| e.finish(),
            )?
        }
        StandardFormat::Deflate => {
            admit(input.len(), options, FLATE_STATE_BYTES)?;
            flate_encode(
                |w| DeflateEncoder::new(w, level),
                input,
                options.output_limit,
                |e| e.finish(),
            )?
        }
        // The providers calculate a bounded destination before they allocate it.
        StandardFormat::Xz => {
            admit(
                input.len(),
                options,
                external::xz_easy_memory_usage(xz_preset(options.level)?),
            )?;
            external::raw_xz_encode(input, profile(options.level)?, options.output_limit)
                .map_err(StandardError::Codec)?
        }
        StandardFormat::Brotli => {
            admit(
                input.len(),
                options,
                brotli_workspace(options.level, input.len()),
            )?;
            external::raw_brotli_encode(input, options.level as u32, 22, options.output_limit)
                .map_err(StandardError::Codec)?
        }
        StandardFormat::Bzip2 | StandardFormat::Zstd => encode_external(format, input, options)?,
        StandardFormat::Lz4Frame => lz4_encode(input, options)?,
        StandardFormat::SnappyFramed => snappy_encode(input, options)?,
    };
    if out.len() > options.output_limit {
        Err(StandardError::OutputLimit)
    } else {
        Ok(out)
    }
}
fn encode_external(
    format: StandardFormat,
    input: &[u8],
    options: &StandardOptions,
) -> Result<Vec<u8>, StandardError> {
    let backend = if format == StandardFormat::Bzip2 {
        "bzip2"
    } else {
        "zstd"
    };
    let candidate = external::best_candidates()
        .iter()
        .find(|candidate| candidate.backend == backend)
        .ok_or(StandardError::Unavailable(format))?;
    let upper = input
        .len()
        .checked_add(EXTERNAL_EXPANSION_BOUND)
        .ok_or(StandardError::OutputLimit)?;
    if upper > options.output_limit {
        return Err(StandardError::OutputLimit);
    }
    admit(
        input.len(),
        options,
        candidate
            .encoder_peak_bytes(input.len())
            .map_err(StandardError::Codec)?,
    )?;
    external::encode_payload(backend, profile(options.level)?, input).map_err(StandardError::Codec)
}

pub fn decode(
    format: StandardFormat,
    input: &[u8],
    options: &StandardOptions,
) -> Result<Vec<u8>, StandardError> {
    check(input.len(), options)?;
    profile(options.level)?;
    let out = match format {
        StandardFormat::Gzip => {
            admit(input.len(), options, FLATE_STATE_BYTES)?;
            gzip_decode(input, options.output_limit)?
        }
        StandardFormat::Zlib => {
            admit(input.len(), options, FLATE_STATE_BYTES)?;
            inflate(input, options.output_limit, true)?
        }
        StandardFormat::Deflate => {
            admit(input.len(), options, FLATE_STATE_BYTES)?;
            inflate(input, options.output_limit, false)?
        }
        StandardFormat::Xz => {
            external::raw_xz_decode_bounded(input, options.output_limit, options.memory_limit)
                .map_err(StandardError::Codec)?
        }
        StandardFormat::Brotli => {
            admit(
                input.len(),
                options,
                brotli_workspace(options.level, input.len()),
            )?;
            external::raw_brotli_decode_bounded(input, options.output_limit)
                .map_err(StandardError::Codec)?
        }
        StandardFormat::Bzip2 | StandardFormat::Zstd => decode_external(format, input, options)?,
        StandardFormat::Lz4Frame => lz4_decode(input, options)?,
        StandardFormat::SnappyFramed => snappy_decode(input, options)?,
    };
    if out.len() > options.output_limit {
        Err(StandardError::OutputLimit)
    } else {
        Ok(out)
    }
}
fn gzip_decode(input: &[u8], limit: usize) -> Result<Vec<u8>, StandardError> {
    let mut decoder = GzDecoder::new(BufReader::new(Cursor::new(input)));
    let mut out = output_buffer(limit)?;
    let mut written = 0;
    loop {
        let n = decoder
            .read(&mut out[written..])
            .map_err(|e| StandardError::Codec(e.to_string()))?;
        if n == 0 {
            break;
        }
        written += n;
        if written > limit {
            return Err(StandardError::OutputLimit);
        }
    }
    let reader = decoder.into_inner();
    if !reader.buffer().is_empty() || reader.get_ref().position() as usize != input.len() {
        return Err(StandardError::Codec(
            "gzip trailing or concatenated data".into(),
        ));
    }
    out.truncate(written);
    Ok(out)
}
fn decode_external(
    format: StandardFormat,
    input: &[u8],
    options: &StandardOptions,
) -> Result<Vec<u8>, StandardError> {
    let mut output = output_buffer(options.output_limit)?;
    let native = options
        .memory_limit
        .checked_sub(input.len())
        .and_then(|n| n.checked_sub(output.len()))
        .ok_or_else(|| {
            StandardError::Codec("standard-format decoder memory admission failed".into())
        })?;
    let id = if format == StandardFormat::Bzip2 {
        2
    } else {
        4
    };
    let written = external::decode_payload(id, input, &mut output, options.output_limit, native)
        .map_err(StandardError::Codec)?;
    if written > options.output_limit {
        return Err(StandardError::OutputLimit);
    }
    output.truncate(written);
    Ok(output)
}
fn inflate(input: &[u8], limit: usize, zlib: bool) -> Result<Vec<u8>, StandardError> {
    let mut out = output_buffer(limit)?;
    let mut decoder = Decompress::new(zlib);
    let status = decoder
        .decompress(input, &mut out, FlushDecompress::Finish)
        .map_err(|e| StandardError::Codec(e.to_string()))?;
    if status != Status::StreamEnd || decoder.total_in() != input.len() as u64 {
        return Err(StandardError::Codec(
            "truncated or trailing deflate stream".into(),
        ));
    }
    if decoder.total_out() > limit as u64 {
        return Err(StandardError::OutputLimit);
    }
    out.truncate(decoder.total_out() as usize);
    Ok(out)
}

#[repr(C)]
struct Lz4Dctx {
    _private: [u8; 0],
}
#[link(name = "lz4")]
unsafe extern "C" {
    fn LZ4F_isError(code: usize) -> u32;
    fn LZ4F_getErrorName(code: usize) -> *const c_char;
    fn LZ4F_compressFrameBound(src_size: usize, prefs: *const c_void) -> usize;
    fn LZ4F_compressFrame(
        dst: *mut c_void,
        dst_cap: usize,
        src: *const c_void,
        src_size: usize,
        prefs: *const c_void,
    ) -> usize;
    fn LZ4F_createDecompressionContext(ctx: *mut *mut Lz4Dctx, version: u32) -> usize;
    fn LZ4F_freeDecompressionContext(ctx: *mut Lz4Dctx) -> usize;
    fn LZ4F_decompress(
        ctx: *mut Lz4Dctx,
        dst: *mut c_void,
        dst_size: *mut usize,
        src: *const c_void,
        src_size: *mut usize,
        opts: *const c_void,
    ) -> usize;
}
fn lz4_error(code: usize) -> StandardError {
    let name = unsafe { CStr::from_ptr(LZ4F_getErrorName(code)) }
        .to_string_lossy()
        .into_owned();
    StandardError::Codec(format!("LZ4 frame: {name}"))
}
fn lz4_encode(input: &[u8], options: &StandardOptions) -> Result<Vec<u8>, StandardError> {
    let bound = unsafe { LZ4F_compressFrameBound(input.len(), ptr::null()) };
    if unsafe { LZ4F_isError(bound) } != 0 {
        return Err(lz4_error(bound));
    }
    if bound > options.output_limit {
        return Err(StandardError::OutputLimit);
    }
    admit(input.len(), options, LZ4_STATE_BYTES)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(bound)
        .map_err(|_| StandardError::Codec("LZ4 frame allocation failed".into()))?;
    output.resize(bound, 0);
    let used = unsafe {
        LZ4F_compressFrame(
            output.as_mut_ptr().cast(),
            output.len(),
            input.as_ptr().cast(),
            input.len(),
            ptr::null(),
        )
    };
    if unsafe { LZ4F_isError(used) } != 0 {
        return Err(lz4_error(used));
    }
    output.truncate(used);
    Ok(output)
}
fn lz4_decode(input: &[u8], options: &StandardOptions) -> Result<Vec<u8>, StandardError> {
    admit(input.len(), options, LZ4_STATE_BYTES)?;
    let mut output = output_buffer(options.output_limit)?;
    let mut context = ptr::null_mut();
    let create = unsafe { LZ4F_createDecompressionContext(&mut context, 100) };
    if unsafe { LZ4F_isError(create) } != 0 {
        return Err(lz4_error(create));
    }
    let result = (|| {
        let (mut source, mut written) = (0usize, 0usize);
        loop {
            let mut source_size = input.len() - source;
            let mut destination_size = output.len() - written;
            let hint = unsafe {
                LZ4F_decompress(
                    context,
                    output[written..].as_mut_ptr().cast(),
                    &mut destination_size,
                    input[source..].as_ptr().cast(),
                    &mut source_size,
                    ptr::null(),
                )
            };
            if unsafe { LZ4F_isError(hint) } != 0 {
                return Err(lz4_error(hint));
            }
            source += source_size;
            written += destination_size;
            if written > options.output_limit {
                return Err(StandardError::OutputLimit);
            }
            if hint == 0 {
                if source != input.len() {
                    return Err(StandardError::Codec("LZ4 frame trailing data".into()));
                }
                output.truncate(written);
                return Ok(output);
            }
            if source_size == 0 && destination_size == 0 {
                return Err(StandardError::Codec("truncated LZ4 frame".into()));
            }
        }
    })();
    unsafe {
        LZ4F_freeDecompressionContext(context);
    }
    result
}

#[link(name = "snappy")]
unsafe extern "C" {
    fn snappy_compress(
        input: *const c_char,
        input_len: usize,
        output: *mut c_char,
        output_len: *mut usize,
    ) -> i32;
    fn snappy_uncompress(
        input: *const c_char,
        input_len: usize,
        output: *mut c_char,
        output_len: *mut usize,
    ) -> i32;
    fn snappy_max_compressed_length(input_len: usize) -> usize;
    fn snappy_uncompressed_length(
        input: *const c_char,
        input_len: usize,
        output_len: *mut usize,
    ) -> i32;
}
fn crc32c(input: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in input {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f6_3b78 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
fn masked_crc32c(input: &[u8]) -> u32 {
    crc32c(input).rotate_right(15).wrapping_add(0xa282_ead8)
}
fn append_snappy_chunk(
    output: &mut Vec<u8>,
    kind: u8,
    data: &[u8],
    limit: usize,
) -> Result<(), StandardError> {
    if data.len() > 0x00ff_ffff {
        return Err(StandardError::Codec("Snappy frame chunk too large".into()));
    }
    let total = output
        .len()
        .checked_add(4)
        .and_then(|n| n.checked_add(data.len()))
        .ok_or(StandardError::OutputLimit)?;
    if total > limit {
        return Err(StandardError::OutputLimit);
    }
    output.push(kind);
    output.extend_from_slice(&(data.len() as u32).to_le_bytes()[..3]);
    output.extend_from_slice(data);
    Ok(())
}
fn snappy_encode(input: &[u8], options: &StandardOptions) -> Result<Vec<u8>, StandardError> {
    let scratch = unsafe { snappy_max_compressed_length(SNAPPY_CHUNK) };
    admit(
        input.len(),
        options,
        scratch
            .checked_add(SNAPPY_CHUNK + 4)
            .ok_or(StandardError::OutputLimit)?,
    )?;
    if options.output_limit < SNAPPY_IDENTIFIER.len() {
        return Err(StandardError::OutputLimit);
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(options.output_limit)
        .map_err(|_| StandardError::Codec("Snappy frame allocation failed".into()))?;
    output.extend_from_slice(SNAPPY_IDENTIFIER);
    if output.len() > options.output_limit {
        return Err(StandardError::OutputLimit);
    }
    for raw in input.chunks(SNAPPY_CHUNK) {
        let capacity = unsafe { snappy_max_compressed_length(raw.len()) };
        let mut compressed = Vec::new();
        compressed
            .try_reserve_exact(capacity)
            .map_err(|_| StandardError::Codec("Snappy scratch allocation failed".into()))?;
        compressed.resize(capacity, 0);
        let mut used = compressed.len();
        if unsafe {
            snappy_compress(
                raw.as_ptr().cast(),
                raw.len(),
                compressed.as_mut_ptr().cast(),
                &mut used,
            )
        } != 0
        {
            return Err(StandardError::Codec("Snappy compression failed".into()));
        }
        compressed.truncate(used);
        let payload_len = 4usize
            .checked_add(raw.len().min(compressed.len()))
            .ok_or(StandardError::OutputLimit)?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(payload_len)
            .map_err(|_| StandardError::Codec("Snappy payload allocation failed".into()))?;
        payload.extend_from_slice(&masked_crc32c(raw).to_le_bytes());
        if compressed.len() < raw.len() {
            payload.extend_from_slice(&compressed);
            append_snappy_chunk(&mut output, 0x00, &payload, options.output_limit)?;
        } else {
            payload.extend_from_slice(raw);
            append_snappy_chunk(&mut output, 0x01, &payload, options.output_limit)?;
        }
    }
    Ok(output)
}
fn snappy_decode(input: &[u8], options: &StandardOptions) -> Result<Vec<u8>, StandardError> {
    admit(input.len(), options, SNAPPY_CHUNK)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(options.output_limit)
        .map_err(|_| StandardError::Codec("Snappy output allocation failed".into()))?;
    let mut offset = 0usize;
    let mut identifier = false;
    while offset < input.len() {
        if input.len() - offset < 4 {
            return Err(StandardError::Codec("truncated Snappy frame header".into()));
        }
        let kind = input[offset];
        let len = (input[offset + 1] as usize)
            | ((input[offset + 2] as usize) << 8)
            | ((input[offset + 3] as usize) << 16);
        offset += 4;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| StandardError::Codec("Snappy frame length overflow".into()))?;
        if end > input.len() {
            return Err(StandardError::Codec("truncated Snappy frame chunk".into()));
        }
        let chunk = &input[offset..end];
        offset = end;
        match kind {
            0xff => {
                if chunk != b"sNaPpY" {
                    return Err(StandardError::Codec(
                        "invalid Snappy stream identifier".into(),
                    ));
                }
                identifier = true;
            }
            0x00 | 0x01 => {
                if !identifier {
                    return Err(StandardError::Codec(
                        "Snappy frame lacks stream identifier".into(),
                    ));
                }
                if chunk.len() < 4 {
                    return Err(StandardError::Codec("truncated Snappy data chunk".into()));
                }
                let wanted = u32::from_le_bytes(chunk[..4].try_into().unwrap());
                let data = &chunk[4..];
                let before = output.len();
                if kind == 1 {
                    if data.len() > SNAPPY_CHUNK
                        || data.len() > options.output_limit.saturating_sub(before)
                    {
                        return Err(StandardError::OutputLimit);
                    }
                    output.extend_from_slice(data);
                } else {
                    let mut raw_len = 0usize;
                    if unsafe {
                        snappy_uncompressed_length(data.as_ptr().cast(), data.len(), &mut raw_len)
                    } != 0
                        || raw_len > SNAPPY_CHUNK
                    {
                        return Err(StandardError::Codec(
                            "invalid Snappy compressed chunk".into(),
                        ));
                    }
                    if raw_len > options.output_limit.saturating_sub(before) {
                        return Err(StandardError::OutputLimit);
                    }
                    output.resize(before + raw_len, 0);
                    let mut written = raw_len;
                    if unsafe {
                        snappy_uncompress(
                            data.as_ptr().cast(),
                            data.len(),
                            output[before..].as_mut_ptr().cast(),
                            &mut written,
                        )
                    } != 0
                        || written != raw_len
                    {
                        return Err(StandardError::Codec(
                            "invalid Snappy compressed chunk".into(),
                        ));
                    }
                }
                if masked_crc32c(&output[before..]) != wanted {
                    return Err(StandardError::Codec(
                        "Snappy frame checksum mismatch".into(),
                    ));
                }
            }
            0x02..=0x7f => {
                return Err(StandardError::Codec(
                    "unsupported unskippable Snappy chunk".into(),
                ))
            }
            _ => {}
        }
    }
    if !identifier {
        return Err(StandardError::Codec(
            "Snappy frame lacks stream identifier".into(),
        ));
    }
    Ok(output)
}
