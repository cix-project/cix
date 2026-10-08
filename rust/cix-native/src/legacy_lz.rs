//! Decoder for the frozen Python CIXM6 mode-3 LZ payload.
//!
//! The payload stores four independently coded substreams: tags, literals,
//! match lengths, and match distances. Exact-rank substreams retain arbitrary-
//! precision combinatorial ranks through `rank`.

use num_bigint::BigUint;
use num_traits::{One, Zero};
use std::collections::{BTreeMap, HashMap};

const LZ_VERSION: u8 = 3;
const DEFAULT_MIN_MATCH: usize = 4;
const DEFAULT_MAX_MATCH: usize = 65535;

/// Literal-context candidate exposed to bounded selector integrations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextSpec {
    pub order: u8,
    pub bucket_bits: u8,
}

/// One independently comparable mode-3 payload candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedCandidate {
    pub payload: Vec<u8>,
    pub description: String,
}

/// Fixed mode-3 candidate recipe. `version` may be 1 or 3.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodeConfig {
    pub version: u8,
    pub window: usize,
    pub chain: usize,
    pub min_match: usize,
    pub lazy: bool,
    pub context_models: Vec<ContextSpec>,
    pub allow_fixed_mix: bool,
}

impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            version: LZ_VERSION,
            window: 1 << 20,
            chain: 32,
            min_match: DEFAULT_MIN_MATCH,
            lazy: false,
            context_models: vec![
                ContextSpec {
                    order: 1,
                    bucket_bits: 8,
                },
                ContextSpec {
                    order: 2,
                    bucket_bits: 10,
                },
            ],
            allow_fixed_mix: false,
        }
    }
}

#[derive(Clone)]
struct LzParse {
    tags: Vec<u8>,
    literals: Vec<u8>,
    literal_positions: Vec<usize>,
    lengths: Vec<usize>,
    distances: Vec<usize>,
}

type IntegerClassLows = Vec<(usize, usize)>;
type IntegerClasses = (Vec<u8>, IntegerClassLows);

fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

struct BitWriter {
    data: Vec<u8>,
    current: u8,
    used: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            current: 0,
            used: 0,
        }
    }

    fn bit(&mut self, value: u8) {
        self.current = (self.current << 1) | (value & 1);
        self.used += 1;
        if self.used == 8 {
            self.data.push(self.current);
            self.current = 0;
            self.used = 0;
        }
    }

    fn bits(&mut self, value: usize, width: usize) {
        for shift in (0..width).rev() {
            self.bit(((value >> shift) & 1) as u8);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            self.data.push(self.current << (8 - self.used));
        }
        self.data
    }
}

fn choose(n: usize, k: usize) -> BigUint {
    if k > n {
        return BigUint::zero();
    }
    let k = k.min(n - k);
    let mut value = BigUint::one();
    for i in 1..=k {
        value *= n - k + i;
        value /= i;
    }
    value
}

fn fixed_bytes(value: &BigUint, width: usize) -> Vec<u8> {
    let size = width.div_ceil(8);
    let bytes = value.to_bytes_be();
    let mut out = vec![0; size.saturating_sub(bytes.len())];
    out.extend_from_slice(&bytes);
    out
}

fn rank_width(cardinality: BigUint) -> usize {
    if cardinality.is_zero() {
        0
    } else {
        (cardinality - BigUint::one()).bits() as usize
    }
}

fn colex_rank(values: &[usize]) -> BigUint {
    values
        .iter()
        .enumerate()
        .fold(BigUint::zero(), |sum, (i, &value)| {
            sum + choose(value, i + 1)
        })
}

fn composition_rank(parts: &[usize]) -> Result<BigUint, String> {
    if parts.is_empty() || parts.contains(&0) {
        return Err("invalid LZ composition".into());
    }
    let mut total = 0usize;
    let mut cuts = Vec::with_capacity(parts.len().saturating_sub(1));
    for &part in &parts[..parts.len() - 1] {
        total = total.checked_add(part).ok_or("LZ composition overflow")?;
        cuts.push(total - 1);
    }
    Ok(colex_rank(&cuts))
}

fn bit_length(value: usize) -> usize {
    usize::BITS as usize - value.leading_zeros() as usize
}

fn insert_match(
    table: &mut HashMap<[u8; 4], Vec<usize>>,
    base: &[u8],
    position: usize,
    chain: usize,
) {
    if position + 4 > base.len() {
        return;
    }
    let key = [
        base[position],
        base[position + 1],
        base[position + 2],
        base[position + 3],
    ];
    let entries = table.entry(key).or_default();
    entries.push(position);
    if entries.len() > chain {
        entries.remove(0);
    }
}

fn find_match(
    table: &HashMap<[u8; 4], Vec<usize>>,
    base: &[u8],
    at: usize,
    min_match: usize,
    max_match: usize,
    window: usize,
    chain: usize,
) -> (usize, usize) {
    if at + 4 > base.len() {
        return (0, 0);
    }
    let key = [base[at], base[at + 1], base[at + 2], base[at + 3]];
    let Some(entries) = table.get(&key) else {
        return (0, 0);
    };
    let mut best_length = 0usize;
    let mut best_distance = 0usize;
    let mut best_score = i64::MIN;
    for &candidate in entries.iter().rev().take(chain) {
        let distance = at - candidate;
        if distance == 0 || distance > window {
            continue;
        }
        let limit = max_match.min(base.len() - at);
        let mut length = 4usize;
        while length < limit && base[at - distance + length] == base[at + length] {
            length += 1;
        }
        if length < min_match {
            continue;
        }
        let score =
            (length * 8) as i64 - bit_length(distance) as i64 - bit_length(length) as i64 - 3;
        if score > best_score || (score == best_score && length > best_length) {
            best_score = score;
            best_length = length;
            best_distance = distance;
        }
    }
    (best_length, best_distance)
}

fn push_lz_literal(parsed: &mut LzParse, value: u8, position: usize, history_len: usize) {
    parsed.tags.push(0);
    parsed.literals.push(value);
    parsed.literal_positions.push(position - history_len);
}

fn insert_lz_range(
    table: &mut HashMap<[u8; 4], Vec<usize>>,
    base: &[u8],
    start: usize,
    stop: usize,
    chain: usize,
) {
    for at in start..stop {
        insert_match(table, base, at, chain);
    }
}

fn parse_lz(
    data: &[u8],
    prefix_history: &[u8],
    min_match: usize,
    max_match: usize,
    window: usize,
    chain: usize,
    lazy: bool,
) -> Result<LzParse, String> {
    if window == 0 || chain == 0 {
        return Err("LZ window and chain must be nonzero".into());
    }
    let min_match = min_match.max(4);
    let history = &prefix_history[prefix_history.len().saturating_sub(window)..];
    let mut base = Vec::with_capacity(history.len().saturating_add(data.len()));
    base.extend_from_slice(history);
    base.extend_from_slice(data);
    let history_len = history.len();
    let mut table = HashMap::<[u8; 4], Vec<usize>>::new();
    let seed_start = history_len.saturating_sub(window);
    let seed_end = history_len.saturating_sub(3).max(seed_start);
    for position in seed_start..seed_end {
        insert_match(&mut table, &base, position, chain);
    }

    let mut parsed = LzParse {
        tags: Vec::new(),
        literals: Vec::new(),
        literal_positions: Vec::new(),
        lengths: Vec::new(),
        distances: Vec::new(),
    };
    let mut position = history_len;
    while position < base.len() {
        let (best_length, best_distance) =
            find_match(&table, &base, position, min_match, max_match, window, chain);
        if best_length >= min_match && lazy && position + 1 < base.len() {
            insert_match(&mut table, &base, position, chain);
            let (next_length, _) = find_match(
                &table,
                &base,
                position + 1,
                min_match,
                max_match,
                window,
                chain,
            );
            if next_length > best_length + 1 {
                push_lz_literal(&mut parsed, base[position], position, history_len);
                position += 1;
                continue;
            }
        }
        if best_length >= min_match {
            parsed.tags.push(1);
            parsed.lengths.push(best_length - min_match);
            parsed.distances.push(best_distance);
            let stop = base.len().min(position + best_length);
            insert_lz_range(&mut table, &base, position, stop, chain);
            position += best_length;
        } else {
            push_lz_literal(&mut parsed, base[position], position, history_len);
            insert_match(&mut table, &base, position, chain);
            position += 1;
        }
    }
    Ok(parsed)
}

fn read_varint(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..usize::BITS as usize).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated LZ varint")?;
        *pos += 1;
        let low = (byte & 0x7f) as usize;
        if low > (usize::MAX >> shift) {
            return Err("LZ varint overflow".into());
        }
        let part = low.checked_shl(shift as u32).ok_or("LZ varint overflow")?;
        value = value.checked_add(part).ok_or("LZ varint overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized LZ varint".into())
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn read(&mut self, width: usize) -> Result<usize, String> {
        if width >= usize::BITS as usize || self.pos.saturating_add(width) > self.data.len() * 8 {
            return Err("truncated LZ bitstream".into());
        }
        let mut value = 0usize;
        for _ in 0..width {
            value = (value << 1) | (((self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1) as usize);
            self.pos += 1;
        }
        Ok(value)
    }
}

fn context_bucket(tail: &[u8], order: usize, bucket_bits: usize) -> usize {
    if order == 0 {
        return 0;
    }
    let context = &tail[tail.len().saturating_sub(order)..];
    if order == 1 && bucket_bits >= 8 {
        return context.last().copied().unwrap_or(0) as usize;
    }
    let mut hash = 1469598103934665603u64;
    for &byte in context {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    (hash & ((1u64 << bucket_bits) - 1)) as usize
}

fn encode_wavelet_node(values: &[u8], depth: usize, encoder: &mut crate::rank::ArithmeticEncoder) {
    if values.is_empty() || depth == 8 {
        return;
    }
    let shift = 7 - depth;
    let bits: Vec<u8> = values.iter().map(|&value| (value >> shift) & 1).collect();
    let ones = bits.iter().map(|&bit| bit as usize).sum::<usize>();
    encoder.encode(ones, ones + 1, values.len() + 1);
    let (mut zero, mut remaining) = (values.len() - ones, values.len());
    for &bit in &bits {
        if bit == 0 {
            encoder.encode(0, zero, remaining);
            zero -= 1;
        } else {
            encoder.encode(zero, remaining, remaining);
        }
        remaining -= 1;
    }
    if ones == 0 || ones == values.len() {
        encode_wavelet_node(values, depth + 1, encoder);
    } else {
        let zeros: Vec<u8> = values
            .iter()
            .zip(&bits)
            .filter_map(|(&value, &bit)| (bit == 0).then_some(value))
            .collect();
        let ones: Vec<u8> = values
            .iter()
            .zip(&bits)
            .filter_map(|(&value, &bit)| (bit == 1).then_some(value))
            .collect();
        encode_wavelet_node(&zeros, depth + 1, encoder);
        encode_wavelet_node(&ones, depth + 1, encoder);
    }
}

fn encode_wavelet(data: &[u8]) -> Vec<u8> {
    let mut encoder = crate::rank::ArithmeticEncoder::new();
    encode_wavelet_node(data, 0, &mut encoder);
    let (payload, bit_length) = encoder.finish();
    let mut out = vec![1];
    put_varint(bit_length, &mut out);
    out.extend(payload);
    out
}

fn log_factorials(n: usize) -> Vec<f64> {
    let mut values = Vec::with_capacity(n + 1);
    values.push(0.0);
    for value in 1..=n {
        values.push(values[value - 1] + (value as f64).log2());
    }
    values
}

fn log_choose(log_factorial: &[f64], n: usize, k: usize) -> f64 {
    if k > n {
        f64::NEG_INFINITY
    } else {
        log_factorial[n] - log_factorial[k] - log_factorial[n - k]
    }
}

fn wavelet_estimate_node(
    counts: &[usize; 256],
    lo: usize,
    hi: usize,
    depth: usize,
    log_factorial: &[f64],
) -> f64 {
    let n = counts[lo..hi].iter().sum::<usize>();
    if n == 0 || depth == 8 {
        return 0.0;
    }
    let middle = (lo + hi) / 2;
    let ones = counts[middle..hi].iter().sum::<usize>();
    let arrangement = if ones == 0 || ones == n {
        0.0
    } else {
        log_choose(log_factorial, n, ones)
    };
    ((n + 1) as f64).log2()
        + arrangement
        + wavelet_estimate_node(counts, lo, middle, depth + 1, log_factorial)
        + wavelet_estimate_node(counts, middle, hi, depth + 1, log_factorial)
}

fn wavelet_estimate_bits(data: &[u8]) -> f64 {
    let mut counts = [0usize; 256];
    for &value in data {
        counts[value as usize] += 1;
    }
    let log_factorial = log_factorials(data.len());
    wavelet_estimate_node(&counts, 0, 256, 0, &log_factorial) + 24.0
}

fn conditional_bits(values: &[u8], order: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut contexts = HashMap::<Vec<u8>, [usize; 256]>::new();
    let mut history = Vec::<u8>::with_capacity(order);
    for &value in values {
        let key = if order == 0 {
            Vec::new()
        } else {
            history.clone()
        };
        contexts.entry(key).or_insert([0; 256])[value as usize] += 1;
        if order > 0 {
            history.push(value);
            if history.len() > order {
                history.remove(0);
            }
        }
    }
    contexts
        .values()
        .map(|counts| {
            let n = counts.iter().sum::<usize>();
            (n as f64) * (n as f64).log2()
                - counts
                    .iter()
                    .filter(|&&count| count > 0)
                    .map(|&count| (count as f64) * (count as f64).log2())
                    .sum::<f64>()
        })
        .sum()
}

fn zigzag(value: i128) -> Result<usize, String> {
    let encoded = if value >= 0 {
        value.checked_mul(2)
    } else {
        value
            .checked_neg()
            .and_then(|v| v.checked_mul(2))
            .and_then(|v| v.checked_sub(1))
    }
    .ok_or("LZ histogram zigzag overflow")?;
    usize::try_from(encoded).map_err(|_| "LZ histogram zigzag overflow".into())
}

fn active_prefix(active: &[usize]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(active.len(), &mut out);
    if active.len() < 256 {
        out.extend(fixed_bytes(
            &colex_rank(active),
            rank_width(choose(256, active.len())),
        ));
    }
    out
}

fn histogram_candidates(counts: &[usize; 256]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let total = counts.iter().sum::<usize>();
    if total == 0 {
        return Err("empty LZ histogram".into());
    }
    let active: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(symbol, &count)| (count > 0).then_some(symbol))
        .collect();
    let active_counts: Vec<usize> = active.iter().map(|&symbol| counts[symbol]).collect();
    let prefix = active_prefix(&active);
    let active_center = total / active.len();
    let dense_center = total / 256;

    let mut active_delta = Vec::with_capacity(active.len().saturating_sub(1));
    let mut previous = 0i128;
    for &count in &active_counts[..active_counts.len() - 1] {
        active_delta.push(zigzag(count as i128 - previous)?);
        previous = count as i128;
    }
    let mut dense_delta = Vec::with_capacity(255);
    previous = 0;
    for &count in &counts[..255] {
        dense_delta.push(zigzag(count as i128 - previous)?);
        previous = count as i128;
    }
    let active_uint: Vec<usize> = active_counts[..active_counts.len() - 1]
        .iter()
        .map(|&count| count - 1)
        .collect();
    let active_centered: Vec<usize> = active_counts[..active_counts.len() - 1]
        .iter()
        .map(|&count| zigzag(count as i128 - active_center as i128))
        .collect::<Result<_, _>>()?;
    let dense_centered: Vec<usize> = counts[..255]
        .iter()
        .map(|&count| zigzag(count as i128 - dense_center as i128))
        .collect::<Result<_, _>>()?;

    let mut out = Vec::new();
    for (name, mode, values, sparse) in [
        ("active-centered", 4u8, active_centered, true),
        ("active-delta", 1, active_delta, true),
        ("active-uint", 0, active_uint, true),
        ("dense-centered", 5, dense_centered, false),
        ("dense-delta", 3, dense_delta, false),
        ("dense-uint", 2, counts[..255].to_vec(), false),
    ] {
        let mut candidate = vec![1, mode];
        if sparse {
            candidate.extend_from_slice(&prefix);
        }
        candidate.extend(encode_uints(&values)?);
        out.push((name.to_string(), candidate));
    }
    Ok(out)
}

fn histogram_typeclass_candidate(data: &[u8], incumbent: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if data.len() < 512 {
        return Ok(None);
    }
    let mut counts = [0usize; 256];
    for &value in data {
        counts[value as usize] += 1;
    }
    let active: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(symbol, &count)| (count > 0).then_some(symbol))
        .collect();
    if active.len() < 2 {
        return Ok(None);
    }
    let mut header_size = active_prefix(&active).len();
    header_size += rank_width(choose(data.len() - 1, active.len() - 1)).div_ceil(8);
    if header_size < 64 || header_size > incumbent.len() {
        return Ok(None);
    }
    let candidates = histogram_candidates(&counts)?;
    let histogram = &candidates
        .iter()
        .min_by(|left, right| (left.1.len(), &left.0).cmp(&(right.1.len(), &right.0)))
        .ok_or("no LZ histogram candidate")?
        .1;
    let mut prefix = Vec::new();
    put_varint(histogram.len(), &mut prefix);
    prefix.extend_from_slice(histogram);
    if prefix.len() >= header_size {
        return Ok(None);
    }
    prefix.extend_from_slice(&incumbent[header_size..]);
    Ok(Some(prefix))
}

fn lz_raw_uints(values: &[usize]) -> Vec<u8> {
    let mut raw = vec![0];
    for &value in values {
        put_varint(value, &mut raw);
    }
    raw
}

fn lz_rice_candidate(values: &[usize], k: usize, incumbent_length: usize) -> Option<Vec<u8>> {
    let payload_limit = incumbent_length.saturating_sub(2) * 8;
    let mut cost = 0usize;
    for &value in values {
        cost = cost.saturating_add((value >> k).saturating_add(1 + k));
        if cost >= payload_limit {
            return None;
        }
    }
    let mut writer = BitWriter::new();
    for &value in values {
        for _ in 0..(value >> k) {
            writer.bit(1);
        }
        writer.bit(0);
        writer.bits(value & ((1usize << k) - 1), k);
    }
    let mut candidate = vec![1, k as u8];
    candidate.extend(writer.finish());
    Some(candidate)
}

fn lz_integer_classes(values: &[usize]) -> Result<Option<IntegerClasses>, String> {
    let mut classes = Vec::with_capacity(values.len());
    let mut lows = Vec::with_capacity(values.len());
    for &value in values {
        let shifted = value.checked_add(1).ok_or("LZ integer value overflow")?;
        let width = bit_length(shifted) - 1;
        if width > 255 {
            return Ok(None);
        }
        classes.push(width as u8);
        lows.push((shifted - (1usize << width), width));
    }
    Ok(Some((classes, lows)))
}

fn lz_class_candidate(mode: u8, class_blob: Vec<u8>, lows: &[(usize, usize)]) -> Vec<u8> {
    let mut writer = BitWriter::new();
    for &(low, width) in lows {
        writer.bits(low, width);
    }
    let mut candidate = vec![mode];
    put_varint(class_blob.len(), &mut candidate);
    candidate.extend(class_blob);
    candidate.extend_from_slice(&writer.finish());
    candidate
}

fn encode_uints(values: &[usize]) -> Result<Vec<u8>, String> {
    if values.is_empty() {
        return Ok(vec![0]);
    }
    let mut best = lz_raw_uints(values);
    for k in 0..=16usize {
        if let Some(candidate) = lz_rice_candidate(values, k, best.len()) {
            if candidate.len() < best.len() {
                best = candidate;
            }
        }
    }
    let Some((classes, lows)) = lz_integer_classes(values)? else {
        return Ok(best);
    };
    for (mode, class_blob) in [
        (2u8, crate::rank::encode_type_class(&classes)?),
        (3u8, encode_bytes(&classes)?),
    ] {
        let candidate = lz_class_candidate(mode, class_blob, &lows);
        if candidate.len() < best.len() {
            best = candidate;
        }
    }
    Ok(best)
}

fn replace_if_smaller(best: &mut Vec<u8>, candidate: Vec<u8>) {
    if candidate.len() < best.len() {
        *best = candidate;
    }
}

fn lz_chunked_bytes(values: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut candidates = Vec::new();
    for chunk_size in [128usize, 256, 512, 1024, 2048, 4096] {
        if values.len() < chunk_size * 2 {
            continue;
        }
        let mut candidate = vec![1];
        put_varint(chunk_size, &mut candidate);
        for chunk in values.chunks(chunk_size) {
            candidate.extend(crate::rank::encode_type_class(chunk)?);
        }
        candidates.push(candidate);
    }
    Ok(candidates)
}

fn lz_wavelet_candidate(values: &[u8], incumbent_length: usize) -> Option<Vec<u8>> {
    if values.len() < 64
        || wavelet_estimate_bits(values) / 8.0 > incumbent_length as f64 * 1.03 + 8.0
    {
        return None;
    }
    let mut candidate = vec![4];
    candidate.extend(encode_wavelet(values));
    Some(candidate)
}

fn lz_apply_adaptive_candidates(values: &[u8], best: &mut Vec<u8>) -> Result<(), String> {
    if values.len() < 256 {
        return Ok(());
    }
    let mut order_zero = vec![2];
    order_zero.extend(crate::adaptive::encode_general(values, 0, 8, &[], 1)?);
    replace_if_smaller(best, order_zero);
    for (order, bucket_bits) in [(1usize, 8usize), (2, 10)] {
        if order == 2 && values.len() < 2048 {
            continue;
        }
        if conditional_bits(values, order) / 8.0 > best.len() as f64 * 1.10 + 16.0 {
            continue;
        }
        let mut candidate = vec![3, order as u8];
        candidate.extend(crate::adaptive::encode_general(
            values,
            order,
            bucket_bits,
            &[],
            1usize << bucket_bits,
        )?);
        replace_if_smaller(best, candidate);
    }
    Ok(())
}

fn encode_bytes(values: &[u8]) -> Result<Vec<u8>, String> {
    let static_stream = crate::rank::encode_type_class(values)?;
    let mut best = [vec![0], static_stream.clone()].concat();
    if values.is_empty() {
        return Ok(best);
    }
    for candidate in lz_chunked_bytes(values)? {
        replace_if_smaller(&mut best, candidate);
    }
    if let Some(candidate) = lz_wavelet_candidate(values, best.len()) {
        replace_if_smaller(&mut best, candidate);
    }
    lz_apply_adaptive_candidates(values, &mut best)?;
    if let Some(histogram) = histogram_typeclass_candidate(values, &static_stream)? {
        let mut candidate = vec![5];
        candidate.extend(histogram);
        replace_if_smaller(&mut best, candidate);
    }
    Ok(best)
}

fn encode_tags(tags: &[u8]) -> Result<Vec<u8>, String> {
    let encoded = crate::rank::encode_type_class(tags)?;
    let mut best = Vec::with_capacity(encoded.len() + 1);
    best.push(0);
    best.extend(encoded);

    let mut local = vec![2];
    local.extend(encode_bytes(tags)?);
    if local.len() < best.len() {
        best = local;
    }
    if tags.is_empty() {
        return Ok(best);
    }

    let mut runs = Vec::<usize>::new();
    let mut current = tags[0];
    let mut length = 1usize;
    for &tag in &tags[1..] {
        if tag == current {
            length += 1;
        } else {
            runs.push(length - 1);
            current = tag;
            length = 1;
        }
    }
    runs.push(length - 1);
    let mut candidate = vec![1, tags[0]];
    put_varint(runs.len(), &mut candidate);
    candidate.extend(encode_uints(&runs)?);
    if candidate.len() < best.len() {
        best = candidate;
    }
    Ok(best)
}

fn varint_size(mut value: usize) -> usize {
    let mut size = 1usize;
    while value >= 128 {
        size += 1;
        value >>= 7;
    }
    size
}

fn approximate_typeclass_bytes(values: &[u8], log_factorial: &[f64]) -> f64 {
    if values.is_empty() {
        return 1.0;
    }
    let mut counts = [0usize; 256];
    for &value in values {
        counts[value as usize] += 1;
    }
    let active: Vec<usize> = counts.iter().copied().filter(|&count| count > 0).collect();
    let m = active.len();
    let mut size = varint_size(m) as f64;
    if m < 256 {
        size += (log_choose(log_factorial, 256, m) + 7.0) / 8.0;
    }
    if m > 1 {
        size += (log_choose(log_factorial, values.len() - 1, m - 1) + 7.0) / 8.0;
    }
    let log_cardinality = log_factorial[values.len()]
        - active
            .iter()
            .map(|&count| log_factorial[count])
            .sum::<f64>();
    size + (log_cardinality.max(0.0) + 7.0) / 8.0
}

fn literal_context_tail<'a>(
    prefix: &'a [u8],
    source: &'a [u8],
    position: usize,
    order: usize,
) -> Vec<u8> {
    if order == 0 {
        return Vec::new();
    }
    if position >= order {
        return source[position - order..position].to_vec();
    }
    let missing = order - position;
    let mut out = prefix[prefix.len().saturating_sub(missing)..].to_vec();
    out.extend_from_slice(&source[..position]);
    out
}

fn encode_bucketed(
    buckets: &BTreeMap<usize, Vec<u8>>,
    bucket_count: usize,
    total_items: usize,
) -> Result<Vec<u8>, String> {
    let ids: Vec<usize> = buckets
        .iter()
        .filter_map(|(&bucket, values)| (!values.is_empty()).then_some(bucket))
        .collect();
    let q = ids.len();
    if buckets.values().map(Vec::len).sum::<usize>() != total_items
        || q > bucket_count.min(total_items)
    {
        return Err("invalid LZ literal buckets".into());
    }
    let mut out = Vec::new();
    put_varint(q, &mut out);
    if q > 0 && q < bucket_count {
        out.extend(fixed_bytes(
            &colex_rank(&ids),
            rank_width(choose(bucket_count, q)),
        ));
    }
    let lengths: Vec<usize> = ids.iter().map(|id| buckets[id].len()).collect();
    if q > 1 {
        out.extend(fixed_bytes(
            &composition_rank(&lengths)?,
            rank_width(choose(total_items - 1, q - 1)),
        ));
    }
    for id in ids {
        out.extend(crate::rank::encode_type_class(&buckets[&id])?);
    }
    Ok(out)
}

fn encode_literals(
    parsed: &LzParse,
    source: &[u8],
    prefix_history: &[u8],
    context_models: &[ContextSpec],
    allow_fixed_mix: bool,
) -> Result<(Vec<u8>, String), String> {
    let encoded = crate::rank::encode_type_class(&parsed.literals)?;
    let mut candidates = vec![(
        {
            let mut payload = vec![0];
            payload.extend(encoded);
            payload
        },
        "literal-multinomial".to_string(),
    )];
    let mut local = vec![2];
    local.extend(encode_bytes(&parsed.literals)?);
    candidates.push((local, "literal-local-multinomial".to_string()));

    let max_n = parsed.literals.len().max(256);
    let log_factorial = log_factorials(max_n);
    let mut profiled = Vec::new();
    for &model in context_models {
        let order = model.order as usize;
        let bucket_bits = model.bucket_bits as usize;
        if order > 16 || !(1..=16).contains(&bucket_bits) {
            return Err("invalid LZ literal context model".into());
        }
        let mut buckets = BTreeMap::<usize, Vec<u8>>::new();
        for (&literal, &position) in parsed.literals.iter().zip(&parsed.literal_positions) {
            let tail = literal_context_tail(prefix_history, source, position, order);
            let bucket = context_bucket(&tail, order, bucket_bits);
            buckets.entry(bucket).or_default().push(literal);
        }
        let estimate = 3.0
            + buckets.len() as f64 * 2.0
            + buckets
                .values()
                .map(|values| approximate_typeclass_bytes(values, &log_factorial))
                .sum::<f64>();
        profiled.push((estimate, order, bucket_bits, buckets));
    }
    profiled.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
    });
    for (_, order, bucket_bits, buckets) in profiled.into_iter().take(2) {
        let mut payload = vec![1, order as u8, bucket_bits as u8];
        payload.extend(encode_bucketed(
            &buckets,
            1usize << bucket_bits,
            parsed.literals.len(),
        )?);
        candidates.push((payload, format!("literal-context-o{order}-b{bucket_bits}")));
    }

    if parsed.literals.len() >= 64 {
        let mut estimates = Vec::new();
        for order in 1..=4usize {
            estimates.push((crate::ppm::estimate_bits(&parsed.literals, order)?, order));
        }
        estimates.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));
        let (estimate, order) = estimates[0];
        let current_best = candidates
            .iter()
            .map(|candidate| candidate.0.len())
            .min()
            .unwrap_or(usize::MAX);
        if estimate / 8.0 <= current_best as f64 * 1.08 + 16.0 {
            let mut payload = vec![3];
            payload.extend(crate::ppm::encode_best(
                &parsed.literals,
                order,
                &[],
                None,
                None,
                None,
            )?);
            candidates.push((payload, format!("literal-ppm-o{order}")));
        }
    }
    if allow_fixed_mix && (64..=16384).contains(&parsed.literals.len()) {
        let mut payload = vec![4];
        payload.extend(crate::fixedmix::encode(&parsed.literals));
        candidates.push((payload, "literal-fixed-mix".to_string()));
    }
    candidates
        .into_iter()
        .min_by_key(|candidate| candidate.0.len())
        .ok_or_else(|| "no LZ literal candidates".into())
}

fn encode_distances_legacy(distances: &[usize]) -> Result<Vec<u8>, String> {
    if distances.is_empty() {
        return Ok(Vec::new());
    }
    let mut cache = Vec::<usize>::new();
    let mut symbols = Vec::<u8>::with_capacity(distances.len());
    let mut lows = Vec::<(usize, usize)>::with_capacity(distances.len());
    for &distance in distances {
        if distance == 0 {
            return Err("LZ distances must be positive".into());
        }
        if let Some(index) = cache.iter().position(|&cached| cached == distance) {
            symbols.push(index as u8);
            lows.push((0, 0));
            let cached = cache.remove(index);
            cache.insert(0, cached);
        } else {
            let width = bit_length(distance) - 1;
            symbols.push((4 + width) as u8);
            lows.push((distance - (1usize << width), width));
            cache.insert(0, distance);
            cache.truncate(4);
        }
    }
    let encoded = crate::rank::encode_type_class(&symbols)?;
    let mut out = Vec::new();
    put_varint(encoded.len(), &mut out);
    out.extend(encoded);
    let mut writer = BitWriter::new();
    for (low, width) in lows {
        writer.bits(low, width);
    }
    out.extend(writer.finish());
    Ok(out)
}

fn encode_distance_candidate(distances: &[usize], cache_size: usize) -> Result<Vec<u8>, String> {
    let mut cache = Vec::<usize>::new();
    let mut symbols = Vec::<u8>::with_capacity(distances.len());
    let mut lows = Vec::<(usize, usize)>::with_capacity(distances.len());
    for &distance in distances {
        if distance == 0 {
            return Err("LZ distances must be positive".into());
        }
        if let Some(index) = cache.iter().position(|&cached| cached == distance) {
            symbols.push(index as u8);
            lows.push((0, 0));
            let cached = cache.remove(index);
            cache.insert(0, cached);
        } else {
            let width = bit_length(distance) - 1;
            let symbol = cache_size + width;
            if symbol > 255 {
                return Err("LZ distance class too large".into());
            }
            symbols.push(symbol as u8);
            lows.push((distance - (1usize << width), width));
            if cache_size > 0 {
                cache.insert(0, distance);
                cache.truncate(cache_size);
            }
        }
    }
    let encoded = encode_bytes(&symbols)?;
    let mut candidate = vec![cache_size as u8];
    put_varint(encoded.len(), &mut candidate);
    candidate.extend(encoded);
    let mut writer = BitWriter::new();
    for (low, width) in lows {
        writer.bits(low, width);
    }
    candidate.extend(writer.finish());
    Ok(candidate)
}

fn encode_distances_best(distances: &[usize]) -> Result<Vec<u8>, String> {
    if distances.is_empty() {
        return Ok(vec![0]);
    }
    let mut best: Option<Vec<u8>> = None;
    for cache_size in [0usize, 1, 2, 4, 8, 16] {
        let candidate = encode_distance_candidate(distances, cache_size)?;
        if best
            .as_ref()
            .is_none_or(|incumbent| candidate.len() < incumbent.len())
        {
            best = Some(candidate);
        }
    }
    best.ok_or_else(|| "no LZ distance candidates".into())
}

fn encode_parse(
    parsed: &LzParse,
    source: &[u8],
    prefix_history: &[u8],
    config: &EncodeConfig,
) -> Result<EncodedCandidate, String> {
    if !matches!(config.version, 1 | 3) {
        return Err("LZ encoder version must be 1 or 3".into());
    }
    let tags = if config.version == 1 {
        crate::rank::encode_type_class(&parsed.tags)?
    } else {
        encode_tags(&parsed.tags)?
    };
    let (literals, literal_name) = encode_literals(
        parsed,
        source,
        prefix_history,
        &config.context_models,
        config.allow_fixed_mix,
    )?;
    let lengths = encode_uints(&parsed.lengths)?;
    let distances = if config.version == 1 {
        encode_distances_legacy(&parsed.distances)?
    } else {
        encode_distances_best(&parsed.distances)?
    };
    let mut payload = vec![config.version, config.min_match.max(4) as u8];
    put_varint(parsed.tags.len(), &mut payload);
    put_varint(parsed.literals.len(), &mut payload);
    put_varint(parsed.lengths.len(), &mut payload);
    for stream in [tags, literals, lengths, distances] {
        put_varint(stream.len(), &mut payload);
        payload.extend(stream);
    }
    Ok(EncodedCandidate {
        payload,
        description: format!(
            "lz-{literal_name}-m{}-w{}-c{}{}-v{}",
            config.min_match.max(4),
            config.window,
            config.chain,
            if config.lazy { "-lazy" } else { "" },
            config.version,
        ),
    })
}

/// Encode one fixed, deterministic parser/model recipe.
pub fn encode_with_config(
    data: &[u8],
    prefix_history: &[u8],
    config: &EncodeConfig,
) -> Result<EncodedCandidate, String> {
    if config.min_match > u8::MAX as usize {
        return Err("LZ minimum match exceeds format range".into());
    }
    let parsed = parse_lz(
        data,
        prefix_history,
        config.min_match,
        DEFAULT_MAX_MATCH,
        config.window,
        config.chain,
        config.lazy,
    )?;
    encode_parse(&parsed, data, prefix_history, config)
}

/// Produce a bounded effort portfolio for complete-archive selector comparison.
pub fn encode_candidates(
    data: &[u8],
    prefix_history: &[u8],
    effort: u8,
) -> Result<Vec<EncodedCandidate>, String> {
    if !(1..=9).contains(&effort) {
        return Err("LZ effort must be between 1 and 9".into());
    }
    let chain = if effort <= 2 {
        8
    } else if effort <= 6 {
        32
    } else {
        96
    };
    let contexts = if effort <= 2 {
        Vec::new()
    } else if effort <= 6 {
        vec![
            ContextSpec {
                order: 1,
                bucket_bits: 8,
            },
            ContextSpec {
                order: 2,
                bucket_bits: 10,
            },
        ]
    } else {
        vec![
            ContextSpec {
                order: 1,
                bucket_bits: 8,
            },
            ContextSpec {
                order: 2,
                bucket_bits: 10,
            },
            ContextSpec {
                order: 3,
                bucket_bits: 10,
            },
            ContextSpec {
                order: 4,
                bucket_bits: 12,
            },
        ]
    };
    let mut recipes = vec![(DEFAULT_MIN_MATCH, false)];
    if chain >= 24 && data.len() >= 128 {
        recipes.push((DEFAULT_MIN_MATCH, true));
        recipes.push((5, true));
    }
    if chain >= 96 && data.len() >= 1024 {
        for min_match in [8usize, 16, 32, 48, 64] {
            recipes.push((min_match, true));
        }
    }
    let mut candidates = Vec::with_capacity(recipes.len());
    for (min_match, lazy) in recipes {
        let config = EncodeConfig {
            version: LZ_VERSION,
            window: 1 << 20,
            chain,
            min_match,
            lazy,
            context_models: contexts.clone(),
            allow_fixed_mix: effort >= 7,
        };
        candidates.push(encode_with_config(data, prefix_history, &config)?);
    }
    Ok(candidates)
}

/// Select the smallest physical mode-3 payload from the bounded effort portfolio.
pub fn encode(data: &[u8], prefix_history: &[u8], effort: u8) -> Result<EncodedCandidate, String> {
    encode_candidates(data, prefix_history, effort)?
        .into_iter()
        .min_by_key(|candidate| candidate.payload.len())
        .ok_or_else(|| "no LZ candidates".into())
}

fn decode_bytes(blob: &[u8], count: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    let mode = *blob.first().ok_or("empty LZ byte substream")?;
    let mut pos = 1usize;
    match mode {
        0 => {
            let out = crate::rank::decode_type_class(blob, &mut pos, count)?;
            if pos != blob.len() {
                return Err("trailing static LZ byte-substream bytes".into());
            }
            Ok(out)
        }
        1 => {
            let chunk_size = read_varint(blob, &mut pos)?;
            if chunk_size == 0 {
                return Err("invalid LZ byte-substream chunk size".into());
            }
            let mut out = Vec::with_capacity(count);
            while out.len() < count {
                let n = chunk_size.min(count - out.len());
                out.extend(crate::rank::decode_type_class(blob, &mut pos, n)?);
            }
            if pos != blob.len() {
                return Err("trailing chunked LZ byte-substream bytes".into());
            }
            Ok(out)
        }
        2 => crate::adaptive::decode_general(&blob[1..], count, &[], model_limit),
        3 => {
            if !matches!(blob.get(1), Some(1 | 2)) {
                return Err("invalid context LZ byte-substream order".into());
            }
            crate::adaptive::decode_general(&blob[2..], count, &[], model_limit)
        }
        4 => decode_wavelet(&blob[1..], count),
        5 => decode_histogram_type_class(&blob[1..], count, model_limit),
        other => Err(format!("unsupported LZ byte-substream mode {other}")),
    }
}

fn decode_wavelet_node(
    n: usize,
    depth: usize,
    decoder: &mut crate::rank::ArithmeticDecoder<'_>,
    prefix: u8,
) -> Result<Vec<u8>, String> {
    if n == 0 {
        return Ok(Vec::new());
    }
    if depth == 8 {
        return Ok(vec![prefix; n]);
    }
    let total = n + 1;
    let ones = decoder.target(total);
    if ones > n {
        return Err("LZ wavelet child count out of range".into());
    }
    decoder.update(ones, ones + 1, total);
    let (original_ones, original_zeros) = (ones, n - ones);
    let (mut zero, mut one, mut remaining) = (original_zeros, original_ones, n);
    let mut bits = Vec::with_capacity(n);
    for _ in 0..n {
        let bit = if zero == 0 {
            decoder.update(0, one, remaining);
            one -= 1;
            1
        } else if one == 0 || decoder.target(remaining) < zero {
            decoder.update(0, zero, remaining);
            zero -= 1;
            0
        } else {
            decoder.update(zero, remaining, remaining);
            one -= 1;
            1
        };
        remaining -= 1;
        bits.push(bit);
    }
    let zeros = decode_wavelet_node(original_zeros, depth + 1, decoder, prefix << 1)?;
    let ones = decode_wavelet_node(original_ones, depth + 1, decoder, (prefix << 1) | 1)?;
    let (mut zero_pos, mut one_pos) = (0usize, 0usize);
    let mut out = Vec::with_capacity(n);
    for bit in bits {
        if bit == 0 {
            out.push(zeros[zero_pos]);
            zero_pos += 1;
        } else {
            out.push(ones[one_pos]);
            one_pos += 1;
        }
    }
    Ok(out)
}

fn decode_wavelet(blob: &[u8], count: usize) -> Result<Vec<u8>, String> {
    if blob.first() != Some(&1) {
        return Err("unknown LZ wavelet version".into());
    }
    let mut pos = 1usize;
    let bit_length = read_varint(blob, &mut pos)?;
    if pos.checked_add(bit_length.div_ceil(8)) != Some(blob.len()) {
        return Err("LZ wavelet payload length mismatch".into());
    }
    let mut decoder = crate::rank::ArithmeticDecoder::new(&blob[pos..], bit_length);
    decode_wavelet_node(count, 0, &mut decoder, 0)
}

fn unzig(value: usize) -> i128 {
    let value = value as i128;
    if value & 1 == 0 {
        value / 2
    } else {
        -(value / 2) - 1
    }
}

fn lz_decode_active_values(
    values: Vec<usize>,
    mode: u8,
    total: usize,
    active_count: usize,
) -> Result<Vec<usize>, String> {
    match mode {
        0 => Ok(values
            .into_iter()
            .map(|value| value.saturating_add(1))
            .collect()),
        1 | 4 => {
            let mut previous = 0i128;
            let center = total / active_count;
            values
                .into_iter()
                .map(|value| {
                    let candidate = if mode == 1 {
                        previous + unzig(value)
                    } else {
                        center as i128 + unzig(value)
                    };
                    if candidate <= 0 {
                        return Err(if mode == 1 {
                            "invalid LZ active count delta"
                        } else {
                            "invalid LZ centered active count"
                        }
                        .into());
                    }
                    previous = candidate;
                    Ok(candidate as usize)
                })
                .collect()
        }
        _ => unreachable!(),
    }
}

fn lz_decode_dense_values(
    values: Vec<usize>,
    mode: u8,
    total: usize,
) -> Result<Vec<usize>, String> {
    match mode {
        2 => Ok(values),
        3 | 5 => {
            let mut previous = 0i128;
            let center = total / 256;
            values
                .into_iter()
                .map(|value| {
                    let candidate = if mode == 3 {
                        previous + unzig(value)
                    } else {
                        center as i128 + unzig(value)
                    };
                    if candidate < 0 {
                        return Err(if mode == 3 {
                            "negative LZ dense histogram count"
                        } else {
                            "negative LZ centered histogram count"
                        }
                        .into());
                    }
                    previous = candidate;
                    Ok(candidate as usize)
                })
                .collect()
        }
        _ => unreachable!(),
    }
}

fn decode_histogram_counts(
    blob: &[u8],
    total: usize,
    model_limit: usize,
) -> Result<Vec<usize>, String> {
    if blob.len() < 2 || blob[0] != 1 {
        return Err("unknown LZ count-vector version".into());
    }
    let mode = blob[1];
    let mut pos = 2usize;
    if matches!(mode, 0 | 1 | 4) {
        let active_count = read_varint(blob, &mut pos)?;
        if active_count == 0 || active_count > 256 || active_count > total {
            return Err("invalid LZ active count alphabet".into());
        }
        let active = if active_count == 256 {
            (0..256).collect()
        } else {
            crate::rank::decode_colex_positions(blob, &mut pos, 256, active_count)?
        };
        let mut first = lz_decode_active_values(
            decode_uints(&blob[pos..], active_count - 1, model_limit)?,
            mode,
            total,
            active_count,
        )?;
        let last = total
            .checked_sub(first.iter().sum())
            .ok_or("LZ active histogram exceeds total")?;
        if last == 0 {
            return Err("zero implied LZ active count".into());
        }
        first.push(last);
        let mut counts = vec![0usize; 256];
        for (symbol, count) in active.into_iter().zip(first) {
            counts[symbol] = count;
        }
        return Ok(counts);
    }
    if matches!(mode, 2 | 3 | 5) {
        let mut first =
            lz_decode_dense_values(decode_uints(&blob[pos..], 255, model_limit)?, mode, total)?;
        let last = total
            .checked_sub(first.iter().sum())
            .ok_or("LZ dense histogram exceeds total")?;
        first.push(last);
        return Ok(first);
    }
    Err(format!("unknown LZ count-vector mode {mode}"))
}

fn decode_histogram_type_class(
    blob: &[u8],
    total: usize,
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    let mut pos = 0usize;
    let length = read_varint(blob, &mut pos)?;
    let end = pos
        .checked_add(length)
        .ok_or("LZ histogram length overflow")?;
    if end > blob.len() {
        return Err("truncated LZ histogram".into());
    }
    let counts = decode_histogram_counts(&blob[pos..end], total, model_limit)?;
    crate::rank::decode_type_class_with_counts(&blob[end..], total, &counts)
}

fn decode_uints(blob: &[u8], count: usize, model_limit: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mode = *blob.first().ok_or("empty LZ integer substream")?;
    let mut pos = 1usize;
    match mode {
        0 => decode_uint_varints(blob, count, &mut pos),
        1 => decode_uint_rice(blob, count, &mut pos),
        2 | 3 => decode_uint_classes(blob, count, model_limit, &mut pos, mode),
        other => Err(format!("unsupported LZ integer substream mode {other}")),
    }
}

fn decode_uint_varints(blob: &[u8], count: usize, pos: &mut usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_varint(blob, pos)?);
    }
    if *pos != blob.len() {
        return Err("trailing LZ integer varints".into());
    }
    Ok(out)
}
fn decode_uint_rice(blob: &[u8], count: usize, pos: &mut usize) -> Result<Vec<usize>, String> {
    let k = *blob.get(*pos).ok_or("missing LZ Rice parameter")? as usize;
    *pos += 1;
    if k >= usize::BITS as usize {
        return Err("LZ Rice parameter is too wide".into());
    }
    let mut bits = BitReader {
        data: &blob[*pos..],
        pos: 0,
    };
    (0..count)
        .map(|_| {
            let mut q = 0usize;
            while bits.read(1)? != 0 {
                q = q.checked_add(1).ok_or("LZ Rice quotient overflow")?;
            }
            if q > usize::MAX >> k {
                return Err("LZ Rice value overflow".into());
            }
            Ok(q.checked_shl(k as u32).ok_or("LZ Rice value overflow")? | bits.read(k)?)
        })
        .collect()
}
fn decode_uint_classes(
    blob: &[u8],
    count: usize,
    limit: usize,
    pos: &mut usize,
    mode: u8,
) -> Result<Vec<usize>, String> {
    let n = read_varint(blob, pos)?;
    let end = pos
        .checked_add(n)
        .ok_or("LZ integer class length overflow")?;
    if end > blob.len() {
        return Err("truncated LZ integer class stream".into());
    }
    let c = if mode == 2 {
        let mut p = *pos;
        let v = crate::rank::decode_type_class(blob, &mut p, count)?;
        if p != end {
            return Err("trailing LZ integer class bytes".into());
        }
        v
    } else {
        decode_bytes(&blob[*pos..end], count, limit)?
    };
    let mut b = BitReader {
        data: &blob[end..],
        pos: 0,
    };
    c.into_iter()
        .map(|x| {
            let w = x as usize;
            if w >= usize::BITS as usize {
                return Err("LZ integer class is too wide".into());
            }
            let low = b.read(w)?;
            (1usize << w)
                .checked_add(low)
                .and_then(|v| v.checked_sub(1))
                .ok_or_else(|| "LZ integer value overflow".into())
        })
        .collect()
}

fn decode_tags(blob: &[u8], count: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    let mode = *blob.first().ok_or("empty LZ tag stream")?;
    match mode {
        0 => {
            let mut pos = 1usize;
            let out = crate::rank::decode_type_class(blob, &mut pos, count)?;
            if pos != blob.len() {
                return Err("trailing LZ tag bytes".into());
            }
            Ok(out)
        }
        1 => {
            let first = *blob.get(1).ok_or("truncated LZ run tag stream")?;
            if first > 1 {
                return Err("invalid LZ run tag".into());
            }
            let mut pos = 2usize;
            let run_count = read_varint(blob, &mut pos)?;
            let runs = decode_uints(&blob[pos..], run_count, model_limit)?;
            let mut out = Vec::with_capacity(count);
            let mut tag = first;
            for encoded_length in runs {
                let run_length = encoded_length
                    .checked_add(1)
                    .ok_or("LZ tag run length overflow")?;
                if out.len().saturating_add(run_length) > count {
                    return Err("LZ run tags exceed token count".into());
                }
                out.resize(out.len() + run_length, tag);
                tag ^= 1;
            }
            if out.len() != count {
                return Err("LZ run tags length mismatch".into());
            }
            Ok(out)
        }
        2 => decode_bytes(&blob[1..], count, model_limit),
        other => Err(format!("unsupported LZ tag mode {other}")),
    }
}

fn decode_bucketed(
    blob: &[u8],
    pos: &mut usize,
    bucket_count: usize,
    total_items: usize,
) -> Result<HashMap<usize, Vec<u8>>, String> {
    let q = read_varint(blob, pos)?;
    if q > bucket_count.min(total_items) {
        return Err("invalid active LZ literal bucket count".into());
    }
    if q == 0 {
        return if total_items == 0 {
            Ok(HashMap::new())
        } else {
            Err("non-empty LZ literal stream has no active buckets".into())
        };
    }
    let ids = if q == bucket_count {
        (0..bucket_count).collect::<Vec<_>>()
    } else {
        crate::rank::decode_colex_positions(blob, pos, bucket_count, q)?
    };
    let lengths = if q == 1 {
        vec![total_items]
    } else {
        let cuts = crate::rank::decode_colex_positions(blob, pos, total_items - 1, q - 1)?;
        let mut lengths = Vec::with_capacity(q);
        let mut previous = 0usize;
        for cut in cuts {
            let boundary = cut + 1;
            lengths.push(boundary - previous);
            previous = boundary;
        }
        lengths.push(total_items - previous);
        lengths
    };
    let mut streams = HashMap::with_capacity(q);
    for (id, length) in ids.into_iter().zip(lengths) {
        streams.insert(id, crate::rank::decode_type_class(blob, pos, length)?);
    }
    Ok(streams)
}

struct LiteralDecoder {
    plain: Option<Vec<u8>>,
    plain_pos: usize,
    lanes: HashMap<usize, Vec<u8>>,
    lane_pos: HashMap<usize, usize>,
    order: usize,
    bucket_bits: usize,
}

impl LiteralDecoder {
    fn plain(values: Vec<u8>) -> Self {
        Self {
            plain: Some(values),
            plain_pos: 0,
            lanes: HashMap::new(),
            lane_pos: HashMap::new(),
            order: 0,
            bucket_bits: 0,
        }
    }

    fn next(&mut self, prefix: &[u8], produced: &[u8]) -> Result<u8, String> {
        if let Some(values) = &self.plain {
            let value = *values
                .get(self.plain_pos)
                .ok_or("LZ literal stream exhausted")?;
            self.plain_pos += 1;
            return Ok(value);
        }
        if produced.len() >= self.order {
            self.next_for_tail(&produced[produced.len() - self.order..])
        } else {
            let missing = self.order - produced.len();
            let start = prefix.len().saturating_sub(missing);
            let mut joined = Vec::with_capacity(prefix.len() - start + produced.len());
            joined.extend_from_slice(&prefix[start..]);
            joined.extend_from_slice(produced);
            self.next_for_tail(&joined)
        }
    }

    fn next_for_tail(&mut self, tail: &[u8]) -> Result<u8, String> {
        let bucket = if self.order == 1 && self.bucket_bits >= 8 {
            tail.last().copied().unwrap_or(0) as usize
        } else {
            let mut hash = 1469598103934665603u64;
            for &byte in tail {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(1099511628211);
            }
            (hash & ((1u64 << self.bucket_bits) - 1)) as usize
        };
        let values = self
            .lanes
            .get(&bucket)
            .ok_or("LZ literal context selected absent bucket")?;
        let cursor = self
            .lane_pos
            .get_mut(&bucket)
            .ok_or("missing LZ literal bucket cursor")?;
        let value = *values
            .get(*cursor)
            .ok_or("LZ literal context bucket exhausted")?;
        *cursor += 1;
        Ok(value)
    }

    fn finish(&self, expected: usize) -> Result<(), String> {
        if let Some(values) = &self.plain {
            if self.plain_pos != expected || values.len() != expected {
                return Err("unused LZ literal symbols".into());
            }
        } else if self
            .lanes
            .iter()
            .any(|(bucket, values)| self.lane_pos.get(bucket).copied() != Some(values.len()))
        {
            return Err("unused LZ literal-context symbols".into());
        }
        Ok(())
    }
}

fn decode_literals(
    blob: &[u8],
    count: usize,
    model_limit: usize,
) -> Result<LiteralDecoder, String> {
    let mode = *blob.first().ok_or("empty LZ literal stream")?;
    match mode {
        0 => {
            let mut pos = 1usize;
            let values = crate::rank::decode_type_class(blob, &mut pos, count)?;
            if pos != blob.len() {
                return Err("trailing LZ literal bytes".into());
            }
            Ok(LiteralDecoder::plain(values))
        }
        1 => {
            if blob.len() < 3 {
                return Err("truncated LZ literal context parameters".into());
            }
            let order = blob[1] as usize;
            let bucket_bits = blob[2] as usize;
            if order > 16 || !(1..=16).contains(&bucket_bits) {
                return Err("invalid LZ literal context parameters".into());
            }
            let mut pos = 3usize;
            let lanes = decode_bucketed(blob, &mut pos, 1usize << bucket_bits, count)?;
            if pos != blob.len() {
                return Err("trailing LZ literal-context bytes".into());
            }
            let lane_pos = lanes.keys().map(|&bucket| (bucket, 0usize)).collect();
            Ok(LiteralDecoder {
                plain: None,
                plain_pos: 0,
                lanes,
                lane_pos,
                order,
                bucket_bits,
            })
        }
        2 => Ok(LiteralDecoder::plain(decode_bytes(
            &blob[1..],
            count,
            model_limit,
        )?)),
        3 => Ok(LiteralDecoder::plain(crate::ppm::decode(
            &blob[1..],
            count,
        )?)),
        4 => Ok(LiteralDecoder::plain(crate::fixedmix::decode(
            &blob[1..],
            count,
        )?)),
        other => Err(format!("unsupported LZ literal mode {other}")),
    }
}

fn decode_distances_legacy(blob: &[u8], count: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut pos = 0usize;
    let symbol_len = read_varint(blob, &mut pos)?;
    let end = pos
        .checked_add(symbol_len)
        .ok_or("LZ distance class length overflow")?;
    if end > blob.len() {
        return Err("truncated LZ distance classes".into());
    }
    let mut symbol_pos = pos;
    let symbols = crate::rank::decode_type_class(blob, &mut symbol_pos, count)?;
    if symbol_pos != end {
        return Err("trailing LZ distance class bytes".into());
    }
    decode_distance_symbols(&symbols, &blob[end..], 4)
}

fn decode_distances_best(
    blob: &[u8],
    count: usize,
    model_limit: usize,
) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let cache_size = *blob.first().ok_or("empty adaptive LZ distance stream")? as usize;
    let mut pos = 1usize;
    let symbol_len = read_varint(blob, &mut pos)?;
    let end = pos
        .checked_add(symbol_len)
        .ok_or("LZ distance symbol length overflow")?;
    if end > blob.len() {
        return Err("truncated adaptive LZ distance classes".into());
    }
    let symbols = decode_bytes(&blob[pos..end], count, model_limit)?;
    decode_distance_symbols(&symbols, &blob[end..], cache_size)
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
                return Err("invalid LZ distance cache reference".into());
            }
            let distance = cache.remove(symbol);
            cache.insert(0, distance);
            distance
        } else {
            let width = symbol - cache_size;
            if width >= usize::BITS as usize {
                return Err("LZ distance class is too wide".into());
            }
            let low = reader.read(width)?;
            let distance = (1usize << width)
                .checked_add(low)
                .ok_or("LZ distance overflow")?;
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

/// Decode a frozen Python CIXM6 mode-3 payload.
///
/// `prefix_history` is already reconstructed stream history visible to the
/// block. `model_limit` bounds adaptive substream context tables.
fn decode_streams<'a>(blob: &'a [u8], pos: &mut usize) -> Result<Vec<&'a [u8]>, String> {
    let mut streams = Vec::with_capacity(4);
    for _ in 0..4 {
        let length = read_varint(blob, pos)?;
        let end = pos
            .checked_add(length)
            .ok_or("LZ substream length overflow")?;
        if end > blob.len() {
            return Err("truncated LZ substream".into());
        }
        streams.push(&blob[*pos..end]);
        *pos = end;
    }
    if *pos != blob.len() {
        return Err("trailing LZ block bytes".into());
    }
    Ok(streams)
}

fn decode_version_tags(
    version: u8,
    stream: &[u8],
    count: usize,
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    if version != 1 {
        return decode_tags(stream, count, model_limit);
    }
    let mut pos = 0;
    let tags = crate::rank::decode_type_class(stream, &mut pos, count)?;
    if pos != stream.len() {
        return Err("trailing version-1 LZ tag bytes".into());
    }
    Ok(tags)
}

fn decode_match(
    produced: &mut Vec<u8>,
    prefix_history: &[u8],
    distance: usize,
    length: usize,
    source_length: usize,
) -> Result<(), String> {
    if distance == 0
        || distance > prefix_history.len().saturating_add(produced.len())
        || produced
            .len()
            .checked_add(length)
            .is_none_or(|end| end > source_length)
    {
        return Err("invalid LZ match descriptor".into());
    }
    for _ in 0..length {
        let source = prefix_history.len() + produced.len() - distance;
        let value = if source < prefix_history.len() {
            prefix_history[source]
        } else {
            produced[source - prefix_history.len()]
        };
        produced.push(value);
    }
    Ok(())
}

struct ReconstructionContext<'a> {
    min_match: usize,
    source_length: usize,
    prefix_history: &'a [u8],
    literal_count: usize,
    match_count: usize,
}

fn reconstruct_tokens(
    tags: Vec<u8>,
    mut literals: LiteralDecoder,
    lengths: Vec<usize>,
    distances: Vec<usize>,
    context: ReconstructionContext<'_>,
) -> Result<Vec<u8>, String> {
    let mut produced = Vec::with_capacity(context.source_length);
    let mut literal_used = 0usize;
    let mut match_used = 0usize;
    for tag in tags {
        match tag {
            0 => {
                produced.push(literals.next(context.prefix_history, &produced)?);
                literal_used += 1;
            }
            1 => {
                let encoded_length = *lengths
                    .get(match_used)
                    .ok_or("LZ match-length stream exhausted")?;
                let distance = *distances
                    .get(match_used)
                    .ok_or("LZ distance stream exhausted")?;
                match_used += 1;
                let length = encoded_length
                    .checked_add(context.min_match)
                    .ok_or("LZ match length overflow")?;
                decode_match(
                    &mut produced,
                    context.prefix_history,
                    distance,
                    length,
                    context.source_length,
                )?;
            }
            _ => return Err("invalid LZ tag".into()),
        }
    }
    if produced.len() != context.source_length
        || literal_used != context.literal_count
        || match_used != context.match_count
    {
        return Err("LZ decoded length or token count mismatch".into());
    }
    literals.finish(context.literal_count)?;
    Ok(produced)
}

pub fn decode(
    blob: &[u8],
    source_length: usize,
    prefix_history: &[u8],
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    if blob.len() < 2 || !matches!(blob[0], 1 | 3) {
        return Err("unknown LZ block version".into());
    }
    let version = blob[0];
    let min_match = blob[1] as usize;
    let mut pos = 2usize;
    let token_count = read_varint(blob, &mut pos)?;
    let literal_count = read_varint(blob, &mut pos)?;
    let match_count = read_varint(blob, &mut pos)?;
    if token_count > source_length
        || literal_count > token_count
        || match_count > token_count
        || literal_count.saturating_add(match_count) != token_count
    {
        return Err("invalid LZ token counts".into());
    }
    let streams = decode_streams(blob, &mut pos)?;
    let tags = decode_version_tags(version, streams[0], token_count, model_limit)?;
    let literals = decode_literals(streams[1], literal_count, model_limit)?;
    let lengths = decode_uints(streams[2], match_count, model_limit)?;
    let distances = if version >= 3 {
        decode_distances_best(streams[3], match_count, model_limit)?
    } else {
        decode_distances_legacy(streams[3], match_count)?
    };

    reconstruct_tokens(
        tags,
        literals,
        lengths,
        distances,
        ReconstructionContext {
            min_match,
            source_length,
            prefix_history,
            literal_count,
            match_count,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{decode, encode_with_config, EncodeConfig};

    #[test]
    fn rejects_unknown_version() {
        assert!(decode(&[2, 4], 1, &[], 1024).is_err());
    }

    #[test]
    fn rejects_inconsistent_counts_before_stream_allocation() {
        assert!(decode(&[3, 4, 2, 2, 1], 2, &[], 1024).is_err());
    }

    #[test]
    fn legacy_round_trip_after_helper_extraction() {
        let history = b"dictionary prefix ";
        let source = b"dictionary prefix dictionary prefix dictionary prefix payload";
        let payload = encode_with_config(source, history, &EncodeConfig::default())
            .unwrap()
            .payload;
        assert_eq!(
            decode(&payload, source.len(), history, 1 << 16).unwrap(),
            source
        );
    }
}

#[cfg(test)]
mod integer_overflow_tests {
    use super::*;
    #[test]
    fn varint_accepts_max_rejects_high_bits() {
        let mut value = usize::MAX;
        let mut encoded = Vec::new();
        while value >= 128 {
            encoded.push((value as u8 & 127) | 128);
            value >>= 7;
        }
        encoded.push(value as u8);
        assert_eq!(read_varint(&encoded, &mut 0).unwrap(), usize::MAX);
        let groups = (usize::BITS as usize).div_ceil(7);
        let mut malformed = vec![128; groups - 1];
        malformed.push(1u8 << ((usize::BITS as usize - 1) % 7 + 1));
        assert!(read_varint(&malformed, &mut 0).is_err());
    }
}

#[cfg(test)]
mod rice_overflow_tests {
    use super::*;
    fn payload(q: usize, low: usize) -> Vec<u8> {
        let width = usize::BITS as usize - 1;
        let mut writer = BitWriter::new();
        for _ in 0..q {
            writer.bit(1);
        }
        writer.bit(0);
        writer.bits(low, width);
        let mut blob = vec![1, width as u8];
        blob.extend(writer.finish());
        blob
    }
    #[test]
    fn rice_accepts_max_rejects_high_bits() {
        assert_eq!(
            decode_uints(&payload(1, usize::MAX >> 1), 1, 1 << 20).unwrap(),
            vec![usize::MAX]
        );
        assert!(decode_uints(&payload(2, 0), 1, 1 << 20).is_err());
        assert!(decode_uints(&[1, usize::BITS as u8], 1, 1 << 20).is_err());
    }
}
