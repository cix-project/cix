//! Decoder for legacy CIXM6 mode 4 (BWT/MTF payload versions 1 through 6).
//!
//! This is deliberately separate from the CIXG1 BWT route. CIXM6 uses local
//! byte/integer substreams and libdivsufsort's one-based primary-index
//! convention; the CIXG1 route has different framing.

use super::{
    combinatorics,
    rank::{self, ArithmeticDecoder},
};
use num_bigint::BigUint;
use num_traits::{One, ToPrimitive, Zero};
use std::collections::HashMap;

const MAX_ENCODE_BLOCK: usize = 65_536;

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
        if self.used > 0 {
            self.data.push(self.current << (8 - self.used));
        }
        self.data
    }
}

fn read_varint(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..usize::BITS as usize).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated BWT varint")?;
        *pos += 1;
        let low = (byte & 0x7f) as usize;
        if shift + 7 > usize::BITS as usize && low > (usize::MAX >> shift) {
            return Err("BWT varint overflow".into());
        }
        value |= low.checked_shl(shift as u32).ok_or("BWT varint overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized BWT varint".into())
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bit(&mut self) -> Result<usize, String> {
        if self.pos >= self.data.len().saturating_mul(8) {
            return Err("truncated BWT bitstream".into());
        }
        let value = ((self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1) as usize;
        self.pos += 1;
        Ok(value)
    }

    fn bits(&mut self, width: usize) -> Result<usize, String> {
        if width >= usize::BITS as usize {
            return Err("BWT integer class too wide".into());
        }
        let mut value = 0usize;
        for _ in 0..width {
            value = (value << 1) | self.bit()?;
        }
        Ok(value)
    }
}

fn decode_wavelet_bit(
    decoder: &mut ArithmeticDecoder<'_>,
    zero: &mut usize,
    one: &mut usize,
    remaining: usize,
) -> Result<u8, String> {
    if *zero == 0 {
        decoder.update(0, *one, remaining);
        *one -= 1;
        return Ok(1);
    }
    if *one == 0 {
        decoder.update(0, *zero, remaining);
        *zero -= 1;
        return Ok(0);
    }
    if decoder.target(remaining) < *zero {
        decoder.update(0, *zero, remaining);
        *zero -= 1;
        Ok(0)
    } else {
        decoder.update(*zero, remaining, remaining);
        *one -= 1;
        Ok(1)
    }
}

fn decode_wavelet_node(
    n: usize,
    depth: usize,
    decoder: &mut ArithmeticDecoder<'_>,
    prefix: u8,
) -> Result<Vec<u8>, String> {
    if n == 0 {
        return Ok(Vec::new());
    }
    if depth == 8 {
        return Ok(vec![prefix; n]);
    }
    let total = n.checked_add(1).ok_or("BWT wavelet size overflow")?;
    let ones = decoder.target(total);
    if ones > n {
        return Err("BWT wavelet child count out of range".into());
    }
    decoder.update(ones, ones + 1, total);
    let (original_ones, original_zeros) = (ones, n - ones);
    let (mut one, mut zero, mut remaining) = (original_ones, original_zeros, n);
    let mut bits = Vec::with_capacity(n);
    for _ in 0..n {
        let bit = decode_wavelet_bit(decoder, &mut zero, &mut one, remaining)?;
        remaining -= 1;
        bits.push(bit);
    }
    let zero_values = decode_wavelet_node(original_zeros, depth + 1, decoder, prefix << 1)?;
    let one_values = decode_wavelet_node(original_ones, depth + 1, decoder, (prefix << 1) | 1)?;
    let (mut zero_pos, mut one_pos) = (0usize, 0usize);
    let mut out = Vec::with_capacity(n);
    for bit in bits {
        if bit == 0 {
            out.push(zero_values[zero_pos]);
            zero_pos += 1;
        } else {
            out.push(one_values[one_pos]);
            one_pos += 1;
        }
    }
    Ok(out)
}

fn decode_wavelet(blob: &[u8], count: usize) -> Result<Vec<u8>, String> {
    if blob.first() != Some(&1) {
        return Err("unknown BWT wavelet version".into());
    }
    let mut pos = 1;
    let bit_length = read_varint(blob, &mut pos)?;
    let byte_length = bit_length.div_ceil(8);
    if pos.checked_add(byte_length) != Some(blob.len()) {
        return Err("BWT wavelet payload length mismatch".into());
    }
    let mut decoder = ArithmeticDecoder::new(&blob[pos..], bit_length);
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

fn bwt_sum_counts(values: &[usize]) -> Result<usize, String> {
    values
        .iter()
        .try_fold(0usize, |sum, &value| sum.checked_add(value))
        .ok_or("BWT count total overflow".into())
}

fn bwt_positive_delta(values: Vec<usize>) -> Result<Vec<usize>, String> {
    let mut previous = 0i128;
    values
        .into_iter()
        .map(|value| {
            previous += unzig(value);
            if previous <= 0 || previous > usize::MAX as i128 {
                return Err("invalid BWT active count delta".into());
            }
            Ok(previous as usize)
        })
        .collect()
}

fn bwt_positive_centered(values: Vec<usize>, center: usize) -> Result<Vec<usize>, String> {
    values
        .into_iter()
        .map(|value| {
            let count = center as i128 + unzig(value);
            if count <= 0 || count > usize::MAX as i128 {
                return Err("invalid BWT centered active count".into());
            }
            Ok(count as usize)
        })
        .collect()
}

fn bwt_active_values(
    mode: u8,
    values: Vec<usize>,
    total: usize,
    active_count: usize,
) -> Result<Vec<usize>, String> {
    match mode {
        0 => values
            .into_iter()
            .map(|value| value.checked_add(1).ok_or("BWT count overflow".into()))
            .collect(),
        1 => bwt_positive_delta(values),
        4 => bwt_positive_centered(values, total / active_count),
        _ => unreachable!(),
    }
}

fn bwt_dense_delta(values: Vec<usize>) -> Result<Vec<usize>, String> {
    let mut previous = 0i128;
    values
        .into_iter()
        .map(|value| {
            previous += unzig(value);
            if previous < 0 || previous > usize::MAX as i128 {
                return Err("negative BWT dense histogram count".into());
            }
            Ok(previous as usize)
        })
        .collect()
}

fn bwt_dense_centered(values: Vec<usize>, center: usize) -> Result<Vec<usize>, String> {
    values
        .into_iter()
        .map(|value| {
            let count = center as i128 + unzig(value);
            if count < 0 || count > usize::MAX as i128 {
                return Err("negative BWT centered histogram count".into());
            }
            Ok(count as usize)
        })
        .collect()
}

fn bwt_dense_values(mode: u8, values: Vec<usize>, total: usize) -> Result<Vec<usize>, String> {
    match mode {
        2 => Ok(values),
        3 => bwt_dense_delta(values),
        5 => bwt_dense_centered(values, total / 256),
        _ => unreachable!(),
    }
}

fn decode_active_histogram(
    blob: &[u8],
    pos: &mut usize,
    mode: u8,
    total: usize,
    model_limit: usize,
) -> Result<Vec<usize>, String> {
    let active_count = read_varint(blob, pos)?;
    if active_count == 0 || active_count > 256 || active_count > total {
        return Err("invalid BWT active count alphabet".into());
    }
    let active = if active_count == 256 {
        (0..256).collect()
    } else {
        rank::decode_colex_positions(blob, pos, 256, active_count)?
    };
    let mut first = bwt_active_values(
        mode,
        decode_uints(&blob[*pos..], active_count - 1, model_limit)?,
        total,
        active_count,
    )?;
    let used = bwt_sum_counts(&first)?;
    let last = total
        .checked_sub(used)
        .ok_or("BWT histogram exceeds total")?;
    if last == 0 {
        return Err("zero implied BWT active count".into());
    }
    first.push(last);
    let mut counts = vec![0usize; 256];
    for (symbol, count) in active.into_iter().zip(first) {
        counts[symbol] = count;
    }
    Ok(counts)
}

fn decode_dense_histogram(
    blob: &[u8],
    pos: usize,
    mode: u8,
    total: usize,
    model_limit: usize,
) -> Result<Vec<usize>, String> {
    let mut first = bwt_dense_values(mode, decode_uints(&blob[pos..], 255, model_limit)?, total)?;
    let used = bwt_sum_counts(&first)?;
    first.push(
        total
            .checked_sub(used)
            .ok_or("BWT dense histogram exceeds total")?,
    );
    Ok(first)
}

fn decode_histogram_counts(
    blob: &[u8],
    total: usize,
    model_limit: usize,
) -> Result<Vec<usize>, String> {
    if blob.len() < 2 || blob[0] != 1 {
        return Err("unknown BWT count-vector version".into());
    }
    let mode = blob[1];
    let mut pos = 2;
    match mode {
        0 | 1 | 4 => decode_active_histogram(blob, &mut pos, mode, total, model_limit),
        2 | 3 | 5 => decode_dense_histogram(blob, pos, mode, total, model_limit),
        _ => Err(format!("unknown BWT count-vector mode {mode}")),
    }
}

fn decode_histogram(blob: &[u8], total: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    let mut pos = 0;
    let count_length = read_varint(blob, &mut pos)?;
    let end = pos
        .checked_add(count_length)
        .ok_or("BWT histogram length overflow")?;
    if end > blob.len() {
        return Err("truncated BWT histogram".into());
    }
    let counts = decode_histogram_counts(&blob[pos..end], total, model_limit)?;
    rank::decode_type_class_with_counts(&blob[end..], total, &counts)
}

fn decode_bytes(blob: &[u8], count: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    let mode = *blob.first().ok_or("empty BWT byte substream")?;
    let mut pos = 1;
    match mode {
        0 => {
            let out = rank::decode_type_class(blob, &mut pos, count)?;
            if pos != blob.len() {
                return Err("unused static BWT byte-substream bytes".into());
            }
            Ok(out)
        }
        1 => {
            let chunk_size = read_varint(blob, &mut pos)?;
            if chunk_size == 0 {
                return Err("invalid BWT byte-substream chunk size".into());
            }
            let mut out = Vec::with_capacity(count);
            while out.len() < count {
                let size = chunk_size.min(count - out.len());
                out.extend(rank::decode_type_class(blob, &mut pos, size)?);
            }
            if pos != blob.len() {
                return Err("unused chunked BWT byte-substream bytes".into());
            }
            Ok(out)
        }
        2 => super::adaptive::decode_general(&blob[1..], count, &[], model_limit),
        3 => {
            if blob.len() < 2 || !matches!(blob[1], 1 | 2) {
                return Err("invalid BWT context byte-substream order".into());
            }
            super::adaptive::decode_general(&blob[2..], count, &[], model_limit)
        }
        4 => decode_wavelet(&blob[1..], count),
        5 => decode_histogram(&blob[1..], count, model_limit),
        _ => Err(format!("unknown BWT byte-substream mode {mode}")),
    }
}

fn decode_raw_uints(blob: &[u8], count: usize) -> Result<Vec<usize>, String> {
    let mut pos = 1;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_varint(blob, &mut pos)?);
    }
    if pos != blob.len() {
        return Err("trailing BWT integer varints".into());
    }
    Ok(out)
}

fn decode_rice_uints(blob: &[u8], count: usize) -> Result<Vec<usize>, String> {
    let mut pos = 1;
    let width = *blob.get(pos).ok_or("missing BWT Rice parameter")? as usize;
    pos += 1;
    if width >= usize::BITS as usize {
        return Err("BWT Rice parameter too wide".into());
    }
    let mut reader = BitReader::new(&blob[pos..]);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut quotient = 0usize;
        while reader.bit()? != 0 {
            quotient = quotient
                .checked_add(1)
                .ok_or("BWT Rice quotient overflow")?;
        }
        let low = reader.bits(width)?;
        if quotient > (usize::MAX >> width) {
            return Err("BWT Rice value overflow".into());
        }
        out.push(
            quotient
                .checked_shl(width as u32)
                .and_then(|value| value.checked_add(low))
                .ok_or("BWT Rice value overflow")?,
        );
    }
    Ok(out)
}

fn decode_class_uints(
    blob: &[u8],
    count: usize,
    mode: u8,
    model_limit: usize,
) -> Result<Vec<usize>, String> {
    let mut pos = 1;
    let class_length = read_varint(blob, &mut pos)?;
    let end = pos
        .checked_add(class_length)
        .ok_or("BWT integer class length overflow")?;
    if end > blob.len() {
        return Err("truncated BWT integer class stream".into());
    }
    let classes = if mode == 2 {
        let mut class_pos = 0;
        let values = rank::decode_type_class(&blob[pos..end], &mut class_pos, count)?;
        if class_pos != class_length {
            return Err("unused BWT integer class bytes".into());
        }
        values
    } else {
        decode_bytes(&blob[pos..end], count, model_limit)?
    };
    let mut reader = BitReader::new(&blob[end..]);
    let mut out = Vec::with_capacity(count);
    for width in classes {
        let width = width as usize;
        let low = reader.bits(width)?;
        out.push(
            (1usize << width)
                .checked_add(low)
                .and_then(|value| value.checked_sub(1))
                .ok_or("BWT integer overflow")?,
        );
    }
    Ok(out)
}

fn decode_uints(blob: &[u8], count: usize, model_limit: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    match *blob.first().ok_or("empty BWT integer stream")? {
        0 => decode_raw_uints(blob, count),
        1 => decode_rice_uints(blob, count),
        mode @ (2 | 3) => decode_class_uints(blob, count, mode, model_limit),
        mode => Err(format!("unknown BWT integer stream mode {mode}")),
    }
}

fn decode_binary_tags(blob: &[u8], count: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    let mode = *blob.first().ok_or("empty BWT tag stream")?;
    match mode {
        0 => decode_bytes(&blob[1..], count, model_limit),
        1 => {
            let first = *blob.get(1).ok_or("truncated BWT run-tag stream")?;
            if first > 1 {
                return Err("invalid BWT run-tag stream".into());
            }
            let mut pos = 2;
            let run_count = read_varint(blob, &mut pos)?;
            let runs = decode_uints(&blob[pos..], run_count, model_limit)?;
            let mut tag = first;
            let mut out = Vec::with_capacity(count);
            for encoded_length in runs {
                let length = encoded_length
                    .checked_add(1)
                    .ok_or("BWT tag run overflow")?;
                let end = out
                    .len()
                    .checked_add(length)
                    .filter(|&end| end <= count)
                    .ok_or("BWT run-tag length mismatch")?;
                out.resize(end, tag);
                tag ^= 1;
            }
            if out.len() != count {
                return Err("BWT run-tag length mismatch".into());
            }
            Ok(out)
        }
        _ => Err("unknown BWT tag stream mode".into()),
    }
}

fn join_zero_runs(
    tags: &[u8],
    nonzero: &[u8],
    runs: &[usize],
    source_length: usize,
) -> Result<Vec<u8>, String> {
    let (mut nonzero_pos, mut run_pos) = (0usize, 0usize);
    let mut out = Vec::with_capacity(source_length);
    for &tag in tags {
        match tag {
            1 => {
                let value = *nonzero
                    .get(nonzero_pos)
                    .ok_or("nonzero BWT MTF stream exhausted")?;
                out.push(value);
                nonzero_pos += 1;
            }
            0 => {
                let run = runs
                    .get(run_pos)
                    .ok_or("BWT zero-run stream exhausted")?
                    .checked_add(1)
                    .ok_or("BWT zero-run overflow")?;
                let end = out
                    .len()
                    .checked_add(run)
                    .filter(|&end| end <= source_length)
                    .ok_or("BWT transformed length mismatch")?;
                out.resize(end, 0);
                run_pos += 1;
            }
            _ => return Err("invalid BWT tag value".into()),
        }
    }
    if nonzero_pos != nonzero.len() || run_pos != runs.len() {
        return Err("unused BWT substream symbols".into());
    }
    Ok(out)
}

fn decode_runa_zero_run(tokens: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    let mut weight = 1usize;
    while *pos < tokens.len() && tokens[*pos] <= 1 {
        let factor = if tokens[*pos] == 0 { 1 } else { 2 };
        value = value
            .checked_add(weight.checked_mul(factor).ok_or("BWT run overflow")?)
            .ok_or("BWT run overflow")?;
        weight = weight.checked_mul(2).ok_or("BWT run overflow")?;
        *pos += 1;
    }
    Ok(value)
}

fn decode_runa_literal(token: u8, side_bits: &[usize], side_pos: &mut usize) -> Result<u8, String> {
    if token < 255 {
        return Ok(token - 1);
    }
    let side = *side_bits
        .get(*side_pos)
        .ok_or("BWT RUNA/RUNB side stream exhausted")?;
    *side_pos += 1;
    Ok(254 + side as u8)
}

fn decode_runa_runb(
    tokens: &[u8],
    side_bits: &[usize],
    source_length: usize,
) -> Result<Vec<u8>, String> {
    let (mut pos, mut side_pos) = (0usize, 0usize);
    let mut out = Vec::with_capacity(source_length);
    while pos < tokens.len() {
        let token = tokens[pos];
        if token <= 1 {
            let value = decode_runa_zero_run(tokens, &mut pos)?;
            let end = out
                .len()
                .checked_add(value)
                .filter(|&end| end <= source_length)
                .ok_or("BWT RUNA/RUNB transformed length mismatch")?;
            out.resize(end, 0);
            continue;
        }
        out.push(decode_runa_literal(token, side_bits, &mut side_pos)?);
        if out.len() > source_length {
            return Err("BWT RUNA/RUNB transformed length mismatch".into());
        }
        pos += 1;
    }
    if side_pos != side_bits.len() {
        return Err("unused BWT RUNA/RUNB side bits".into());
    }
    Ok(out)
}

fn mtf_decode(data: &[u8]) -> Vec<u8> {
    let mut symbols: Vec<u8> = (0..=255).collect();
    let mut out = Vec::with_capacity(data.len());
    for &index in data {
        let index = index as usize;
        let symbol = symbols[index];
        out.push(symbol);
        if index > 0 {
            symbols.copy_within(0..index, 1);
            symbols[0] = symbol;
        }
    }
    out
}

fn bwt_inverse(primary: usize, transformed: &[u8]) -> Result<Vec<u8>, String> {
    let n = transformed.len();
    if n == 0 {
        return if matches!(primary, 0 | 1) {
            Ok(Vec::new())
        } else {
            Err("invalid empty BWT primary index".into())
        };
    }
    if primary == 0 || primary > n {
        return Err("invalid BWT primary index".into());
    }
    let last: Vec<(u8, usize)> = transformed
        .iter()
        .copied()
        .enumerate()
        .map(|(index, byte)| (byte, index))
        .collect();
    let mut first = last.clone();
    first.sort_unstable();
    let mut inverse_last = vec![0usize; n];
    for (index, &(_, original_index)) in last.iter().enumerate() {
        inverse_last[original_index] = index + usize::from(index >= primary);
    }
    let mut current = first[primary - 1];
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(current.0);
        let index = inverse_last[current.1];
        current = first[if index == 0 { n - 1 } else { index - 1 }];
    }
    Ok(out)
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
    let mut out = vec![0; size - bytes.len()];
    out.extend(bytes);
    out
}

fn colex_rank(positions: &[usize]) -> Result<BigUint, String> {
    combinatorics::colex_rank_checked(positions)
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
    .ok_or("BWT zigzag overflow")?;
    usize::try_from(encoded).map_err(|_| "BWT zigzag outside machine range".into())
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

fn suffix_rank_pair(ranks: &[i64], index: usize, width: usize) -> (i64, i64) {
    (
        ranks[index],
        ranks.get(index + width).copied().unwrap_or(-1),
    )
}

fn advance_suffix_ranks(suffixes: &[usize], ranks: &[i64], width: usize) -> Vec<i64> {
    let mut next = vec![0i64; ranks.len()];
    let mut class = 0i64;
    let mut previous = None;
    for &index in suffixes {
        let pair = suffix_rank_pair(ranks, index, width);
        if previous.is_some() && previous != Some(pair) {
            class += 1;
        }
        next[index] = class;
        previous = Some(pair);
    }
    next
}

fn bwt_output(data: &[u8], suffixes: &[usize]) -> Result<(usize, Vec<u8>), String> {
    let primary = suffixes
        .iter()
        .position(|&index| index == 0)
        .ok_or("BWT suffix array omitted source start")?
        + 1;
    let mut transformed = Vec::with_capacity(data.len());
    transformed.push(data[data.len() - 1]);
    transformed.extend(
        suffixes
            .iter()
            .filter(|&&index| index > 0)
            .map(|&index| data[index - 1]),
    );
    if transformed.len() != data.len() {
        return Err("invalid BWT transform size".into());
    }
    Ok((primary, transformed))
}

fn bwt_transform(data: &[u8]) -> Result<(usize, Vec<u8>), String> {
    let n = data.len();
    if n == 0 {
        return Ok((0, Vec::new()));
    }
    let mut suffixes: Vec<usize> = (0..n).collect();
    let mut ranks: Vec<i64> = data.iter().map(|&byte| byte as i64).collect();
    let mut width = 1usize;
    loop {
        suffixes.sort_unstable_by_key(|&index| suffix_rank_pair(&ranks, index, width));
        ranks = advance_suffix_ranks(&suffixes, &ranks, width);
        if ranks.iter().copied().max() == Some((n - 1) as i64) {
            break;
        }
        width = width.checked_mul(2).ok_or("BWT suffix width overflow")?;
    }
    bwt_output(data, &suffixes)
}

fn mtf_encode(data: &[u8]) -> Vec<u8> {
    let mut symbols: Vec<u8> = (0..=255).collect();
    let mut positions: Vec<usize> = (0..256).collect();
    let mut out = Vec::with_capacity(data.len());
    for &byte in data {
        let index = positions[byte as usize];
        out.push(index as u8);
        if index > 0 {
            let moved = symbols[index];
            for position in (1..=index).rev() {
                symbols[position] = symbols[position - 1];
                positions[symbols[position] as usize] = position;
            }
            symbols[0] = moved;
            positions[moved as usize] = 0;
        }
    }
    out
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

fn adaptive_bucket(history: &[u8], order: usize, bucket_bits: usize) -> usize {
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
    let mut encoder = rank::ArithmeticEncoder::new();
    for &symbol in data {
        let bucket = adaptive_bucket(&history, order, bucket_bits);
        let model = models.entry(bucket).or_insert_with(AdaptiveModel::new);
        let (low, high, total) = model.interval(symbol as usize);
        encoder.encode(low, high, total);
        model.observe(symbol as usize);
        if order > 0 {
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

fn encode_wavelet_node(values: &[u8], depth: usize, encoder: &mut rank::ArithmeticEncoder) {
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
    let mut encoder = rank::ArithmeticEncoder::new();
    encode_wavelet_node(data, 0, &mut encoder);
    let (payload, bit_length) = encoder.finish();
    let mut out = vec![1];
    put_varint(bit_length, &mut out);
    out.extend(payload);
    out
}

fn estimate_wavelet_node(counts: &[usize; 256], lo: usize, hi: usize, depth: usize) -> f64 {
    let n: usize = counts[lo..hi].iter().sum();
    if n == 0 || depth == 8 {
        return 0.0;
    }
    let middle = (lo + hi) / 2;
    let ones: usize = counts[middle..hi].iter().sum();
    let mut bits = (n as f64 + 1.0).log2();
    if ones > 0 && ones < n {
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
    if order == 0 {
        let mut counts = [0usize; 256];
        for &value in data {
            counts[value as usize] += 1;
        }
        let n = data.len() as f64;
        return n * n.log2()
            - counts
                .iter()
                .filter(|&&count| count > 0)
                .map(|&count| count as f64 * (count as f64).log2())
                .sum::<f64>();
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
                    .filter(|&&count| count > 0)
                    .map(|&count| count as f64 * (count as f64).log2())
                    .sum::<f64>()
        })
        .sum()
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
        let shifted = value.checked_add(1).ok_or("BWT integer class overflow")?;
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
        if width > 0 {
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
    let payload_limit_bits = best_len.saturating_sub(2) * 8;
    let mut bit_cost = 0usize;
    for &value in values {
        bit_cost = bit_cost
            .checked_add(value >> width)
            .and_then(|cost| cost.checked_add(1 + width))
            .unwrap_or(usize::MAX);
        if bit_cost >= payload_limit_bits {
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
            writer.bits(value & ((1usize << width) - 1), width);
        }
    }
    let mut candidate = vec![1, width as u8];
    candidate.extend(writer.finish());
    Some(candidate)
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
        return Err("empty BWT count vector".into());
    }
    let active: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(symbol, &count)| (count > 0).then_some(symbol))
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
        let current = count as i128;
        deltas.push(zigzag(current - previous)?);
        previous = current;
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
        let current = count as i128;
        deltas.push(zigzag(current - previous)?);
        previous = current;
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

fn bwt_counts(data: &[u8]) -> [usize; 256] {
    let mut counts = [0usize; 256];
    for &value in data {
        counts[value as usize] += 1;
    }
    counts
}

fn bwt_histogram_header_size(total: usize, active: &[usize]) -> Result<usize, String> {
    let mut header_size = {
        let mut encoded = Vec::new();
        put_varint(active.len(), &mut encoded);
        encoded.len()
    };
    if active.len() < 256 {
        header_size +=
            fixed_width(&(combinatorics::choose_checked(256, active.len())? - BigUint::one()))
                .div_ceil(8);
    }
    header_size += fixed_width(
        &(combinatorics::choose_checked(total - 1, active.len() - 1)? - BigUint::one()),
    )
    .div_ceil(8);
    Ok(header_size)
}

fn histogram_candidate(data: &[u8], incumbent: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if data.len() < 512 {
        return Ok(None);
    }
    let counts = bwt_counts(data);
    let active: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(symbol, &count)| (count > 0).then_some(symbol))
        .collect();
    if active.len() < 2 {
        return Ok(None);
    }
    let header_size = bwt_histogram_header_size(data.len(), &active)?;
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

fn encode_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let static_payload = rank::encode_type_class(data)?;
    let mut best = vec![0];
    best.extend(&static_payload);
    if data.is_empty() {
        return Ok(best);
    }
    for chunk_size in [128usize, 256, 512, 1024, 2048, 4096] {
        if let Some(candidate) = chunked_candidate(data, chunk_size)? {
            consider_smaller(&mut best, candidate);
        }
    }
    if data.len() >= 64 && estimate_wavelet_bits(data) / 8.0 <= best.len() as f64 * 1.03 + 8.0 {
        let mut candidate = vec![4];
        candidate.extend(encode_wavelet(data));
        consider_smaller(&mut best, candidate);
    }
    if data.len() >= 256 {
        let mut candidate = vec![2];
        candidate.extend(encode_adaptive(data, 0, 8));
        consider_smaller(&mut best, candidate);
        consider_context_candidates(data, &mut best);
    }
    if let Some(histogram) = histogram_candidate(data, &static_payload)? {
        let mut candidate = vec![5];
        candidate.extend(histogram);
        consider_smaller(&mut best, candidate);
    }
    Ok(best)
}

fn consider_smaller(best: &mut Vec<u8>, candidate: Vec<u8>) {
    if candidate.len() < best.len() {
        *best = candidate;
    }
}

fn chunked_candidate(data: &[u8], chunk_size: usize) -> Result<Option<Vec<u8>>, String> {
    if data.len() < chunk_size * 2 {
        return Ok(None);
    }
    let mut candidate = vec![1];
    put_varint(chunk_size, &mut candidate);
    for chunk in data.chunks(chunk_size) {
        candidate.extend(rank::encode_type_class(chunk)?);
    }
    Ok(Some(candidate))
}

fn consider_context_candidates(data: &[u8], best: &mut Vec<u8>) {
    let orders: &[usize] = if data.len() >= 2048 { &[1, 2] } else { &[1] };
    for &order in orders {
        if conditional_bits(data, order) / 8.0 <= best.len() as f64 * 1.10 + 16.0 {
            let mut candidate = vec![3, order as u8];
            candidate.extend(encode_adaptive(
                data,
                order,
                if order == 1 { 8 } else { 10 },
            ));
            consider_smaller(best, candidate);
        }
    }
}

fn split_zero_runs(data: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<usize>) {
    let (mut tags, mut nonzero, mut runs) = (Vec::new(), Vec::new(), Vec::new());
    let mut position = 0usize;
    while position < data.len() {
        if data[position] != 0 {
            tags.push(1);
            nonzero.push(data[position]);
            position += 1;
            continue;
        }
        let mut stop = position + 1;
        while stop < data.len() && data[stop] == 0 {
            stop += 1;
        }
        tags.push(0);
        runs.push(stop - position - 1);
        position = stop;
    }
    (tags, nonzero, runs)
}

fn encode_binary_tags(tags: &[u8]) -> Result<Vec<u8>, String> {
    let mut best = vec![0];
    best.extend(encode_bytes(tags)?);
    if tags.is_empty() {
        return Ok(best);
    }
    let mut runs = Vec::new();
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

fn pack_stream_variant(
    version: u8,
    primary: usize,
    tags: &[u8],
    nonzero: &[u8],
    runs: &[usize],
    streams: [&[u8]; 3],
) -> Vec<u8> {
    let mut out = vec![version];
    put_varint(primary, &mut out);
    put_varint(tags.len(), &mut out);
    put_varint(nonzero.len(), &mut out);
    put_varint(runs.len(), &mut out);
    for stream in streams {
        put_varint(stream.len(), &mut out);
        out.extend_from_slice(stream);
    }
    out
}

fn push_runa_zero(tokens: &mut Vec<u8>, value: usize) {
    let mut value = value;
    loop {
        tokens.push(if value & 1 == 0 { 0 } else { 1 });
        if value < 2 {
            return;
        }
        value = (value - 2) / 2;
    }
}

fn push_runa_nonzero(tokens: &mut Vec<u8>, side_bits: &mut Vec<usize>, rank: u8) {
    if rank <= 253 {
        tokens.push(rank + 1);
    } else {
        tokens.push(255);
        side_bits.push((rank - 254) as usize);
    }
}

fn runa_runb_tokens(mtf: &[u8]) -> (Vec<u8>, Vec<usize>) {
    let (mut tokens, mut side_bits) = (Vec::new(), Vec::new());
    let mut position = 0usize;
    while position < mtf.len() {
        if mtf[position] == 0 {
            let mut stop = position + 1;
            while stop < mtf.len() && mtf[stop] == 0 {
                stop += 1;
            }
            push_runa_zero(&mut tokens, stop - position - 1);
            position = stop;
            continue;
        }
        push_runa_nonzero(&mut tokens, &mut side_bits, mtf[position]);
        position += 1;
    }
    (tokens, side_bits)
}

fn pack_runa_runb(
    version: u8,
    primary: usize,
    tokens: &[u8],
    side_bits: &[usize],
    token_blob: &[u8],
) -> Vec<u8> {
    let mut writer = BitWriter::new();
    for &bit in side_bits {
        writer.bit(bit);
    }
    let mut out = vec![version];
    put_varint(primary, &mut out);
    put_varint(tokens.len(), &mut out);
    put_varint(side_bits.len(), &mut out);
    put_varint(token_blob.len(), &mut out);
    out.extend_from_slice(token_blob);
    out.extend(writer.finish());
    out
}

fn encode_from_mtf(primary: usize, mtf: &[u8], version: u8) -> Result<Option<Vec<u8>>, String> {
    let (tags, nonzero, runs) = split_zero_runs(mtf);
    if matches!(version, 1..=4) {
        let tag_blob = match version {
            1 => rank::encode_type_class(&tags)?,
            4 => encode_binary_tags(&tags)?,
            _ => encode_bytes(&tags)?,
        };
        let nonzero_blob = match version {
            1 => rank::encode_type_class(&nonzero)?,
            2 => encode_bytes(&nonzero)?,
            _ => {
                let mut byte_candidate = vec![0];
                byte_candidate.extend(encode_bytes(&nonzero)?);
                let values: Vec<usize> = nonzero.iter().map(|&value| value as usize - 1).collect();
                let mut uint_candidate = vec![1];
                uint_candidate.extend(encode_uints(&values)?);
                if uint_candidate.len() < byte_candidate.len() {
                    uint_candidate
                } else {
                    byte_candidate
                }
            }
        };
        let run_blob = encode_uints(&runs)?;
        return Ok(Some(pack_stream_variant(
            version,
            primary,
            &tags,
            &nonzero,
            &runs,
            [&tag_blob, &nonzero_blob, &run_blob],
        )));
    }
    let (tokens, side_bits) = runa_runb_tokens(mtf);
    let local = encode_bytes(&tokens)?;
    if version == 5 {
        return Ok(Some(pack_runa_runb(
            5, primary, &tokens, &side_bits, &local,
        )));
    }
    if version != 6 {
        return Err("unknown requested BWT encoder version".into());
    }
    if tokens.len() < 32 {
        return Ok(None);
    }
    let mut profiled = Vec::new();
    for order in 1..=4 {
        profiled.push((super::ppm::estimate_bits(&tokens, order)?, order));
    }
    profiled.sort_by(|(bits_a, order_a), (bits_b, order_b)| {
        bits_a
            .partial_cmp(bits_b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(order_a.cmp(order_b))
    });
    if profiled[0].0 / 8.0 > local.len() as f64 * 1.03 + 16.0 {
        return Ok(None);
    }
    let ppm = super::ppm::encode_best(&tokens, profiled[0].1, &[], None, None, None)?;
    if ppm.len() >= local.len() {
        return Ok(None);
    }
    let mut token_blob = vec![1];
    token_blob.extend(ppm);
    Ok(Some(pack_runa_runb(
        6,
        primary,
        &tokens,
        &side_bits,
        &token_blob,
    )))
}

/// Encode a specific legacy BWT payload version. Version 6 returns `None` when
/// the frozen estimator or complete-size gate rejects its optional PPM route.
#[cfg(test)]
pub fn encode_version(data: &[u8], version: u8) -> Result<Option<Vec<u8>>, String> {
    if data.len() > MAX_ENCODE_BLOCK {
        return Err("legacy BWT block exceeds 65536-byte format bound".into());
    }
    if !matches!(version, 1..=6) {
        return Err("unknown requested BWT encoder version".into());
    }
    let (primary, transformed) = bwt_transform(data)?;
    encode_from_mtf(primary, &mtf_encode(&transformed), version)
}

/// Frozen canonical CIXM6 mode-4 encoder. It compares complete v4/v5/v6
/// payloads and retains the first candidate on equal byte length.
pub fn encode(data: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    if data.len() > MAX_ENCODE_BLOCK {
        return Err("legacy BWT block exceeds 65536-byte format bound".into());
    }
    let (primary, transformed) = bwt_transform(data)?;
    let mtf = mtf_encode(&transformed);
    let mut best = (
        encode_from_mtf(primary, &mtf, 4)?.ok_or("missing BWT v4 candidate")?,
        "bwt-mtf-runs",
    );
    let candidate = encode_from_mtf(primary, &mtf, 5)?.ok_or("missing BWT v5 candidate")?;
    if candidate.len() < best.0.len() {
        best = (candidate, "bwt-mtf-runa-runb");
    }
    if let Some(candidate) = encode_from_mtf(primary, &mtf, 6)? {
        if candidate.len() < best.0.len() {
            best = (candidate, "bwt-mtf-runa-runb-ppm");
        }
    }
    Ok(best)
}

/// Decode one CIXM6 mode-4 payload.
///
/// `model_limit` bounds the number of adaptive context models used by nested
/// byte substreams. Callers should derive it from their total working-memory
/// budget, as the other legacy decoders do.
fn decode_runa_payload(
    blob: &[u8],
    pos: &mut usize,
    version: u8,
    source_length: usize,
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    let token_count = read_varint(blob, pos)?;
    let side_count = read_varint(blob, pos)?;
    if token_count > source_length || side_count > token_count {
        return Err("invalid BWT RUNA/RUNB stream counts".into());
    }
    let token_length = read_varint(blob, pos)?;
    let token_end = pos
        .checked_add(token_length)
        .ok_or("BWT token length overflow")?;
    if token_end > blob.len() {
        return Err("truncated BWT RUNA/RUNB token stream".into());
    }
    let tokens = if version == 5 {
        decode_bytes(&blob[*pos..token_end], token_count, model_limit)?
    } else {
        let stream = &blob[*pos..token_end];
        match stream.first() {
            Some(0) => decode_bytes(&stream[1..], token_count, model_limit)?,
            Some(1) => super::ppm::decode(&stream[1..], token_count)?,
            Some(_) => return Err("unknown BWT token coding mode".into()),
            None => return Err("missing BWT token coding mode".into()),
        }
    };
    *pos = token_end;
    let side_bytes = side_count.div_ceil(8);
    if pos.checked_add(side_bytes) != Some(blob.len()) {
        return Err("BWT RUNA/RUNB side length mismatch".into());
    }
    let mut reader = BitReader::new(&blob[*pos..]);
    let mut side_bits = Vec::with_capacity(side_count);
    for _ in 0..side_count {
        side_bits.push(reader.bit()?);
    }
    let decoded = decode_runa_runb(&tokens, &side_bits, source_length)?;
    if decoded.len() != source_length {
        return Err("BWT RUNA/RUNB transformed length mismatch".into());
    }
    Ok(decoded)
}

fn decode_type_class_exact(blob: &[u8], count: usize, label: &str) -> Result<Vec<u8>, String> {
    let mut pos = 0;
    let values = rank::decode_type_class(blob, &mut pos, count)?;
    if pos != blob.len() {
        return Err(format!("unused BWT {label} bytes"));
    }
    Ok(values)
}

fn decode_legacy_tags(
    version: u8,
    stream: &[u8],
    count: usize,
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    if version == 1 {
        decode_type_class_exact(stream, count, "tag")
    } else if version >= 4 {
        decode_binary_tags(stream, count, model_limit)
    } else {
        decode_bytes(stream, count, model_limit)
    }
}

fn decode_legacy_nonzero(
    version: u8,
    stream: &[u8],
    count: usize,
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    if version == 1 {
        return decode_type_class_exact(stream, count, "nonzero");
    }
    if version == 2 {
        return decode_bytes(stream, count, model_limit);
    }
    match stream.first() {
        Some(0) => decode_bytes(&stream[1..], count, model_limit),
        Some(1) => decode_mtf_ranks(decode_uints(&stream[1..], count, model_limit)?),
        Some(_) => Err("unknown BWT nonzero stream mode".into()),
        None => Err("missing BWT nonzero stream mode".into()),
    }
}

fn decode_mtf_ranks(values: Vec<usize>) -> Result<Vec<u8>, String> {
    values
        .into_iter()
        .map(|value| {
            if value > 254 {
                return Err("BWT MTF rank outside byte range".into());
            }
            Ok((value + 1) as u8)
        })
        .collect()
}

fn decode_legacy_payload(
    blob: &[u8],
    pos: &mut usize,
    version: u8,
    source_length: usize,
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    let tag_count = read_varint(blob, pos)?;
    let nonzero_count = read_varint(blob, pos)?;
    let run_count = read_varint(blob, pos)?;
    if tag_count > source_length || nonzero_count > source_length || run_count > source_length {
        return Err("invalid BWT substream counts".into());
    }
    let mut streams: Vec<&[u8]> = Vec::with_capacity(3);
    for _ in 0..3 {
        let length = read_varint(blob, pos)?;
        let end = pos
            .checked_add(length)
            .ok_or("BWT substream length overflow")?;
        if end > blob.len() {
            return Err("truncated BWT substream".into());
        }
        streams.push(&blob[*pos..end]);
        *pos = end;
    }
    if *pos != blob.len() {
        return Err("trailing BWT bytes".into());
    }
    let tags = decode_legacy_tags(version, streams[0], tag_count, model_limit)?;
    let nonzero = decode_legacy_nonzero(version, streams[1], nonzero_count, model_limit)?;
    let runs = decode_uints(streams[2], run_count, model_limit)?;
    let decoded = join_zero_runs(&tags, &nonzero, &runs, source_length)?;
    if decoded.len() != source_length {
        return Err("BWT transformed length mismatch".into());
    }
    Ok(decoded)
}

pub fn decode(blob: &[u8], source_length: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    let version = *blob.first().ok_or("empty BWT block")?;
    if !matches!(version, 1..=6) {
        return Err("unknown BWT block version".into());
    }
    let mut pos = 1;
    let primary = read_varint(blob, &mut pos)?;
    let mtf = if matches!(version, 5 | 6) {
        decode_runa_payload(blob, &mut pos, version, source_length, model_limit)?
    } else {
        decode_legacy_payload(blob, &mut pos, version, source_length, model_limit)?
    };
    let restored = bwt_inverse(primary, &mtf_decode(&mtf))?;
    if restored.len() != source_length {
        return Err("BWT source length mismatch".into());
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    #[test]
    fn malformed_nonzero_rank_does_not_overflow() {
        for value in [255, usize::MAX] {
            let mut stream = vec![1, 0];
            super::put_varint(value, &mut stream);
            assert_eq!(
                super::decode_legacy_nonzero(3, &stream, 1, 1024).unwrap_err(),
                "BWT MTF rank outside byte range"
            );
        }
        #[cfg(target_pointer_width = "64")]
        {
            let blob = [
                3, 0, 0, 1, 0, 2, 0, 0, 12, 1, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 1,
                1, 0,
            ];
            assert_eq!(
                super::decode(&blob, 1, 1024).unwrap_err(),
                "BWT MTF rank outside byte range"
            );
        }
    }
    use super::{decode, decode_mtf_ranks};
    use std::path::PathBuf;

    #[test]
    fn frozen_python_fixtures() {
        let Some(path) = std::env::var_os("CIX_BWT_FIXTURES") else {
            return;
        };
        let data = std::fs::read(PathBuf::from(path)).expect("read Python BWT fixtures");
        assert!(data.starts_with(b"CIXBWT1\0"));
        let mut pos = 8usize;
        let count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        for _ in 0..count {
            let source_length = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let payload_length =
                u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let source = &data[pos..pos + source_length];
            pos += source_length;
            let payload = &data[pos..pos + payload_length];
            pos += payload_length;
            assert_eq!(decode(payload, source_length, 1 << 16).unwrap(), source);
        }
        assert_eq!(pos, data.len());
    }

    #[test]
    fn rejects_bad_version_and_truncation() {
        assert!(decode(&[], 0, 16).is_err());
        assert!(decode(&[7], 0, 16).is_err());
        assert!(decode(&[5, 1], 1, 16).is_err());
    }

    #[test]
    fn rejects_mtf_uint_overflow_before_conversion() {
        assert!(decode_mtf_ranks(vec![255]).is_err());
        assert!(decode_mtf_ranks(vec![usize::MAX]).is_err());
    }

    #[test]
    fn rice_uints_reject_high_bit_overflow() {
        let width = usize::BITS as usize - 1;
        let mut valid_bits = super::BitWriter::new();
        valid_bits.bit(0);
        valid_bits.bits(usize::MAX >> 1, width);
        let mut valid = vec![1, width as u8];
        valid.extend(valid_bits.finish());
        assert_eq!(
            super::decode_rice_uints(&valid, 1).unwrap(),
            vec![usize::MAX >> 1]
        );

        let mut overflow_bits = super::BitWriter::new();
        overflow_bits.bit(1);
        overflow_bits.bit(1);
        overflow_bits.bit(0);
        overflow_bits.bits(0, width);
        let mut overflow = vec![1, width as u8];
        overflow.extend(overflow_bits.finish());
        assert_eq!(
            super::decode_rice_uints(&overflow, 1).unwrap_err(),
            "BWT Rice value overflow"
        );
    }

    #[test]
    fn legacy_round_trip_after_helper_extraction() {
        let source =
            b"legacy bwt helper extraction round trip; legacy bwt helper extraction round trip";
        let (payload, _) = super::encode(source).unwrap();
        assert_eq!(decode(&payload, source.len(), 1 << 16).unwrap(), source);
    }

    #[test]
    fn frozen_python_encoder_fixtures() {
        let Some(path) = std::env::var_os("CIX_BWT_ENCODE_FIXTURES") else {
            return;
        };
        let data = std::fs::read(PathBuf::from(path)).expect("read Python BWT encoder fixtures");
        assert!(data.starts_with(b"CIXBWTE1"));
        let mut pos = 8usize;
        let count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        for _ in 0..count {
            let source_length = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let canonical_length =
                u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let description_length = data[pos] as usize;
            pos += 1;
            let source = &data[pos..pos + source_length];
            pos += source_length;
            let canonical = &data[pos..pos + canonical_length];
            pos += canonical_length;
            let description = std::str::from_utf8(&data[pos..pos + description_length]).unwrap();
            pos += description_length;
            let (payload, actual_description) = super::encode(source).unwrap();
            assert_eq!(payload, canonical);
            assert_eq!(actual_description, description);
            assert_eq!(decode(&payload, source.len(), 1 << 16).unwrap(), source);
            for version in 1..=6u8 {
                let payload_length = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
                pos += 4;
                let expected = if payload_length == u32::MAX {
                    None
                } else {
                    let length = payload_length as usize;
                    let value = data[pos..pos + length].to_vec();
                    pos += length;
                    Some(value)
                };
                let actual = super::encode_version(source, version).unwrap();
                assert_eq!(actual, expected, "BWT encoder version {version}");
            }
        }
        assert_eq!(pos, data.len());
    }
}
