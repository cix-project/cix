//! Canonical legacy CIXM6 sparse (mode 11) and run (mode 12) payload encoders.
//!
//! These routines mirror the frozen Python `cix.sparse` and `cix.runs`
//! representations. Integer and byte substreams retain the complete deterministic
//! candidate set used by the reference, including local models and histogram
//! replacement. All combinatorial ranks remain arbitrary precision.

use super::combinatorics;
use crate::rank::{self, ArithmeticEncoder};
use num_bigint::BigUint;
use num_traits::{One, ToPrimitive, Zero};
use std::collections::HashMap;

fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
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

fn fixed_bytes(value: &BigUint, width_bits: usize) -> Vec<u8> {
    let size = width_bits.div_ceil(8);
    if size == 0 {
        return Vec::new();
    }
    let bytes = value.to_bytes_be();
    let mut out = vec![0; size.saturating_sub(bytes.len())];
    out.extend(bytes);
    out
}

fn colex_rank(positions: &[usize]) -> Result<BigUint, String> {
    combinatorics::colex_rank_checked(positions)
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

    fn bit(&mut self, value: usize) {
        self.current = (self.current << 1) | (value as u8 & 1);
        self.used += 1;
        if self.used == 8 {
            self.data.push(self.current);
            self.current = 0;
            self.used = 0;
        }
    }

    fn bits(&mut self, value: usize, width: usize) {
        for shift in (0..width).rev() {
            self.bit((value >> shift) & 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.used != 0 {
            self.data.push(self.current << (8 - self.used));
        }
        self.data
    }
}

#[derive(Clone)]
struct AdaptiveModel {
    counts: [usize; 256],
    seen: usize,
}

impl AdaptiveModel {
    fn new() -> Self {
        Self {
            counts: [0; 256],
            seen: 0,
        }
    }

    fn interval(&self, symbol: usize) -> (usize, usize, usize) {
        let before: usize = self.counts[..symbol].iter().sum();
        let low = 2 * before + symbol;
        (low, low + 2 * self.counts[symbol] + 1, 2 * self.seen + 256)
    }

    fn observe(&mut self, symbol: usize) {
        self.counts[symbol] += 1;
        self.seen += 1;
    }
}

fn context_bucket(history: &[u8], order: usize, bucket_bits: usize) -> usize {
    if order == 0 {
        return 0;
    }
    if order == 1 && bucket_bits >= 8 {
        return history.last().copied().unwrap_or(0) as usize;
    }
    let mut hash = 1469598103934665603u64;
    for &byte in history {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    (hash & ((1u64 << bucket_bits) - 1)) as usize
}

fn encode_adaptive(data: &[u8], order: usize, bucket_bits: usize) -> Vec<u8> {
    let mut models = HashMap::<usize, AdaptiveModel>::new();
    let mut history = Vec::with_capacity(order);
    let mut encoder = ArithmeticEncoder::new();
    for &symbol in data {
        let bucket = context_bucket(&history, order, bucket_bits);
        let model = models.entry(bucket).or_insert_with(AdaptiveModel::new);
        let (low, high, total) = model.interval(symbol as usize);
        encoder.encode(low, high, total);
        model.observe(symbol as usize);
        if order != 0 {
            history.push(symbol);
            if history.len() > order {
                history.remove(0);
            }
        }
    }
    let (payload, bit_length) = encoder.finish();
    let mut out = vec![1, order as u8, bucket_bits as u8];
    put_varint(bit_length, &mut out);
    out.extend(payload);
    out
}

fn encode_wavelet_node(values: &[u8], depth: usize, encoder: &mut ArithmeticEncoder) {
    if values.is_empty() || depth == 8 {
        return;
    }
    let shift = 7 - depth;
    let ones = values
        .iter()
        .filter(|&&value| ((value >> shift) & 1) != 0)
        .count();
    encoder.encode(ones, ones + 1, values.len() + 1);
    let (mut zero, mut remaining) = (values.len() - ones, values.len());
    for &value in values {
        if ((value >> shift) & 1) != 0 {
            encoder.encode(zero, remaining, remaining);
        } else {
            encoder.encode(0, zero, remaining);
            zero -= 1;
        }
        remaining -= 1;
    }
    if ones == 0 || ones == values.len() {
        encode_wavelet_node(values, depth + 1, encoder);
    } else {
        let zeros: Vec<u8> = values
            .iter()
            .copied()
            .filter(|value| ((value >> shift) & 1) == 0)
            .collect();
        let ones_values: Vec<u8> = values
            .iter()
            .copied()
            .filter(|value| ((value >> shift) & 1) != 0)
            .collect();
        encode_wavelet_node(&zeros, depth + 1, encoder);
        encode_wavelet_node(&ones_values, depth + 1, encoder);
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
    if value.is_zero() {
        return f64::NEG_INFINITY;
    }
    let bits = value.bits() as usize;
    if bits <= 53 {
        return (value.to_u64().unwrap() as f64).log2();
    }
    let shift = bits - 53;
    let top = (value >> shift).to_u64().unwrap();
    shift as f64 + (top as f64).log2()
}

fn estimate_wavelet_node(counts: &[usize; 256], lo: usize, hi: usize, depth: usize) -> f64 {
    let n: usize = counts[lo..hi].iter().sum();
    if n == 0 || depth == 8 {
        return 0.0;
    }
    let middle = (lo + hi) / 2;
    let ones: usize = counts[middle..hi].iter().sum();
    let mut bits = (n as f64 + 1.0).log2();
    if ones != 0 && ones != n {
        bits += big_log2(&choose(n, ones));
    }
    bits + estimate_wavelet_node(counts, lo, middle, depth + 1)
        + estimate_wavelet_node(counts, middle, hi, depth + 1)
}

fn estimate_wavelet_bits(data: &[u8]) -> f64 {
    let mut counts = [0usize; 256];
    for &value in data {
        counts[value as usize] += 1;
    }
    estimate_wavelet_node(&counts, 0, 256, 0) + 24.0
}

fn conditional_bits(data: &[u8], order: usize) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut indices = HashMap::<Vec<u8>, usize>::new();
    let mut contexts = Vec::<[usize; 256]>::new();
    let mut history = Vec::with_capacity(order);
    for &value in data {
        let key = history.clone();
        let index = if let Some(&index) = indices.get(&key) {
            index
        } else {
            let index = contexts.len();
            indices.insert(key, index);
            contexts.push([0; 256]);
            index
        };
        contexts[index][value as usize] += 1;
        history.push(value);
        if history.len() > order {
            history.remove(0);
        }
    }
    contexts
        .iter()
        .map(|counts| {
            let n: usize = counts.iter().sum();
            n as f64 * (n as f64).log2()
                - counts
                    .iter()
                    .filter(|&&count| count != 0)
                    .map(|&count| count as f64 * (count as f64).log2())
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
            .and_then(|value| value.checked_mul(2))
            .and_then(|value| value.checked_sub(1))
    }
    .ok_or("run histogram zigzag overflow")?;
    usize::try_from(encoded).map_err(|_| "run histogram zigzag outside machine range".into())
}

fn active_prefix(active: &[usize]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    put_varint(active.len(), &mut out);
    if active.len() < 256 {
        let width = fixed_width(&(choose(256, active.len()) - BigUint::one()));
        out.extend(fixed_bytes(&colex_rank(active)?, width));
    }
    Ok(out)
}

fn encode_count_vector(counts: &[usize; 256]) -> Result<Vec<u8>, String> {
    let total: usize = counts.iter().sum();
    if total == 0 {
        return Err("empty run count vector".into());
    }
    let active: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(symbol, &count)| (count != 0).then_some(symbol))
        .collect();
    let active_counts: Vec<usize> = active.iter().map(|&symbol| counts[symbol]).collect();
    let prefix = active_prefix(&active)?;
    let mut candidates = Vec::<(&'static str, Vec<u8>)>::new();

    let mut candidate = vec![1, 0];
    candidate.extend(&prefix);
    candidate.extend(encode_uints(
        &active_counts[..active_counts.len() - 1]
            .iter()
            .map(|count| count - 1)
            .collect::<Vec<_>>(),
    )?);
    candidates.push(("active-uint", candidate));

    let mut previous = 0i128;
    let mut deltas = Vec::with_capacity(active_counts.len().saturating_sub(1));
    for &count in &active_counts[..active_counts.len() - 1] {
        deltas.push(zigzag(count as i128 - previous)?);
        previous = count as i128;
    }
    let mut candidate = vec![1, 1];
    candidate.extend(&prefix);
    candidate.extend(encode_uints(&deltas)?);
    candidates.push(("active-delta", candidate));

    let dense = &counts[..255];
    let mut candidate = vec![1, 2];
    candidate.extend(encode_uints(dense)?);
    candidates.push(("dense-uint", candidate));

    previous = 0;
    let mut deltas = Vec::with_capacity(255);
    for &count in dense {
        deltas.push(zigzag(count as i128 - previous)?);
        previous = count as i128;
    }
    let mut candidate = vec![1, 3];
    candidate.extend(encode_uints(&deltas)?);
    candidates.push(("dense-delta", candidate));

    let center = (total / active_counts.len()) as i128;
    let centered: Vec<usize> = active_counts[..active_counts.len() - 1]
        .iter()
        .map(|&count| zigzag(count as i128 - center))
        .collect::<Result<_, _>>()?;
    let mut candidate = vec![1, 4];
    candidate.extend(&prefix);
    candidate.extend(encode_uints(&centered)?);
    candidates.push(("active-centered", candidate));

    let center = (total / 256) as i128;
    let centered: Vec<usize> = dense
        .iter()
        .map(|&count| zigzag(count as i128 - center))
        .collect::<Result<_, _>>()?;
    let mut candidate = vec![1, 5];
    candidate.extend(encode_uints(&centered)?);
    candidates.push(("dense-centered", candidate));

    candidates.sort_by(|(name_a, data_a), (name_b, data_b)| {
        (data_a.len(), *name_a).cmp(&(data_b.len(), *name_b))
    });
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
        .filter_map(|(symbol, &count)| (count != 0).then_some(symbol))
        .collect();
    if active.len() < 2 {
        return Ok(None);
    }
    let mut active_prefix = Vec::new();
    put_varint(active.len(), &mut active_prefix);
    let mut header_size = active_prefix.len();
    if active.len() < 256 {
        header_size +=
            fixed_width(&(combinatorics::choose_checked(256, active.len())? - BigUint::one()))
                .div_ceil(8);
    }
    header_size += fixed_width(
        &(combinatorics::choose_checked(data.len() - 1, active.len() - 1)? - BigUint::one()),
    )
    .div_ceil(8);
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

/// Encode the frozen Python local byte-substream candidate portfolio.
pub fn encode_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let static_payload = rank::encode_type_class(data)?;
    let mut best = tagged(0, static_payload.clone());
    if data.is_empty() {
        return Ok(best);
    }
    consider_tiled(data, &mut best)?;
    consider_wavelet(data, &mut best);
    consider_adaptive(data, &mut best);
    consider_histogram(data, &static_payload, &mut best)?;
    Ok(best)
}

fn tagged(tag: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut candidate = vec![tag];
    candidate.extend(payload);
    candidate
}

fn retain_shorter(best: &mut Vec<u8>, candidate: Vec<u8>) {
    if candidate.len() < best.len() {
        *best = candidate;
    }
}

fn consider_tiled(data: &[u8], best: &mut Vec<u8>) -> Result<(), String> {
    for chunk_size in [128usize, 256, 512, 1024, 2048, 4096] {
        if data.len() >= chunk_size * 2 {
            retain_shorter(best, encode_tiled(data, chunk_size)?);
        }
    }
    Ok(())
}

fn encode_tiled(data: &[u8], chunk_size: usize) -> Result<Vec<u8>, String> {
    let mut candidate = vec![1];
    put_varint(chunk_size, &mut candidate);
    for chunk in data.chunks(chunk_size) {
        candidate.extend(rank::encode_type_class(chunk)?);
    }
    Ok(candidate)
}

fn consider_wavelet(data: &[u8], best: &mut Vec<u8>) {
    if data.len() >= 64 && estimate_wavelet_bits(data) / 8.0 <= best.len() as f64 * 1.03 + 8.0 {
        retain_shorter(best, tagged(4, encode_wavelet(data)));
    }
}

fn consider_adaptive(data: &[u8], best: &mut Vec<u8>) {
    if data.len() < 256 {
        return;
    }
    retain_shorter(best, tagged(2, encode_adaptive(data, 0, 8)));
    for &(order, bucket_bits) in adaptive_orders(data.len()) {
        if conditional_bits(data, order) / 8.0 <= best.len() as f64 * 1.10 + 16.0 {
            let mut candidate = vec![3, order as u8];
            candidate.extend(encode_adaptive(data, order, bucket_bits));
            retain_shorter(best, candidate);
        }
    }
}

fn adaptive_orders(length: usize) -> &'static [(usize, usize)] {
    if length >= 2048 {
        &[(1, 8), (2, 10)]
    } else {
        &[(1, 8)]
    }
}

fn consider_histogram(
    data: &[u8],
    static_payload: &[u8],
    best: &mut Vec<u8>,
) -> Result<(), String> {
    if let Some(histogram) = histogram_candidate(data, static_payload)? {
        retain_shorter(best, tagged(5, histogram));
    }
    Ok(())
}

fn encode_uints(values: &[usize]) -> Result<Vec<u8>, String> {
    if values.is_empty() {
        return Ok(vec![0]);
    }
    let mut best = varint_candidate(values);
    for width in 0..=16usize {
        if let Some(candidate) = rice_candidate(values, width, best.len()) {
            if candidate.len() < best.len() {
                best = candidate;
            }
        }
    }
    let Some((classes, lows)) = integer_classes(values)? else {
        return Ok(best);
    };
    let low_blob = encode_integer_lows(&lows);
    retain_uint_candidate(&mut best, 2, rank::encode_type_class(&classes)?, &low_blob);
    retain_uint_candidate(&mut best, 3, encode_bytes(&classes)?, &low_blob);
    Ok(best)
}

fn varint_candidate(values: &[usize]) -> Vec<u8> {
    let mut raw = vec![0];
    for &value in values {
        put_varint(value, &mut raw);
    }
    raw
}

type IntegerClasses = (Vec<u8>, Vec<(usize, usize)>);

fn integer_classes(values: &[usize]) -> Result<Option<IntegerClasses>, String> {
    let mut classes = Vec::with_capacity(values.len());
    let mut lows = Vec::with_capacity(values.len());
    for &value in values {
        let shifted = value.checked_add(1).ok_or("run integer class overflow")?;
        let width = (usize::BITS - shifted.leading_zeros() - 1) as usize;
        if width > 255 {
            return Ok(None);
        }
        classes.push(width as u8);
        lows.push((shifted - (1usize << width), width));
    }
    Ok(Some((classes, lows)))
}

fn encode_integer_lows(lows: &[(usize, usize)]) -> Vec<u8> {
    let mut writer = BitWriter::new();
    for &(low, width) in lows {
        if width != 0 {
            writer.bits(low, width);
        }
    }
    writer.finish()
}

fn retain_uint_candidate(best: &mut Vec<u8>, tag: u8, classes: Vec<u8>, lows: &[u8]) {
    let mut candidate = vec![tag];
    put_varint(classes.len(), &mut candidate);
    candidate.extend(classes);
    candidate.extend_from_slice(lows);
    if candidate.len() < best.len() {
        *best = candidate;
    }
}

fn rice_candidate(values: &[usize], width: usize, best_len: usize) -> Option<Vec<u8>> {
    let limit = best_len.saturating_sub(2) * 8;
    let mut cost = 0usize;
    for &value in values {
        cost = cost
            .checked_add(value >> width)
            .and_then(|v| v.checked_add(1 + width))
            .unwrap_or(usize::MAX);
        if cost >= limit {
            return None;
        }
    }
    let mut writer = BitWriter::new();
    for &value in values {
        for _ in 0..(value >> width) {
            writer.bit(1);
        }
        writer.bit(0);
        if width != 0 {
            writer.bits(value & ((1usize << width) - 1), width);
        }
    }
    let mut candidate = vec![1, width as u8];
    candidate.extend(writer.finish());
    Some(candidate)
}

/// Encode a frozen CIX sparse payload (CIXM6 mode 11).
pub fn encode_sparse(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.is_empty() {
        return Err("empty sparse block".into());
    }
    let mut counts = [0usize; 256];
    for &byte in data {
        counts[byte as usize] += 1;
    }
    let mut dominant = 0usize;
    for symbol in 1..256 {
        if counts[symbol] > counts[dominant] {
            dominant = symbol;
        }
    }
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
    put_varint(positions.len(), &mut out);
    out.extend(fixed_bytes(&colex_rank(&positions)?, width));
    out.extend(rank::encode_type_class(&values)?);
    Ok(out)
}

/// Encode a frozen CIX run-decomposition payload (CIXM6 mode 12).
pub fn encode_runs(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.is_empty() {
        return Err("empty run block".into());
    }
    let mut symbols = Vec::new();
    let mut lengths = Vec::new();
    let mut current = data[0];
    let mut run = 1usize;
    for &byte in &data[1..] {
        if byte == current {
            run += 1;
        } else {
            symbols.push(current);
            lengths.push(run - 1);
            current = byte;
            run = 1;
        }
    }
    symbols.push(current);
    lengths.push(run - 1);

    let symbol_blob = rank::encode_type_class(&symbols)?;
    let length_blob = encode_uints(&lengths)?;
    let mut out = vec![1];
    put_varint(symbols.len(), &mut out);
    put_varint(symbol_blob.len(), &mut out);
    out.extend(symbol_blob);
    out.extend(length_blob);
    Ok(out)
}
