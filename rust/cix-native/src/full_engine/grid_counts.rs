//! Historical coordinate-count grid parents (`SGCC1` and `SGCC2`).
//!
//! This module is kept distinct from the later grouped PAQ route: SGCC1 uses
//! predictor modes and SGCC2 records an explicit context/coding pair.

use super::arithmetic::{vdecode, vencode};
use super::conditional_values::{
    base_field, base_prefix, decode_integers, encode_integers, feedback, pack, q_bytes, read,
    read_signed, signed, write_base_field,
};
use super::strided_values::{
    inverse_wcs_grid_numeric_with_limit, transform_wcs_grid_numeric, type_class_decode,
    type_class_encode,
};
use std::collections::{BTreeMap, HashMap};

pub const COUNT_RAW_MAGIC: &[u8] = b"SGCC1";
pub const CONDITIONAL_RAW_MAGIC: &[u8] = b"SGCC2";
pub const COUNT_MAGIC: &[u8] = b"CIXB\x1f";
pub const CONDITIONAL_MAGIC: &[u8] = b"CIXB\x20";
const WIDTHS: [usize; 4] = [8, 8, 4, 4];
const TYPE_CLASS_BLOCK: usize = 1024;
const MAX_RAW_INVERSE_BYTES: usize = 256 * 1024 * 1024;
const INVERSE_WORKING_MULTIPLIER: usize = 7;
pub type EncodeBackend<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a;
pub type DecodeBackend<'a> = dyn Fn(&[u8], usize) -> Result<Vec<u8>, String> + 'a;

fn modal(map: Option<&BTreeMap<i64, usize>>) -> i64 {
    map.and_then(|m| {
        m.iter()
            .min_by_key(|(v, n)| (std::cmp::Reverse(**n), v.unsigned_abs(), **v))
            .map(|(v, _)| *v)
    })
    .unwrap_or(0)
}
fn key(q: i64) -> (bool, u32, i64) {
    let magnitude = q.unsigned_abs();
    (q < 0, 64 - magnitude.leading_zeros(), q.rem_euclid(16))
}
fn predict(
    q: &[i64],
    values: &[i64],
    width: usize,
    mode: u8,
    reverse: bool,
) -> Result<Vec<i64>, String> {
    if mode > 4 || q.len() != values.len() {
        return Err("invalid correction predictor".into());
    }
    let (m, h) = if width == 4 {
        (1i128 << 32, 1i128 << 31)
    } else if width == 8 {
        (1i128 << 64, 1i128 << 63)
    } else {
        return Err("unsupported correction width".into());
    };
    let mut last = HashMap::new();
    let mut counts = HashMap::<i64, BTreeMap<i64, usize>>::new();
    let mut contexts = HashMap::<(bool, u32, i64), BTreeMap<i64, usize>>::new();
    let mut out = Vec::with_capacity(q.len());
    for (&c, &v) in q.iter().zip(values) {
        let k = key(c);
        let cg = modal(contexts.get(&k));
        let guess = match mode {
            0 => *last.get(&c).unwrap_or(&0),
            1 => modal(counts.get(&c)),
            2 => *last.get(&c).unwrap_or(&cg),
            3 => {
                if counts.contains_key(&c) {
                    modal(counts.get(&c))
                } else {
                    cg
                }
            }
            _ => cg,
        };
        let actual = if reverse {
            i64::try_from((i128::from(v) + i128::from(guess) + h).rem_euclid(m) - h)
                .map_err(|_| "feedback range")?
        } else {
            v
        };
        out.push(if reverse {
            actual
        } else {
            i64::try_from((i128::from(actual) - i128::from(guess) + h).rem_euclid(m) - h)
                .map_err(|_| "feedback range")?
        });
        last.insert(c, actual);
        if mode == 1 || mode == 3 {
            *counts.entry(c).or_default().entry(actual).or_default() += 1
        };
        if mode >= 2 {
            let t = contexts.entry(k).or_default();
            *t.entry(actual).or_default() += 1;
            if t.values().sum::<usize>() >= 1024 {
                for n in t.values_mut() {
                    *n = (*n).div_ceil(2)
                }
            }
        }
    }
    Ok(out)
}

fn partitions(q: &[i64], context: u8) -> Result<Vec<Vec<usize>>, String> {
    if context > 3 {
        return Err("invalid coordinate partition".into());
    }
    let mut groups = BTreeMap::<Vec<i64>, Vec<usize>>::new();
    for (index, value) in q.iter().enumerate() {
        let key = match context {
            0 => vec![0],
            1 => vec![
                i64::from(*value < 0),
                i64::from(64 - value.unsigned_abs().leading_zeros()),
            ],
            2 => vec![
                i64::from(*value < 0),
                i64::from(64 - value.unsigned_abs().leading_zeros()),
                value.rem_euclid(16),
            ],
            _ => vec![*value],
        };
        groups.entry(key).or_default().push(index);
    }
    Ok(groups.into_values().collect())
}

fn correction_palette(values: &[i64]) -> Vec<i64> {
    let mut counts = BTreeMap::<i64, usize>::new();
    for value in values {
        *counts.entry(*value).or_default() += 1;
    }
    let mut palette = counts.into_iter().collect::<Vec<_>>();
    palette.sort_by_key(|(value, count)| (std::cmp::Reverse(*count), *value));
    palette.into_iter().map(|(value, _)| value).collect()
}

fn encode_conditional_corrections(
    q: &[i64],
    residual: &[i64],
    width: usize,
    context: u8,
    coding: u8,
) -> Result<Vec<u8>, String> {
    if q.len() != residual.len() || coding > 2 {
        return Err("invalid correction coding".into());
    }
    let palette = correction_palette(residual);
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
    for value in palette {
        out.extend(signed(value)?);
    }
    for partition in partitions(q, context)? {
        let symbols = partition
            .iter()
            .map(|index| {
                lookup
                    .get(&residual[*index])
                    .copied()
                    .ok_or("correction palette")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if coding == 0 {
            out.extend(symbols);
        } else {
            for block in symbols.chunks(TYPE_CLASS_BLOCK) {
                out.extend(pack(&type_class_encode(block, coding == 2)?));
            }
        }
    }
    Ok(out)
}

fn decode_conditional_corrections(
    data: &[u8],
    q: &[i64],
    width: usize,
    context: u8,
    coding: u8,
) -> Result<Vec<i64>, String> {
    if coding > 2 {
        return Err("invalid correction coding".into());
    }
    match *data.first().ok_or("missing conditional corrections")? {
        0 => {
            let qbytes = q_bytes(q)?;
            let mut temporary = vec![0; 8];
            temporary.extend(pack(&qbytes));
            temporary.extend_from_slice(&data[1..]);
            let (_, values, _, used) = base_field(&temporary, 0, q.len(), width)?;
            if used != temporary.len() {
                return Err("trailing fallback".into());
            }
            Ok(values)
        }
        1 => {
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
                let ids = if coding == 0 {
                    let end = at
                        .checked_add(partition.len())
                        .ok_or("conditional ID length")?;
                    let ids = data
                        .get(at..end)
                        .ok_or("truncated conditional IDs")?
                        .to_vec();
                    at = end;
                    ids
                } else {
                    let mut ids = Vec::new();
                    ids.try_reserve(partition.len())
                        .map_err(|_| "conditional ID allocation")?;
                    for offset in (0..partition.len()).step_by(TYPE_CLASS_BLOCK) {
                        let (frame, next) = read(data, at)?;
                        at = next;
                        let expected = (partition.len() - offset).min(TYPE_CLASS_BLOCK);
                        let (part, used) = type_class_decode(frame, 0, expected)
                            .map_err(|_| "invalid conditional type class")?;
                        if used != frame.len() || part.len() != expected {
                            return Err("invalid conditional type class".into());
                        }
                        ids.extend(part);
                    }
                    ids
                };
                if ids.len() != partition.len() {
                    return Err("invalid conditional correction IDs".into());
                }
                for (index, id) in partition.into_iter().zip(ids) {
                    out[index] = *palette
                        .get(usize::from(id))
                        .ok_or("invalid conditional correction IDs")?;
                }
            }
            if at != data.len() {
                return Err("trailing correction groups".into());
            }
            Ok(out)
        }
        _ => Err("invalid correction representation".into()),
    }
}

pub fn transform_counts(source: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    if mode > 4 {
        return Err("invalid correction predictor".into());
    }
    let base = transform_wcs_grid_numeric(source, 1)?;
    let (n, mut at) = base_prefix(&base)?;
    let mut out = COUNT_RAW_MAGIC.to_vec();
    out.push(mode);
    out.extend_from_slice(&base[..at]);
    for width in WIDTHS {
        let (q, v, p, next) = base_field(&base, at, n, width)?;
        at = next;
        out.extend_from_slice(&p[..8]);
        out.extend(pack(&encode_integers(&q)?));
        out.extend(write_base_field(
            &predict(&q, &v, width, mode, false)?,
            width,
        )?);
    }
    if at != base.len() {
        return Err("unused grid bytes".into());
    }
    Ok(out)
}
pub fn inverse_counts(raw: &[u8], max: usize) -> Result<Vec<u8>, String> {
    if !raw.starts_with(COUNT_RAW_MAGIC) || raw.len() < 6 || raw[5] > 4 {
        return Err("invalid correction context stream".into());
    }
    let mode = raw[5];
    let inner = &raw[6..];
    let (n, mut at) = base_prefix(inner)?;
    let output = 28usize
        .checked_add(28usize.checked_mul(n).ok_or("count output overflow")?)
        .ok_or("count output overflow")?;
    if output > max {
        return Err("count output exceeds admission".into());
    }
    let mut out = inner[..at].to_vec();
    for width in WIDTHS {
        let scale_end = at.checked_add(8).ok_or("grid scale overflow")?;
        let scale = inner.get(at..scale_end).ok_or("truncated grid scale")?;
        at = scale_end;
        let (ints, next) = read(inner, at)?;
        at = next;
        let q = decode_integers(ints, n)?;
        let qb = q_bytes(&q)?;
        let prefix_len = scale.len() + pack(&qb).len();
        let mut tmp = scale.to_vec();
        tmp.extend(pack(&qb));
        tmp.extend_from_slice(&inner[at..]);
        let (_, v, _, used) = base_field(&tmp, 0, n, width)?;
        let rest = used.checked_sub(prefix_len).ok_or("count field length")?;
        at = at.checked_add(rest).ok_or("count field length")?;
        out.extend(scale);
        out.extend(pack(&qb));
        out.extend(write_base_field(
            &predict(&q, &v, width, mode, true)?,
            width,
        )?);
    }
    if at != inner.len() {
        return Err("trailing correction context bytes".into());
    }
    inverse_wcs_grid_numeric_with_limit(&out, max)
}

/// Exact `SGCC2` transform for every historical context/coding selector.
pub fn transform_conditional(source: &[u8], context: u8, coding: u8) -> Result<Vec<u8>, String> {
    if context > 3 || coding > 2 {
        return Err("invalid conditional policy".into());
    }
    let base = transform_wcs_grid_numeric(source, 1)?;
    let (n, mut at) = base_prefix(&base)?;
    let mut out = CONDITIONAL_RAW_MAGIC.to_vec();
    out.extend([context, coding]);
    out.extend_from_slice(&base[..at]);
    for width in WIDTHS {
        let (q, values, prefix, next) = base_field(&base, at, n, width)?;
        at = next;
        let residual = feedback(&q, &values, width, false)?;
        out.extend_from_slice(&prefix[..8]);
        out.extend(pack(&encode_integers(&q)?));
        out.extend(pack(&encode_conditional_corrections(
            &q, &residual, width, context, coding,
        )?));
    }
    if at != base.len() {
        return Err("trailing base grid fields".into());
    }
    Ok(out)
}

fn inverse_conditional_bounded(raw: &[u8], max: usize) -> Result<Vec<u8>, String> {
    if !raw.starts_with(CONDITIONAL_RAW_MAGIC) || raw.len() < 7 || raw[5] > 3 || raw[6] > 2 {
        return Err("invalid conditional grid stream".into());
    }
    let (context, coding) = (raw[5], raw[6]);
    let inner = &raw[7..];
    let (n, mut at) = base_prefix(inner)?;
    let output = 28usize
        .checked_add(
            28usize
                .checked_mul(n)
                .ok_or("conditional output overflow")?,
        )
        .ok_or("conditional output overflow")?;
    if output > max {
        return Err("conditional output exceeds admission".into());
    }
    let mut out = Vec::new();
    out.try_reserve(inner[..at].len())
        .map_err(|_| "conditional working allocation")?;
    out.extend_from_slice(&inner[..at]);
    for width in WIDTHS {
        let scale_end = at.checked_add(8).ok_or("grid scale overflow")?;
        let scale = inner.get(at..scale_end).ok_or("truncated grid scale")?;
        at = scale_end;
        let (integers, next) = read(inner, at)?;
        at = next;
        let q = decode_integers(integers, n)?;
        let (corrections, next) = read(inner, at)?;
        at = next;
        let residual = decode_conditional_corrections(corrections, &q, width, context, coding)?;
        let values = feedback(&q, &residual, width, true)?;
        let qbytes = q_bytes(&q)?;
        out.extend_from_slice(scale);
        out.extend(pack(&qbytes));
        out.extend(write_base_field(&values, width)?);
    }
    if at != inner.len() {
        return Err("trailing conditional grid fields".into());
    }
    inverse_wcs_grid_numeric_with_limit(&out, max)
}

pub fn inverse_conditional(raw: &[u8]) -> Result<Vec<u8>, String> {
    inverse_conditional_bounded(raw, MAX_RAW_INVERSE_BYTES)
}
pub fn inverse_conditional_with_limit(raw: &[u8], max: usize) -> Result<Vec<u8>, String> {
    inverse_conditional_bounded(raw, max)
}

fn backend_budget(output: usize, memory: usize) -> Result<usize, String> {
    let inverse = output
        .checked_mul(INVERSE_WORKING_MULTIPLIER)
        .ok_or("conditional working-memory overflow")?;
    memory
        .checked_sub(
            output
                .checked_add(inverse)
                .ok_or("conditional working-memory overflow")?,
        )
        .ok_or("conditional combined live-buffer limit".into())
}
fn encode_frame(
    source: &[u8],
    magic: &[u8],
    raw: &[u8],
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    let payload = backend(raw)?;
    let mut out = magic.to_vec();
    out.extend(vencode(source.len() as u64));
    out.extend_from_slice(&crc32fast::hash(source).to_be_bytes());
    out.extend(payload);
    Ok(out)
}
fn decode_frame(
    frame: &[u8],
    magic: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
    inverse: impl Fn(&[u8], usize) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    if !frame.starts_with(magic) || memory == 0 {
        return Err("invalid grid-count frame".into());
    }
    let (length, at) = vdecode(frame, magic.len())?;
    let length = usize::try_from(length).map_err(|_| "grid-count output size overflow")?;
    if length > limit || length > memory {
        return Err("grid-count output exceeds limit".into());
    }
    let end = at.checked_add(4).ok_or("grid-count frame overflow")?;
    let checksum: [u8; 4] = frame
        .get(at..end)
        .ok_or("truncated grid-count frame")?
        .try_into()
        .map_err(|_| "truncated grid-count checksum")?;
    let budget = backend_budget(length, memory)?;
    let raw = backend(
        frame.get(end..).ok_or("truncated grid-count payload")?,
        budget,
    )?;
    if raw.len() > budget {
        return Err("backend ignored grid-count memory limit".into());
    }
    let restored = inverse(&raw, length)?;
    if restored.len() != length || crc32fast::hash(&restored) != u32::from_be_bytes(checksum) {
        return Err("grid-count source mismatch".into());
    }
    Ok(restored)
}
pub fn encode_counts_frame(
    source: &[u8],
    mode: u8,
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    let raw = transform_counts(source, mode)?;
    encode_frame(source, COUNT_MAGIC, &raw, backend)
}
pub fn decode_counts_frame(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    decode_frame(frame, COUNT_MAGIC, limit, memory, backend, inverse_counts)
}
pub fn encode_conditional_frame(
    source: &[u8],
    context: u8,
    coding: u8,
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    let raw = transform_conditional(source, context, coding)?;
    encode_frame(source, CONDITIONAL_MAGIC, &raw, backend)
}
pub fn decode_conditional_frame(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    decode_frame(
        frame,
        CONDITIONAL_MAGIC,
        limit,
        memory,
        backend,
        inverse_conditional_bounded,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_bytes_eq(label: &str, actual: &[u8], expected: &[u8]) {
        if actual == expected {
            return;
        }
        let first = actual
            .iter()
            .zip(expected)
            .position(|(left, right)| left != right)
            .unwrap_or_else(|| actual.len().min(expected.len()));
        panic!(
            "{label}: byte mismatch at {first}; actual length {}, expected length {}; actual byte {:?}, expected byte {:?}",
            actual.len(),
            expected.len(),
            actual.get(first),
            expected.get(first),
        );
    }

    fn input() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/grid_counts/input.bin"),
        )
        .unwrap()
    }
    #[test]
    fn sgcc1_frozen_modes_restore_exactly() {
        let input = input();
        for mode in 0..=4 {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/grid_counts")
                .join(format!("sgcc1-mode{mode}.raw"));
            let expected = std::fs::read(path).unwrap();
            assert_bytes_eq(
                &format!("SGCC1 mode {mode} transform"),
                &transform_counts(&input, mode).unwrap(),
                &expected,
            );
            assert_bytes_eq(
                &format!("SGCC1 mode {mode} inverse"),
                &inverse_counts(&expected, input.len()).unwrap(),
                &input,
            );
        }
    }
    #[test]
    fn sgcc2_frozen_contexts_codings_restore_exactly() {
        let input = input();
        assert_eq!(input.len(), 28 + 28 * 1100, "fixture lost branch coverage");
        for context in 0..=3 {
            for coding in 0..=2 {
                let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/grid_counts")
                    .join(format!("sgcc2-context{context}-coding{coding}.raw"));
                let expected = std::fs::read(path).unwrap();
                assert_bytes_eq(
                    &format!("SGCC2 context {context} coding {coding} transform"),
                    &transform_conditional(&input, context, coding).unwrap(),
                    &expected,
                );
                assert_bytes_eq(
                    &format!("SGCC2 context {context} coding {coding} inverse"),
                    &inverse_conditional(&expected).unwrap(),
                    &input,
                );
            }
        }
    }
    #[test]
    fn frames_are_bounded_and_reject_trailing_payloads() {
        let input = input();
        let frame = encode_conditional_frame(&input, 2, 2, &|raw| Ok(raw.to_vec())).unwrap();
        let restored = decode_conditional_frame(&frame, input.len(), 1 << 20, &|raw, cap| {
            if raw.len() <= cap {
                Ok(raw.to_vec())
            } else {
                Err("cap".into())
            }
        })
        .unwrap();
        assert_bytes_eq("SGCC2 CIXB frame inverse", &restored, &input);
        assert!(
            decode_conditional_frame(&frame, input.len(), input.len() * 8 - 1, &|raw, _| Ok(
                raw.to_vec()
            ))
            .is_err()
        );
        let mut raw = transform_conditional(&input, 0, 1).unwrap();
        raw.push(0);
        assert!(inverse_conditional(&raw).is_err());
    }
}
