//! Frozen CIX numeric word transforms (CIXM6 mode 8).
use super::{adaptive, histogram, wavelet};
use crate::rank::{self, ArithmeticEncoder};
use num_bigint::BigUint;
use num_traits::{One, ToPrimitive, Zero};
use std::collections::{BTreeSet, HashMap};

/// One independently comparable mode-8 payload candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedCandidate {
    pub payload: Vec<u8>,
    pub description: String,
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
    fn write(&mut self, value: u64, width: usize) {
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

fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_varint_u64(mut value: u64, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn get_varint_u64(data: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated numeric varint")?;
        *pos += 1;
        let low = (byte & 0x7f) as u64;
        if shift == 63 && low > 1 {
            return Err("numeric varint overflow".into());
        }
        value |= low
            .checked_shl(shift as u32)
            .ok_or("numeric varint overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized numeric varint".into())
}

fn get_varint(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    usize::try_from(get_varint_u64(data, pos)?)
        .map_err(|_| "numeric varint exceeds host width".into())
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

fn fixed_width(value: &BigUint) -> usize {
    value.bits() as usize
}

fn fixed_bytes(value: &BigUint, width: usize) -> Vec<u8> {
    let size = width.div_ceil(8);
    if size == 0 {
        return Vec::new();
    }
    let bytes = value.to_bytes_be();
    let mut out = vec![0; size.saturating_sub(bytes.len())];
    out.extend_from_slice(&bytes);
    out
}

fn colex_rank(values: &[usize]) -> BigUint {
    values
        .iter()
        .enumerate()
        .fold(BigUint::zero(), |sum, (index, &value)| {
            sum + choose(value, index + 1)
        })
}

fn bit_width(value: u64) -> usize {
    (u64::BITS - value.leading_zeros()) as usize
}

#[derive(Clone)]
struct LocalModel {
    counts: [u32; 256],
    seen: u32,
}

impl LocalModel {
    fn new() -> Self {
        Self {
            counts: [0; 256],
            seen: 0,
        }
    }
    fn interval(&self, symbol: usize) -> (usize, usize, usize) {
        let before: u32 = self.counts[..symbol].iter().sum();
        let low = 2 * before as usize + symbol;
        (
            low,
            low + 2 * self.counts[symbol] as usize + 1,
            2 * self.seen as usize + 256,
        )
    }
    fn add(&mut self, symbol: usize) {
        self.counts[symbol] += 1;
        self.seen += 1;
    }
}

fn context_bucket(tail: &[u8], order: usize, bucket_bits: usize) -> usize {
    if order == 0 {
        return 0;
    }
    if order == 1 && bucket_bits >= 8 {
        return tail.last().copied().unwrap_or(0) as usize;
    }
    let mut hash = 1469598103934665603u64;
    for &byte in tail {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    (hash & ((1u64 << bucket_bits) - 1)) as usize
}

fn encode_adaptive(data: &[u8], order: usize, bucket_bits: usize) -> Vec<u8> {
    let mut models = HashMap::<usize, LocalModel>::new();
    let mut tail = Vec::with_capacity(order);
    let mut encoder = ArithmeticEncoder::new();
    for &symbol in data {
        let bucket = context_bucket(&tail, order, bucket_bits);
        let model = models.entry(bucket).or_insert_with(LocalModel::new);
        let (low, high, total) = model.interval(symbol as usize);
        encoder.encode(low, high, total);
        model.add(symbol as usize);
        if order > 0 {
            tail.push(symbol);
            if tail.len() > order {
                tail.remove(0);
            }
        }
    }
    let (payload, bit_length) = encoder.finish();
    let mut out = vec![1, order as u8, bucket_bits as u8];
    put_varint(bit_length, &mut out);
    out.extend(payload);
    out
}

fn conditional_bits(data: &[u8], order: usize) -> f64 {
    let mut contexts = HashMap::<Vec<u8>, [usize; 256]>::new();
    let mut history = Vec::with_capacity(order);
    for &value in data {
        let counts = contexts.entry(history.clone()).or_insert([0; 256]);
        counts[value as usize] += 1;
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
            let n: usize = counts.iter().sum();
            n as f64 * (n as f64).log2()
                - counts
                    .iter()
                    .filter(|&&count| count > 0)
                    .map(|&count| count as f64 * (count as f64).log2())
                    .sum::<f64>()
        })
        .sum()
}

fn encode_wavelet_node(data: &[u8], depth: usize, encoder: &mut ArithmeticEncoder) {
    if data.is_empty() || depth == 8 {
        return;
    }
    let shift = 7 - depth;
    let ones = data
        .iter()
        .filter(|&&value| ((value >> shift) & 1) != 0)
        .count();
    encoder.encode(ones, ones + 1, data.len() + 1);
    let mut zero = data.len() - ones;
    let mut remaining = data.len();
    for &value in data {
        if ((value >> shift) & 1) != 0 {
            encoder.encode(zero, remaining, remaining);
        } else {
            encoder.encode(0, zero, remaining);
            zero -= 1;
        }
        remaining -= 1;
    }
    if ones == 0 || ones == data.len() {
        encode_wavelet_node(data, depth + 1, encoder);
    } else {
        let zeros: Vec<u8> = data
            .iter()
            .copied()
            .filter(|value| ((value >> shift) & 1) == 0)
            .collect();
        let ones: Vec<u8> = data
            .iter()
            .copied()
            .filter(|value| ((value >> shift) & 1) != 0)
            .collect();
        encode_wavelet_node(&zeros, depth + 1, encoder);
        encode_wavelet_node(&ones, depth + 1, encoder);
    }
}

fn encode_wavelet(data: &[u8]) -> Vec<u8> {
    let mut encoder = ArithmeticEncoder::new();
    encode_wavelet_node(data, 0, &mut encoder);
    let (payload, bit_length) = encoder.finish();
    let mut out = vec![1];
    put_varint(bit_length, &mut out);
    out.extend(payload);
    out
}

fn big_log2(value: &BigUint) -> f64 {
    let bits = value.bits() as usize;
    if bits <= 53 {
        return (value.to_u64().unwrap_or(1) as f64).log2();
    }
    let shift = bits - 53;
    let top = (value >> shift).to_u64().unwrap();
    (top as f64).log2() + shift as f64
}

fn wavelet_estimate(data: &[u8]) -> f64 {
    fn node(counts: &[usize; 256], lo: usize, hi: usize, depth: usize) -> f64 {
        let n: usize = counts[lo..hi].iter().sum();
        if n == 0 || depth == 8 {
            return 0.0;
        }
        let mid = (lo + hi) / 2;
        let ones: usize = counts[mid..hi].iter().sum();
        let arrangement = if ones == 0 || ones == n {
            0.0
        } else {
            big_log2(&choose(n, ones))
        };
        (n as f64 + 1.0).log2()
            + arrangement
            + node(counts, lo, mid, depth + 1)
            + node(counts, mid, hi, depth + 1)
    }
    let mut counts = [0usize; 256];
    for &value in data {
        counts[value as usize] += 1;
    }
    node(&counts, 0, 256, 0) + 24.0
}

fn zig_usize(value: i128) -> Result<usize, String> {
    let encoded = if value >= 0 {
        (value as u128)
            .checked_mul(2)
            .ok_or("numeric integer zigzag overflow")?
    } else {
        value
            .unsigned_abs()
            .checked_mul(2)
            .and_then(|v| v.checked_sub(1))
            .ok_or("numeric integer zigzag overflow")?
    };
    usize::try_from(encoded).map_err(|_| "numeric integer exceeds host width".into())
}

fn active_prefix(active: &[usize]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(active.len(), &mut out);
    if active.len() < 256 {
        let width = fixed_width(&(choose(256, active.len()) - BigUint::one()));
        out.extend(fixed_bytes(&colex_rank(active), width));
    }
    out
}

fn encode_count_vector(counts: &[usize; 256]) -> Result<Vec<u8>, String> {
    let total: usize = counts.iter().sum();
    if total == 0 {
        return Err("numeric count vector is empty".into());
    }
    let active: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(symbol, &count)| (count > 0).then_some(symbol))
        .collect();
    let active_counts: Vec<usize> = active.iter().map(|&symbol| counts[symbol]).collect();
    let prefix = active_prefix(&active);
    let mut candidates = Vec::<(&'static str, Vec<u8>)>::new();

    let active_uint: Vec<usize> = active_counts[..active_counts.len() - 1]
        .iter()
        .map(|&count| count - 1)
        .collect();
    let mut payload = vec![1, 0];
    payload.extend_from_slice(&prefix);
    payload.extend(encode_uints(&active_uint)?);
    candidates.push(("active-uint", payload));

    let mut previous = 0i128;
    let mut values = Vec::with_capacity(active_counts.len().saturating_sub(1));
    for &count in &active_counts[..active_counts.len() - 1] {
        values.push(zig_usize(count as i128 - previous)?);
        previous = count as i128;
    }
    let mut payload = vec![1, 1];
    payload.extend_from_slice(&prefix);
    payload.extend(encode_uints(&values)?);
    candidates.push(("active-delta", payload));

    let dense = &counts[..255];
    let mut payload = vec![1, 2];
    payload.extend(encode_uints(dense)?);
    candidates.push(("dense-uint", payload));

    previous = 0;
    values.clear();
    for &count in dense {
        values.push(zig_usize(count as i128 - previous)?);
        previous = count as i128;
    }
    let mut payload = vec![1, 3];
    payload.extend(encode_uints(&values)?);
    candidates.push(("dense-delta", payload));

    let center = total / active_counts.len();
    values.clear();
    for &count in &active_counts[..active_counts.len() - 1] {
        values.push(zig_usize(count as i128 - center as i128)?);
    }
    let mut payload = vec![1, 4];
    payload.extend_from_slice(&prefix);
    payload.extend(encode_uints(&values)?);
    candidates.push(("active-centered", payload));

    let center = total / 256;
    values.clear();
    for &count in dense {
        values.push(zig_usize(count as i128 - center as i128)?);
    }
    let mut payload = vec![1, 5];
    payload.extend(encode_uints(&values)?);
    candidates.push(("dense-centered", payload));

    candidates.sort_by(|left, right| (left.1.len(), left.0).cmp(&(right.1.len(), right.0)));
    Ok(candidates.remove(0).1)
}

fn histogram_candidate(data: &[u8], incumbent: &[u8]) -> Result<Option<Vec<u8>>, String> {
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
    let mut varint = Vec::new();
    put_varint(active.len(), &mut varint);
    let mut header_size = varint.len();
    if active.len() < 256 {
        header_size += fixed_width(&(choose(256, active.len()) - BigUint::one())).div_ceil(8);
    }
    header_size +=
        fixed_width(&(choose(data.len() - 1, active.len() - 1) - BigUint::one())).div_ceil(8);
    if header_size < 64 || header_size > incumbent.len() {
        return Ok(None);
    }
    let histogram = encode_count_vector(&counts)?;
    let mut prefix = Vec::new();
    put_varint(histogram.len(), &mut prefix);
    prefix.extend(histogram);
    if prefix.len() >= header_size {
        return Ok(None);
    }
    prefix.extend_from_slice(&incumbent[header_size..]);
    Ok(Some(prefix))
}

fn keep_smaller(best: &mut Vec<u8>, candidate: Vec<u8>) {
    if candidate.len() < best.len() {
        *best = candidate;
    }
}

fn consider_chunked_bytes(data: &[u8], best: &mut Vec<u8>) -> Result<(), String> {
    for chunk_size in [128usize, 256, 512, 1024, 2048, 4096] {
        if data.len() >= chunk_size * 2 {
            let mut candidate = vec![1];
            put_varint(chunk_size, &mut candidate);
            for chunk in data.chunks(chunk_size) {
                candidate.extend(rank::encode_type_class(chunk)?);
            }
            keep_smaller(best, candidate);
        }
    }
    Ok(())
}

fn consider_adaptive_bytes(data: &[u8], best: &mut Vec<u8>) {
    if data.len() < 256 {
        return;
    }
    keep_smaller(best, [vec![2], encode_adaptive(data, 0, 8)].concat());
    let orders: &[usize] = if data.len() >= 2048 { &[1, 2] } else { &[1] };
    for &order in orders {
        if conditional_bits(data, order) / 8.0 <= best.len() as f64 * 1.10 + 16.0 {
            let mut candidate = vec![3, order as u8];
            candidate.extend(encode_adaptive(
                data,
                order,
                if order == 1 { 8 } else { 10 },
            ));
            keep_smaller(best, candidate);
        }
    }
}

fn encode_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let static_blob = rank::encode_type_class(data)?;
    let mut best = vec![0];
    best.extend_from_slice(&static_blob);
    if data.is_empty() {
        return Ok(best);
    }

    consider_chunked_bytes(data, &mut best)?;

    if data.len() >= 64 && wavelet_estimate(data) / 8.0 <= best.len() as f64 * 1.03 + 8.0 {
        let mut candidate = vec![4];
        candidate.extend(encode_wavelet(data));
        keep_smaller(&mut best, candidate);
    }

    consider_adaptive_bytes(data, &mut best);

    if let Some(histogram) = histogram_candidate(data, &static_blob)? {
        let mut candidate = vec![5];
        candidate.extend(histogram);
        keep_smaller(&mut best, candidate);
    }
    Ok(best)
}

fn rice_candidate(values: &[usize], width: usize, best_len: usize) -> Option<Vec<u8>> {
    let payload_limit = best_len.saturating_sub(2).saturating_mul(8);
    let mut cost = 0usize;
    for &value in values {
        cost = cost.saturating_add((value >> width).saturating_add(1 + width));
        if cost >= payload_limit {
            return None;
        }
    }
    let mut writer = BitWriter::new();
    for &value in values {
        for _ in 0..(value >> width) {
            writer.bit(1);
        }
        writer.bit(0);
        if width > 0 {
            writer.write((value & ((1usize << width) - 1)) as u64, width);
        }
    }
    let mut candidate = vec![1, width as u8];
    candidate.extend(writer.finish());
    Some(candidate)
}

fn encode_uints(values: &[usize]) -> Result<Vec<u8>, String> {
    if values.is_empty() {
        return Ok(vec![0]);
    }
    let mut best = vec![0];
    for &value in values {
        put_varint(value, &mut best);
    }
    for width in 0..=16usize {
        if let Some(candidate) = rice_candidate(values, width, best.len()) {
            keep_smaller(&mut best, candidate);
        }
    }

    let mut classes = Vec::with_capacity(values.len());
    let mut lows = Vec::with_capacity(values.len());
    for &value in values {
        let Some(shifted) = value.checked_add(1) else {
            // The class representation stores value + 1; mode 0 already
            // represents usize::MAX exactly, so only class modes are ineligible.
            return Ok(best);
        };
        let width = (usize::BITS - shifted.leading_zeros() - 1) as usize;
        if width > 255 {
            return Ok(best);
        }
        classes.push(width as u8);
        lows.push((shifted - (1usize << width), width));
    }
    let mut writer = BitWriter::new();
    for &(low, width) in &lows {
        writer.write(low as u64, width);
    }
    let low_blob = writer.finish();

    let class_blob = rank::encode_type_class(&classes)?;
    let mut candidate = vec![2];
    put_varint(class_blob.len(), &mut candidate);
    candidate.extend(class_blob);
    candidate.extend_from_slice(&low_blob);
    keep_smaller(&mut best, candidate);

    let class_blob = encode_bytes(&classes)?;
    let mut candidate = vec![3];
    put_varint(class_blob.len(), &mut candidate);
    candidate.extend(class_blob);
    candidate.extend(low_blob);
    keep_smaller(&mut best, candidate);
    Ok(best)
}

fn split_words(data: &[u8], width: usize, big: bool) -> (Vec<u64>, &[u8]) {
    let usable = data.len() / width * width;
    let values = data[..usable]
        .chunks_exact(width)
        .map(|word| {
            let mut value = 0u64;
            if big {
                for &byte in word {
                    value = (value << 8) | byte as u64;
                }
            } else {
                for (index, &byte) in word.iter().enumerate() {
                    value |= (byte as u64) << (8 * index);
                }
            }
            value
        })
        .collect();
    (values, &data[usable..])
}

fn word_code(width: usize, big: bool) -> u8 {
    let width_code = match width {
        2 => 0,
        4 => 1,
        8 => 2,
        _ => unreachable!(),
    };
    width_code * 2 + u8::from(big)
}

fn pack(values: &[u64], width: usize) -> Vec<u8> {
    let mut writer = BitWriter::new();
    for &value in values {
        writer.write(value, width);
    }
    writer.finish()
}

fn signed_mod(value: i128, bits: usize) -> i128 {
    let modulus = 1i128 << bits;
    let wrapped = value.rem_euclid(modulus);
    if wrapped < modulus / 2 {
        wrapped
    } else {
        wrapped - modulus
    }
}

fn zig_u64(value: i128) -> u64 {
    if value >= 0 {
        (value as u64) << 1
    } else {
        (value.unsigned_abs() as u64)
            .wrapping_shl(1)
            .wrapping_sub(1)
    }
}

fn encode_for(values: &[u64], tail: &[u8], width: usize, big: bool) -> Vec<u8> {
    let base = *values.iter().min().unwrap();
    let residuals: Vec<u64> = values.iter().map(|&value| value - base).collect();
    let bits = residuals
        .iter()
        .map(|&value| bit_width(value))
        .max()
        .unwrap_or(0);
    let mut out = vec![1, 1, word_code(width, big), bits as u8];
    push_word(&mut out, base, width, big);
    out.extend(pack(&residuals, bits));
    out.extend_from_slice(tail);
    out
}

fn delta_values(values: &[u64], width: usize) -> Vec<u64> {
    let bits = width * 8;
    values
        .windows(2)
        .map(|pair| zig_u64(signed_mod(pair[1] as i128 - pair[0] as i128, bits)))
        .collect()
}

fn encode_delta(values: &[u64], tail: &[u8], width: usize, big: bool) -> Vec<u8> {
    let residuals = delta_values(values, width);
    let bits = residuals
        .iter()
        .map(|&value| bit_width(value))
        .max()
        .unwrap_or(0);
    let mut out = vec![1, 2, word_code(width, big), bits as u8];
    push_word(&mut out, values[0], width, big);
    out.extend(pack(&residuals, bits));
    out.extend_from_slice(tail);
    out
}

fn encode_delta2(values: &[u64], tail: &[u8], width: usize, big: bool) -> Vec<u8> {
    let bits = width * 8;
    let deltas: Vec<i128> = values
        .windows(2)
        .map(|pair| signed_mod(pair[1] as i128 - pair[0] as i128, bits))
        .collect();
    let first_delta = deltas.first().copied().unwrap_or(0);
    let residuals: Vec<u64> = deltas
        .windows(2)
        .map(|pair| zig_u64(signed_mod(pair[1] - pair[0], bits)))
        .collect();
    let packed_width = residuals
        .iter()
        .map(|&value| bit_width(value))
        .max()
        .unwrap_or(0);
    let mut out = vec![1, 3, word_code(width, big), packed_width as u8];
    push_word(&mut out, values[0], width, big);
    put_varint_u64(zig_u64(first_delta), &mut out);
    out.extend(pack(&residuals, packed_width));
    out.extend_from_slice(tail);
    out
}

fn encode_pfor(
    values: &[u64],
    tail: &[u8],
    width: usize,
    big: bool,
    low_width: usize,
) -> Result<Vec<u8>, String> {
    let base = *values.iter().min().unwrap();
    let mask = if low_width == 0 {
        0
    } else {
        (1u64 << low_width) - 1
    };
    let residuals: Vec<u64> = values.iter().map(|&value| value - base).collect();
    let positions: Vec<usize> = residuals
        .iter()
        .enumerate()
        .filter_map(|(index, &value)| ((value >> low_width) != 0).then_some(index))
        .collect();
    let highs: Vec<usize> = residuals
        .iter()
        .filter_map(|&value| {
            let high = value >> low_width;
            (high != 0).then(|| usize::try_from(high))
        })
        .collect::<Result<_, _>>()
        .map_err(|_| "numeric PFOR exception exceeds host width")?;
    let lows: Vec<u64> = residuals.iter().map(|&value| value & mask).collect();
    let position_width = fixed_width(&(choose(values.len(), positions.len()) - BigUint::one()));
    let high_blob = encode_uints(&highs)?;
    let mut out = vec![1, 4, word_code(width, big), low_width as u8];
    push_word(&mut out, base, width, big);
    put_varint(positions.len(), &mut out);
    out.extend(fixed_bytes(&colex_rank(&positions), position_width));
    put_varint(high_blob.len(), &mut out);
    out.extend(high_blob);
    out.extend(pack(&lows, low_width));
    out.extend_from_slice(tail);
    Ok(out)
}

/// Materialize the frozen reference's complete ordered mode-8 candidate set.
fn pfor_widths(values: &[u64]) -> BTreeSet<usize> {
    let base = *values.iter().min().unwrap();
    let mut widths: Vec<usize> = values
        .iter()
        .map(|&value| bit_width(value - base))
        .collect();
    widths.sort_unstable();
    let mut choices = BTreeSet::new();
    choices.insert(0usize);
    for fraction in [0.90f64, 0.95, 0.99] {
        let index = ((widths.len() as f64 * fraction) as usize).min(widths.len() - 1);
        choices.insert(widths[index]);
    }
    choices.insert(*widths.last().unwrap());
    choices
}

fn append_word_candidates(
    candidates: &mut Vec<EncodedCandidate>,
    data: &[u8],
    width: usize,
    big: bool,
) -> Result<(), String> {
    let (values, tail) = split_words(data, width, big);
    if values.len() < 3 {
        return Ok(());
    }
    let suffix = format!("{}{}", width * 8, if big { 'b' } else { 'l' });
    for (name, payload) in [
        ("for", encode_for(&values, tail, width, big)),
        ("delta", encode_delta(&values, tail, width, big)),
        ("delta2", encode_delta2(&values, tail, width, big)),
    ] {
        candidates.push(EncodedCandidate {
            payload,
            description: format!("{name}-{suffix}"),
        });
    }
    for low_width in pfor_widths(&values) {
        if low_width < width * 8 {
            candidates.push(EncodedCandidate {
                payload: encode_pfor(&values, tail, width, big, low_width)?,
                description: format!("pfor{low_width}-{suffix}"),
            });
        }
    }
    Ok(())
}

pub fn encode_candidates(data: &[u8]) -> Result<Vec<EncodedCandidate>, String> {
    let mut candidates = Vec::new();
    for width in [2usize, 4, 8] {
        for big in [false, true] {
            append_word_candidates(&mut candidates, data, width, big)?;
        }
    }
    if candidates.is_empty() {
        return Err("no numeric candidates".into());
    }
    Ok(candidates)
}

/// Select the first smallest payload, preserving frozen Python candidate order on ties.
pub fn encode(data: &[u8]) -> Result<EncodedCandidate, String> {
    let mut candidates = encode_candidates(data)?.into_iter();
    let mut best = candidates.next().unwrap();
    for candidate in candidates {
        if candidate.payload.len() < best.payload.len() {
            best = candidate;
        }
    }
    Ok(best)
}

fn bits(data: &[u8], pos: &mut usize, width: usize) -> Result<u64, String> {
    if width > 64 || pos.saturating_add(width) > data.len() * 8 {
        return Err("truncated numeric bitpack".into());
    }
    let mut v = 0u64;
    for _ in 0..width {
        v = (v << 1) | ((data[*pos / 8] >> (7 - *pos % 8)) & 1) as u64;
        *pos += 1;
    }
    Ok(v)
}
fn read_word(data: &[u8], pos: &mut usize, width: usize, big: bool) -> Result<u64, String> {
    let end = pos.checked_add(width).ok_or("numeric word overflow")?;
    let b = data.get(*pos..end).ok_or("truncated numeric word")?;
    *pos = end;
    let mut v = 0u64;
    if big {
        for x in b {
            v = (v << 8) | *x as u64;
        }
    } else {
        for (i, x) in b.iter().enumerate() {
            v |= (*x as u64) << (8 * i);
        }
    }
    Ok(v)
}
fn push_word(out: &mut Vec<u8>, v: u64, width: usize, big: bool) {
    if big {
        for i in (0..width).rev() {
            out.push((v >> (8 * i)) as u8);
        }
    } else {
        for i in 0..width {
            out.push((v >> (8 * i)) as u8);
        }
    }
}
fn unzig(v: u64) -> i128 {
    if v & 1 == 0 {
        (v / 2) as i128
    } else {
        -((v / 2) as i128) - 1
    }
}

fn decode_bytes(blob: &[u8], count: usize) -> Result<Vec<u8>, String> {
    let mode = *blob.first().ok_or("empty numeric byte substream")?;
    let mut pos = 1;
    match mode {
        0 => {
            let values = rank::decode_type_class(blob, &mut pos, count)?;
            if pos != blob.len() {
                return Err("trailing numeric static byte stream".into());
            }
            Ok(values)
        }
        1 => {
            let chunk = get_varint(blob, &mut pos)?;
            if chunk == 0 {
                return Err("invalid numeric byte chunk size".into());
            }
            let mut out = Vec::with_capacity(count);
            while out.len() < count {
                let n = chunk.min(count - out.len());
                out.extend(rank::decode_type_class(blob, &mut pos, n)?);
            }
            if pos != blob.len() {
                return Err("trailing numeric chunked byte stream".into());
            }
            Ok(out)
        }
        2 => adaptive::decode_general(&blob[1..], count, &[], 1 << 16),
        3 => {
            if blob.len() < 2 || !matches!(blob[1], 1 | 2) {
                return Err("invalid numeric context byte stream".into());
            }
            adaptive::decode_general(&blob[2..], count, &[], 1 << 16)
        }
        4 => wavelet::decode(&blob[1..], count),
        5 => histogram::decode_type_class(&blob[1..], count),
        _ => Err(format!("unsupported numeric byte stream mode {mode}")),
    }
}

fn decode_uint_varints(blob: &[u8], count: usize, p: &mut usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(get_varint(blob, p)?);
    }
    if *p != blob.len() {
        return Err("trailing numeric exception varints".into());
    }
    Ok(out)
}

fn decode_uint_rice(blob: &[u8], count: usize, p: &mut usize) -> Result<Vec<usize>, String> {
    let k = *blob.get(*p).ok_or("missing Rice parameter")? as usize;
    *p += 1;
    if k >= usize::BITS as usize {
        return Err("Rice parameter is too wide".into());
    }
    let mut bp = 0;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut q = 0usize;
        while bits(&blob[*p..], &mut bp, 1)? != 0 {
            q = q.checked_add(1).ok_or("Rice overflow")?;
        }
        if q > (usize::MAX >> k) {
            return Err("Rice overflow".into());
        }
        let low = bits(&blob[*p..], &mut bp, k)? as usize;
        out.push(q.checked_shl(k as u32).ok_or("Rice overflow")? | low);
    }
    Ok(out)
}

fn decode_uint_classes(
    blob: &[u8],
    count: usize,
    mode: u8,
    p: &mut usize,
) -> Result<Vec<usize>, String> {
    let class_len = get_varint(blob, p)?;
    let end = (*p)
        .checked_add(class_len)
        .ok_or("numeric integer class length overflow")?;
    if end > blob.len() {
        return Err("truncated numeric integer classes".into());
    }
    let classes = if mode == 2 {
        let mut class_pos = *p;
        let values = rank::decode_type_class(blob, &mut class_pos, count)?;
        if class_pos != end {
            return Err("trailing numeric integer classes".into());
        }
        values
    } else {
        decode_bytes(&blob[*p..end], count)?
    };
    let mut bit_pos = 0;
    let mut out = Vec::with_capacity(count);
    for class in classes {
        let width = class as usize;
        if width >= usize::BITS as usize {
            return Err("numeric integer class exceeds host width".into());
        }
        let low = bits(&blob[end..], &mut bit_pos, width)? as usize;
        out.push(
            (1usize << width)
                .checked_add(low)
                .and_then(|value| value.checked_sub(1))
                .ok_or("numeric integer overflow")?,
        );
    }
    Ok(out)
}

fn decode_uints(blob: &[u8], count: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mode = *blob.first().ok_or("empty numeric exception stream")?;
    let mut p = 1;
    match mode {
        0 => decode_uint_varints(blob, count, &mut p),
        1 => decode_uint_rice(blob, count, &mut p),
        2 | 3 => decode_uint_classes(blob, count, mode, &mut p),
        _ => Err(format!("unsupported numeric integer stream mode {mode}")),
    }
}
fn numeric_header(data: &[u8]) -> Result<(u8, usize, bool, usize), String> {
    if data.len() < 4 || data[0] != 1 {
        return Err("unknown numeric block version".into());
    }
    let width = match data[2] >> 1 {
        0 => 2,
        1 => 4,
        2 => 8,
        _ => return Err("invalid numeric word width".into()),
    };
    Ok((data[1], width, data[2] & 1 != 0, 3))
}

fn append_tail(mut out: Vec<u8>, tail: &[u8]) -> Vec<u8> {
    out.extend_from_slice(tail);
    out
}

fn payload_end(end: usize, tail: usize, overflow: &'static str) -> Result<usize, String> {
    end.checked_add(tail).ok_or_else(|| overflow.into())
}

fn decode_for(
    data: &[u8],
    n: usize,
    width: usize,
    big: bool,
    mut p: usize,
) -> Result<Vec<u8>, String> {
    let b = *data.get(p).ok_or("missing FOR bit width")? as usize;
    p += 1;
    let base = read_word(data, &mut p, width, big)?;
    let count = n / width;
    let tail = n % width;
    let packed = count
        .checked_mul(b)
        .ok_or("FOR bit count overflow")?
        .div_ceil(8);
    let end = p.checked_add(packed).ok_or("FOR length overflow")?;
    let expected_end = payload_end(end, tail, "FOR payload length overflow")?;
    if expected_end != data.len() {
        return Err("numeric FOR payload length mismatch".into());
    }
    let mut bp = 0;
    let mut out = Vec::with_capacity(n);
    for _ in 0..count {
        let value = base
            .checked_add(bits(&data[p..end], &mut bp, b)?)
            .ok_or("FOR value overflow")?;
        push_word(&mut out, value, width, big);
    }
    Ok(append_tail(out, &data[end..]))
}

fn decode_delta(
    data: &[u8],
    n: usize,
    method: u8,
    width: usize,
    big: bool,
    mut p: usize,
) -> Result<Vec<u8>, String> {
    let b = *data.get(p).ok_or("missing delta bit width")? as usize;
    p += 1;
    let first = read_word(data, &mut p, width, big)?;
    let count = n / width;
    let tail = n % width;
    let bits_count = if method == 2 {
        count
            .saturating_sub(1)
            .checked_mul(b)
            .ok_or("delta bit count overflow")?
    } else {
        count
            .saturating_sub(2)
            .checked_mul(b)
            .ok_or("delta2 bit count overflow")?
    };
    let packed = bits_count.div_ceil(8);
    let first_delta = if method == 3 {
        Some(get_varint_u64(data, &mut p)?)
    } else {
        None
    };
    let end = p.checked_add(packed).ok_or("delta payload overflow")?;
    let expected_end = payload_end(end, tail, "delta payload length overflow")?;
    if expected_end != data.len() {
        return Err("numeric delta payload length mismatch".into());
    }
    let mut bp = 0;
    let modulus = 1i128 << (width * 8);
    let mut vals = Vec::with_capacity(count);
    vals.push(first);
    let mut prev_delta = first_delta.map(unzig).unwrap_or(0);
    if method == 2 {
        for _ in 1..count {
            let d = unzig(bits(&data[p..end], &mut bp, b)?);
            let prev = *vals.last().unwrap() as i128;
            vals.push((prev + d).rem_euclid(modulus) as u64);
        }
    } else if count >= 2 {
        vals.push((first as i128 + prev_delta).rem_euclid(modulus) as u64);
        for _ in 2..count {
            let dd = unzig(bits(&data[p..end], &mut bp, b)?);
            prev_delta = (prev_delta + dd).rem_euclid(modulus);
            let prev = *vals.last().unwrap() as i128;
            vals.push((prev + prev_delta).rem_euclid(modulus) as u64);
        }
    }
    let mut out = Vec::with_capacity(n);
    for v in vals {
        push_word(&mut out, v, width, big);
    }
    Ok(append_tail(out, &data[end..]))
}

fn decode_pfor(
    data: &[u8],
    n: usize,
    width: usize,
    big: bool,
    mut p: usize,
) -> Result<Vec<u8>, String> {
    let low_width = *data.get(p).ok_or("missing PFOR width")? as usize;
    p += 1;
    if low_width >= u64::BITS as usize {
        return Err("PFOR width is too wide".into());
    }
    let base = read_word(data, &mut p, width, big)?;
    let k = get_varint(data, &mut p)?;
    let indices = rank::decode_colex_positions(data, &mut p, n / width, k)?;
    let hl = get_varint(data, &mut p)?;
    let end = p.checked_add(hl).ok_or("PFOR exception length overflow")?;
    if end > data.len() {
        return Err("truncated PFOR exceptions".into());
    }
    let highs = decode_uints(&data[p..end], k)?;
    p = end;
    let count = n / width;
    let tail = n % width;
    let packed = count
        .checked_mul(low_width)
        .ok_or("PFOR bit count overflow")?
        .div_ceil(8);
    let lows_end = p
        .checked_add(packed)
        .ok_or("PFOR low-bit length overflow")?;
    let expected_end = payload_end(lows_end, tail, "PFOR payload length overflow")?;
    if expected_end != data.len() {
        return Err("numeric PFOR payload length mismatch".into());
    }
    let mut bp = 0;
    let mut ex = 0;
    let mut out = Vec::with_capacity(n);
    for i in 0..count {
        let lo = bits(&data[p..lows_end], &mut bp, low_width)?;
        let hi = if ex < indices.len() && indices[ex] == i {
            let v = highs[ex] as u64;
            ex += 1;
            if v > (u64::MAX >> low_width) {
                return Err("PFOR overflow".into());
            }
            v.checked_shl(low_width as u32).ok_or("PFOR overflow")?
        } else {
            0
        };
        let value = base.checked_add(hi | lo).ok_or("PFOR value overflow")?;
        push_word(&mut out, value, width, big);
    }
    Ok(append_tail(out, &data[lows_end..]))
}

pub fn decode(data: &[u8], n: usize) -> Result<Vec<u8>, String> {
    let (method, width, big, p) = numeric_header(data)?;
    match method {
        1 => decode_for(data, n, width, big, p),
        2 | 3 => decode_delta(data, n, method, width, big, p),
        4 => decode_pfor(data, n, width, big, p),
        _ => Err(format!("unsupported numeric transform mode {method}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_numeric_candidate_round_trips_exactly() {
        let input: Vec<u8> = (0..257)
            .flat_map(|index| [(index * 17) as u8, (index >> 1) as u8])
            .collect();
        let candidates = encode_candidates(&input).unwrap();
        assert!(candidates
            .iter()
            .any(|candidate| candidate.description.starts_with("for-")));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.description.starts_with("delta-")));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.description.starts_with("delta2-")));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.description.starts_with("pfor")));
        for candidate in candidates {
            assert_eq!(decode(&candidate.payload, input.len()).unwrap(), input);
        }
    }

    #[test]
    fn numeric_bit_count_overflow_is_rejected() {
        let error = decode_for(&[3, 0, 0], usize::MAX, 2, false, 0).unwrap_err();
        assert_eq!(error, "FOR bit count overflow");
    }

    #[test]
    fn numeric_payload_end_overflow_is_rejected() {
        assert_eq!(
            payload_end(usize::MAX, 1, "numeric test payload overflow").unwrap_err(),
            "numeric test payload overflow"
        );
    }

    #[test]
    fn numeric_u64_varint_maximum_round_trips_canonically() {
        let mut encoded = Vec::new();
        put_varint_u64(u64::MAX, &mut encoded);
        let mut position = 0;
        assert_eq!(get_varint_u64(&encoded, &mut position).unwrap(), u64::MAX);
        assert_eq!(position, encoded.len());
    }

    #[test]
    fn numeric_u64_varint_rejects_high_bits_in_final_group() {
        let mut encoded = vec![0xff; 9];
        encoded.push(2);
        let mut position = 0;
        assert_eq!(
            get_varint_u64(&encoded, &mut position).unwrap_err(),
            "numeric varint overflow"
        );
    }

    #[test]
    fn uint_classes_skip_maximum_without_losing_varint_round_trip() {
        let highest_bit = 1usize << (usize::BITS - 1);
        let values = [highest_bit - 1, highest_bit, usize::MAX - 1, usize::MAX];
        let encoded = encode_uints(&values).unwrap();
        assert_eq!(encoded[0], 0);
        assert_eq!(
            decode_uints(&encoded, values.len()).unwrap(),
            values.to_vec()
        );
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn pfor_maximum_exception_round_trips_both_endiannesses_with_tail() {
        let values = [0, u64::MAX, 0];
        for big in [false, true] {
            let payload = encode_pfor(&values, &[0xa5], 8, big, 0).unwrap();
            let mut expected = Vec::new();
            for &value in &values {
                push_word(&mut expected, value, 8, big);
            }
            expected.push(0xa5);
            assert_eq!(decode(&payload, expected.len()).unwrap(), expected);
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn numeric_candidates_restore_ptt5_extrema_pattern() {
        // ptt5[81920..98304] contains an aligned zero u64 followed later by
        // ff×8 at offset 90976; this compact fixture keeps that PFOR extrema.
        let mut input = [0u8; 8].to_vec();
        input.extend([0xff; 8]);
        input.extend([0u8; 8]);
        input.push(0xa5);
        for candidate in encode_candidates(&input).unwrap() {
            assert_eq!(decode(&candidate.payload, input.len()).unwrap(), input);
        }
    }

    #[test]
    fn truncated_numeric_candidate_is_rejected() {
        let input: Vec<u8> = (0..128).map(|index| (index * 13) as u8).collect();
        let candidate = encode_candidates(&input)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.description.starts_with("pfor"))
            .unwrap();
        assert!(decode(
            &candidate.payload[..candidate.payload.len() - 1],
            input.len()
        )
        .is_err());
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
        writer.write(low as u64, width);
        let mut blob = vec![1, width as u8];
        blob.extend(writer.finish());
        blob
    }
    #[test]
    fn rice_accepts_max_rejects_high_bits() {
        assert_eq!(
            decode_uints(&payload(1, usize::MAX >> 1), 1).unwrap(),
            vec![usize::MAX]
        );
        assert!(decode_uints(&payload(2, 0), 1).is_err());
        assert!(decode_uints(&[1, usize::BITS as u8], 1).is_err());
    }
}

#[cfg(test)]
mod pfor_overflow_tests {
    use super::*;
    fn payload(high: u8, low: u64) -> Vec<u8> {
        let mut data = vec![63];
        data.extend([0; 8]); // base
        data.push(1); // exception at the sole position: zero-bit colex rank
        data.extend([2, 0, high]); // length, varint coder, high value
        let mut writer = BitWriter::new();
        writer.write(low, 63);
        data.extend(writer.finish());
        data
    }
    #[test]
    fn pfor_accepts_max_rejects_high_bits() {
        assert_eq!(
            decode_pfor(&payload(1, u64::MAX >> 1), 8, 8, false, 0).unwrap(),
            u64::MAX.to_le_bytes()
        );
        assert!(decode_pfor(&payload(2, 0), 8, 8, false, 0).is_err());
        assert!(decode_pfor(&[64], 8, 8, false, 0).is_err());
    }
}
