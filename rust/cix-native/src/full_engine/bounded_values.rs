//! Retained CIXB1E grid-backend raw parents.
//!
//! This module owns the exact mode-zero source control, SGFB1 feedback mode,
//! and SGBI1 bounded-coordinate policies one, two and three.  Each CIXB1E
//! mode preserves its historical policy tag; no mode is aliased to another.

use super::arithmetic::{vdecode, vencode, ArithmeticDecoder, ArithmeticEncoder};
use super::conditional_values::{
    base_field, base_prefix, feedback, pack, q_bytes, read, read_signed, rice, signed, unrice,
    write_base_field,
};
use super::strided_values::{inverse_wcs_grid_numeric, transform_wcs_grid_numeric};
use num_bigint::BigUint;
use num_traits::{One, ToPrimitive, Zero};

pub const BACKEND_MAGIC: &[u8] = b"CIXB\x1e";
pub const FEEDBACK_RAW_MAGIC: &[u8] = b"SGFB1";
pub const INTERVAL_RAW_MAGIC: &[u8] = b"SGBI1";
const WIDTHS: [usize; 4] = [8, 8, 4, 4];

pub type EncodeBackend<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a;
pub type DecodeBackend<'a> = dyn Fn(&[u8], usize) -> Result<Vec<u8>, String> + 'a;

fn fixed_bytes(value: &BigUint, bits: usize) -> Vec<u8> {
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
fn fixed_width(span: u64) -> usize {
    if span <= 1 {
        0
    } else {
        (64 - (span - 1).leading_zeros()) as usize
    }
}

fn policy_interval(values: &[i64], policy: u8) -> Result<Option<Vec<u8>>, String> {
    let lo = *values.iter().min().ok_or("empty interval")?;
    let hi = *values.iter().max().ok_or("empty interval")?;
    let span = u64::try_from(
        hi.checked_sub(lo)
            .and_then(|v| v.checked_add(1))
            .ok_or("integer bound overflow")?,
    )
    .map_err(|_| "integer bound overflow")?;
    let mut out = signed(lo)?;
    out.extend(vencode(span));
    if policy == 1 {
        let width = fixed_width(span);
        let mut rank = BigUint::zero();
        for value in values {
            rank = (rank << width)
                | BigUint::from(u64::try_from(value - lo).map_err(|_| "integer bound")?);
        }
        out.extend(fixed_bytes(
            &rank,
            width.checked_mul(values.len()).ok_or("interval length")?,
        ));
        return Ok(Some(out));
    }
    if policy == 2 {
        let mut rank = BigUint::zero();
        for value in values {
            rank = rank * span + u64::try_from(value - lo).map_err(|_| "integer bound")?;
        }
        let card =
            BigUint::from(span).pow(u32::try_from(values.len()).map_err(|_| "interval length")?);
        out.extend(fixed_bytes(&rank, (&card - BigUint::one()).bits() as usize));
        return Ok(Some(out));
    }
    if span > (1 << 29) {
        return Ok(None);
    }
    let mut coder = ArithmeticEncoder::new();
    for value in values {
        let digit = u64::try_from(value - lo).map_err(|_| "integer bound")?;
        coder.encode(digit, digit.checked_add(1).ok_or("integer bound")?, span)?;
    }
    let (payload, bits) = coder.finish();
    out.extend(vencode(bits as u64));
    out.extend(payload);
    Ok(Some(out))
}

fn policy_uninterval(data: &[u8], n: usize, policy: u8) -> Result<Vec<i64>, String> {
    let (lo, at) = read_signed(data, 0)?;
    let (span, at) = vdecode(data, at)?;
    if span == 0 || span >= (1 << 34) {
        return Err("invalid integer bound".into());
    }
    let mut digits = Vec::new();
    digits.try_reserve(n).map_err(|_| "interval allocation")?;
    if policy == 1 || policy == 2 {
        let bits = if policy == 1 {
            fixed_width(span).checked_mul(n).ok_or("interval length")?
        } else {
            let card = BigUint::from(span).pow(u32::try_from(n).map_err(|_| "interval length")?);
            (&card - BigUint::one()).bits() as usize
        };
        if data.len().saturating_sub(at) != bits.div_ceil(8) {
            return Err("interval rank length".into());
        }
        let mut rank = BigUint::from_bytes_be(&data[at..]);
        if rank.bits() > bits as u64 {
            return Err("rank outside interval".into());
        }
        for _ in 0..n {
            let digit = if policy == 1 {
                let width = fixed_width(span);
                let shift = width
                    .checked_mul(n - digits.len() - 1)
                    .ok_or("interval length")?;
                ((&rank >> shift) & ((BigUint::one() << width) - BigUint::one()))
                    .to_u64()
                    .ok_or("interval digit")?
            } else {
                0
            };
            digits.push(digit);
        }
        if policy == 1 {
            if digits.iter().any(|d| *d >= span) {
                return Err("rank outside interval".into());
            }
        } else {
            digits.clear();
            for _ in 0..n {
                let digit = (&rank % span).to_u64().ok_or("interval digit")?;
                rank /= span;
                digits.push(digit);
            }
            digits.reverse();
            if !rank.is_zero() {
                return Err("rank outside interval".into());
            }
        }
    } else {
        if span > (1 << 29) {
            return Err("invalid arithmetic interval".into());
        }
        let (bits, pos) = vdecode(data, at)?;
        let bits = usize::try_from(bits).map_err(|_| "arithmetic bits")?;
        if data.len().saturating_sub(pos) != bits.div_ceil(8) {
            return Err("invalid arithmetic interval".into());
        }
        let mut decoder = ArithmeticDecoder::new(&data[pos..], bits)?;
        for _ in 0..n {
            let digit = decoder.target(span)?;
            if digit >= span {
                return Err("integer outside interval".into());
            }
            decoder.update(digit, digit + 1, span)?;
            digits.push(digit);
        }
    }
    digits
        .into_iter()
        .map(|digit| {
            lo.checked_add(i64::try_from(digit).map_err(|_| "integer bound")?)
                .ok_or("integer bound".into())
        })
        .collect()
}

fn encode_integers_policy(values: &[i64], policy: u8, block: usize) -> Result<Vec<u8>, String> {
    if !matches!(policy, 1..=3) || !(1..=4096).contains(&block) {
        return Err("invalid integer policy".into());
    }
    let mut out = vencode(block as u64);
    for block in values.chunks(block) {
        let delta = block
            .iter()
            .enumerate()
            .map(|(i, v)| {
                if i == 0 {
                    Ok(*v)
                } else {
                    v.checked_sub(block[i - 1])
                        .ok_or("integer delta overflow".into())
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut raw = vec![0];
        for value in block {
            raw.extend(signed(*value)?);
        }
        let mut choices = vec![raw];
        for (delta_flag, series) in [block, delta.as_slice()].into_iter().enumerate() {
            if let Some(encoded) = policy_interval(series, policy)? {
                let mut item = vec![2 * policy - 1 + delta_flag as u8];
                item.extend(encoded);
                choices.push(item);
            }
            let mut rice_item = vec![7 + delta_flag as u8];
            rice_item.extend(rice(series)?);
            choices.push(rice_item);
        }
        let winner = choices
            .into_iter()
            .min_by_key(|item| (item.len(), item[0]))
            .ok_or("integer candidates")?;
        out.extend(pack(&winner));
    }
    Ok(out)
}

fn decode_integers_policy(data: &[u8], n: usize, policy: u8) -> Result<Vec<i64>, String> {
    if policy > 3 {
        return Err("invalid integer policy".into());
    }
    let (block, mut at) = vdecode(data, 0)?;
    let block = usize::try_from(block).map_err(|_| "integer block")?;
    if block == 0 || block > 4096 {
        return Err("invalid integer block".into());
    }
    let mut out = Vec::new();
    for start in (0..n).step_by(block) {
        let (part, next) = read(data, at)?;
        at = next;
        let count = (n - start).min(block);
        let kind = *part.first().ok_or("truncated integer block")?;
        let mut values = match kind {
            0 => {
                let mut pos = 1;
                let mut values = Vec::new();
                for _ in 0..count {
                    let (value, next) = read_signed(part, pos)?;
                    pos = next;
                    values.push(value);
                }
                if pos != part.len() {
                    return Err("trailing raw integers".into());
                }
                values
            }
            1..=6 if policy == 0 || kind.div_ceil(2) == policy => {
                policy_uninterval(&part[1..], count, kind.div_ceil(2))?
            }
            7 | 8 => unrice(&part[1..], count)?,
            _ => return Err("invalid integer representation".into()),
        };
        if kind % 2 == 0 && kind != 0 {
            for index in 1..values.len() {
                values[index] = values[index]
                    .checked_add(values[index - 1])
                    .ok_or("coordinate outside range")?;
            }
        }
        if values
            .iter()
            .any(|v| *v <= i64::from(i32::MIN) || *v >= i64::from(i32::MAX))
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

fn feedback_transform(source: &[u8]) -> Result<Vec<u8>, String> {
    let base = transform_wcs_grid_numeric(source, 1)?;
    let (n, mut at) = base_prefix(&base)?;
    let mut out = FEEDBACK_RAW_MAGIC.to_vec();
    out.extend([1, 1]);
    out.extend_from_slice(&base[..at]);
    for width in WIDTHS {
        let (q, values, prefix, next) = base_field(&base, at, n, width)?;
        at = next;
        out.extend_from_slice(prefix);
        out.extend(write_base_field(
            &feedback(&q, &values, width, false)?,
            width,
        )?);
    }
    if at != base.len() {
        return Err("trailing feedback base".into());
    }
    Ok(out)
}

fn feedback_inverse(raw: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    if !raw.starts_with(FEEDBACK_RAW_MAGIC) || raw.get(5..7) != Some(b"\x01\x01") {
        return Err("invalid feedback transform".into());
    }
    let inner = &raw[7..];
    let (n, mut at) = base_prefix(inner)?;
    let output = 28usize
        .checked_add(28usize.checked_mul(n).ok_or("feedback output overflow")?)
        .ok_or("feedback output overflow")?;
    if output > max_output {
        return Err("feedback output exceeds admission".into());
    }
    let mut out = inner[..at].to_vec();
    for width in WIDTHS {
        let (q, values, prefix, next) = base_field(inner, at, n, width)?;
        at = next;
        out.extend_from_slice(prefix);
        out.extend(write_base_field(
            &feedback(&q, &values, width, true)?,
            width,
        )?);
    }
    if at != inner.len() {
        return Err("trailing feedback bytes".into());
    }
    inverse_wcs_grid_numeric(&out)
}

fn interval_transform(source: &[u8], policy: u8, block: usize) -> Result<Vec<u8>, String> {
    let feedback = feedback_transform(source)?;
    let inner = &feedback[7..];
    let (n, mut at) = base_prefix(inner)?;
    let mut out = INTERVAL_RAW_MAGIC.to_vec();
    out.extend_from_slice(&feedback[5..7]);
    out.extend_from_slice(&inner[..at]);
    for width in WIDTHS {
        let start = at;
        let (q, _, prefix, next) = base_field(inner, at, n, width)?;
        at = next;
        let integers = encode_integers_policy(&q, policy, block)?;
        out.extend_from_slice(&prefix[..8]);
        out.extend(pack(&integers));
        out.extend_from_slice(&inner[start + prefix.len()..next]);
    }
    if at != inner.len() {
        return Err("trailing interval base".into());
    }
    Ok(out)
}

fn interval_inverse(raw: &[u8], max_output: usize, policy: u8) -> Result<Vec<u8>, String> {
    if !raw.starts_with(INTERVAL_RAW_MAGIC) || raw.get(5..7) != Some(b"\x01\x01") {
        return Err("invalid bounded-grid transform".into());
    }
    let inner = &raw[7..];
    let (n, mut at) = base_prefix(inner)?;
    let output = 28usize
        .checked_add(28usize.checked_mul(n).ok_or("interval output overflow")?)
        .ok_or("interval output overflow")?;
    if output > max_output {
        return Err("interval output exceeds admission".into());
    }
    let mut out = FEEDBACK_RAW_MAGIC.to_vec();
    out.extend_from_slice(&raw[5..7]);
    out.extend_from_slice(&inner[..at]);
    for width in WIDTHS {
        let scale_end = at.checked_add(8).ok_or("grid scale overflow")?;
        let scale = inner.get(at..scale_end).ok_or("truncated grid scale")?;
        at = scale_end;
        let (integers, next) = read(inner, at)?;
        at = next;
        let q = decode_integers_policy(integers, n, policy)?;
        let qb = q_bytes(&q)?;
        let mut temporary = Vec::new();
        temporary.extend_from_slice(scale);
        temporary.extend(pack(&qb));
        temporary.extend_from_slice(&inner[at..]);
        let (_, _, _, used) = base_field(&temporary, 0, n, width)?;
        let rest = used
            .checked_sub(scale.len() + pack(&qb).len())
            .ok_or("interval field length")?;
        let end = at.checked_add(rest).ok_or("interval field length")?;
        out.extend_from_slice(scale);
        out.extend(pack(&qb));
        out.extend_from_slice(inner.get(at..end).ok_or("truncated interval field")?);
        at = end;
    }
    if at != inner.len() {
        return Err("trailing bounded-grid fields".into());
    }
    feedback_inverse(&out, max_output)
}

/// Exact historical `SGBI1` transform used by the `CIXZ\x01` envelope.
/// Unlike the later CIXB route, CIXZ stores its original 256-record integer
/// block size.  The policy is an encoder choice; it is not a raw header byte.
pub(crate) fn transform_interval_raw(
    source: &[u8],
    policy: u8,
    block: usize,
) -> Result<Vec<u8>, String> {
    interval_transform(source, policy, block)
}

/// Decodes every legal historical integer representation.  `SGBI1` has no
/// policy tag, so a raw decoder must accept the fixed-bit, mixed-radix and
/// arithmetic interval kinds emitted by all three encoders.
pub(crate) fn inverse_interval_raw(raw: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    interval_inverse(raw, max_output, 0)
}

pub fn transform_backend(source: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    match mode {
        0 => Ok(source.to_vec()),
        1 => feedback_transform(source),
        2 => interval_transform(source, 2, 4096),
        3 => interval_transform(source, 3, 4096),
        4 => interval_transform(source, 1, 4096),
        _ => Err("invalid grid mode".into()),
    }
}

pub fn inverse_backend(raw: &[u8], mode: u8, max_output: usize) -> Result<Vec<u8>, String> {
    match mode {
        0 => {
            if raw.len() > max_output {
                Err("grid output exceeds admission".into())
            } else {
                Ok(raw.to_vec())
            }
        }
        1 => feedback_inverse(raw, max_output),
        2 => interval_inverse(raw, max_output, 2),
        3 => interval_inverse(raw, max_output, 3),
        4 => interval_inverse(raw, max_output, 1),
        _ => Err("invalid grid mode".into()),
    }
}

pub fn encode_backend_frame(
    source: &[u8],
    mode: u8,
    backend_id: u8,
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if backend_id > 1 {
        return Err("invalid grid backend".into());
    }
    let raw = transform_backend(source, mode)?;
    let payload = backend(&raw)?;
    let mut out = BACKEND_MAGIC.to_vec();
    out.extend([mode, backend_id]);
    out.extend(vencode(source.len() as u64));
    out.extend_from_slice(&crc32fast::hash(source).to_be_bytes());
    out.extend(payload);
    Ok(out)
}

pub fn decode_backend_frame(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if !frame.starts_with(BACKEND_MAGIC) || memory == 0 {
        return Err("invalid grid backend frame".into());
    }
    let mode = *frame.get(5).ok_or("invalid grid backend frame")?;
    let backend_id = *frame.get(6).ok_or("invalid grid backend frame")?;
    if backend_id > 1 {
        return Err("invalid grid backend".into());
    }
    let (length, at) = vdecode(frame, 7)?;
    let length = usize::try_from(length).map_err(|_| "grid output overflow")?;
    if length > limit || length > memory {
        return Err("grid output exceeds limit".into());
    }
    let checksum: [u8; 4] = frame
        .get(at..at.checked_add(4).ok_or("grid frame overflow")?)
        .ok_or("truncated grid frame")?
        .try_into()
        .map_err(|_| "truncated grid checksum")?;
    let budget = memory
        .checked_sub(
            length
                .checked_mul(8)
                .ok_or("grid working-memory overflow")?,
        )
        .ok_or("grid combined live-buffer limit")?;
    let raw = backend(frame.get(at + 4..).ok_or("truncated grid payload")?, budget)?;
    if raw.len() > budget {
        return Err("backend ignored grid memory limit".into());
    }
    let restored = inverse_backend(&raw, mode, length)?;
    if restored.len() != length || crc32fast::hash(&restored) != u32::from_be_bytes(checksum) {
        return Err("grid backend restoration mismatch".into());
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn singleton_interval_uses_zero_bits_and_padding_is_rejected() {
        for policy in [1, 2] {
            let encoded = policy_interval(&[0, 0, 0], policy).unwrap().unwrap();
            assert_eq!(encoded, [0, 1]);
            assert_eq!(policy_uninterval(&encoded, 3, policy).unwrap(), [0, 0, 0]);
        }
        // The two-bit digit is zero; its nonzero high padding is invalid.
        assert!(policy_uninterval(&[0, 3, 0x80], 1, 1).is_err());
    }
    #[test]
    fn retained_feedback_and_policy_two_raw_parity() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bounded_values/input.bin"
        ));
        for mode in [1u8, 2, 3, 4] {
            let expected = match mode {
                1 => include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bounded_values/mode1.raw"
                ))
                .as_slice(),
                2 => include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bounded_values/mode2.raw"
                ))
                .as_slice(),
                3 => include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bounded_values/mode3.raw"
                ))
                .as_slice(),
                _ => include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bounded_values/mode4.raw"
                ))
                .as_slice(),
            };
            assert_eq!(transform_backend(source, mode).unwrap(), expected);
            assert_eq!(
                inverse_backend(expected, mode, source.len()).unwrap(),
                source
            );
        }
    }
    #[test]
    fn invalid_mode_is_not_aliased() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bounded_values/input.bin"
        ));
        assert!(transform_backend(source, 5).is_err());
    }
}
