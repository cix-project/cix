//! Frozen CIXM6 record (mode 14) and periodic residual (mode 15) encoders.
//!
//! Candidate discovery, ordering, framing, and tie handling mirror the Python
//! reference at commit 80b95427725ecf59d626b2982d50f6349abd56e1.

use super::{bitplane, rank, sparse_runs};
use num_bigint::BigUint;
use num_traits::{One, ToPrimitive, Zero};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

const RECORD_VERSION: u8 = 2;
const PERIODIC_VERSION: u8 = 2;
const OP_XOR: u8 = 0;
const OP_SUB: u8 = 1;
const INNER_MULTINOMIAL: u8 = 0;
const INNER_SPARSE: u8 = 1;
const INNER_RUNS: u8 = 2;
const INNER_BITPLANE: u8 = 3;
const INNER_LOCAL: u8 = 4;
const BASE_WIDTHS: &[usize] = &[
    2, 3, 4, 5, 6, 8, 10, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536,
    2048,
];

fn put_v(mut value: usize, out: &mut Vec<u8>) {
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

fn big_log2(value: &BigUint) -> f64 {
    let bits = value.bits();
    if bits == 0 {
        return f64::NEG_INFINITY;
    }
    let keep = bits.min(53);
    let top = (value >> (bits - keep)).to_u64().unwrap() as f64;
    top.log2() + (bits - keep) as f64
}

fn log_choose(n: usize, k: usize) -> f64 {
    big_log2(&choose(n, k))
}

fn log_factorial(value: usize) -> f64 {
    unsafe extern "C" {
        fn lgamma(value: f64) -> f64;
    }
    unsafe { lgamma(value as f64 + 1.0) }
}

fn typeclass_estimate(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 1.0;
    }
    let mut counts = [0usize; 256];
    for &byte in data {
        counts[byte as usize] += 1;
    }
    let m = counts.iter().filter(|&&count| count != 0).count();
    let mut bits = 8.0;
    if m < 256 {
        bits += log_choose(256, m);
    }
    if m > 1 {
        bits += log_choose(data.len() - 1, m - 1);
    }
    bits += (log_factorial(data.len())
        - counts
            .iter()
            .filter(|&&count| count != 0)
            .map(|&count| log_factorial(count))
            .sum::<f64>())
        / std::f64::consts::LN_2;
    bits / 8.0
}

fn local_estimate(data: &[u8], chunks: &[usize]) -> f64 {
    let mut best = typeclass_estimate(data);
    for &chunk_size in chunks {
        if data.len() >= chunk_size * 2 {
            let cost = 2.0 + data.chunks(chunk_size).map(typeclass_estimate).sum::<f64>();
            best = best.min(cost);
        }
    }
    best
}

fn divisors(value: usize) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let root = (value as f64).sqrt() as usize;
    for d in 2..=root {
        if value.is_multiple_of(d) {
            out.insert(d);
            out.insert(value / d);
        }
    }
    out
}

pub fn discover_record_widths(data: &[u8], max_width: usize) -> Vec<usize> {
    let limit = max_width.min(data.len() / 16);
    if limit < 2 {
        return Vec::new();
    }
    let mut candidates: BTreeSet<usize> = BASE_WIDTHS
        .iter()
        .copied()
        .filter(|&width| width <= limit)
        .collect();
    let sample = &data[..data.len().min(65_536)];
    let mut previous = HashMap::<[u8; 4], usize>::new();
    let mut votes = HashMap::<usize, (usize, usize)>::new();
    let mut next_vote_order = 0usize;
    for pos in (0..sample.len().saturating_sub(4)).step_by(4) {
        let token: [u8; 4] = sample[pos..pos + 4].try_into().unwrap();
        if let Some(last) = previous.insert(token, pos) {
            let distance = pos - last;
            if (2..=limit).contains(&distance) {
                let entry = votes.entry(distance).or_insert_with(|| {
                    let order = next_vote_order;
                    next_vote_order += 1;
                    (0, order)
                });
                entry.0 += 1;
            }
        }
    }
    let mut ranked: Vec<(usize, usize, usize)> = votes
        .into_iter()
        .map(|(distance, (count, order))| (count, order, distance))
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for &(_, _, distance) in ranked.iter().take(16) {
        candidates.insert(distance);
        candidates.extend(divisors(distance).into_iter().filter(|&d| d <= limit));
    }
    candidates.into_iter().collect()
}

fn profile_record_widths(data: &[u8]) -> Vec<(f64, usize)> {
    let mut profiled = Vec::new();
    for width in discover_record_widths(data, 2048) {
        let estimate: f64 = (0..width)
            .map(|lane| {
                let stream: Vec<u8> = data.iter().skip(lane).step_by(width).copied().collect();
                local_estimate(&stream, &[128, 256, 512, 1024])
            })
            .sum();
        profiled.push((estimate, width));
    }
    profiled.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    profiled
}

pub fn encode_record(data: &[u8], beam: usize) -> Result<(Vec<u8>, String), String> {
    let profiled = profile_record_widths(data);
    if profiled.is_empty() || beam == 0 {
        return Err("block too short for record model".into());
    }
    let mut candidates = Vec::<(Vec<u8>, String)>::new();
    for &(_, width) in profiled.iter().take(beam) {
        let mut static_blob = vec![1];
        put_v(width, &mut static_blob);
        for lane in 0..width {
            let stream: Vec<u8> = data.iter().skip(lane).step_by(width).copied().collect();
            static_blob.extend(rank::encode_type_class(&stream)?);
        }
        candidates.push((static_blob, format!("record-multinomial-w{width}")));

        let mut local = vec![RECORD_VERSION];
        put_v(width, &mut local);
        for lane in 0..width {
            let stream: Vec<u8> = data.iter().skip(lane).step_by(width).copied().collect();
            let encoded = sparse_runs::encode_bytes(&stream)?;
            put_v(encoded.len(), &mut local);
            local.extend(encoded);
        }
        candidates.push((local, format!("record-local-w{width}")));
    }
    candidates
        .into_iter()
        .min_by_key(|candidate| candidate.0.len())
        .ok_or_else(|| "no record candidates".into())
}

fn xor_lag(data: &[u8], lag: usize) -> Vec<u8> {
    let mut out = data[..lag].to_vec();
    out.extend((lag..data.len()).map(|i| data[i] ^ data[i - lag]));
    out
}

fn sub_lag(data: &[u8], lag: usize) -> Vec<u8> {
    let mut out = data[..lag].to_vec();
    out.extend((lag..data.len()).map(|i| data[i].wrapping_sub(data[i - lag])));
    out
}

fn profile_periods(data: &[u8], limit: usize) -> Vec<(f64, usize, u8, Vec<u8>)> {
    let mut profiled = Vec::new();
    for lag in discover_record_widths(data, 2048) {
        if lag * 2 > data.len() {
            continue;
        }
        for op in [OP_XOR, OP_SUB] {
            let residual = if op == OP_XOR {
                xor_lag(data, lag)
            } else {
                sub_lag(data, lag)
            };
            profiled.push((
                local_estimate(&residual, &[128, 256, 512, 1024, 2048]),
                lag,
                op,
                residual,
            ));
        }
    }
    profiled.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    profiled.truncate(limit);
    profiled
}

pub fn encode_periodic(data: &[u8], beam: usize) -> Result<(Vec<u8>, String), String> {
    if beam == 0 {
        return Err("no periodic candidates".into());
    }
    let mut candidates = Vec::<(Vec<u8>, String)>::new();
    for (_, lag, op, residual) in profile_periods(data, beam.max(1)).into_iter().take(beam) {
        let mut inners = vec![
            (
                INNER_MULTINOMIAL,
                rank::encode_type_class(&residual)?,
                "multinomial".to_string(),
            ),
            (
                INNER_LOCAL,
                sparse_runs::encode_bytes(&residual)?,
                "local-multinomial".to_string(),
            ),
        ];
        let mut counts = [0usize; 256];
        for &byte in &residual {
            counts[byte as usize] += 1;
        }
        if counts.iter().copied().max().unwrap_or(0) * 4 >= residual.len() {
            let dominant = counts
                .iter()
                .enumerate()
                .max_by_key(|&(symbol, count)| (*count, std::cmp::Reverse(symbol)))
                .map(|(symbol, _)| symbol)
                .unwrap_or(0);
            inners.push((
                INNER_SPARSE,
                sparse_runs::encode_sparse(&residual)?,
                format!(
                    "dominant-{dominant:02x}-exceptions-{}",
                    residual.len() - counts[dominant]
                ),
            ));
        }
        let run_count = if residual.is_empty() {
            0
        } else {
            1 + residual
                .windows(2)
                .filter(|pair| pair[0] != pair[1])
                .count()
        };
        if !residual.is_empty() && run_count * 3 <= residual.len() * 2 {
            inners.push((
                INNER_RUNS,
                sparse_runs::encode_runs(&residual)?,
                format!("runs-{run_count}"),
            ));
        }
        inners.push((
            INNER_BITPLANE,
            bitplane::encode(&residual),
            "bitplane-enumerative-range".to_string(),
        ));
        let (inner, payload, description) = inners
            .into_iter()
            .min_by_key(|candidate| candidate.1.len())
            .ok_or("no periodic inner candidate")?;
        let mut out = vec![PERIODIC_VERSION, op, inner];
        put_v(lag, &mut out);
        out.extend(payload);
        candidates.push((
            out,
            format!(
                "period-{lag}-{}->{description}",
                if op == OP_XOR { "xor" } else { "sub" }
            ),
        ));
    }
    candidates
        .into_iter()
        .min_by_key(|candidate| candidate.0.len())
        .ok_or_else(|| "no periodic candidates".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_tiny_and_zero_beam() {
        assert!(encode_record(b"tiny", 2).is_err());
        assert!(encode_record(&[0; 64], 0).is_err());
        assert!(encode_periodic(b"tiny", 3).is_err());
        assert!(encode_periodic(&[0; 64], 0).is_err());
    }
}
