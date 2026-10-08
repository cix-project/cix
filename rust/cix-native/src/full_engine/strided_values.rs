//! Bounded recognition primitives for the historical 28-byte WCSTools grid.
//!
//! The full grid frame codecs retain several independently paid representations
//! (CIXS1, CIXY1, CIXZ1 and CIXB variants).  Their shared admission condition is
//! deliberately narrow: exactly the standard seven-int header followed by the
//! declared count of 28-byte records.

use super::arithmetic::{
    colex_rank, colex_unrank, fixed_be, multinomial_cardinality, read_fixed, vdecode, vencode,
    verify_frame, ArithmeticDecoder, ArithmeticEncoder, Fenwick,
};
use num_bigint::BigUint;
use num_traits::{ToPrimitive, Zero};

pub const RECORD_BYTES: usize = 28;
pub const MAX_RECORDS: usize = 8_000_000;
pub const GRID_MAGIC: &[u8] = b"CIXS\x01";
pub const GRID_RAW_MAGIC: &[u8] = b"SAGR1";
// The decoder retains the backend result while reconstructing a grid.  Keep
// three declared-output lengths for inverse rows, coordinate/residue working
// vectors, and the returned output before assigning a backend-output budget.
const INVERSE_WORKING_MULTIPLIER: usize = 3;
const MAX_RAW_INVERSE_BYTES: usize = 256 * 1024 * 1024;
const GRID_FIELDS: &[(usize, usize, u64)] = &[
    (0, 8, 0x3e73_856b_df09_d18e),
    (8, 8, 0x3e6a_073a_7eb7_c212),
    (20, 4, 0x3e3f_3bdf_cb42_e8e1),
    (24, 4, 0x3e34_d295_322c_9b40),
];
pub type EncodeBackend<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a;
pub type DecodeBackend<'a> = dyn Fn(&[u8], usize) -> Result<Vec<u8>, String> + 'a;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WcsGrid {
    pub records: usize,
}

fn le_i32(data: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// Returns a grid only for the exact historical WCSTools subset.  The count's
/// sign is retained only for compatibility with the source recognizer; the
/// archive transform consumes the absolute number of records.
pub fn recognize_wcs_grid(data: &[u8]) -> Option<WcsGrid> {
    if data.len() < RECORD_BYTES {
        return None;
    }
    let count = le_i32(data, 8)?;
    let stnum = le_i32(data, 12)?;
    let mprop = le_i32(data, 16)?;
    let nmag = le_i32(data, 20)?;
    let width = le_i32(data, 24)?;
    let records = count.unsigned_abs() as usize;
    if count == 0
        || records > MAX_RECORDS
        || stnum != 0
        || mprop != 1
        || nmag.unsigned_abs() != 1
        || width != RECORD_BYTES as i32
    {
        return None;
    }
    let expected = RECORD_BYTES.checked_add(RECORD_BYTES.checked_mul(records)?)?;
    (expected == data.len()).then_some(WcsGrid { records })
}

pub fn zigzag_i64(value: i64) -> u64 {
    ((value as u64) << 1) ^ ((value >> 63) as u64)
}
pub fn unzigzag_i64(value: u64) -> i64 {
    if value & 1 == 0 {
        (value >> 1) as i64
    } else {
        -((value >> 1) as i64) - 1
    }
}

/// Reversible mode-zero `SAGR1` transform.  This is the historic paid record
/// transpose: the seven-int header, mode and record count precede 28 column
/// streams.  No shape, corpus or source bytes are implicit in the inverse.
pub fn transform_wcs_grid_transpose(data: &[u8]) -> Result<Vec<u8>, String> {
    let grid = recognize_wcs_grid(data).ok_or("unsupported fixed-stride grid")?;
    let mut out = GRID_RAW_MAGIC.to_vec();
    out.push(0);
    out.extend(vencode(grid.records as u64));
    out.push(0); // historical tail length; exact admission requires no tail
    out.extend_from_slice(&data[..RECORD_BYTES]);
    for column in 0..RECORD_BYTES {
        for row in 0..grid.records {
            out.push(data[RECORD_BYTES + row * RECORD_BYTES + column]);
        }
    }
    Ok(out)
}

fn inverse_wcs_grid_transpose_bounded(blob: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    if !blob.starts_with(GRID_RAW_MAGIC) || blob.get(GRID_RAW_MAGIC.len()) != Some(&0) {
        return Err("invalid grid transpose".into());
    }
    let (records, mut at) = vdecode(blob, GRID_RAW_MAGIC.len() + 1)?;
    let records = usize::try_from(records).map_err(|_| "grid record count overflow")?;
    if records > MAX_RECORDS {
        return Err("grid record count exceeds limit".into());
    }
    let tail_length = usize::from(*blob.get(at).ok_or("missing grid tail length")?);
    at += 1;
    if tail_length != 0 {
        return Err("unsupported grid tail".into());
    }
    let header_end = at.checked_add(RECORD_BYTES).ok_or("grid header overflow")?;
    let header = blob.get(at..header_end).ok_or("truncated grid header")?;
    at = header_end;
    let body_length = records
        .checked_mul(RECORD_BYTES)
        .ok_or("grid extent overflow")?;
    let output_length = RECORD_BYTES
        .checked_add(body_length)
        .ok_or("grid extent overflow")?;
    if output_length > max_output {
        return Err("grid output exceeds admission".into());
    }
    let end = at.checked_add(body_length).ok_or("grid extent overflow")?;
    let columns = blob.get(at..end).ok_or("truncated grid transpose")?;
    if end != blob.len() {
        return Err("trailing grid transpose".into());
    }
    let mut out = Vec::new();
    out.try_reserve(output_length)
        .map_err(|_| "grid output allocation")?;
    out.extend_from_slice(header);
    for row in 0..records {
        for column in 0..RECORD_BYTES {
            out.push(columns[column * records + row]);
        }
    }
    if recognize_wcs_grid(&out).is_none() {
        return Err("restored grid extent rejected".into());
    }
    Ok(out)
}

pub fn inverse_wcs_grid_transpose(blob: &[u8]) -> Result<Vec<u8>, String> {
    inverse_wcs_grid_transpose_bounded(blob, MAX_RAW_INVERSE_BYTES)
}

pub fn inverse_wcs_grid_transpose_with_limit(
    blob: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, String> {
    inverse_wcs_grid_transpose_bounded(blob, max_output)
}

fn signed(value: i64) -> Result<Vec<u8>, String> {
    let encoded = if value >= 0 {
        u64::try_from(value)
            .map_err(|_| "signed overflow")?
            .checked_mul(2)
            .ok_or("signed overflow")?
    } else {
        value
            .unsigned_abs()
            .checked_mul(2)
            .and_then(|v| v.checked_sub(1))
            .ok_or("signed overflow")?
    };
    Ok(vencode(encoded))
}

fn read_signed(data: &[u8], at: usize) -> Result<(i64, usize), String> {
    let (value, next) = vdecode(data, at)?;
    let result = if value & 1 == 0 {
        i64::try_from(value / 2).map_err(|_| "signed overflow")?
    } else {
        i64::try_from(value / 2)
            .map_err(|_| "signed overflow")?
            .checked_neg()
            .and_then(|v| v.checked_sub(1))
            .ok_or("signed overflow")?
    };
    Ok((result, next))
}

fn type_counts(data: &[u8]) -> [usize; 256] {
    let mut counts = [0usize; 256];
    for value in data {
        counts[usize::from(*value)] += 1;
    }
    counts
}

fn composition_rank(parts: &[usize]) -> Result<BigUint, String> {
    if parts.is_empty() || parts.contains(&0) {
        return Err("invalid composition".into());
    }
    let mut total = 0usize;
    let mut cuts = Vec::new();
    for value in &parts[..parts.len().saturating_sub(1)] {
        total = total.checked_add(*value).ok_or("composition overflow")?;
        cuts.push(total.checked_sub(1).ok_or("composition overflow")?);
    }
    colex_rank(&cuts)
}

fn composition_unrank(rank: &BigUint, total: usize, count: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return if total == 0 {
            Ok(Vec::new())
        } else {
            Err("invalid composition".into())
        };
    }
    if count == 1 {
        return Ok(vec![total]);
    }
    if total == 0 {
        return Err("invalid composition".into());
    }
    let cuts = colex_unrank(rank, count - 1, total - 1)?;
    let mut previous = 0usize;
    let mut parts = Vec::with_capacity(count);
    for cut in cuts {
        let current = cut.checked_add(1).ok_or("composition overflow")?;
        parts.push(
            current
                .checked_sub(previous)
                .ok_or("composition overflow")?,
        );
        previous = current;
    }
    parts.push(total.checked_sub(previous).ok_or("composition overflow")?);
    if parts.contains(&0) {
        return Err("invalid composition".into());
    }
    Ok(parts)
}

fn range_encode(data: &[u8], counts: &[usize]) -> Result<(Vec<u8>, usize), String> {
    let mut mutable = counts.to_vec();
    let mut tree = Fenwick::new(&mutable)?;
    let mut remaining = data.len();
    let mut coder = ArithmeticEncoder::new();
    for value in data {
        let symbol = usize::from(*value);
        if mutable[symbol] == 0 {
            return Err("type class composition".into());
        }
        let low = u64::try_from(tree.prefix(symbol)?).map_err(|_| "type class prefix")?;
        let high = low
            .checked_add(mutable[symbol] as u64)
            .ok_or("type class interval")?;
        coder.encode(low, high, remaining as u64)?;
        mutable[symbol] -= 1;
        tree.add(symbol, -1)?;
        remaining -= 1;
    }
    Ok(coder.finish())
}

fn range_decode(payload: &[u8], bits: usize, counts: &[usize]) -> Result<Vec<u8>, String> {
    if payload.len() != bits.div_ceil(8) {
        return Err("type class range length".into());
    }
    let mut mutable = counts.to_vec();
    let mut tree = Fenwick::new(&mutable)?;
    let mut remaining = counts.iter().sum::<usize>();
    let mut coder = ArithmeticDecoder::new(payload, bits)?;
    let mut out = Vec::with_capacity(remaining);
    while remaining != 0 {
        let target = i128::from(coder.target(remaining as u64)?);
        let symbol = tree.select(target)?;
        let low = u64::try_from(tree.prefix(symbol)?).map_err(|_| "type class prefix")?;
        coder.update(
            low,
            low.checked_add(mutable[symbol] as u64)
                .ok_or("type class interval")?,
            remaining as u64,
        )?;
        out.push(u8::try_from(symbol).map_err(|_| "type class symbol")?);
        mutable[symbol] -= 1;
        tree.add(symbol, -1)?;
        remaining -= 1;
    }
    Ok(out)
}

pub(crate) fn type_class_encode(data: &[u8], force_range: bool) -> Result<Vec<u8>, String> {
    let counts = type_counts(data);
    let active = counts
        .iter()
        .enumerate()
        .filter_map(|(symbol, count)| (*count != 0).then_some(symbol))
        .collect::<Vec<_>>();
    let n = data.len();
    let m = active.len();
    // `record_float_grid.type_frame` stores the complete multinomial frame,
    // including its own paid stream length.  The outer residue chunk length is
    // not a substitute: this byte is part of the frozen SAGR1 representation.
    let mut out = vencode(n as u64);
    out.extend(vencode(m as u64));
    if n == 0 {
        return Ok(out);
    }
    if m == 0 || m > n || m > 256 {
        return Err("invalid active alphabet".into());
    }
    if m < 256 {
        out.extend(fixed_be(
            &colex_rank(&active)?,
            (&super::arithmetic::choose(256, m) - BigUint::from(1u8)).bits() as usize,
        )?);
    }
    let parts = active
        .iter()
        .map(|symbol| counts[*symbol])
        .collect::<Vec<_>>();
    if m > 1 {
        out.extend(fixed_be(
            &composition_rank(&parts)?,
            (&super::arithmetic::choose(n - 1, m - 1) - BigUint::from(1u8)).bits() as usize,
        )?);
    }
    let card = multinomial_cardinality(&counts)?;
    // `type_frame(..., range_control=True)` replaces the rank suffix even for
    // a deterministic class.  That deliberately retains the arithmetic
    // terminator (`bits=2`, payload `0x40`) in frozen SAGR1 mode three.
    if force_range {
        let (payload, bits) = range_encode(data, &counts)?;
        out.push(1);
        out.extend(vencode(bits as u64));
        out.extend(payload);
    } else {
        let ids = data
            .iter()
            .map(|value| usize::from(*value))
            .collect::<Vec<_>>();
        let rank = remaining_rank(&ids, &counts)?;
        out.push(0);
        out.extend(fixed_be(
            &rank,
            (&card - BigUint::from(1u8)).bits() as usize,
        )?);
    }
    Ok(out)
}

fn remaining_rank(ids: &[usize], counts: &[usize]) -> Result<BigUint, String> {
    let mut total = multinomial_cardinality(counts)?;
    let mut tree = Fenwick::new(counts)?;
    let mut mutable = counts.to_vec();
    let mut remaining = ids.len();
    let mut rank = BigUint::zero();
    for symbol in ids {
        if *symbol >= mutable.len() || mutable[*symbol] == 0 {
            return Err("type class composition".into());
        }
        let prefix = u64::try_from(tree.prefix(*symbol)?).map_err(|_| "type class prefix")?;
        rank += (&total * BigUint::from(prefix)) / remaining;
        total = (&total * BigUint::from(mutable[*symbol])) / remaining;
        mutable[*symbol] -= 1;
        tree.add(*symbol, -1)?;
        remaining -= 1;
    }
    Ok(rank)
}

pub(crate) fn type_class_decode(
    data: &[u8],
    mut at: usize,
    n: usize,
) -> Result<(Vec<u8>, usize), String> {
    let (encoded_n, next) = vdecode(data, at)?;
    at = next;
    if usize::try_from(encoded_n).map_err(|_| "type-class length overflow")? != n {
        return Err("type-class length mismatch".into());
    }
    let (m, next) = vdecode(data, at)?;
    at = next;
    let m = usize::try_from(m).map_err(|_| "active alphabet overflow")?;
    if n == 0 {
        return if m == 0 {
            Ok((Vec::new(), at))
        } else {
            Err("active symbols in empty type class".into())
        };
    }
    if m == 0 || m > n || m > 256 {
        return Err("invalid active alphabet".into());
    }
    let active = if m == 256 {
        (0..256).collect::<Vec<_>>()
    } else {
        let width = (&super::arithmetic::choose(256, m) - BigUint::from(1u8)).bits() as usize;
        let (rank, next) = read_fixed(data, at, width)?;
        at = next;
        colex_unrank(&rank, m, 256)?
    };
    let parts = if m == 1 {
        vec![n]
    } else {
        let width = (&super::arithmetic::choose(n - 1, m - 1) - BigUint::from(1u8)).bits() as usize;
        let (rank, next) = read_fixed(data, at, width)?;
        at = next;
        composition_unrank(&rank, n, m)?
    };
    let mut counts = vec![0usize; 256];
    for (symbol, count) in active.into_iter().zip(parts) {
        counts[symbol] = count;
    }
    let mode = *data.get(at).ok_or("missing type-class sequence")?;
    at += 1;
    let card = multinomial_cardinality(&counts)?;
    if mode == 0 {
        let width = (&card - BigUint::from(1u8)).bits() as usize;
        let (rank, next) = read_fixed(data, at, width)?;
        at = next;
        if rank >= card {
            return Err("invalid type-class rank".into());
        }
        Ok((remaining_unrank(rank, &counts)?, at))
    } else if mode == 1 {
        let (bits, next) = vdecode(data, at)?;
        at = next;
        let bits = usize::try_from(bits).map_err(|_| "type-class bit length")?;
        let end = at
            .checked_add(bits.div_ceil(8))
            .ok_or("type-class range length")?;
        let stream = range_decode(
            data.get(at..end).ok_or("truncated type-class range")?,
            bits,
            &counts,
        )?;
        Ok((stream, end))
    } else {
        Err("unknown type-class sequence mode".into())
    }
}

fn remaining_unrank(mut rank: BigUint, counts: &[usize]) -> Result<Vec<u8>, String> {
    let mut total = multinomial_cardinality(counts)?;
    if rank >= total {
        return Err("type class rank outside range".into());
    }
    let mut tree = Fenwick::new(counts)?;
    let mut mutable = counts.to_vec();
    let mut remaining = counts.iter().sum::<usize>();
    let mut out = Vec::with_capacity(remaining);
    while remaining != 0 {
        let target = ((&rank * remaining) / &total)
            .to_u64()
            .ok_or("type class target")?;
        let symbol = tree.select(i128::from(target))?;
        let prefix = u64::try_from(tree.prefix(symbol)?).map_err(|_| "type class prefix")?;
        rank -= (&total * BigUint::from(prefix)) / remaining;
        total = (&total * BigUint::from(mutable[symbol])) / remaining;
        out.push(u8::try_from(symbol).map_err(|_| "type class symbol")?);
        mutable[symbol] -= 1;
        tree.add(symbol, -1)?;
        remaining -= 1;
    }
    Ok(out)
}

fn round_ties_even(value: f64) -> i64 {
    let floor = value.floor();
    let fraction = value - floor;
    let rounded = if fraction < 0.5 {
        floor
    } else if fraction > 0.5 {
        floor + 1.0
    } else if (floor as i64) & 1 == 0 {
        floor
    } else {
        floor + 1.0
    };
    rounded as i64
}

fn field_value(record: &[u8], start: usize, width: usize) -> Result<(f64, u64), String> {
    let bytes = record
        .get(start..start.checked_add(width).ok_or("field overflow")?)
        .ok_or("truncated grid field")?;
    if width == 8 {
        let raw = u64::from_le_bytes(bytes.try_into().map_err(|_| "field width")?);
        Ok((f64::from_bits(raw), raw))
    } else {
        let raw = u32::from_le_bytes(bytes.try_into().map_err(|_| "field width")?);
        Ok((f64::from(f32::from_bits(raw)), u64::from(raw)))
    }
}

fn predicted_bits(value: i64, scale: f64, width: usize) -> u64 {
    if width == 8 {
        (value as f64 * scale).to_bits()
    } else {
        u64::from(((value as f64 * scale) as f32).to_bits())
    }
}

fn encode_grid_field(
    records: &[Vec<u8>],
    start: usize,
    width: usize,
    scale: f64,
    mode: u8,
) -> Result<Vec<u8>, String> {
    let mut q = Vec::with_capacity(records.len());
    let mut residues = Vec::with_capacity(records.len());
    let bound = (i32::MAX as f64) * scale;
    for record in records {
        let (value, raw) = field_value(record, start, width)?;
        let quantized = if value.is_finite() && value.abs() <= bound {
            round_ties_even(value / scale)
        } else {
            0
        };
        let predicted = predicted_bits(quantized, scale, width);
        let mask = if width == 8 {
            u64::MAX
        } else {
            u64::from(u32::MAX)
        };
        let residue = raw.wrapping_sub(predicted) & mask;
        q.push(quantized);
        residues.push(if width == 8 {
            residue as i64
        } else {
            i64::from((residue as u32) as i32)
        });
    }
    let mut qstream = Vec::new();
    let mut previous = 0i64;
    for value in &q {
        qstream.extend(signed(
            value.checked_sub(previous).ok_or("grid delta overflow")?,
        )?);
        previous = *value;
    }
    let mut out = scale.to_le_bytes().to_vec();
    out.extend(vencode(qstream.len() as u64));
    out.extend(qstream);
    let mut palette = residues.clone();
    palette.sort_unstable();
    palette.dedup();
    if palette.len() > 256 {
        out.push(0);
        let mut raw = Vec::with_capacity(residues.len() * width);
        for value in residues {
            if width == 8 {
                raw.extend_from_slice(&value.to_le_bytes());
            } else {
                raw.extend_from_slice(&(value as i32).to_le_bytes());
            }
        }
        out.extend(vencode(raw.len() as u64));
        out.extend(raw);
        return Ok(out);
    }
    out.push(1);
    out.extend(vencode(palette.len() as u64));
    for value in &palette {
        out.extend(signed(*value)?);
    }
    let lookup = palette
        .iter()
        .enumerate()
        .map(|(id, value)| (*value, id as u8))
        .collect::<std::collections::BTreeMap<_, _>>();
    let stream = residues
        .iter()
        .map(|value| lookup.get(value).copied().ok_or("residue palette"))
        .collect::<Result<Vec<_>, _>>()?;
    let payload = if mode == 1 {
        stream
    } else {
        let mut payload = Vec::new();
        for chunk in stream.chunks(1024) {
            let frame = type_class_encode(chunk, mode == 3)?;
            payload.extend(vencode(frame.len() as u64));
            payload.extend(frame);
        }
        payload
    };
    out.extend(vencode(payload.len() as u64));
    out.extend(payload);
    Ok(out)
}

fn decode_grid_field(
    data: &[u8],
    mut at: usize,
    records: &mut [Vec<u8>],
    start: usize,
    width: usize,
    mode: u8,
) -> Result<usize, String> {
    let scale_bytes: [u8; 8] = data
        .get(at..at.checked_add(8).ok_or("grid scale overflow")?)
        .ok_or("truncated grid scale")?
        .try_into()
        .map_err(|_| "truncated grid scale")?;
    let scale = f64::from_le_bytes(scale_bytes);
    at += 8;
    if !scale.is_finite() || scale <= 0.0 {
        return Err("invalid grid scale".into());
    }
    let (size, next) = vdecode(data, at)?;
    at = next;
    let size = usize::try_from(size).map_err(|_| "grid coordinate size")?;
    let end = at.checked_add(size).ok_or("grid coordinate size")?;
    let stream = data.get(at..end).ok_or("truncated integer stream")?;
    at = end;
    let mut q = Vec::with_capacity(records.len());
    let mut position = 0usize;
    let mut previous = 0i64;
    for _ in 0..records.len() {
        let (delta, next) = read_signed(stream, position)?;
        position = next;
        previous = previous
            .checked_add(delta)
            .ok_or("quantized coordinate overflow")?;
        if previous <= i64::from(i32::MIN) || previous >= i64::from(i32::MAX) {
            return Err("quantized coordinate outside range".into());
        }
        q.push(previous);
    }
    if position != stream.len() {
        return Err("integer stream length mismatch".into());
    }
    let coding = *data.get(at).ok_or("missing residual mode")?;
    at += 1;
    let mut palette = Vec::new();
    if coding == 1 {
        let (count, next) = vdecode(data, at)?;
        at = next;
        let count = usize::try_from(count).map_err(|_| "correction alphabet")?;
        if count > 256 || (count == 0 && !records.is_empty()) {
            return Err("invalid correction alphabet".into());
        }
        for _ in 0..count {
            let (value, next) = read_signed(data, at)?;
            at = next;
            if width == 4 && (value < i64::from(i32::MIN) || value > i64::from(i32::MAX)) {
                return Err("correction outside range".into());
            }
            palette.push(value);
        }
    } else if coding != 0 {
        return Err("invalid correction representation".into());
    }
    let (payload_size, next) = vdecode(data, at)?;
    at = next;
    let payload_size = usize::try_from(payload_size).map_err(|_| "correction payload")?;
    let payload_end = at.checked_add(payload_size).ok_or("correction payload")?;
    let payload = data
        .get(at..payload_end)
        .ok_or("truncated correction payload")?;
    at = payload_end;
    let residues = if coding == 0 {
        if payload.len() != records.len().checked_mul(width).ok_or("correction size")? {
            return Err("raw correction size mismatch".into());
        }
        if width == 8 {
            payload
                .as_chunks::<8>()
                .0
                .iter()
                .map(|chunk| i64::from_le_bytes(*chunk))
                .collect::<Vec<_>>()
        } else {
            payload
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| i64::from(i32::from_le_bytes(*chunk)))
                .collect::<Vec<_>>()
        }
    } else {
        let ids = if mode == 1 {
            payload.to_vec()
        } else {
            let mut ids = Vec::new();
            let mut position = 0usize;
            while ids.len() < records.len() {
                let (size, next) = vdecode(payload, position)?;
                position = next;
                let size = usize::try_from(size).map_err(|_| "type class size")?;
                let end = position.checked_add(size).ok_or("type class size")?;
                let chunk = payload.get(position..end).ok_or("truncated type class")?;
                let expected = (records.len() - ids.len()).min(1024);
                let (decoded, used) = type_class_decode(chunk, 0, expected)?;
                if used != chunk.len() || decoded.len() != expected {
                    return Err("correction class mismatch".into());
                }
                ids.extend(decoded);
                position = end;
            }
            if position != payload.len() {
                return Err("trailing correction classes".into());
            }
            ids
        };
        if ids.len() != records.len() || ids.iter().any(|id| usize::from(*id) >= palette.len()) {
            return Err("invalid correction symbols".into());
        }
        ids.into_iter()
            .map(|id| palette[usize::from(id)])
            .collect::<Vec<_>>()
    };
    for ((record, quantized), residue) in records.iter_mut().zip(q).zip(residues) {
        let predicted = predicted_bits(quantized, scale, width);
        let bits = if width == 8 {
            predicted.wrapping_add(residue as u64)
        } else {
            u64::from((predicted as u32).wrapping_add(residue as i32 as u32))
        };
        if width == 8 {
            record[start..start + 8].copy_from_slice(&bits.to_le_bytes());
        } else {
            record[start..start + 4].copy_from_slice(&(bits as u32).to_le_bytes());
        }
    }
    Ok(at)
}

/// Exact numerical `SAGR1` modes one through three.  The input admission is
/// the same strict seven-int layout as mode zero; numerical metadata, scales,
/// palettes, type classes and opaque bytes are all stored in the raw stream.
pub fn transform_wcs_grid_numeric(data: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    let grid = recognize_wcs_grid(data).ok_or("unsupported fixed-stride grid")?;
    if !matches!(mode, 1..=3) {
        return Err("invalid grid mode".into());
    }
    let records = (0..grid.records)
        .map(|row| {
            data[RECORD_BYTES + row * RECORD_BYTES..RECORD_BYTES + (row + 1) * RECORD_BYTES]
                .to_vec()
        })
        .collect::<Vec<_>>();
    let mut out = GRID_RAW_MAGIC.to_vec();
    out.push(mode);
    out.extend(vencode(grid.records as u64));
    out.push(0);
    out.extend_from_slice(&data[..RECORD_BYTES]);
    for column in 16..20 {
        for record in &records {
            out.push(record[column]);
        }
    }
    for (start, width, scale) in GRID_FIELDS {
        out.extend(encode_grid_field(
            &records,
            *start,
            *width,
            f64::from_bits(*scale),
            mode,
        )?);
    }
    Ok(out)
}

fn inverse_wcs_grid_numeric_bounded(blob: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    if !blob.starts_with(GRID_RAW_MAGIC) {
        return Err("invalid grid transform".into());
    }
    let mode = *blob
        .get(GRID_RAW_MAGIC.len())
        .ok_or("invalid grid transform")?;
    if !matches!(mode, 1..=3) {
        return Err("invalid grid transform".into());
    }
    let (records, mut at) = vdecode(blob, GRID_RAW_MAGIC.len() + 1)?;
    let records = usize::try_from(records).map_err(|_| "grid record count overflow")?;
    if records > MAX_RECORDS {
        return Err("grid record count exceeds limit".into());
    }
    let output_length = RECORD_BYTES
        .checked_add(
            records
                .checked_mul(RECORD_BYTES)
                .ok_or("grid extent overflow")?,
        )
        .ok_or("grid extent overflow")?;
    if output_length > max_output {
        return Err("grid output exceeds admission".into());
    }
    let tail = usize::from(*blob.get(at).ok_or("missing grid tail")?);
    at += 1;
    if tail != 0 {
        return Err("unsupported grid tail".into());
    }
    let header_end = at.checked_add(RECORD_BYTES).ok_or("grid header overflow")?;
    let header = blob
        .get(at..header_end)
        .ok_or("truncated grid header")?
        .to_vec();
    at = header_end;
    let mut values = vec![vec![0u8; RECORD_BYTES]; records];
    let opaque = records.checked_mul(4).ok_or("opaque field overflow")?;
    let opaque_end = at.checked_add(opaque).ok_or("opaque field overflow")?;
    let opaque_data = blob.get(at..opaque_end).ok_or("truncated opaque fields")?;
    at = opaque_end;
    for column in 0..4 {
        for row in 0..records {
            values[row][16 + column] = opaque_data[column * records + row];
        }
    }
    for (start, width, _) in GRID_FIELDS {
        at = decode_grid_field(blob, at, &mut values, *start, *width, mode)?;
    }
    if at != blob.len() {
        return Err("trailing grid transform".into());
    }
    let mut out = Vec::new();
    out.try_reserve(output_length)
        .map_err(|_| "grid output allocation")?;
    out.extend(header);
    for record in values {
        out.extend(record);
    }
    if recognize_wcs_grid(&out).is_none() {
        return Err("restored grid extent rejected".into());
    }
    Ok(out)
}

pub fn inverse_wcs_grid_numeric(blob: &[u8]) -> Result<Vec<u8>, String> {
    inverse_wcs_grid_numeric_bounded(blob, MAX_RAW_INVERSE_BYTES)
}

pub fn inverse_wcs_grid_numeric_with_limit(
    blob: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, String> {
    inverse_wcs_grid_numeric_bounded(blob, max_output)
}

pub fn inverse_wcs_grid(blob: &[u8]) -> Result<Vec<u8>, String> {
    inverse_wcs_grid_bounded(blob, MAX_RAW_INVERSE_BYTES)
}

fn inverse_wcs_grid_bounded(blob: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    match blob.get(GRID_RAW_MAGIC.len()).copied() {
        Some(0) => inverse_wcs_grid_transpose_bounded(blob, max_output),
        Some(1..=3) => inverse_wcs_grid_numeric_bounded(blob, max_output),
        _ => Err("invalid grid transform".into()),
    }
}

pub fn inverse_wcs_grid_with_limit(blob: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    inverse_wcs_grid_bounded(blob, max_output)
}

fn pack(source: &[u8], transformed: &[u8], backend: &EncodeBackend<'_>) -> Result<Vec<u8>, String> {
    let payload = backend(transformed)?;
    let mut frame = GRID_MAGIC.to_vec();
    frame.push(0); // historic XZ backend identifier
    frame.extend(vencode(source.len() as u64));
    frame.extend_from_slice(&crc32fast::hash(source).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn transformed_budget(output: usize, memory: usize) -> Result<usize, String> {
    let inverse = output
        .checked_mul(INVERSE_WORKING_MULTIPLIER)
        .ok_or("grid working-memory overflow")?;
    let reserved = output
        .checked_add(inverse)
        .ok_or("grid working-memory overflow")?;
    memory
        .checked_sub(reserved)
        .ok_or_else(|| "grid combined live-buffer limit".to_string())
}

/// Constructs a historical `CIXS1` frame around the exact mode-zero raw
/// transform.  The backend callback is intentionally injected so archive
/// admission remains separate from the fixed XZ provider qualification.
pub fn encode_wcs_grid_transpose(
    source: &[u8],
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    pack(source, &transform_wcs_grid_transpose(source)?, backend)
}

pub fn encode_wcs_grid_numeric_frame(
    source: &[u8],
    mode: u8,
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    pack(source, &transform_wcs_grid_numeric(source, mode)?, backend)
}

pub fn decode_wcs_grid_transpose(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if !frame.starts_with(GRID_MAGIC) || memory == 0 {
        return Err("invalid grid frame".into());
    }
    let backend_id = *frame
        .get(GRID_MAGIC.len())
        .ok_or("truncated grid backend")?;
    if backend_id != 0 {
        return Err("unexpected grid backend".into());
    }
    let (length, at) = vdecode(frame, GRID_MAGIC.len() + 1)?;
    let length = usize::try_from(length).map_err(|_| "grid output size overflow")?;
    if length > limit || length > memory {
        return Err("grid output exceeds admission limit".into());
    }
    let backend_budget = transformed_budget(length, memory)?;
    let checksum: [u8; 4] = frame
        .get(at..at.checked_add(4).ok_or("grid frame overflow")?)
        .ok_or("truncated grid frame")?
        .try_into()
        .map_err(|_| "truncated grid checksum")?;
    let transformed = backend(
        frame.get(at + 4..).ok_or("truncated grid payload")?,
        backend_budget,
    )?;
    if transformed.len() > backend_budget {
        return Err("backend ignored grid memory limit".into());
    }
    let restored = inverse_wcs_grid_transpose_bounded(&transformed, length)?;
    verify_frame(&restored, length, u32::from_be_bytes(checksum))?;
    Ok(restored)
}

pub fn decode_wcs_grid(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if !frame.starts_with(GRID_MAGIC) || memory == 0 {
        return Err("invalid grid frame".into());
    }
    if frame.get(GRID_MAGIC.len()) != Some(&0) {
        return Err("unexpected grid backend".into());
    }
    let (length, at) = vdecode(frame, GRID_MAGIC.len() + 1)?;
    let length = usize::try_from(length).map_err(|_| "grid output size overflow")?;
    if length > limit || length > memory {
        return Err("grid output exceeds admission limit".into());
    }
    let backend_budget = transformed_budget(length, memory)?;
    let checksum: [u8; 4] = frame
        .get(at..at.checked_add(4).ok_or("grid frame overflow")?)
        .ok_or("truncated grid frame")?
        .try_into()
        .map_err(|_| "truncated grid checksum")?;
    let transformed = backend(
        frame.get(at + 4..).ok_or("truncated grid payload")?,
        backend_budget,
    )?;
    if transformed.len() > backend_budget {
        return Err("backend ignored grid memory limit".into());
    }
    let restored = inverse_wcs_grid_bounded(&transformed, length)?;
    verify_frame(&restored, length, u32::from_be_bytes(checksum))?;
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_exact_wcs_extent() {
        let mut data = vec![0u8; RECORD_BYTES * 2];
        data[8..12].copy_from_slice(&(1i32).to_le_bytes());
        data[16..20].copy_from_slice(&(1i32).to_le_bytes());
        data[20..24].copy_from_slice(&(1i32).to_le_bytes());
        data[24..28].copy_from_slice(&(28i32).to_le_bytes());
        assert_eq!(recognize_wcs_grid(&data), Some(WcsGrid { records: 1 }));
        data.push(0);
        assert_eq!(recognize_wcs_grid(&data), None);
    }

    #[test]
    fn signed_varint_mapping_round_trips() {
        for value in [i64::MIN, i64::MIN + 1, -123, -1, 0, 1, 127, i64::MAX] {
            assert_eq!(unzigzag_i64(zigzag_i64(value)), value);
        }
        assert_eq!(zigzag_i64(i64::MIN), u64::MAX);
        assert_eq!(zigzag_i64(i64::MAX), u64::MAX - 1);
    }

    #[test]
    fn forced_type_class_range_keeps_deterministic_terminator() {
        // Frozen Python `type_frame` replaces the zero-bit rank marker with a
        // two-bit arithmetic terminator even when the sole symbol is forced.
        let frame = type_class_encode(&[9, 9, 9], true).unwrap();
        assert_eq!(frame, [3, 1, 9, 1, 2, 64]);
        assert_eq!(
            type_class_decode(&frame, 0, 3).unwrap(),
            (vec![9, 9, 9], frame.len())
        );
    }

    fn grid(records: usize) -> Vec<u8> {
        let mut data = vec![0u8; RECORD_BYTES * (records + 1)];
        data[8..12].copy_from_slice(&(records as i32).to_le_bytes());
        data[16..20].copy_from_slice(&(1i32).to_le_bytes());
        data[20..24].copy_from_slice(&(1i32).to_le_bytes());
        data[24..28].copy_from_slice(&(RECORD_BYTES as i32).to_le_bytes());
        for (index, byte) in data[RECORD_BYTES..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37);
        }
        data
    }

    #[test]
    fn paid_grid_transpose_and_frame_restore() {
        let source = grid(3);
        let raw = transform_wcs_grid_transpose(&source).unwrap();
        assert_eq!(inverse_wcs_grid_transpose(&raw).unwrap(), source);
        let frame = encode_wcs_grid_transpose(&source, &|value| Ok(value.to_vec())).unwrap();
        assert_eq!(
            decode_wcs_grid_transpose(&frame, 4096, 4096, &|value, limit| {
                if value.len() > limit {
                    Err("limit".into())
                } else {
                    Ok(value.to_vec())
                }
            })
            .unwrap(),
            source
        );
        assert!(inverse_wcs_grid_transpose(&raw[..raw.len() - 1]).is_err());
    }

    #[test]
    fn matches_tiny_legacy_sagr_vectors_for_every_mode() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/record_stride/grid.input"
        ));
        let expected = [
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/record_stride/grid-sagr-mode0.raw"
            ))
            .as_slice(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/record_stride/grid-sagr-mode1.raw"
            ))
            .as_slice(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/record_stride/grid-sagr-mode2.raw"
            ))
            .as_slice(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/record_stride/grid-sagr-mode3.raw"
            ))
            .as_slice(),
        ];
        assert_eq!(transform_wcs_grid_transpose(source).unwrap(), expected[0]);
        assert_eq!(inverse_wcs_grid_transpose(expected[0]).unwrap(), source);
        for mode in 1..=3 {
            assert_eq!(
                transform_wcs_grid_numeric(source, mode).unwrap(),
                expected[mode as usize]
            );
            assert_eq!(
                inverse_wcs_grid_numeric(expected[mode as usize]).unwrap(),
                source
            );
        }
    }
}
