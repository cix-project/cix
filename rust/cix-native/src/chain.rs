//! Decoder for the frozen CIXM6 transform-chain representation (mode 7).
//!
//! The chain header selects one reversible byte/word transform followed by one
//! of the legacy CIX inner codecs.  Decoding is deliberately self-contained so
//! every length-delimited inner stream is checked before transform inversion.

use super::{
    adaptive, bitplane, bwt_legacy, combinatorics, fixedmix, histogram, legacy_lz, ppm, rank,
    transform, wavelet,
};
use num_bigint::BigUint;
use num_traits::{One, Zero};
#[cfg(test)]
use std::cell::Cell;
use std::collections::HashMap;

const CHAIN_VERSION: u8 = 1;
const INNER_MULTINOMIAL: u8 = 1;
const INNER_CONTEXT: u8 = 2;
const INNER_LZ: u8 = 3;
const INNER_BWT: u8 = 4;
const INNER_BITPLANE: u8 = 5;
const INNER_SPARSE: u8 = 6;
const INNER_RUNS: u8 = 7;
const CHAIN_SPEC_IDS: [u8; 9] = [12, 13, 16, 20, 21, 29, 30, 31, 33];
const MAX_INNER_CANDIDATES: usize = 9;

#[cfg(test)]
thread_local! {
    static CACHE_HITS: Cell<usize> = const { Cell::new(0) };
}

/// Bounded transform-chain selector controls.
#[derive(Clone, Copy, Debug)]
pub struct EncodeOptions {
    /// Number of the nine frozen transforms admitted after cheap profiling.
    pub beam: usize,
    /// Effort 1..9. Higher values add inner codecs but never unbound the beam.
    pub effort: u8,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self { beam: 4, effort: 6 }
    }
}

/// Complete chain-payload byte count for one materialized candidate.
#[cfg(test)]
#[derive(Clone, Debug)]
pub struct CandidateSize {
    pub transform_id: u8,
    pub inner_mode: u8,
    pub bytes: usize,
    pub description: String,
}

/// Selected chain payload plus exact sizes for every materialized candidate.
#[derive(Clone, Debug)]
pub struct EncodeResult {
    pub payload: Vec<u8>,
    #[cfg(test)]
    pub description: String,
    #[cfg(test)]
    pub candidates: Vec<CandidateSize>,
}

#[cfg(test)]
type Measurements = Vec<CandidateSize>;

#[cfg(not(test))]
#[derive(Default)]
struct Measurements;

#[derive(Clone)]
struct MaterializedInner {
    mode: u8,
    payload: Vec<u8>,
    description: String,
}

/// A per-encode exact transformed-byte entry. The bounded outer vector is
/// reserved to the frozen transform count; this entry accounts for every heap
/// buffer it retains before admission.
struct CachedTransform {
    key: Vec<u8>,
    inners: Vec<MaterializedInner>,
}

/// Retains a streaming candidate sequence only while every retained buffer
/// fits the allowance. A failed capture is dropped before the next candidate.
struct CacheCapture {
    inners: Vec<MaterializedInner>,
    retained_bytes: usize,
    allowance: usize,
}

struct CandidateSelectionState<'a> {
    cache_allowance: usize,
    cache: &'a mut Vec<CachedTransform>,
    cached_bytes: &'a mut usize,
    best: &'a mut Option<(Vec<u8>, String)>,
    measurements: &'a mut Measurements,
}

fn cache_table_bytes(slots: usize) -> usize {
    std::mem::size_of::<Vec<CachedTransform>>()
        .saturating_add(slots.saturating_mul(std::mem::size_of::<CachedTransform>()))
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

#[derive(Clone, Copy)]
enum Operation {
    Xor,
    Delta,
    Delta2,
    Delta2Zigzag,
    MatchXor,
}

#[derive(Clone, Copy)]
struct Transform {
    width: usize,
    endian: Endian,
    operation: Operation,
    shuffle: bool,
    byte_lag: Option<usize>,
}

fn transform(id: u8) -> Option<Transform> {
    use Endian::{Big, Little};
    use Operation::{Delta, Delta2, Delta2Zigzag, MatchXor, Xor};
    let (width, endian, operation, shuffle, byte_lag) = match id {
        1 => (1, Little, Xor, false, Some(1)),
        2 => (2, Little, Xor, false, Some(2)),
        3 => (4, Little, Xor, false, Some(4)),
        4 => (1, Little, Delta, false, Some(1)),
        5 => (2, Little, Delta, true, None),
        6 => (4, Little, Delta, true, None),
        7 => (8, Little, Delta, true, None),
        8 => (2, Big, Delta, true, None),
        9 => (4, Big, Delta, true, None),
        10 => (8, Big, Delta, true, None),
        11 => (2, Little, Delta2, true, None),
        12 => (4, Little, Delta2, true, None),
        13 => (8, Little, Delta2, true, None),
        14 => (2, Big, Delta2, true, None),
        15 => (4, Big, Delta2, true, None),
        16 => (8, Big, Delta2, true, None),
        17 => (4, Little, Xor, true, None),
        18 => (8, Little, Xor, true, None),
        19 => (8, Little, Xor, false, Some(8)),
        20 => (12, Little, Xor, false, Some(12)),
        21 => (16, Little, Xor, false, Some(16)),
        22 => (24, Little, Xor, false, Some(24)),
        23 => (32, Little, Xor, false, Some(32)),
        24 => (64, Little, Xor, false, Some(64)),
        25 => (2, Little, Delta, false, Some(2)),
        26 => (4, Little, Delta, false, Some(4)),
        27 => (8, Little, Delta, false, Some(8)),
        28 => (16, Little, Delta, false, Some(16)),
        29 => (4, Little, Delta2Zigzag, true, None),
        30 => (8, Little, Delta2Zigzag, true, None),
        31 => (8, Big, Delta2Zigzag, true, None),
        33 => (1, Little, MatchXor, false, None),
        _ => return None,
    };
    Some(Transform {
        width,
        endian,
        operation,
        shuffle,
        byte_lag,
    })
}

fn transform_name(id: u8) -> &'static str {
    match id {
        12 => "word32le-delta2-shuffle",
        13 => "word64le-delta2-shuffle",
        16 => "word64be-delta2-shuffle",
        20 => "byte-xor-12",
        21 => "byte-xor-16",
        29 => "word32le-delta2zz-shuffle",
        30 => "word64le-delta2zz-shuffle",
        31 => "word64be-delta2zz-shuffle",
        33 => "matchxor4",
        _ => "unknown-transform",
    }
}

fn put_var(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn unvar(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..usize::BITS as usize).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated transform-chain varint")?;
        *pos += 1;
        let low = (byte & 0x7f) as usize;
        if shift + 7 > usize::BITS as usize && low > (usize::MAX >> shift) {
            return Err("transform-chain varint overflow".into());
        }
        value |= low
            .checked_shl(shift as u32)
            .ok_or("transform-chain varint overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized transform-chain varint".into())
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn bit(&mut self) -> Result<usize, String> {
        if self.pos >= self.data.len().saturating_mul(8) {
            return Err("truncated transform-chain bitstream".into());
        }
        let value = ((self.data[self.pos >> 3] >> (7 - (self.pos & 7))) & 1) as usize;
        self.pos += 1;
        Ok(value)
    }

    fn bits(&mut self, width: usize) -> Result<usize, String> {
        if width >= usize::BITS as usize {
            return Err("transform-chain integer is too wide".into());
        }
        let mut value = 0usize;
        for _ in 0..width {
            value = (value << 1) | self.bit()?;
        }
        Ok(value)
    }
}

fn decode_bytes(data: &[u8], count: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    let mode = *data.first().ok_or("empty transform-chain byte substream")?;
    let mut pos = 1;
    match mode {
        0 => {
            let out = rank::decode_type_class(data, &mut pos, count)?;
            if pos != data.len() {
                return Err("trailing static byte-substream bytes".into());
            }
            Ok(out)
        }
        1 => {
            let chunk = unvar(data, &mut pos)?;
            if chunk == 0 {
                return Err("invalid byte-substream chunk size".into());
            }
            let mut out = Vec::with_capacity(count);
            while out.len() < count {
                let n = chunk.min(count - out.len());
                out.extend(rank::decode_type_class(data, &mut pos, n)?);
            }
            if pos != data.len() {
                return Err("trailing chunked byte-substream bytes".into());
            }
            Ok(out)
        }
        2 => adaptive::decode_general(&data[1..], count, &[], model_limit),
        3 => {
            if !matches!(data.get(1), Some(1 | 2)) {
                return Err("invalid context byte-substream order".into());
            }
            adaptive::decode_general(&data[2..], count, &[], model_limit)
        }
        4 => wavelet::decode(&data[1..], count),
        5 => histogram::decode_type_class(&data[1..], count),
        _ => Err(format!(
            "unsupported transform-chain byte substream mode {mode}"
        )),
    }
}

fn decode_uints(data: &[u8], count: usize, model_limit: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mode = *data.first().ok_or("empty transform-chain integer stream")?;
    let mut pos = 1;
    match mode {
        0 => decode_uint_varints(data, &mut pos, count),
        1 => decode_uint_rice(data, &mut pos, count),
        2 | 3 => decode_uint_classes(data, &mut pos, count, model_limit, mode),
        _ => Err(format!("unsupported transform-chain integer mode {mode}")),
    }
}

fn decode_uint_varints(data: &[u8], pos: &mut usize, count: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(unvar(data, pos)?);
    }
    if *pos != data.len() {
        return Err("trailing transform-chain varints".into());
    }
    Ok(out)
}

fn decode_uint_rice(data: &[u8], pos: &mut usize, count: usize) -> Result<Vec<usize>, String> {
    let k = *data.get(*pos).ok_or("missing Rice parameter")? as usize;
    *pos += 1;
    let mut reader = BitReader {
        data: &data[*pos..],
        pos: 0,
    };
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut quotient = 0usize;
        while reader.bit()? != 0 {
            quotient = quotient.checked_add(1).ok_or("Rice quotient overflow")?;
        }
        let low = reader.bits(k)?;
        if quotient > (usize::MAX >> k) {
            return Err("Rice integer overflow".into());
        }
        out.push(
            quotient
                .checked_shl(k as u32)
                .ok_or("Rice integer overflow")?
                | low,
        );
    }
    Ok(out)
}

fn decode_uint_classes(
    data: &[u8],
    pos: &mut usize,
    count: usize,
    model_limit: usize,
    mode: u8,
) -> Result<Vec<usize>, String> {
    let class_len = unvar(data, pos)?;
    let end = pos
        .checked_add(class_len)
        .ok_or("integer class length overflow")?;
    if end > data.len() {
        return Err("truncated integer class stream".into());
    }
    let classes = if mode == 2 {
        let mut class_pos = 0;
        let out = rank::decode_type_class(&data[*pos..end], &mut class_pos, count)?;
        if class_pos != class_len {
            return Err("trailing integer class bytes".into());
        }
        out
    } else {
        decode_bytes(&data[*pos..end], count, model_limit)?
    };
    let mut reader = BitReader {
        data: &data[end..],
        pos: 0,
    };
    classes
        .into_iter()
        .map(|width| {
            let width = width as usize;
            let low = reader.bits(width)?;
            (1usize << width)
                .checked_add(low)
                .and_then(|value| value.checked_sub(1))
                .ok_or_else(|| "integer stream overflow".into())
        })
        .collect()
}

fn decode_runs(data: &[u8], source_len: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    if data.first() != Some(&1) {
        return Err("unknown chained run-block version".into());
    }
    let mut pos = 1;
    let run_count = unvar(data, &mut pos)?;
    if run_count == 0 || run_count > source_len {
        return Err("invalid chained run count".into());
    }
    let symbol_len = unvar(data, &mut pos)?;
    let end = pos
        .checked_add(symbol_len)
        .ok_or("run symbol length overflow")?;
    if end > data.len() {
        return Err("truncated chained run symbols".into());
    }
    let mut symbol_pos = 0;
    let symbols = rank::decode_type_class(&data[pos..end], &mut symbol_pos, run_count)?;
    if symbol_pos != symbol_len {
        return Err("unused chained run-symbol bytes".into());
    }
    let lengths = decode_uints(&data[end..], run_count, model_limit)?;
    let mut out = Vec::with_capacity(source_len);
    for (symbol, minus_one) in symbols.into_iter().zip(lengths) {
        let length = minus_one.checked_add(1).ok_or("run length overflow")?;
        let new_len = out.len().checked_add(length).ok_or("run length overflow")?;
        if new_len > source_len {
            return Err("chained runs exceed source length".into());
        }
        out.resize(new_len, symbol);
    }
    if out.len() != source_len {
        return Err("chained run source length mismatch".into());
    }
    Ok(out)
}

fn choose(n: usize, k: usize) -> Result<BigUint, String> {
    combinatorics::choose_checked(n, k)
}

fn fixed_bytes(value: &BigUint, width: usize) -> Vec<u8> {
    let size = width.div_ceil(8);
    if size == 0 {
        return Vec::new();
    }
    let bytes = value.to_bytes_be();
    let mut out = vec![0u8; size.saturating_sub(bytes.len())];
    out.extend_from_slice(&bytes);
    out
}

fn colex_rank(values: &[usize]) -> Result<BigUint, String> {
    combinatorics::colex_rank_checked(values)
}

fn fixed_width(value: &BigUint) -> usize {
    value.bits() as usize
}

fn read_fixed(data: &[u8], pos: &mut usize, width: usize) -> Result<BigUint, String> {
    let bytes = width.div_ceil(8);
    let end = pos.checked_add(bytes).ok_or("rank offset overflow")?;
    if end > data.len() {
        return Err("truncated transform-chain rank".into());
    }
    if !width.is_multiple_of(8) && bytes > 0 && data[*pos] >> (width % 8) != 0 {
        return Err("nonzero transform-chain rank padding".into());
    }
    let value = BigUint::from_bytes_be(&data[*pos..end]);
    *pos = end;
    Ok(value)
}

fn colex_unrank(
    mut rank_value: BigUint,
    count: usize,
    universe: usize,
) -> Result<Vec<usize>, String> {
    if count > universe || rank_value >= choose(universe, count)? {
        return Err("colex rank outside transform-chain universe".into());
    }
    let mut result = vec![0usize; count];
    let mut upper = universe;
    for i in (1..=count).rev() {
        let mut low = i - 1;
        let mut high = upper;
        while low + 1 < high {
            let mid = low + (high - low) / 2;
            if choose(mid, i)? <= rank_value {
                low = mid;
            } else {
                high = mid;
            }
        }
        result[i - 1] = low;
        rank_value -= choose(low, i)?;
        upper = low;
    }
    if !rank_value.is_zero() {
        return Err("invalid transform-chain colex rank".into());
    }
    Ok(result)
}

fn composition_unrank(
    rank_value: BigUint,
    total: usize,
    parts: usize,
) -> Result<Vec<usize>, String> {
    if parts == 0 || total < parts || rank_value >= choose(total - 1, parts - 1)? {
        return Err("composition rank outside transform-chain class".into());
    }
    if parts == 1 {
        return Ok(vec![total]);
    }
    let bars = colex_unrank(rank_value, parts - 1, total - 1)?;
    let mut result = Vec::with_capacity(parts);
    let mut previous = 0usize;
    for bar in bars {
        let boundary = bar + 1;
        result.push(boundary - previous);
        previous = boundary;
    }
    result.push(total - previous);
    Ok(result)
}

fn decode_bucketed(
    data: &[u8],
    pos: &mut usize,
    bucket_count: usize,
    total: usize,
) -> Result<HashMap<usize, Vec<u8>>, String> {
    let q = unvar(data, pos)?;
    if q > bucket_count.min(total) {
        return Err("too many active literal buckets".into());
    }
    if q == 0 {
        if total != 0 {
            return Err("non-empty literal stream has no buckets".into());
        }
        return Ok(HashMap::new());
    }
    let ids = if q == bucket_count {
        (0..bucket_count).collect::<Vec<_>>()
    } else {
        let width = fixed_width(&(choose(bucket_count, q)? - BigUint::one()));
        colex_unrank(read_fixed(data, pos, width)?, q, bucket_count)?
    };
    let lengths = if q == 1 {
        vec![total]
    } else {
        let width = fixed_width(&(choose(total - 1, q - 1)? - BigUint::one()));
        composition_unrank(read_fixed(data, pos, width)?, total, q)?
    };
    let mut streams = HashMap::with_capacity(q);
    for (id, count) in ids.into_iter().zip(lengths) {
        streams.insert(id, rank::decode_type_class(data, pos, count)?);
    }
    Ok(streams)
}

fn context_bucket(tail: &[u8], order: usize, bucket_bits: usize) -> usize {
    let start = tail.len().saturating_sub(order);
    if order == 1 && bucket_bits >= 8 {
        return tail.last().copied().unwrap_or(0) as usize;
    }
    let mut hash = 1469598103934665603u64;
    for &byte in &tail[start..] {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    (hash & ((1u64 << bucket_bits) - 1)) as usize
}

enum Literals {
    Plain {
        bytes: Vec<u8>,
        pos: usize,
    },
    Context {
        streams: HashMap<usize, Vec<u8>>,
        cursors: HashMap<usize, usize>,
        order: usize,
        bucket_bits: usize,
    },
}

impl Literals {
    fn next(&mut self, produced: &[u8]) -> Result<u8, String> {
        match self {
            Self::Plain { bytes, pos } => {
                let value = *bytes
                    .get(*pos)
                    .ok_or("chained LZ literal stream exhausted")?;
                *pos += 1;
                Ok(value)
            }
            Self::Context {
                streams,
                cursors,
                order,
                bucket_bits,
            } => {
                let bucket = context_bucket(produced, *order, *bucket_bits);
                let stream = streams
                    .get(&bucket)
                    .ok_or("chained LZ selected absent literal bucket")?;
                let cursor = cursors.entry(bucket).or_default();
                let value = *stream
                    .get(*cursor)
                    .ok_or("chained LZ literal bucket exhausted")?;
                *cursor += 1;
                Ok(value)
            }
        }
    }

    fn fully_consumed(&self) -> bool {
        match self {
            Self::Plain { bytes, pos } => *pos == bytes.len(),
            Self::Context {
                streams, cursors, ..
            } => streams
                .iter()
                .all(|(bucket, stream)| cursors.get(bucket).copied().unwrap_or(0) == stream.len()),
        }
    }
}

fn decode_literals(data: &[u8], count: usize, model_limit: usize) -> Result<Literals, String> {
    let mode = *data.first().ok_or("empty chained LZ literal stream")?;
    match mode {
        0 => {
            let mut pos = 1;
            let bytes = rank::decode_type_class(data, &mut pos, count)?;
            if pos != data.len() {
                return Err("unused chained LZ literal bytes".into());
            }
            Ok(Literals::Plain { bytes, pos: 0 })
        }
        1 => {
            let order = *data.get(1).ok_or("missing literal context order")? as usize;
            let bucket_bits = *data.get(2).ok_or("missing literal context bucket bits")? as usize;
            if !(1..=16).contains(&order) || !(1..=16).contains(&bucket_bits) {
                return Err("invalid literal context parameters".into());
            }
            let mut pos = 3;
            let streams = decode_bucketed(data, &mut pos, 1usize << bucket_bits, count)?;
            if pos != data.len() {
                return Err("unused literal-context bytes".into());
            }
            let cursors = streams.keys().map(|&bucket| (bucket, 0)).collect();
            Ok(Literals::Context {
                streams,
                cursors,
                order,
                bucket_bits,
            })
        }
        2 => Ok(Literals::Plain {
            bytes: decode_bytes(&data[1..], count, model_limit)?,
            pos: 0,
        }),
        3 => Ok(Literals::Plain {
            bytes: ppm::decode(&data[1..], count)?,
            pos: 0,
        }),
        4 => Ok(Literals::Plain {
            bytes: fixedmix::decode(&data[1..], count)?,
            pos: 0,
        }),
        _ => Err(format!("unsupported chained LZ literal mode {mode}")),
    }
}

fn decode_tags(data: &[u8], count: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    let mode = *data.first().ok_or("empty chained LZ tag stream")?;
    match mode {
        0 => {
            let mut pos = 1;
            let tags = rank::decode_type_class(data, &mut pos, count)?;
            if pos != data.len() {
                return Err("unused chained LZ tag bytes".into());
            }
            Ok(tags)
        }
        1 => {
            let mut tag = *data.get(1).ok_or("missing chained LZ initial tag")?;
            if tag > 1 {
                return Err("invalid chained LZ initial tag".into());
            }
            let mut pos = 2;
            let run_count = unvar(data, &mut pos)?;
            let lengths = decode_uints(&data[pos..], run_count, model_limit)?;
            let mut tags = Vec::with_capacity(count);
            for minus_one in lengths {
                let length = minus_one.checked_add(1).ok_or("tag run overflow")?;
                if tags.len().saturating_add(length) > count {
                    return Err("chained LZ tag runs exceed token count".into());
                }
                tags.resize(tags.len() + length, tag);
                tag ^= 1;
            }
            if tags.len() != count {
                return Err("chained LZ tag-run length mismatch".into());
            }
            Ok(tags)
        }
        2 => decode_bytes(&data[1..], count, model_limit),
        _ => Err(format!("unsupported chained LZ tag mode {mode}")),
    }
}

fn decode_distances_v1(data: &[u8], count: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut pos = 0;
    let symbol_len = unvar(data, &mut pos)?;
    let end = pos
        .checked_add(symbol_len)
        .ok_or("distance symbol length overflow")?;
    if end > data.len() {
        return Err("truncated chained LZ distance classes".into());
    }
    let mut symbol_pos = 0;
    let symbols = rank::decode_type_class(&data[pos..end], &mut symbol_pos, count)?;
    if symbol_pos != symbol_len {
        return Err("unused chained LZ distance classes".into());
    }
    decode_distance_symbols(&symbols, &data[end..], 4)
}

fn decode_distances_v3(
    data: &[u8],
    count: usize,
    model_limit: usize,
) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let cache_size = *data
        .first()
        .ok_or("empty chained LZ adaptive distance stream")? as usize;
    let mut pos = 1;
    let symbol_len = unvar(data, &mut pos)?;
    let end = pos
        .checked_add(symbol_len)
        .ok_or("adaptive distance length overflow")?;
    if end > data.len() {
        return Err("truncated chained LZ adaptive distance classes".into());
    }
    let symbols = decode_bytes(&data[pos..end], count, model_limit)?;
    decode_distance_symbols(&symbols, &data[end..], cache_size)
}

fn decode_distance_symbols(
    symbols: &[u8],
    low_bits: &[u8],
    cache_size: usize,
) -> Result<Vec<usize>, String> {
    let mut reader = BitReader {
        data: low_bits,
        pos: 0,
    };
    let mut cache = Vec::<usize>::new();
    let mut out = Vec::with_capacity(symbols.len());
    for &symbol in symbols {
        let symbol = symbol as usize;
        let distance = if symbol < cache_size {
            if symbol >= cache.len() {
                return Err("invalid chained LZ distance cache reference".into());
            }
            let distance = cache.remove(symbol);
            cache.insert(0, distance);
            distance
        } else {
            let width = symbol - cache_size;
            if width >= usize::BITS as usize {
                return Err("chained LZ distance class too wide".into());
            }
            let low = reader.bits(width)?;
            let distance = (1usize << width)
                .checked_add(low)
                .ok_or("chained LZ distance overflow")?;
            if cache_size > 0 {
                cache.insert(0, distance);
                cache.truncate(cache_size);
            }
            distance
        };
        out.push(distance);
    }
    Ok(out)
}

fn decode_lz(data: &[u8], source_len: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    let (version, min_match, token_count, literal_count, match_count, streams) =
        parse_lz_streams(data)?;
    let tags = decode_lz_tags(version, streams[0], token_count, model_limit)?;
    let mut literals = decode_literals(streams[1], literal_count, model_limit)?;
    let lengths = decode_uints(streams[2], match_count, model_limit)?;
    let distances = decode_lz_distances(version, streams[3], match_count, model_limit)?;
    let mut output = Vec::with_capacity(source_len);
    let streams = LzMatchStreams {
        lengths: &lengths,
        distances: &distances,
    };
    let mut used = LzUsed {
        literals: 0,
        matches: 0,
    };
    let limits = LzLimits {
        min_match,
        source_len,
    };
    for tag in tags {
        decode_lz_tag(tag, &mut output, &mut literals, streams, &mut used, limits)?;
    }
    if output.len() != source_len
        || used.literals != literal_count
        || used.matches != match_count
        || !literals.fully_consumed()
    {
        return Err("chained LZ decoded counts or length mismatch".into());
    }
    Ok(output)
}

type LzHeader<'a> = (u8, usize, usize, usize, usize, Vec<&'a [u8]>);
fn parse_lz_streams(data: &[u8]) -> Result<LzHeader<'_>, String> {
    if data.len() < 2 || !matches!(data[0], 1 | 3) {
        return Err("unknown chained LZ version".into());
    }
    let (version, min_match) = (data[0], data[1] as usize);
    let mut pos = 2;
    let (tokens, literals, matches) = (
        unvar(data, &mut pos)?,
        unvar(data, &mut pos)?,
        unvar(data, &mut pos)?,
    );
    let mut streams = Vec::with_capacity(4);
    for _ in 0..4 {
        let length = unvar(data, &mut pos)?;
        let end = pos
            .checked_add(length)
            .ok_or("LZ substream length overflow")?;
        if end > data.len() {
            return Err("truncated chained LZ substream".into());
        }
        streams.push(&data[pos..end]);
        pos = end;
    }
    if pos != data.len() {
        return Err("trailing chained LZ bytes".into());
    }
    Ok((version, min_match, tokens, literals, matches, streams))
}
fn decode_lz_tags(version: u8, data: &[u8], count: usize, limit: usize) -> Result<Vec<u8>, String> {
    if version != 1 {
        return decode_tags(data, count, limit);
    }
    let mut pos = 0;
    let tags = rank::decode_type_class(data, &mut pos, count)?;
    if pos != data.len() {
        return Err("unused chained LZ v1 tag bytes".into());
    }
    Ok(tags)
}
fn decode_lz_distances(
    version: u8,
    data: &[u8],
    count: usize,
    limit: usize,
) -> Result<Vec<usize>, String> {
    if version == 1 {
        decode_distances_v1(data, count)
    } else {
        decode_distances_v3(data, count, limit)
    }
}
#[derive(Clone, Copy)]
struct LzMatchStreams<'a> {
    lengths: &'a [usize],
    distances: &'a [usize],
}
struct LzUsed {
    literals: usize,
    matches: usize,
}
#[derive(Clone, Copy)]
struct LzLimits {
    min_match: usize,
    source_len: usize,
}

fn decode_lz_tag(
    tag: u8,
    out: &mut Vec<u8>,
    literals: &mut Literals,
    streams: LzMatchStreams<'_>,
    used: &mut LzUsed,
    limits: LzLimits,
) -> Result<(), String> {
    match tag {
        0 => {
            out.push(literals.next(out)?);
            used.literals += 1;
        }
        1 => {
            let encoded = *streams
                .lengths
                .get(used.matches)
                .ok_or("chained LZ match stream exhausted")?;
            let distance = *streams
                .distances
                .get(used.matches)
                .ok_or("chained LZ distance stream exhausted")?;
            used.matches += 1;
            if distance == 0 || distance > out.len() {
                return Err("chained LZ distance before available history".into());
            }
            let length = encoded
                .checked_add(limits.min_match)
                .ok_or("chained LZ match length overflow")?;
            for _ in 0..length {
                let value = out[out.len() - distance];
                out.push(value);
                if out.len() > limits.source_len {
                    return Err("chained LZ expands beyond source length".into());
                }
            }
        }
        _ => return Err("invalid chained LZ tag".into()),
    }
    Ok(())
}

fn encode_uints_raw(values: &[usize]) -> Vec<u8> {
    let mut out = vec![0];
    for &value in values {
        put_var(value, &mut out);
    }
    out
}

fn encode_sparse(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.is_empty() {
        return Err("cannot sparse-encode an empty chain block".into());
    }
    let mut counts = [0usize; 256];
    for &byte in data {
        counts[byte as usize] += 1;
    }
    let dominant = counts
        .iter()
        .enumerate()
        .max_by_key(|&(symbol, count)| (*count, std::cmp::Reverse(symbol)))
        .map(|(symbol, _)| symbol)
        .unwrap();
    let mut positions = Vec::new();
    let mut values = Vec::new();
    for (index, &byte) in data.iter().enumerate() {
        if byte as usize != dominant {
            positions.push(index);
            values.push(byte);
        }
    }
    let width = fixed_width(
        &(combinatorics::choose_checked(data.len(), positions.len())? - BigUint::one()),
    );
    let mut out = vec![1, dominant as u8];
    put_var(positions.len(), &mut out);
    out.extend(fixed_bytes(&colex_rank(&positions)?, width));
    out.extend(rank::encode_type_class(&values)?);
    Ok(out)
}

fn encode_runs(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.is_empty() {
        return Err("cannot run-encode an empty chain block".into());
    }
    let mut symbols = Vec::new();
    let mut lengths = Vec::new();
    let mut current = data[0];
    let mut length = 1usize;
    for &byte in &data[1..] {
        if byte == current {
            length += 1;
        } else {
            symbols.push(current);
            lengths.push(length - 1);
            current = byte;
            length = 1;
        }
    }
    symbols.push(current);
    lengths.push(length - 1);
    let symbol_blob = rank::encode_type_class(&symbols)?;
    let mut out = vec![1];
    put_var(symbols.len(), &mut out);
    put_var(symbol_blob.len(), &mut out);
    out.extend(symbol_blob);
    out.extend(encode_uints_raw(&lengths));
    Ok(out)
}

fn typeclass_score(data: &[u8], log_factorials: &[f64]) -> f64 {
    if data.is_empty() {
        return 1.0;
    }
    let mut counts = [0usize; 256];
    for &byte in data {
        counts[byte as usize] += 1;
    }
    let active = counts.iter().filter(|&&count| count != 0).count();
    let log_choose =
        |n: usize, k: usize| log_factorials[n] - log_factorials[k] - log_factorials[n - k];
    let mut bits = 8.0;
    if active < 256 {
        bits += log_choose(256, active);
    }
    if active > 1 {
        bits += log_choose(data.len() - 1, active - 1);
    }
    bits += log_factorials[data.len()]
        - counts
            .iter()
            .filter(|&&count| count != 0)
            .map(|&count| log_factorials[count])
            .sum::<f64>();
    bits / 8.0
}

fn record_candidate(
    best: &mut Option<(Vec<u8>, String)>,
    measurements: &mut Measurements,
    transform_id: u8,
    inner_mode: u8,
    inner: Vec<u8>,
    inner_name: &str,
) {
    let mut payload = vec![CHAIN_VERSION, transform_id, inner_mode];
    payload.extend(inner);
    let description = format!("{}->{inner_name}", transform_name(transform_id));
    #[cfg(test)]
    measurements.push(CandidateSize {
        transform_id,
        inner_mode,
        bytes: payload.len(),
        description: description.clone(),
    });
    #[cfg(not(test))]
    let _ = measurements;
    if best
        .as_ref()
        .is_none_or(|(current, _)| payload.len() < current.len())
    {
        *best = Some((payload, description));
    }
}

fn context_models(effort: u8) -> &'static [(usize, usize)] {
    match effort {
        0..=4 => &[],
        5..=6 => &[(1, 8)],
        7..=8 => &[(1, 8), (2, 8)],
        _ => &[(1, 8), (2, 8), (2, 10)],
    }
}

fn consider_transform(
    transformed: &[u8],
    transform_id: u8,
    options: EncodeOptions,
    best: &mut Option<(Vec<u8>, String)>,
    measurements: &mut Measurements,
    capture: &mut Option<CacheCapture>,
) -> Result<(), String> {
    record_captured(
        best,
        measurements,
        transform_id,
        INNER_MULTINOMIAL,
        rank::encode_type_class(transformed)?,
        "multinomial",
        capture,
    );
    for &(order, bucket_bits) in context_models(options.effort) {
        record_captured(
            best,
            measurements,
            transform_id,
            INNER_CONTEXT,
            rank::encode_context_type_classes(transformed, order, bucket_bits, &[])?,
            &format!("context-o{order}-b{bucket_bits}"),
            capture,
        );
    }
    if options.effort >= 4 && transformed.len() >= 16 {
        let candidate = legacy_lz::encode(transformed, &[], options.effort)?;
        record_captured(
            best,
            measurements,
            transform_id,
            INNER_LZ,
            candidate.payload,
            &candidate.description,
            capture,
        );
    }
    if options.effort >= 6 && (32..=65_536).contains(&transformed.len()) {
        let (payload, description) = bwt_legacy::encode(transformed)?;
        record_captured(
            best,
            measurements,
            transform_id,
            INNER_BWT,
            payload,
            description,
            capture,
        );
    }
    if options.effort >= 3 && transformed.len() >= 2 {
        record_captured(
            best,
            measurements,
            transform_id,
            INNER_BITPLANE,
            bitplane::encode(transformed),
            "bitplanes",
            capture,
        );
    }
    if options.effort >= 2 && transformed.len() >= 2 {
        consider_simple_transforms(transformed, transform_id, best, measurements, capture)?;
    }
    Ok(())
}

fn consider_simple_transforms(
    transformed: &[u8],
    transform_id: u8,
    best: &mut Option<(Vec<u8>, String)>,
    measurements: &mut Measurements,
    capture: &mut Option<CacheCapture>,
) -> Result<(), String> {
    let mut counts = [0usize; 256];
    for &byte in transformed {
        counts[byte as usize] += 1;
    }
    if counts.iter().copied().max().unwrap_or(0) * 4 >= transformed.len() {
        record_captured(
            best,
            measurements,
            transform_id,
            INNER_SPARSE,
            encode_sparse(transformed)?,
            "sparse",
            capture,
        );
    }
    let run_count = 1 + transformed
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .count();
    if run_count * 3 <= transformed.len() * 2 {
        record_captured(
            best,
            measurements,
            transform_id,
            INNER_RUNS,
            encode_runs(transformed)?,
            "runs",
            capture,
        );
    }
    Ok(())
}

impl CacheCapture {
    fn begin(key_capacity: usize, allowance: usize) -> Option<Self> {
        let minimum_slots =
            MAX_INNER_CANDIDATES.saturating_mul(std::mem::size_of::<MaterializedInner>());
        if key_capacity.saturating_add(minimum_slots) > allowance {
            return None;
        }
        let mut inners = Vec::new();
        if inners.try_reserve_exact(MAX_INNER_CANDIDATES).is_err() {
            return None;
        }
        let retained_bytes = key_capacity.saturating_add(
            inners
                .capacity()
                .saturating_mul(std::mem::size_of::<MaterializedInner>()),
        );
        (retained_bytes <= allowance).then_some(Self {
            inners,
            retained_bytes,
            allowance,
        })
    }

    fn retain(&mut self, mode: u8, payload: &[u8], description: &str) -> bool {
        if self.inners.len() == self.inners.capacity() {
            return false;
        }
        let minimum_bytes = payload.len().saturating_add(description.len());
        if minimum_bytes > self.allowance.saturating_sub(self.retained_bytes) {
            return false;
        }
        let inner = MaterializedInner {
            mode,
            payload: payload.to_vec(),
            description: description.into(),
        };
        let bytes = inner
            .payload
            .capacity()
            .saturating_add(inner.description.capacity());
        if bytes > self.allowance.saturating_sub(self.retained_bytes) {
            return false;
        }
        self.retained_bytes = self.retained_bytes.saturating_add(bytes);
        self.inners.push(inner);
        true
    }
}

fn record_captured(
    best: &mut Option<(Vec<u8>, String)>,
    measurements: &mut Measurements,
    transform_id: u8,
    inner_mode: u8,
    inner: Vec<u8>,
    inner_name: &str,
    capture: &mut Option<CacheCapture>,
) {
    let failed = capture
        .as_mut()
        .is_some_and(|capture| !capture.retain(inner_mode, &inner, inner_name));
    if failed {
        *capture = None;
    }
    record_candidate(
        best,
        measurements,
        transform_id,
        inner_mode,
        inner,
        inner_name,
    );
}

fn record_materialized_cached(
    inners: &[MaterializedInner],
    transform_id: u8,
    best: &mut Option<(Vec<u8>, String)>,
    measurements: &mut Measurements,
) {
    for inner in inners {
        let mut payload = Vec::with_capacity(3usize.saturating_add(inner.payload.len()));
        payload.extend([CHAIN_VERSION, transform_id, inner.mode]);
        payload.extend_from_slice(&inner.payload);
        let description = format!("{}->{}", transform_name(transform_id), inner.description);
        #[cfg(test)]
        measurements.push(CandidateSize {
            transform_id,
            inner_mode: inner.mode,
            bytes: payload.len(),
            description: description.clone(),
        });
        #[cfg(not(test))]
        let _ = measurements;
        if best
            .as_ref()
            .is_none_or(|(current, _)| payload.len() < current.len())
        {
            *best = Some((payload, description));
        }
    }
}

#[inline]
fn consider_selected_transform(
    transform_id: u8,
    transformed: Vec<u8>,
    cacheable: bool,
    options: EncodeOptions,
    state: &mut CandidateSelectionState<'_>,
) -> Result<(), String> {
    if let Some(entry) = state.cache.iter().find(|entry| entry.key == transformed) {
        #[cfg(test)]
        CACHE_HITS.with(|hits| hits.set(hits.get().saturating_add(1)));
        record_materialized_cached(
            &entry.inners,
            transform_id,
            &mut *state.best,
            &mut *state.measurements,
        );
        return Ok(());
    }
    if !cacheable || state.cache.len() == state.cache.capacity() {
        let mut no_capture = None;
        consider_transform(
            &transformed,
            transform_id,
            options,
            &mut *state.best,
            &mut *state.measurements,
            &mut no_capture,
        )?;
        return Ok(());
    }
    let mut capture = if state.cache.len() < state.cache.capacity() {
        CacheCapture::begin(
            transformed.capacity(),
            state.cache_allowance.saturating_sub(*state.cached_bytes),
        )
    } else {
        None
    };
    consider_transform(
        &transformed,
        transform_id,
        options,
        &mut *state.best,
        &mut *state.measurements,
        &mut capture,
    )?;
    if let Some(capture) = capture {
        *state.cached_bytes = (*state.cached_bytes).saturating_add(capture.retained_bytes);
        state.cache.push(CachedTransform {
            key: transformed,
            inners: capture.inners,
        });
    }
    Ok(())
}

/// Encode a frozen chain portfolio with a per-call, exact transformed-byte
/// cache. A zero allowance preserves the historic uncached materialization.
pub fn encode_with_cache(
    data: &[u8],
    options: EncodeOptions,
    cache_allowance: usize,
) -> Result<EncodeResult, String> {
    if data.is_empty() {
        return Err("cannot transform-chain encode an empty block".into());
    }
    if options.beam == 0 || options.beam > CHAIN_SPEC_IDS.len() {
        return Err("transform-chain beam must be between 1 and 9".into());
    }
    if !(1..=9).contains(&options.effort) {
        return Err("transform-chain effort must be between 1 and 9".into());
    }
    let factorial_limit = data.len().max(256);
    let mut log_factorials = vec![0.0f64; factorial_limit + 1];
    for value in 2..=factorial_limit {
        log_factorials[value] = log_factorials[value - 1] + (value as f64).log2();
    }
    let mut profiled = Vec::with_capacity(CHAIN_SPEC_IDS.len());
    for &id in &CHAIN_SPEC_IDS {
        let transformed = transform::apply_id(data, id)?;
        let score = typeclass_score(&transformed, &log_factorials);
        profiled.push((score, id, transformed));
    }
    profiled.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    let selected = options.beam;
    let mut cacheable = [false; CHAIN_SPEC_IDS.len()];
    if cache_allowance != 0 {
        for left in 0..selected {
            cacheable[left] =
                ((left + 1)..selected).any(|right| profiled[left].2 == profiled[right].2);
        }
    }
    let has_duplicate = cacheable[..selected].iter().any(|&value| value);
    let mut best = None::<(Vec<u8>, String)>;
    #[cfg(test)]
    let mut measurements = Measurements::new();
    #[cfg(not(test))]
    let mut measurements = Measurements;
    let mut cache = Vec::<CachedTransform>::new();
    let mut cached_bytes = 0usize;
    if has_duplicate
        && cache_allowance >= std::mem::size_of::<Vec<CachedTransform>>()
        && cache.try_reserve_exact(selected).is_ok()
    {
        let reserved_bytes = cache_table_bytes(cache.capacity());
        if reserved_bytes <= cache_allowance {
            cached_bytes = reserved_bytes;
        } else {
            cache = Vec::new();
        }
    }
    {
        let mut state = CandidateSelectionState {
            cache_allowance,
            cache: &mut cache,
            cached_bytes: &mut cached_bytes,
            best: &mut best,
            measurements: &mut measurements,
        };
        for (index, (_, transform_id, transformed)) in
            profiled.into_iter().take(selected).enumerate()
        {
            consider_selected_transform(
                transform_id,
                transformed,
                cacheable[index],
                options,
                &mut state,
            )?;
        }
    }
    let (payload, description) = best.ok_or("no transform-chain candidates")?;
    #[cfg(not(test))]
    let _ = description;
    Ok(EncodeResult {
        payload,
        #[cfg(test)]
        description,
        #[cfg(test)]
        candidates: measurements,
    })
}

fn unshuffle(data: &[u8], width: usize) -> Vec<u8> {
    let usable = data.len() / width * width;
    let words = usable / width;
    let mut out = vec![0u8; usable];
    for lane in 0..width {
        for word in 0..words {
            out[word * width + lane] = data[lane * words + word];
        }
    }
    out.extend_from_slice(&data[usable..]);
    out
}

fn read_word(data: &[u8], endian: Endian) -> u64 {
    match endian {
        Endian::Little => data.iter().enumerate().fold(0u64, |value, (shift, &byte)| {
            value | ((byte as u64) << (8 * shift))
        }),
        Endian::Big => data
            .iter()
            .fold(0u64, |value, &byte| (value << 8) | byte as u64),
    }
}

fn write_word(value: u64, width: usize, endian: Endian, out: &mut Vec<u8>) {
    match endian {
        Endian::Little => {
            for shift in 0..width {
                out.push((value >> (8 * shift)) as u8);
            }
        }
        Endian::Big => {
            for shift in (0..width).rev() {
                out.push((value >> (8 * shift)) as u8);
            }
        }
    }
}

fn matchxor_decode(data: &[u8]) -> Vec<u8> {
    let mut table = HashMap::<[u8; 4], usize>::new();
    let mut active = None::<usize>;
    let mut out = Vec::with_capacity(data.len());
    for (pos, &residual) in data.iter().enumerate() {
        let source = if active.is_some_and(|source| source < pos) {
            active
        } else if pos >= 4 {
            let key: [u8; 4] = out[pos - 4..pos].try_into().unwrap();
            table.get(&key).copied()
        } else {
            None
        };
        let prediction = source.map_or(0, |source| out[source]);
        let byte = residual ^ prediction;
        out.push(byte);
        if pos >= 4 {
            let key: [u8; 4] = out[pos - 4..pos].try_into().unwrap();
            table.insert(key, pos);
        }
        active = source
            .filter(|_| prediction == byte)
            .and_then(|source| source.checked_add(1));
    }
    out
}

fn restore_byte_lag(raw: &mut [u8], lag: usize, operation: Operation) -> Result<(), String> {
    match operation {
        Operation::Xor => {
            for i in lag..raw.len() {
                raw[i] ^= raw[i - lag];
            }
        }
        Operation::Delta => {
            for i in lag..raw.len() {
                raw[i] = raw[i].wrapping_add(raw[i - lag]);
            }
        }
        _ => return Err("invalid byte-lag transform operation".into()),
    }
    Ok(())
}

fn restore_words(
    mut values: impl Iterator<Item = u64>,
    operation: Operation,
    mask: u64,
) -> Vec<u64> {
    let Some(first) = values.next() else {
        return Vec::new();
    };
    let mut restored = vec![first];
    match operation {
        Operation::Xor => restore_xor(&mut restored, values),
        Operation::Delta => restore_delta(&mut restored, values, mask),
        Operation::Delta2 => restore_delta2(&mut restored, values, mask),
        Operation::Delta2Zigzag => restore_zigzag_delta2(&mut restored, values, mask),
        Operation::MatchXor => unreachable!(),
    }
    restored
}

fn add_masked(left: u64, right: u64, mask: u64) -> u64 {
    left.wrapping_add(right) & mask
}
fn restore_xor(restored: &mut Vec<u64>, values: impl Iterator<Item = u64>) {
    for value in values {
        restored.push(restored.last().copied().unwrap() ^ value);
    }
}
fn restore_delta(restored: &mut Vec<u64>, values: impl Iterator<Item = u64>, mask: u64) {
    for value in values {
        restored.push(add_masked(restored.last().copied().unwrap(), value, mask));
    }
}
fn restore_delta2(restored: &mut Vec<u64>, mut values: impl Iterator<Item = u64>, mask: u64) {
    let Some(mut previous) = values.next() else {
        return;
    };
    restored.push(add_masked(restored[0], previous, mask));
    for value in values {
        previous = add_masked(previous, value, mask);
        restored.push(add_masked(
            restored.last().copied().unwrap(),
            previous,
            mask,
        ));
    }
}
fn unzigzag_masked(value: u64, mask: u64) -> u64 {
    if value & 1 == 0 {
        value >> 1
    } else {
        0u64.wrapping_sub((value >> 1) + 1) & mask
    }
}
fn restore_zigzag_delta2(
    restored: &mut Vec<u64>,
    mut values: impl Iterator<Item = u64>,
    mask: u64,
) {
    let Some(value) = values.next() else {
        return;
    };
    let mut previous = unzigzag_masked(value, mask);
    restored.push(add_masked(restored[0], previous, mask));
    for value in values {
        previous = add_masked(previous, unzigzag_masked(value, mask), mask);
        restored.push(add_masked(
            restored.last().copied().unwrap(),
            previous,
            mask,
        ));
    }
}

fn invert(data: &[u8], spec: Transform) -> Result<Vec<u8>, String> {
    if matches!(spec.operation, Operation::MatchXor) {
        return Ok(matchxor_decode(data));
    }
    let mut raw = if spec.shuffle {
        unshuffle(data, spec.width)
    } else {
        data.to_vec()
    };
    if let Some(lag) = spec.byte_lag {
        restore_byte_lag(&mut raw, lag, spec.operation)?;
        return Ok(raw);
    }
    let usable = raw.len() / spec.width * spec.width;
    let values = raw[..usable]
        .chunks_exact(spec.width)
        .map(|word| read_word(word, spec.endian));
    if usable == 0 {
        return Ok(raw);
    }
    let bits = 8 * spec.width;
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let restored = restore_words(values, spec.operation, mask);
    let mut out = Vec::with_capacity(raw.len());
    for word in restored {
        write_word(word, spec.width, spec.endian, &mut out);
    }
    out.extend_from_slice(&raw[usable..]);
    Ok(out)
}

/// Decode one CIXM6 mode-7 payload.
///
/// `model_limit` bounds adaptive context tables used by nested byte streams.
/// Transform-chain blocks do not inherit preceding CIXM6 history: the frozen
/// Python encoder invokes every inner codec with an empty prefix.
pub fn decode(data: &[u8], source_len: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    if data.len() < 3 || data[0] != CHAIN_VERSION {
        return Err("unknown transform-chain version".into());
    }
    let spec = transform(data[1]).ok_or("unknown transform-chain transform")?;
    let payload = &data[3..];
    let transformed = match data[2] {
        INNER_MULTINOMIAL => {
            let mut pos = 0;
            let out = rank::decode_type_class(payload, &mut pos, source_len)?;
            if pos != payload.len() {
                return Err("unused chained multinomial bytes".into());
            }
            out
        }
        INNER_CONTEXT => rank::decode_context_type_classes(payload, source_len, &[])?,
        INNER_LZ => decode_lz(payload, source_len, model_limit)?,
        INNER_BWT => bwt_legacy::decode(payload, source_len, model_limit)?,
        INNER_BITPLANE => bitplane::decode(payload, source_len)?,
        INNER_SPARSE => rank::decode_sparse_blob(payload, source_len)?,
        INNER_RUNS => decode_runs(payload, source_len, model_limit)?,
        mode => return Err(format!("unsupported transform-chain inner mode {mode}")),
    };
    if transformed.len() != source_len {
        return Err("transform-chain inner source length mismatch".into());
    }
    let restored = invert(&transformed, spec)?;
    if restored.len() != source_len {
        return Err("transform-chain source length mismatch".into());
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    fn verify_and_write(
        payload: Vec<u8>,
        source: &[u8],
        fixture_dir: Option<&PathBuf>,
        index: &mut usize,
    ) {
        assert_eq!(decode(&payload, source.len(), 1 << 16).unwrap(), source);
        if let Some(directory) = fixture_dir {
            fs::write(directory.join(format!("{}.source", *index)), source).unwrap();
            fs::write(directory.join(format!("{}.payload", *index)), payload).unwrap();
        }
        *index += 1;
    }

    #[test]
    fn lz_substream_lengths_start_after_varints() {
        let payloads = [vec![11], vec![22; 128], Vec::new(), vec![33; 257]];
        for version in [1, 3] {
            let mut data = vec![version, 3, 0, 0, 0];
            for payload in &payloads {
                put_var(payload.len(), &mut data);
                data.extend_from_slice(payload);
            }
            let (_, _, _, _, _, streams) = parse_lz_streams(&data).unwrap();
            assert_eq!(
                streams,
                payloads.iter().map(Vec::as_slice).collect::<Vec<_>>()
            );
            data.pop();
            assert!(parse_lz_streams(&data).is_err());
        }
    }

    #[test]
    fn rice_uints_reject_high_bit_overflow() {
        let width = usize::BITS as usize - 1;
        let mut valid = vec![1, width as u8];
        valid.push(0x7f);
        valid.extend(std::iter::repeat_n(0xff, width / 8));
        assert_eq!(decode_uints(&valid, 1, 1).unwrap(), vec![usize::MAX >> 1]);

        let mut overflow = vec![1, width as u8];
        overflow.push(0xc0);
        overflow.extend(std::iter::repeat_n(0, width / 8 + 1));
        assert_eq!(
            decode_uints(&overflow, 1, 1).unwrap_err(),
            "Rice integer overflow"
        );
    }

    fn word_mask(bits: usize) -> u64 {
        if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        }
    }

    fn assert_restore_boundaries(operation: Operation, encoded: &[u64], source: &[u64], mask: u64) {
        assert!(restore_words(std::iter::empty(), operation, mask).is_empty());
        assert_eq!(
            restore_words(encoded[..1].iter().copied(), operation, mask),
            source[..1]
        );
        assert_eq!(
            restore_words(encoded[..2].iter().copied(), operation, mask),
            source[..2]
        );
        assert_eq!(
            restore_words(encoded.iter().copied(), operation, mask),
            source
        );
    }

    fn normal_numeric_cases(source: &[u64; 5], mask: u64) -> [(Operation, Vec<u64>); 3] {
        let xor = vec![
            source[0],
            source[0] ^ source[1],
            source[1] ^ source[2],
            source[2] ^ source[3],
            source[3] ^ source[4],
        ];
        let delta = vec![
            source[0],
            source[1].wrapping_sub(source[0]) & mask,
            source[2].wrapping_sub(source[1]) & mask,
            source[3].wrapping_sub(source[2]) & mask,
            source[4].wrapping_sub(source[3]) & mask,
        ];
        let delta2 = vec![
            source[0],
            delta[1],
            delta[2].wrapping_sub(delta[1]) & mask,
            delta[3].wrapping_sub(delta[2]) & mask,
            delta[4].wrapping_sub(delta[3]) & mask,
        ];
        [
            (Operation::Xor, xor),
            (Operation::Delta, delta),
            (Operation::Delta2, delta2),
        ]
    }

    fn signed_word(value: u64, bits: usize) -> i128 {
        if bits == 64 {
            (value as i64) as i128
        } else if value < (1u64 << (bits - 1)) {
            value as i128
        } else {
            value as i128 - (1i128 << bits)
        }
    }

    fn zigzag(value: i128) -> u64 {
        if value >= 0 {
            (value << 1) as u64
        } else {
            ((-value << 1) - 1) as u64
        }
    }

    fn zigzag_delta2_case(source: &[u64; 5], bits: usize, mask: u64) -> Vec<u64> {
        let first_delta = source[1].wrapping_sub(source[0]) & mask;
        let mut previous_delta = first_delta;
        let mut encoded = vec![source[0], zigzag(signed_word(first_delta, bits))];
        for pair in source.windows(2).skip(1) {
            let delta = pair[1].wrapping_sub(pair[0]) & mask;
            encoded.push(zigzag(signed_word(
                delta.wrapping_sub(previous_delta) & mask,
                bits,
            )));
            previous_delta = delta;
        }
        encoded
    }

    #[test]
    fn restore_words_handles_all_numeric_operations_and_boundaries() {
        for bits in [16usize, 32, 64] {
            let mask = word_mask(bits);
            let source = [mask.wrapping_sub(1), 1, 2, mask, 3];
            // The first word is stored verbatim.  Every operation must also
            // preserve the empty, one-word, and two-word boundaries.
            for (operation, encoded) in normal_numeric_cases(&source, mask) {
                assert_restore_boundaries(operation, &encoded, &source, mask);
            }

            // Encode a delta2 sequence whose first and later changes cross
            // zero, so the zigzag inverse sees both signs and wrapping.
            let zigzag_source = [mask.wrapping_sub(1), 1, mask.wrapping_sub(2), 2, mask];
            let zigzag_encoded = zigzag_delta2_case(&zigzag_source, bits, mask);
            assert_restore_boundaries(
                Operation::Delta2Zigzag,
                &zigzag_encoded,
                &zigzag_source,
                mask,
            );
        }
    }

    #[test]
    fn mode7_multinomial_fixture_restores_fresh_archives() {
        // This keeps mode 7 independent from the selector: each frame is
        // constructed from one explicit transform and the fixed multinomial
        // inner coder, matching the on-disk chain layout exactly.
        let fixture_dir = std::env::var_os("CIX_MODE7_FIXTURE_DIR").map(PathBuf::from);
        if let Some(directory) = &fixture_dir {
            fs::create_dir_all(directory).unwrap();
        }
        for transform_id in [5u8, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 29, 30, 31] {
            let source = (0..64)
                .map(|index| ((index * 37 + index * index * 11 + 19) & 255) as u8)
                .collect::<Vec<_>>();
            let transformed = transform::apply_id(&source, transform_id).unwrap();
            let mut payload = vec![CHAIN_VERSION, transform_id, INNER_MULTINOMIAL];
            payload.extend(rank::encode_type_class(&transformed).unwrap());
            assert_eq!(decode(&payload, source.len(), 1 << 16).unwrap(), source);
            if let Some(directory) = &fixture_dir {
                let mut archive = Vec::new();
                archive.extend_from_slice(b"CIXM6");
                put_var(1, &mut archive); // bounded history window
                archive.push(7); // CIXM6 transform-chain route.
                put_var(source.len(), &mut archive);
                put_var(payload.len(), &mut archive);
                archive.extend_from_slice(&payload);
                archive.push(255);
                put_var(source.len(), &mut archive);
                archive.extend_from_slice(&crc32fast::hash(&source).to_le_bytes());
                let stem = format!("mode7-transform-{transform_id}");
                fs::write(directory.join(format!("{stem}.source")), &source).unwrap();
                fs::write(directory.join(format!("{stem}.payload")), &payload).unwrap();
                fs::write(directory.join(format!("{stem}.cix")), archive).unwrap();
            }
        }
    }

    #[test]
    fn encoder_fixtures_cross_decode_in_rust() {
        let mut words = Vec::new();
        for value in 0..2048u32 {
            words.extend_from_slice(&(value.wrapping_mul(value + 3)).to_le_bytes());
        }
        let repetitive = b"transform chain native encoder fixture::"
            .repeat(180)
            .into_iter()
            .chain((0u8..=127).cycle().take(1024))
            .collect::<Vec<_>>();
        let mut state = 0x9e37_79b9u32;
        let mixed = (0..8192)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect::<Vec<_>>();
        let mut sparse = vec![0u8; 8192];
        for index in (0..sparse.len()).step_by(127) {
            sparse[index] = (index / 127 + 1) as u8;
        }
        let periodic = (0..8192)
            .map(|index| ((index / 4 + index % 17) & 255) as u8)
            .collect::<Vec<_>>();
        let cases = [words, repetitive, mixed, sparse, periodic];
        let fixture_dir = std::env::var_os("CIX_CHAIN_FIXTURE_DIR").map(PathBuf::from);
        if let Some(directory) = &fixture_dir {
            fs::create_dir_all(directory).unwrap();
        }
        let mut fixture_index = 0usize;
        let mut inner_modes = BTreeSet::new();
        for (index, source) in cases.iter().enumerate() {
            let encoded =
                encode_with_cache(source, EncodeOptions { beam: 9, effort: 9 }, 0).unwrap();
            assert!(!encoded.description.is_empty());
            verify_and_write(
                encoded.payload.clone(),
                source,
                fixture_dir.as_ref(),
                &mut fixture_index,
            );
            assert_eq!(
                encoded.candidates.iter().map(|item| item.bytes).min(),
                Some(encoded.payload.len())
            );
            let transforms = encoded
                .candidates
                .iter()
                .map(|item| item.transform_id)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                transforms,
                CHAIN_SPEC_IDS.into_iter().collect::<BTreeSet<_>>()
            );
            inner_modes.extend(encoded.candidates.iter().map(|item| item.inner_mode));
            if let Some(directory) = &fixture_dir {
                let measurements = encoded
                    .candidates
                    .iter()
                    .map(|item| {
                        format!(
                            "{}\t{}\t{}\t{}",
                            item.transform_id, item.inner_mode, item.bytes, item.description
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                fs::write(
                    directory.join(format!("{index}.candidates.tsv")),
                    measurements,
                )
                .unwrap();
            }
        }
        assert_eq!(inner_modes, (1u8..=7).collect::<BTreeSet<_>>());

        let transform_source = &cases[0];
        for transform_id in CHAIN_SPEC_IDS {
            let transformed = transform::apply_id(transform_source, transform_id).unwrap();
            let mut payload = vec![CHAIN_VERSION, transform_id, INNER_MULTINOMIAL];
            payload.extend(rank::encode_type_class(&transformed).unwrap());
            verify_and_write(
                payload,
                transform_source,
                fixture_dir.as_ref(),
                &mut fixture_index,
            );
        }

        let transform_id = 20;
        let spec = transform(transform_id).unwrap();
        let direct_inners = [
            (INNER_CONTEXT, b"context encoder fixture::".repeat(160)),
            (INNER_LZ, b"native LZ chain encoder::".repeat(180)),
            (INNER_BWT, b"banana_bandana::abracadabra::".repeat(150)),
            (
                INNER_BITPLANE,
                (0u8..=63).cycle().take(4096).collect::<Vec<_>>(),
            ),
            (INNER_SPARSE, {
                let mut value = vec![0u8; 4096];
                for index in (0..value.len()).step_by(137) {
                    value[index] = (index / 137 + 1) as u8;
                }
                value
            }),
            (INNER_RUNS, {
                let mut value = vec![b'A'; 1600];
                value.extend(vec![b'B'; 1200]);
                value
            }),
        ];
        for (inner_mode, transformed) in direct_inners {
            let inner = match inner_mode {
                INNER_CONTEXT => {
                    rank::encode_context_type_classes(&transformed, 2, 8, &[]).unwrap()
                }
                INNER_LZ => legacy_lz::encode(&transformed, &[], 9).unwrap().payload,
                INNER_BWT => bwt_legacy::encode(&transformed).unwrap().0,
                INNER_BITPLANE => bitplane::encode(&transformed),
                INNER_SPARSE => encode_sparse(&transformed).unwrap(),
                INNER_RUNS => encode_runs(&transformed).unwrap(),
                _ => unreachable!(),
            };
            let source = invert(&transformed, spec).unwrap();
            let mut payload = vec![CHAIN_VERSION, transform_id, inner_mode];
            payload.extend(inner);
            verify_and_write(payload, &source, fixture_dir.as_ref(), &mut fixture_index);
        }
        assert_eq!(fixture_index, 20);
    }

    fn candidate_table(result: &EncodeResult) -> Vec<(u8, u8, usize, String)> {
        result
            .candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.transform_id,
                    candidate.inner_mode,
                    candidate.bytes,
                    candidate.description.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn exact_inner_cache_preserves_chain_selection_and_fails_open_when_full() {
        let duplicate = vec![0u8; 256];
        let distinct = (0..256usize)
            .map(|index| ((index.wrapping_mul(73) ^ (index >> 3)) & 255) as u8)
            .collect::<Vec<_>>();
        let options = EncodeOptions { beam: 9, effort: 9 };
        for (source, expected_cache_hit) in [(&duplicate, true), (&distinct, false)] {
            let uncached = encode_with_cache(source, options, 0).unwrap();
            CACHE_HITS.with(|hits| hits.set(0));
            let cached = encode_with_cache(source, options, 4 * 1024 * 1024).unwrap();
            assert_eq!(CACHE_HITS.with(|hits| hits.get()) != 0, expected_cache_hit);
            let table_rejected = encode_with_cache(
                source,
                options,
                cache_table_bytes(options.beam).saturating_add(1),
            )
            .unwrap();
            for result in [&cached, &table_rejected] {
                assert_eq!(result.payload, uncached.payload);
                assert_eq!(result.description, uncached.description);
                assert_eq!(candidate_table(result), candidate_table(&uncached));
                assert_eq!(
                    decode(&result.payload, source.len(), 1 << 20)
                        .unwrap()
                        .as_slice(),
                    source.as_slice()
                );
            }
        }
    }

    #[test]
    fn cache_capture_rejects_unfunded_payload_before_retaining_it() {
        let key_capacity = 8usize;
        let slots = CacheCapture::begin(key_capacity, usize::MAX)
            .unwrap()
            .retained_bytes;
        let allowance = slots.saturating_add(1);
        let mut capture = CacheCapture::begin(key_capacity, allowance).unwrap();
        assert!(!capture.retain(INNER_MULTINOMIAL, &[1, 2], "x"));
        assert!(capture.inners.is_empty());
        assert_eq!(capture.retained_bytes, slots);

        let mut best = None;
        let mut measurements = Vec::new();
        let mut capture = Some(capture);
        record_captured(
            &mut best,
            &mut measurements,
            12,
            INNER_MULTINOMIAL,
            vec![1, 2],
            "test",
            &mut capture,
        );
        assert!(capture.is_none());
        assert_eq!(measurements.len(), 1);
    }
}
