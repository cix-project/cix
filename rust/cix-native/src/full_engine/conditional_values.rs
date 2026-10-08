//! Coordinate-grouped corrections for the historical `SGCC2` grid raw stream.
//!
//! This is the transform beneath the `CIXB\x21` PAQ envelope.  It deliberately
//! stops at the raw representation: frame/backend dispatch remains a separate
//! qualification concern.  The implemented successful path is context 3,
//! coding 0, with the complete paid bounded-integer selector needed to match
//! the retained source stream.

use super::arithmetic::{vdecode, vencode};
use super::strided_values::{inverse_wcs_grid_numeric, transform_wcs_grid_numeric, GRID_RAW_MAGIC};
use num_bigint::BigUint;
use num_traits::{One, ToPrimitive, Zero};
use std::collections::{BTreeMap, HashMap};

pub const CONDITIONAL_RAW_MAGIC: &[u8] = b"SGCC2";
pub const GROUPED_MAGIC: &[u8] = b"CIXB\x21";
pub type EncodeBackend<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a;
pub type DecodeBackend<'a> = dyn Fn(&[u8], usize) -> Result<Vec<u8>, String> + 'a;
const INTEGER_BLOCK: usize = 4096;
const MAX_RECORDS: usize = 8_000_000;
const FIELD_WIDTHS: [usize; 4] = [8, 8, 4, 4];
const MAX_RAW_INVERSE_BYTES: usize = 256 * 1024 * 1024;
const INVERSE_WORKING_MULTIPLIER: usize = 7;
type BaseField<'a> = (Vec<i64>, Vec<i64>, &'a [u8], usize);

pub(crate) fn signed(value: i64) -> Result<Vec<u8>, String> {
    let code = if value >= 0 {
        u64::try_from(value)
            .map_err(|_| "signed overflow")?
            .checked_mul(2)
            .ok_or("signed overflow")?
    } else {
        value
            .unsigned_abs()
            .checked_mul(2)
            .and_then(|item| item.checked_sub(1))
            .ok_or("signed overflow")?
    };
    Ok(vencode(code))
}

pub(crate) fn read_signed(data: &[u8], at: usize) -> Result<(i64, usize), String> {
    let (code, next) = vdecode(data, at)?;
    let value = if code & 1 == 0 {
        i64::try_from(code / 2).map_err(|_| "signed overflow")?
    } else {
        i64::try_from(code / 2)
            .map_err(|_| "signed overflow")?
            .checked_neg()
            .and_then(|item| item.checked_sub(1))
            .ok_or("signed overflow")?
    };
    Ok((value, next))
}

pub(crate) fn pack(value: &[u8]) -> Vec<u8> {
    let mut out = vencode(value.len() as u64);
    out.extend_from_slice(value);
    out
}
pub(crate) fn read(data: &[u8], at: usize) -> Result<(&[u8], usize), String> {
    let (length, start) = vdecode(data, at)?;
    let length = usize::try_from(length).map_err(|_| "conditional field length overflow")?;
    let end = start
        .checked_add(length)
        .ok_or("conditional field length overflow")?;
    Ok((
        data.get(start..end).ok_or("truncated conditional field")?,
        end,
    ))
}

pub(crate) fn base_prefix(data: &[u8]) -> Result<(usize, usize), String> {
    if !data.starts_with(GRID_RAW_MAGIC) || data.get(5) != Some(&1) {
        return Err("expected raw-palette grid stream".into());
    }
    let (n, mut at) = vdecode(data, 6)?;
    let n = usize::try_from(n).map_err(|_| "conditional record count overflow")?;
    if n == 0 || n > MAX_RECORDS {
        return Err("conditional record count exceeds limit".into());
    }
    let tail = usize::from(*data.get(at).ok_or("truncated grid prefix")?);
    at += 1;
    if tail != 0 {
        return Err("unsupported conditional grid tail".into());
    }
    at = at
        .checked_add(28)
        .and_then(|value| value.checked_add(4usize.checked_mul(n)?))
        .ok_or("truncated grid prefix")?;
    if at > data.len() {
        return Err("truncated grid prefix".into());
    }
    Ok((n, at))
}

pub(crate) fn decode_q(data: &[u8], n: usize) -> Result<Vec<i64>, String> {
    let mut out = Vec::new();
    out.try_reserve(n).map_err(|_| "coordinate allocation")?;
    let mut at = 0;
    let mut previous = 0i64;
    for _ in 0..n {
        let (delta, next) = read_signed(data, at)?;
        at = next;
        previous = previous
            .checked_add(delta)
            .ok_or("quantized coordinate overflow")?;
        if previous <= i64::from(i32::MIN) || previous >= i64::from(i32::MAX) {
            return Err("quantized coordinate outside range".into());
        }
        out.push(previous);
    }
    if at != data.len() {
        return Err("coordinate stream length mismatch".into());
    }
    Ok(out)
}

pub(crate) fn q_bytes(values: &[i64]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut previous = 0i64;
    for value in values {
        out.extend(signed(
            value
                .checked_sub(previous)
                .ok_or("quantized coordinate overflow")?,
        )?);
        previous = *value;
    }
    Ok(out)
}

fn signed_le(value: i64, width: usize) -> Result<Vec<u8>, String> {
    match width {
        4 => Ok(i32::try_from(value)
            .map_err(|_| "correction outside range")?
            .to_le_bytes()
            .to_vec()),
        8 => Ok(value.to_le_bytes().to_vec()),
        _ => Err("unsupported correction width".into()),
    }
}
fn read_signed_le(data: &[u8], width: usize) -> Result<Vec<i64>, String> {
    if !data.len().is_multiple_of(width) {
        return Err("raw correction size".into());
    }
    match width {
        4 => Ok(data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|item| i64::from(i32::from_le_bytes(*item)))
            .collect()),
        8 => Ok(data
            .as_chunks::<8>()
            .0
            .iter()
            .map(|item| i64::from_le_bytes(*item))
            .collect()),
        _ => Err("unsupported correction width".into()),
    }
}

pub(crate) fn base_field(
    data: &[u8],
    at: usize,
    n: usize,
    width: usize,
) -> Result<BaseField<'_>, String> {
    let start = at;
    let scale_end = at.checked_add(8).ok_or("grid scale overflow")?;
    data.get(at..scale_end).ok_or("truncated grid scale")?;
    let (q_size, q_start) = vdecode(data, scale_end)?;
    let q_size = usize::try_from(q_size).map_err(|_| "coordinate size overflow")?;
    let q_end = q_start
        .checked_add(q_size)
        .ok_or("coordinate size overflow")?;
    let q = decode_q(
        data.get(q_start..q_end)
            .ok_or("truncated coordinate stream")?,
        n,
    )?;
    let prefix = data.get(start..q_end).ok_or("truncated grid field")?;
    let coding = *data.get(q_end).ok_or("missing correction coding")?;
    let mut pos = q_end + 1;
    let values = if coding == 0 {
        let (size, next) = vdecode(data, pos)?;
        pos = next;
        let size = usize::try_from(size).map_err(|_| "raw correction length")?;
        let end = pos.checked_add(size).ok_or("raw correction length")?;
        let raw = data.get(pos..end).ok_or("truncated raw corrections")?;
        pos = end;
        if raw.len() != n.checked_mul(width).ok_or("raw correction length")? {
            return Err("raw correction size".into());
        }
        read_signed_le(raw, width)?
    } else if coding == 1 {
        let (count, next) = vdecode(data, pos)?;
        pos = next;
        let count = usize::try_from(count).map_err(|_| "correction palette overflow")?;
        if count > 256 || (count == 0 && n != 0) {
            return Err("invalid correction palette".into());
        }
        let mut palette = Vec::new();
        palette
            .try_reserve(count)
            .map_err(|_| "correction palette allocation")?;
        for _ in 0..count {
            let (value, next) = read_signed(data, pos)?;
            pos = next;
            palette.push(value);
        }
        let (size, next) = vdecode(data, pos)?;
        pos = next;
        let size = usize::try_from(size).map_err(|_| "correction payload length")?;
        let end = pos.checked_add(size).ok_or("correction payload length")?;
        let ids = data.get(pos..end).ok_or("truncated corrections")?;
        pos = end;
        if ids.len() != n || ids.iter().any(|id| usize::from(*id) >= palette.len()) {
            return Err("invalid correction IDs".into());
        }
        ids.iter().map(|id| palette[usize::from(*id)]).collect()
    } else {
        return Err("invalid correction coding".into());
    };
    Ok((q, values, prefix, pos))
}

pub(crate) fn feedback(
    q: &[i64],
    values: &[i64],
    width: usize,
    reverse: bool,
) -> Result<Vec<i64>, String> {
    if q.len() != values.len() {
        return Err("feedback length mismatch".into());
    }
    let (modulus, half) = if width == 4 {
        (1i128 << 32, 1i128 << 31)
    } else if width == 8 {
        (1i128 << 64, 1i128 << 63)
    } else {
        return Err("unsupported correction width".into());
    };
    let mut history = HashMap::<i64, i64>::new();
    let mut out = Vec::new();
    out.try_reserve(values.len())
        .map_err(|_| "feedback allocation")?;
    for (coordinate, value) in q.iter().zip(values) {
        let prediction = history.get(coordinate).copied().unwrap_or(0);
        let actual = if reverse {
            let raw =
                (i128::from(*value) + i128::from(prediction) + half).rem_euclid(modulus) - half;
            i64::try_from(raw).map_err(|_| "feedback range")?
        } else {
            *value
        };
        let error = (i128::from(*value) - i128::from(prediction) + half).rem_euclid(modulus) - half;
        let error = i64::try_from(error).map_err(|_| "feedback range")?;
        out.push(if reverse { actual } else { error });
        history.insert(*coordinate, actual);
    }
    Ok(out)
}

fn palette(values: &[i64]) -> Vec<i64> {
    let mut counts = BTreeMap::<i64, usize>::new();
    for value in values {
        *counts.entry(*value).or_default() += 1;
    }
    let mut out = counts.into_iter().collect::<Vec<_>>();
    out.sort_by_key(|(value, count)| (std::cmp::Reverse(*count), *value));
    out.into_iter().map(|(value, _)| value).collect()
}

fn bit_length(value: u64) -> i64 {
    if value == 0 {
        0
    } else {
        i64::from(64 - value.leading_zeros())
    }
}

fn partitions(q: &[i64], context: u8) -> Result<Vec<Vec<usize>>, String> {
    if context > 3 {
        return Err("invalid coordinate partition".into());
    }
    let mut map = BTreeMap::<Vec<i64>, Vec<usize>>::new();
    for (index, value) in q.iter().enumerate() {
        let key = match context {
            0 => vec![0],
            1 => vec![i64::from(*value < 0), bit_length(value.unsigned_abs())],
            2 => vec![
                i64::from(*value < 0),
                bit_length(value.unsigned_abs()),
                value.rem_euclid(16),
            ],
            _ => vec![*value],
        };
        map.entry(key).or_default().push(index);
    }
    Ok(map.into_values().collect())
}

fn encode_corrections(
    q: &[i64],
    residual: &[i64],
    width: usize,
    context: u8,
) -> Result<Vec<u8>, String> {
    if q.len() != residual.len() {
        return Err("correction length mismatch".into());
    }
    let palette = palette(residual);
    // Historical fallback is an ordinary mode-one feedback field, preceded by
    // selector zero; it is not a raw little-endian residue payload.
    if palette.len() > 256 {
        let mut out = vec![0];
        out.extend(write_base_field(residual, width)?);
        return Ok(out);
    }
    let mut lookup = BTreeMap::new();
    for (index, value) in palette.iter().enumerate() {
        lookup.insert(
            *value,
            u8::try_from(index).map_err(|_| "correction palette")?,
        );
    }
    let mut out = vec![1];
    out.extend(vencode(palette.len() as u64));
    for value in &palette {
        out.extend(signed(*value)?);
    }
    for partition in partitions(q, context)? {
        for index in partition {
            out.push(*lookup.get(&residual[index]).ok_or("correction palette")?);
        }
    }
    Ok(out)
}

fn decode_corrections(
    data: &[u8],
    q: &[i64],
    width: usize,
    context: u8,
) -> Result<Vec<i64>, String> {
    let coding = *data.first().ok_or("missing conditional corrections")?;
    if coding == 0 {
        let qb = q_bytes(q)?;
        let mut temporary = vec![0; 8];
        temporary.extend(pack(&qb));
        temporary.extend_from_slice(data.get(1..).ok_or("truncated fallback")?);
        let (_, values, _, end) = base_field(&temporary, 0, q.len(), width)?;
        if end != temporary.len() {
            return Err("trailing fallback".into());
        }
        return Ok(values);
    }
    if coding != 1 {
        return Err("invalid correction representation".into());
    }
    let (count, mut at) = vdecode(data, 1)?;
    let count = usize::try_from(count).map_err(|_| "conditional palette overflow")?;
    if count > 256 || (count == 0 && !q.is_empty()) {
        return Err("invalid conditional palette".into());
    }
    let mut palette = Vec::new();
    palette
        .try_reserve(count)
        .map_err(|_| "conditional palette allocation")?;
    for _ in 0..count {
        let (value, next) = read_signed(data, at)?;
        at = next;
        palette.push(value);
    }
    let mut out = vec![0i64; q.len()];
    for partition in partitions(q, context)? {
        let end = at
            .checked_add(partition.len())
            .ok_or("conditional ID length")?;
        let ids = data.get(at..end).ok_or("truncated conditional IDs")?;
        at = end;
        for (index, id) in partition.into_iter().zip(ids) {
            out[index] = *palette
                .get(usize::from(*id))
                .ok_or("invalid conditional correction IDs")?;
        }
    }
    if at != data.len() {
        return Err("trailing correction groups".into());
    }
    Ok(out)
}

fn fold(value: i64) -> Result<u64, String> {
    if value >= 0 {
        u64::try_from(value)
            .map_err(|_| "Rice overflow".to_string())?
            .checked_mul(2)
            .ok_or_else(|| "Rice overflow".to_string())
    } else {
        value
            .unsigned_abs()
            .checked_mul(2)
            .and_then(|item| item.checked_sub(1))
            .ok_or_else(|| "Rice overflow".to_string())
    }
}
fn unfold(value: u64) -> Result<i64, String> {
    if value & 1 == 0 {
        i64::try_from(value / 2).map_err(|_| "Rice overflow".to_string())
    } else {
        i64::try_from(value / 2)
            .map_err(|_| "Rice overflow".to_string())?
            .checked_neg()
            .and_then(|item| item.checked_sub(1))
            .ok_or("Rice overflow".into())
    }
}

fn big_bytes(value: &BigUint, bits: usize) -> Vec<u8> {
    if bits == 0 {
        return Vec::new();
    }
    let mut out = value.to_bytes_be();
    let length = bits.div_ceil(8);
    if out.len() < length {
        let mut padded = vec![0; length - out.len()];
        padded.extend(out);
        out = padded;
    }
    out
}
pub(crate) fn rice(values: &[i64]) -> Result<Vec<u8>, String> {
    if values.is_empty() {
        return Err("empty Rice block".into());
    }
    let folded = values
        .iter()
        .map(|value| fold(*value))
        .collect::<Result<Vec<_>, _>>()?;
    let average = folded.iter().map(|value| *value as f64).sum::<f64>() / folded.len() as f64;
    let estimate = (average + 1.0).log2().floor().max(0.0) as usize
        - ((average + 1.0).log2().floor().max(0.0) as usize).min(1);
    let low = estimate.saturating_sub(2);
    let high = (estimate + 2).min(31);
    let mut winner = (usize::MAX, 0usize);
    for k in low..=high {
        let bits = folded
            .iter()
            .map(|value| {
                let quotient = value >> k;
                if quotient < 32 {
                    quotient as usize + 1 + k
                } else {
                    65
                }
            })
            .sum::<usize>();
        if (bits, k) < winner {
            winner = (bits, k);
        }
    }
    let (bits, k) = winner;
    let mut rank = BigUint::zero();
    for value in folded {
        let quotient = value >> k;
        let (width, word) = if quotient < 32 {
            (
                quotient as usize + 1 + k,
                (((1u64 << quotient) - 1) << (k + 1)) | (value & ((1u64 << k) - 1)),
            )
        } else {
            (65, ((1u64 << 32) - 1) << 33 | value)
        };
        rank = (rank << width) | BigUint::from(word);
    }
    let mut out = vec![u8::try_from(k).map_err(|_| "Rice parameter")?];
    out.extend(vencode(bits as u64));
    out.extend(big_bytes(&rank, bits));
    Ok(out)
}

pub(crate) fn unrice(data: &[u8], n: usize) -> Result<Vec<i64>, String> {
    let k = usize::from(*data.first().ok_or("invalid Rice frame")?);
    let (bits, at) = vdecode(data, 1)?;
    let bits = usize::try_from(bits).map_err(|_| "invalid Rice frame")?;
    if k > 31 || data.len().saturating_sub(at) != bits.div_ceil(8) {
        return Err("invalid Rice frame".into());
    }
    let rank = BigUint::from_bytes_be(&data[at..]);
    let mut remaining = bits;
    let mut out = Vec::new();
    out.try_reserve(n).map_err(|_| "Rice allocation")?;
    let take = |rank: &BigUint, remaining: &mut usize, width: usize| -> Result<u64, String> {
        if *remaining < width {
            return Err("short Rice payload".into());
        }
        *remaining -= width;
        ((rank >> *remaining) & ((BigUint::one() << width) - BigUint::one()))
            .to_u64()
            .ok_or("Rice value overflow".into())
    };
    for _ in 0..n {
        let mut quotient = 0usize;
        while quotient < 32 && take(&rank, &mut remaining, 1)? != 0 {
            quotient += 1;
        }
        let value = if quotient == 32 {
            take(&rank, &mut remaining, 33)?
        } else {
            (u64::try_from(quotient).map_err(|_| "Rice quotient")? << k)
                | take(&rank, &mut remaining, k)?
        };
        out.push(unfold(value)?);
    }
    if remaining != 0 {
        return Err("trailing Rice bits".into());
    }
    Ok(out)
}

fn interval(values: &[i64]) -> Result<Vec<u8>, String> {
    let lo = *values.iter().min().ok_or("empty interval")?;
    let hi = *values.iter().max().ok_or("empty interval")?;
    let span = hi
        .checked_sub(lo)
        .and_then(|value| value.checked_add(1))
        .ok_or("integer bound overflow")?;
    let span_u64 = u64::try_from(span).map_err(|_| "integer bound overflow")?;
    let mut rank = BigUint::zero();
    for value in values {
        rank = rank * span_u64
            + u64::try_from(value.checked_sub(lo).ok_or("integer bound overflow")?)
                .map_err(|_| "integer bound overflow")?;
    }
    let card = BigUint::from(span_u64)
        .pow(u32::try_from(values.len()).map_err(|_| "interval length overflow")?);
    let bits = (&card - BigUint::one()).bits() as usize;
    let mut out = signed(lo)?;
    out.extend(vencode(span_u64));
    out.extend(big_bytes(&rank, bits));
    Ok(out)
}

fn uninterval(data: &[u8], n: usize) -> Result<Vec<i64>, String> {
    let (lo, at) = read_signed(data, 0)?;
    let (span, at) = vdecode(data, at)?;
    if span == 0 || span >= (1u64 << 34) {
        return Err("invalid integer bound".into());
    }
    let card = BigUint::from(span).pow(u32::try_from(n).map_err(|_| "interval length overflow")?);
    let length = ((&card - BigUint::one()).bits() as usize).div_ceil(8);
    if data.len().saturating_sub(at) != length {
        return Err("interval rank length".into());
    }
    let mut rank = BigUint::from_bytes_be(&data[at..]);
    let mut out = vec![0i64; n];
    for index in (0..n).rev() {
        let digit = (&rank % span).to_u64().ok_or("interval digit")?;
        rank /= span;
        out[index] = lo
            .checked_add(i64::try_from(digit).map_err(|_| "interval digit")?)
            .ok_or("integer bound overflow")?;
    }
    if !rank.is_zero() {
        return Err("rank outside interval".into());
    }
    Ok(out)
}

pub(crate) fn encode_integers(values: &[i64]) -> Result<Vec<u8>, String> {
    let mut out = vencode(INTEGER_BLOCK as u64);
    for block in values.chunks(INTEGER_BLOCK) {
        let mut delta = Vec::with_capacity(block.len());
        for (index, value) in block.iter().enumerate() {
            delta.push(if index == 0 {
                *value
            } else {
                value
                    .checked_sub(block[index - 1])
                    .ok_or("integer delta overflow")?
            });
        }
        let mut raw = vec![0];
        for value in block {
            raw.extend(signed(*value)?);
        }
        let mut candidates = vec![raw];
        for (is_delta, series) in [block, delta.as_slice()].into_iter().enumerate() {
            let mut bounded = vec![3 + is_delta as u8];
            bounded.extend(interval(series)?);
            candidates.push(bounded);
            let mut rice_frame = vec![7 + is_delta as u8];
            rice_frame.extend(rice(series)?);
            candidates.push(rice_frame);
        }
        let winner = candidates
            .into_iter()
            .min_by_key(|candidate| (candidate.len(), candidate[0]))
            .ok_or("integer candidates")?;
        out.extend(pack(&winner));
    }
    Ok(out)
}

pub(crate) fn decode_integers(data: &[u8], n: usize) -> Result<Vec<i64>, String> {
    let (block, mut at) = vdecode(data, 0)?;
    let block = usize::try_from(block).map_err(|_| "integer block overflow")?;
    if block == 0 || block > INTEGER_BLOCK {
        return Err("invalid integer block".into());
    }
    let mut out = Vec::new();
    out.try_reserve(n).map_err(|_| "integer allocation")?;
    for start in (0..n).step_by(block) {
        let (part, next) = read(data, at)?;
        at = next;
        let count = (n - start).min(block);
        let kind = *part.first().ok_or("truncated integer block")?;
        let mut values = match kind {
            0 => {
                let mut pos = 1;
                let mut decoded = Vec::new();
                for _ in 0..count {
                    let (value, next) = read_signed(part, pos)?;
                    pos = next;
                    decoded.push(value);
                }
                if pos != part.len() {
                    return Err("trailing raw integers".into());
                }
                decoded
            }
            3 | 4 => uninterval(&part[1..], count)?,
            7 | 8 => unrice(&part[1..], count)?,
            _ => return Err("invalid integer representation".into()),
        };
        if kind != 0 && kind % 2 == 0 {
            for index in 1..values.len() {
                values[index] = values[index]
                    .checked_add(values[index - 1])
                    .ok_or("coordinate outside range")?;
            }
        }
        if values
            .iter()
            .any(|value| *value <= i64::from(i32::MIN) || *value >= i64::from(i32::MAX))
        {
            return Err("coordinate outside range".into());
        }
        out.extend(values);
    }
    if at != data.len() {
        return Err("trailing integer stream".into());
    }
    Ok(out)
}

/// Exact `SGCC2` context-three/raw-ID transform used beneath historical CIXB21.
pub fn transform_coordinate_groups(
    data: &[u8],
    context: u8,
    coding: u8,
) -> Result<Vec<u8>, String> {
    if context > 3 || coding != 0 {
        return Err("unsupported conditional policy".into());
    }
    let base = transform_wcs_grid_numeric(data, 1)?;
    let (n, mut at) = base_prefix(&base)?;
    let mut out = CONDITIONAL_RAW_MAGIC.to_vec();
    out.extend([context, coding]);
    out.extend_from_slice(&base[..at]);
    for width in FIELD_WIDTHS {
        let (q, values, prefix, next) = base_field(&base, at, n, width)?;
        at = next;
        let residual = feedback(&q, &values, width, false)?;
        let integers = encode_integers(&q)?;
        let corrections = encode_corrections(&q, &residual, width, context)?;
        out.extend_from_slice(&prefix[..8]);
        out.extend(pack(&integers));
        out.extend(pack(&corrections));
    }
    if at != base.len() {
        return Err("trailing base grid fields".into());
    }
    Ok(out)
}

fn inverse_coordinate_groups_bounded(raw: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    if !raw.starts_with(CONDITIONAL_RAW_MAGIC) || raw.len() < 7 || raw[5] != 3 || raw[6] != 0 {
        return Err("invalid conditional grid stream".into());
    }
    let inner = &raw[7..];
    let (n, mut at) = base_prefix(inner)?;
    let output = 28usize
        .checked_add(
            28usize
                .checked_mul(n)
                .ok_or("conditional output overflow")?,
        )
        .ok_or("conditional output overflow")?;
    if output > max_output {
        return Err("conditional output exceeds admission".into());
    }
    let mut out = Vec::new();
    out.try_reserve(inner[..at].len())
        .map_err(|_| "conditional working allocation")?;
    out.extend_from_slice(&inner[..at]);
    for width in FIELD_WIDTHS {
        let scale_end = at.checked_add(8).ok_or("grid scale overflow")?;
        let scale = inner.get(at..scale_end).ok_or("truncated grid scale")?;
        at = scale_end;
        let (integers, next) = read(inner, at)?;
        at = next;
        let q = decode_integers(integers, n)?;
        let (corrections, next) = read(inner, at)?;
        at = next;
        let residual = decode_corrections(corrections, &q, width, 3)?;
        let values = feedback(&q, &residual, width, true)?;
        let qbytes = q_bytes(&q)?;
        out.extend_from_slice(scale);
        out.extend(pack(&qbytes));
        out.extend(write_base_field(&values, width)?);
    }
    if at != inner.len() {
        return Err("trailing conditional grid fields".into());
    }
    inverse_wcs_grid_numeric(&out)
}

pub fn inverse_coordinate_groups(raw: &[u8]) -> Result<Vec<u8>, String> {
    inverse_coordinate_groups_bounded(raw, MAX_RAW_INVERSE_BYTES)
}

pub fn inverse_coordinate_groups_with_limit(
    raw: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, String> {
    inverse_coordinate_groups_bounded(raw, max_output)
}

fn backend_budget(output: usize, memory: usize) -> Result<usize, String> {
    let inverse = output
        .checked_mul(INVERSE_WORKING_MULTIPLIER)
        .ok_or_else(|| "conditional working-memory overflow".to_string())?;
    memory
        .checked_sub(
            output
                .checked_add(inverse)
                .ok_or_else(|| "conditional working-memory overflow".to_string())?,
        )
        .ok_or_else(|| "conditional combined live-buffer limit".to_string())
}

/// Complete historical CIXB21 frame. The supplied backend is the pinned PAQ
/// provider selected by the caller; the format itself has no backend byte.
pub fn encode_grouped_frame(source: &[u8], backend: &EncodeBackend<'_>) -> Result<Vec<u8>, String> {
    let raw = transform_coordinate_groups(source, 3, 0)?;
    let payload = backend(&raw)?;
    let mut out = GROUPED_MAGIC.to_vec();
    out.extend(vencode(source.len() as u64));
    out.extend_from_slice(&crc32fast::hash(source).to_be_bytes());
    out.extend(payload);
    Ok(out)
}

pub fn decode_grouped_frame(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if !frame.starts_with(GROUPED_MAGIC) || memory == 0 {
        return Err("invalid grouped PAQ frame".into());
    }
    let (length, at) = vdecode(frame, GROUPED_MAGIC.len())?;
    let length = usize::try_from(length).map_err(|_| "grouped output size overflow")?;
    if length > limit || length > memory {
        return Err("grouped output exceeds limit".into());
    }
    let checksum: [u8; 4] = frame
        .get(at..at.checked_add(4).ok_or("grouped frame overflow")?)
        .ok_or("truncated grouped frame")?
        .try_into()
        .map_err(|_| "truncated grouped checksum")?;
    let budget = backend_budget(length, memory)?;
    let raw = backend(
        frame
            .get(at.checked_add(4).ok_or("grouped frame overflow")?..)
            .ok_or("truncated grouped payload")?,
        budget,
    )?;
    if raw.len() > budget {
        return Err("backend ignored grouped memory limit".into());
    }
    let restored = inverse_coordinate_groups_bounded(&raw, length)?;
    if restored.len() != length || crc32fast::hash(&restored) != u32::from_be_bytes(checksum) {
        return Err("grouped PAQ source mismatch".into());
    }
    Ok(restored)
}

pub(crate) fn write_base_field(values: &[i64], width: usize) -> Result<Vec<u8>, String> {
    let palette = palette(values);
    if palette.len() > 256 {
        let mut raw = Vec::new();
        for value in values {
            raw.extend(signed_le(*value, width)?);
        }
        let mut out = vec![0];
        out.extend(pack(&raw));
        return Ok(out);
    }
    let mut lookup = BTreeMap::new();
    for (index, value) in palette.iter().enumerate() {
        lookup.insert(
            *value,
            u8::try_from(index).map_err(|_| "correction palette")?,
        );
    }
    let mut out = vec![1];
    out.extend(vencode(palette.len() as u64));
    for value in &palette {
        out.extend(signed(*value)?);
    }
    let ids = values.iter().map(|value| lookup[value]).collect::<Vec<_>>();
    out.extend(pack(&ids));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singleton_coordinate_rank_has_no_payload_byte() {
        let values = [7; 8];
        let encoded = encode_integers(&values).unwrap();
        assert_eq!(encoded, [0x80, 0x20, 3, 3, 14, 1]);
        assert_eq!(decode_integers(&encoded, values.len()).unwrap(), values);
    }

    #[test]
    fn matches_frozen_context_three_raw_ids_and_restores() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/conditional_values/input.bin"
        ));
        let expected = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/conditional_values/context3-coding0.raw"
        ));
        assert_eq!(transform_coordinate_groups(source, 3, 0).unwrap(), expected);
        assert_eq!(inverse_coordinate_groups(expected).unwrap(), source);
    }

    #[test]
    fn rejects_unimplemented_codings_and_count_driven_truncation() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/conditional_values/input.bin"
        ));
        assert!(transform_coordinate_groups(source, 3, 1).is_err());
        let raw = transform_coordinate_groups(source, 3, 0).unwrap();
        assert!(inverse_coordinate_groups(&raw[..raw.len() - 1]).is_err());
    }

    #[test]
    fn grouped_frame_charges_backend_and_inverse_working_space() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/conditional_values/input.bin"
        ));
        let frame = encode_grouped_frame(source, &|raw| Ok(raw.to_vec())).unwrap();
        assert_eq!(
            decode_grouped_frame(&frame, 4096, 4096, &|raw, cap| {
                if raw.len() > cap {
                    Err("cap".into())
                } else {
                    Ok(raw.to_vec())
                }
            })
            .unwrap(),
            source
        );
        assert!(
            decode_grouped_frame(&frame, 4096, source.len() * 8 - 1, &|raw, _| Ok(
                raw.to_vec()
            ))
            .is_err()
        );
    }
}
