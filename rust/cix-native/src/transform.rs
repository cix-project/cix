//! Encoder and decoder for the frozen CIX reversible transform representation
//! (CIXM6 mode 5).

use crate::rank;
use std::collections::HashMap;

#[derive(Clone, Copy)]
enum Operation {
    ByteXor(usize),
    ByteDelta(usize),
    WordXor,
    WordDelta,
    WordDelta2,
    WordDelta2Zigzag,
    MatchXor,
}

#[derive(Clone, Copy)]
struct Spec {
    width: usize,
    big_endian: bool,
    shuffled: bool,
    operation: Operation,
}

fn spec(id: u8) -> Option<Spec> {
    let little = false;
    let big = true;
    let word = |width, big_endian, operation| Spec {
        width,
        big_endian,
        shuffled: true,
        operation,
    };
    let byte = |width, operation| Spec {
        width,
        big_endian: little,
        shuffled: false,
        operation,
    };

    Some(match id {
        1 => byte(1, Operation::ByteXor(1)),
        2 => byte(2, Operation::ByteXor(2)),
        3 => byte(4, Operation::ByteXor(4)),
        4 => byte(1, Operation::ByteDelta(1)),
        5 => word(2, little, Operation::WordDelta),
        6 => word(4, little, Operation::WordDelta),
        7 => word(8, little, Operation::WordDelta),
        8 => word(2, big, Operation::WordDelta),
        9 => word(4, big, Operation::WordDelta),
        10 => word(8, big, Operation::WordDelta),
        11 => word(2, little, Operation::WordDelta2),
        12 => word(4, little, Operation::WordDelta2),
        13 => word(8, little, Operation::WordDelta2),
        14 => word(2, big, Operation::WordDelta2),
        15 => word(4, big, Operation::WordDelta2),
        16 => word(8, big, Operation::WordDelta2),
        17 => word(4, little, Operation::WordXor),
        18 => word(8, little, Operation::WordXor),
        19 => byte(8, Operation::ByteXor(8)),
        20 => byte(12, Operation::ByteXor(12)),
        21 => byte(16, Operation::ByteXor(16)),
        22 => byte(24, Operation::ByteXor(24)),
        23 => byte(32, Operation::ByteXor(32)),
        24 => byte(64, Operation::ByteXor(64)),
        25 => byte(2, Operation::ByteDelta(2)),
        26 => byte(4, Operation::ByteDelta(4)),
        27 => byte(8, Operation::ByteDelta(8)),
        28 => byte(16, Operation::ByteDelta(16)),
        29 => word(4, little, Operation::WordDelta2Zigzag),
        30 => word(8, little, Operation::WordDelta2Zigzag),
        31 => word(8, big, Operation::WordDelta2Zigzag),
        33 => byte(1, Operation::MatchXor),
        _ => return None,
    })
}

const TRANSFORM_IDS: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 33,
];

fn name(id: u8) -> Option<&'static str> {
    Some(match id {
        1 => "byte-xor-1",
        2 => "byte-xor-2",
        3 => "byte-xor-4",
        4 => "byte-sub-1",
        5 => "word16le-delta-shuffle",
        6 => "word32le-delta-shuffle",
        7 => "word64le-delta-shuffle",
        8 => "word16be-delta-shuffle",
        9 => "word32be-delta-shuffle",
        10 => "word64be-delta-shuffle",
        11 => "word16le-delta2-shuffle",
        12 => "word32le-delta2-shuffle",
        13 => "word64le-delta2-shuffle",
        14 => "word16be-delta2-shuffle",
        15 => "word32be-delta2-shuffle",
        16 => "word64be-delta2-shuffle",
        17 => "word32le-xor-shuffle",
        18 => "word64le-xor-shuffle",
        19 => "byte-xor-8",
        20 => "byte-xor-12",
        21 => "byte-xor-16",
        22 => "byte-xor-24",
        23 => "byte-xor-32",
        24 => "byte-xor-64",
        25 => "byte-sub-2",
        26 => "byte-sub-4",
        27 => "byte-sub-8",
        28 => "byte-sub-16",
        29 => "word32le-delta2zz-shuffle",
        30 => "word64le-delta2zz-shuffle",
        31 => "word64be-delta2zz-shuffle",
        33 => "matchxor4",
        _ => return None,
    })
}

fn shuffle(data: &[u8], width: usize) -> Vec<u8> {
    let usable = data.len() / width * width;
    let mut out = Vec::with_capacity(data.len());
    for lane in 0..width {
        out.extend(data[..usable].iter().skip(lane).step_by(width));
    }
    out.extend_from_slice(&data[usable..]);
    out
}

fn unshuffle(data: &[u8], width: usize) -> Vec<u8> {
    let usable = data.len() / width * width;
    let words = usable / width;
    let mut out = vec![0; data.len()];
    for lane in 0..width {
        for index in 0..words {
            out[index * width + lane] = data[lane * words + index];
        }
    }
    out[usable..].copy_from_slice(&data[usable..]);
    out
}

fn read_word(data: &[u8], big_endian: bool) -> u64 {
    if big_endian {
        data.iter()
            .fold(0, |value, &byte| (value << 8) | byte as u64)
    } else {
        data.iter().enumerate().fold(0, |value, (shift, &byte)| {
            value | ((byte as u64) << (shift * 8))
        })
    }
}

fn write_word(value: u64, width: usize, big_endian: bool, out: &mut Vec<u8>) {
    if big_endian {
        for shift in (0..width).rev() {
            out.push((value >> (shift * 8)) as u8);
        }
    } else {
        for shift in 0..width {
            out.push((value >> (shift * 8)) as u8);
        }
    }
}

fn word_mask(width: usize) -> u64 {
    if width == 8 {
        u64::MAX
    } else {
        (1u64 << (width * 8)) - 1
    }
}

fn unzigzag(value: u64) -> i128 {
    if value & 1 == 0 {
        (value >> 1) as i128
    } else {
        -((value >> 1) as i128) - 1
    }
}

fn signed_mod(value: i128, width: usize) -> i128 {
    let modulus = 1i128 << (width * 8);
    let unsigned = value.rem_euclid(modulus);
    let half = modulus >> 1;
    if unsigned < half {
        unsigned
    } else {
        unsigned - modulus
    }
}

fn add_signed(value: u64, delta: i128, width: usize) -> u64 {
    let modulus = 1i128 << (width * 8);
    ((value as i128 + delta).rem_euclid(modulus)) as u64
}

fn signed_word(value: u64, width: usize) -> i128 {
    let bits = width * 8;
    if bits == 64 {
        (value as i64) as i128
    } else {
        let modulus = 1u64 << bits;
        let half = modulus >> 1;
        if value < half {
            value as i128
        } else {
            value as i128 - modulus as i128
        }
    }
}

fn zigzag(value: i128) -> u64 {
    if value >= 0 {
        (value << 1) as u64
    } else {
        ((-value << 1) - 1) as u64
    }
}

fn encode_matchxor(data: &[u8]) -> Vec<u8> {
    const KEY_LEN: usize = 4;
    let mut table: HashMap<[u8; KEY_LEN], usize> = HashMap::new();
    let mut active = None;
    let mut out = Vec::with_capacity(data.len());

    for (pos, &byte) in data.iter().enumerate() {
        let source = if active.is_some_and(|source| source < pos) {
            active
        } else if pos >= KEY_LEN {
            let key = [data[pos - 4], data[pos - 3], data[pos - 2], data[pos - 1]];
            table.get(&key).copied()
        } else {
            None
        };
        let prediction = source.map(|index| data[index]).unwrap_or(0);
        out.push(byte ^ prediction);

        if pos >= KEY_LEN {
            let key = [data[pos - 4], data[pos - 3], data[pos - 2], data[pos - 1]];
            table.insert(key, pos);
        }
        active = source
            .filter(|_| prediction == byte)
            .map(|source| source + 1);
    }
    out
}

fn apply(data: &[u8], spec: Spec) -> Vec<u8> {
    match spec.operation {
        Operation::MatchXor => return encode_matchxor(data),
        Operation::ByteXor(lag) => return apply_byte_xor(data, lag),
        Operation::ByteDelta(lag) => return apply_byte_delta(data, lag),
        _ => {}
    }
    let (values, usable) = words(data, spec);
    if values.is_empty() {
        return data.to_vec();
    }
    let transformed = apply_word_operation(&values, spec);
    let mut raw = write_words(transformed, spec, data.len());
    raw.extend_from_slice(&data[usable..]);
    if spec.shuffled {
        shuffle(&raw, spec.width)
    } else {
        raw
    }
}

fn apply_byte_xor(data: &[u8], lag: usize) -> Vec<u8> {
    let mut out = data.to_vec();
    for index in lag..data.len() {
        out[index] = data[index] ^ data[index - lag];
    }
    out
}

fn apply_byte_delta(data: &[u8], lag: usize) -> Vec<u8> {
    let mut out = data.to_vec();
    for index in lag..data.len() {
        out[index] = data[index].wrapping_sub(data[index - lag]);
    }
    out
}

fn words(data: &[u8], spec: Spec) -> (Vec<u64>, usize) {
    let usable = data.len() / spec.width * spec.width;
    (
        data[..usable]
            .chunks_exact(spec.width)
            .map(|word| read_word(word, spec.big_endian))
            .collect(),
        usable,
    )
}

fn write_words(values: Vec<u64>, spec: Spec, capacity: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(capacity);
    for value in values {
        write_word(value, spec.width, spec.big_endian, &mut out);
    }
    out
}

fn apply_word_operation(values: &[u64], spec: Spec) -> Vec<u64> {
    let mask = word_mask(spec.width);
    let mut out = Vec::with_capacity(values.len());
    out.push(values[0]);
    match spec.operation {
        Operation::WordXor => out.extend(values.windows(2).map(|pair| pair[1] ^ pair[0])),
        Operation::WordDelta => out.extend(
            values
                .windows(2)
                .map(|pair| pair[1].wrapping_sub(pair[0]) & mask),
        ),
        Operation::WordDelta2 => append_delta2(values, mask, &mut out),
        Operation::WordDelta2Zigzag => append_zigzag_delta2(values, spec.width, mask, &mut out),
        Operation::ByteXor(_) | Operation::ByteDelta(_) | Operation::MatchXor => unreachable!(),
    }
    out
}

fn append_delta2(values: &[u64], mask: u64, out: &mut Vec<u64>) {
    if values.len() < 2 {
        return;
    }
    let mut previous = values[1].wrapping_sub(values[0]) & mask;
    out.push(previous);
    for pair in values.windows(2).skip(1) {
        let delta = pair[1].wrapping_sub(pair[0]) & mask;
        out.push(delta.wrapping_sub(previous) & mask);
        previous = delta;
    }
}

fn append_zigzag_delta2(values: &[u64], width: usize, mask: u64, out: &mut Vec<u64>) {
    if values.len() < 2 {
        return;
    }
    let mut previous = signed_word(values[1].wrapping_sub(values[0]) & mask, width);
    out.push(zigzag(previous));
    for pair in values.windows(2).skip(1) {
        let delta = signed_word(pair[1].wrapping_sub(pair[0]) & mask, width);
        out.push(zigzag(signed_mod(delta - previous, width)));
        previous = delta;
    }
}

fn log2_factorials(n: usize) -> Vec<f64> {
    let mut values = Vec::with_capacity(n + 1);
    values.push(0.0);
    for value in 1..=n {
        values.push(values[value - 1] + (value as f64).log2());
    }
    values
}

fn log2_choose(log_factorials: &[f64], n: usize, k: usize) -> f64 {
    log_factorials[n] - log_factorials[k] - log_factorials[n - k]
}

fn type_class_estimate(data: &[u8], log_factorials: &[f64]) -> f64 {
    if data.is_empty() {
        return 1.0;
    }
    let mut counts = [0usize; 256];
    for &byte in data {
        counts[byte as usize] += 1;
    }
    let active = counts.iter().filter(|&&count| count > 0).count();
    let mut bits = 8.0;
    if active < 256 {
        bits += log2_choose(log_factorials, 256, active);
    }
    if active > 1 {
        bits += log2_choose(log_factorials, data.len() - 1, active - 1);
    }
    bits += log_factorials[data.len()]
        - counts
            .iter()
            .map(|&count| log_factorials[count])
            .sum::<f64>();
    bits / 8.0
}

/// Apply one explicit frozen transform without coding the resulting bytes.
///
/// Transform-chain encoders use this boundary before selecting their inner
/// representation, so the forward transform semantics stay shared with mode 5.
pub fn apply_id(data: &[u8], id: u8) -> Result<Vec<u8>, String> {
    Ok(apply(data, spec(id).ok_or("unknown transform ID")?))
}

/// Encode with one explicit frozen transform ID.
///
/// This API is useful to a bounded selector that has already chosen a route.
#[cfg(test)]
pub fn encode_id(data: &[u8], id: u8) -> Result<Vec<u8>, String> {
    let transformed = apply_id(data, id)?;
    let mut out = vec![1, id];
    out.extend(rank::encode_type_class(&transformed)?);
    Ok(out)
}

/// Run the frozen Python-compatible beam selector and encode a mode-5 payload.
pub fn encode(data: &[u8], beam: usize) -> Result<(Vec<u8>, &'static str), String> {
    let log_factorials = log2_factorials(data.len().max(256));
    let mut profiled = Vec::new();
    for id in TRANSFORM_IDS {
        let candidate_spec = spec(id).expect("listed transform ID must have a spec");
        if data.len() < usize::max(2, candidate_spec.width * 2) {
            continue;
        }
        let transformed = apply(data, candidate_spec);
        let estimate = type_class_estimate(&transformed, &log_factorials);
        profiled.push((estimate, id));
    }
    profiled.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut best: Option<(Vec<u8>, &'static str)> = None;
    for (_, id) in profiled.into_iter().take(beam) {
        let transformed = apply(
            data,
            spec(id).expect("profiled transform ID must have a spec"),
        );
        let mut payload = vec![1, id];
        payload.extend(rank::encode_type_class(&transformed)?);
        if best
            .as_ref()
            .is_none_or(|(current, _)| payload.len() < current.len())
        {
            best = Some((
                payload,
                name(id).expect("listed transform ID must have a name"),
            ));
        }
    }
    best.ok_or_else(|| "no transform candidates".into())
}

fn decode_matchxor(residual: &[u8]) -> Vec<u8> {
    const KEY_LEN: usize = 4;
    let mut table: HashMap<[u8; KEY_LEN], usize> = HashMap::new();
    let mut active = None;
    let mut out = Vec::with_capacity(residual.len());

    for (pos, &value) in residual.iter().enumerate() {
        let source = if active.is_some_and(|source| source < pos) {
            active
        } else if pos >= KEY_LEN {
            let key = [out[pos - 4], out[pos - 3], out[pos - 2], out[pos - 1]];
            table.get(&key).copied()
        } else {
            None
        };
        let prediction = source.map(|index| out[index]).unwrap_or(0);
        let byte = value ^ prediction;
        out.push(byte);

        if pos >= KEY_LEN {
            let key = [out[pos - 4], out[pos - 3], out[pos - 2], out[pos - 1]];
            table.insert(key, pos);
        }
        active = source
            .filter(|_| prediction == byte)
            .map(|source| source + 1);
    }
    out
}

fn invert(data: &[u8], spec: Spec) -> Vec<u8> {
    match spec.operation {
        Operation::MatchXor => return decode_matchxor(data),
        Operation::ByteXor(lag) => return invert_byte_xor(data, lag),
        Operation::ByteDelta(lag) => return invert_byte_delta(data, lag),
        _ => {}
    }

    let raw = if spec.shuffled {
        unshuffle(data, spec.width)
    } else {
        data.to_vec()
    };
    let usable = raw.len() / spec.width * spec.width;
    let mut values: Vec<u64> = raw[..usable]
        .chunks_exact(spec.width)
        .map(|word| read_word(word, spec.big_endian))
        .collect();
    invert_word_operation(&mut values, spec);
    let mut out = write_words(values, spec, raw.len());
    out.extend_from_slice(&raw[usable..]);
    out
}

fn invert_byte_xor(data: &[u8], lag: usize) -> Vec<u8> {
    let mut out = data.to_vec();
    for index in lag..out.len() {
        out[index] ^= out[index - lag];
    }
    out
}

fn invert_byte_delta(data: &[u8], lag: usize) -> Vec<u8> {
    let mut out = data.to_vec();
    for index in lag..out.len() {
        out[index] = out[index].wrapping_add(out[index - lag]);
    }
    out
}

fn invert_word_operation(values: &mut [u64], spec: Spec) {
    if values.len() < 2 {
        return;
    }
    let mask = word_mask(spec.width);
    match spec.operation {
        Operation::WordXor => {
            for index in 1..values.len() {
                values[index] ^= values[index - 1];
            }
        }
        Operation::WordDelta => {
            for index in 1..values.len() {
                values[index] = values[index - 1].wrapping_add(values[index]) & mask;
            }
        }
        Operation::WordDelta2 => invert_delta2(values, mask),
        Operation::WordDelta2Zigzag => invert_zigzag_delta2(values, spec.width),
        Operation::ByteXor(_) | Operation::ByteDelta(_) | Operation::MatchXor => unreachable!(),
    }
}

fn invert_delta2(values: &mut [u64], mask: u64) {
    let mut previous = values[1];
    values[1] = values[0].wrapping_add(previous) & mask;
    for index in 2..values.len() {
        previous = previous.wrapping_add(values[index]) & mask;
        values[index] = values[index - 1].wrapping_add(previous) & mask;
    }
}

fn invert_zigzag_delta2(values: &mut [u64], width: usize) {
    let mut previous = unzigzag(values[1]);
    values[1] = add_signed(values[0], previous, width);
    for index in 2..values.len() {
        previous = signed_mod(previous + unzigzag(values[index]), width);
        values[index] = add_signed(values[index - 1], previous, width);
    }
}

/// Decode one complete mode-5 payload.
pub fn decode(data: &[u8], source_length: usize) -> Result<Vec<u8>, String> {
    if data.len() < 2 || data[0] != 1 {
        return Err("unknown transform block version".into());
    }
    let spec = spec(data[1]).ok_or("unknown transform ID")?;
    let mut pos = 2;
    let transformed = rank::decode_type_class(data, &mut pos, source_length)?;
    if pos != data.len() {
        return Err("unused transform block bytes".into());
    }
    let restored = invert(&transformed, spec);
    if restored.len() != source_length {
        return Err("transform source length mismatch".into());
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_frozen_transforms_round_trip() {
        let input: Vec<u8> = (0..521)
            .map(|index| ((index * index + 31 * index + 17) & 255) as u8)
            .collect();
        for id in TRANSFORM_IDS {
            let payload = encode_id(&input, id).unwrap();
            assert_eq!(decode(&payload, input.len()).unwrap(), input, "ID {id}");
        }
    }

    #[test]
    fn forward_transform_api_covers_frozen_ids() {
        let input: Vec<u8> = (0..263)
            .map(|index| ((19 * index + index * index * index) & 255) as u8)
            .collect();
        for id in TRANSFORM_IDS {
            let transformed = apply_id(&input, id).unwrap();
            let restored = invert(&transformed, spec(id).unwrap());
            assert_eq!(restored, input, "ID {id}");
        }
        assert_eq!(apply_id(&input, 32), Err("unknown transform ID".into()));
    }
}
