//! Legacy CIXM6 count-vector histogram type classes (mode 20).
use super::{rank, wavelet};
use num_bigint::BigUint;
use num_traits::{One, ToPrimitive, Zero};

const VERSION: u8 = 1;

fn put_v(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_v(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..usize::BITS as usize).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated histogram varint")?;
        *pos += 1;
        let part = (byte & 0x7f) as usize;
        if part > (usize::MAX >> shift) {
            return Err("histogram varint overflow".into());
        }
        value |= part
            .checked_shl(shift as u32)
            .ok_or("histogram varint overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized histogram varint".into())
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
    let bytes = value.to_bytes_be();
    let mut out = vec![0; size.saturating_sub(bytes.len())];
    out.extend(bytes);
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

fn active_prefix(active: &[usize]) -> Vec<u8> {
    let mut out = Vec::new();
    put_v(active.len(), &mut out);
    if active.len() < 256 {
        let width = fixed_width(&(choose(256, active.len()) - BigUint::one()));
        out.extend(fixed_bytes(&colex_rank(active), width));
    }
    out
}

fn zig(value: i128) -> Result<usize, String> {
    let encoded = if value >= 0 {
        value.checked_mul(2)
    } else {
        value
            .checked_neg()
            .and_then(|v| v.checked_mul(2))
            .and_then(|v| v.checked_sub(1))
    }
    .ok_or("histogram zigzag overflow")?;
    usize::try_from(encoded).map_err(|_| "histogram zigzag outside machine range".into())
}

fn unzig(value: usize) -> i128 {
    let value = value as i128;
    if value & 1 == 0 {
        value / 2
    } else {
        -(value / 2) - 1
    }
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
    fn bit(&mut self, bit: usize) {
        self.current = (self.current << 1) | (bit != 0) as u8;
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

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn bit(&mut self) -> Result<usize, String> {
        if self.pos >= self.data.len().saturating_mul(8) {
            return Err("truncated histogram bitstream".into());
        }
        let bit = ((self.data[self.pos / 8] >> (7 - self.pos % 8)) & 1) as usize;
        self.pos += 1;
        Ok(bit)
    }
    fn bits(&mut self, width: usize) -> Result<usize, String> {
        let mut value = 0usize;
        for _ in 0..width {
            value = (value << 1) | self.bit()?;
        }
        Ok(value)
    }
}

fn log_choose(n: usize, k: usize) -> f64 {
    choose(n, k)
        .to_f64()
        .expect("byte-alphabet binomial fits finite f64")
        .log2()
}

fn estimate_wavelet_node(counts: &[usize; 256], lo: usize, hi: usize, depth: usize) -> f64 {
    let n: usize = counts[lo..hi].iter().sum();
    if n == 0 || depth == 8 {
        return 0.0;
    }
    let middle = (lo + hi) / 2;
    let ones: usize = counts[middle..hi].iter().sum();
    let mut bits = ((n + 1) as f64).log2();
    if ones > 0 && ones < n {
        bits += log_choose(n, ones);
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

// Count-vector integer streams contain at most 255 classes. At that length the
// frozen byte-stream portfolio can reach only static and wavelet candidates.
fn encode_class_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut best = vec![0];
    best.extend(rank::encode_type_class(data)?);
    if data.len() >= 64 && estimate_wavelet_bits(data) / 8.0 <= best.len() as f64 * 1.03 + 8.0 {
        let mut candidate = vec![4];
        candidate.extend(wavelet::encode(data));
        if candidate.len() < best.len() {
            best = candidate;
        }
    }
    Ok(best)
}

fn encode_uints(values: &[usize]) -> Result<Vec<u8>, String> {
    if values.is_empty() {
        return Ok(vec![0]);
    }
    let mut best = vec![0];
    for &value in values {
        put_v(value, &mut best);
    }
    for width in 0..=16usize {
        if let Some(candidate) = rice_candidate(values, width, best.len()) {
            best = candidate;
        }
    }
    for (mode, class_blob) in class_blobs(values)? {
        let candidate = class_candidate(mode, class_blob, values)?;
        if candidate.len() < best.len() {
            best = candidate;
        }
    }
    Ok(best)
}

fn rice_candidate(values: &[usize], width: usize, incumbent_bytes: usize) -> Option<Vec<u8>> {
    let limit = incumbent_bytes.saturating_sub(2) * 8;
    let mut cost = 0usize;
    for &value in values {
        cost = cost.checked_add(value >> width)?.checked_add(1 + width)?;
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
        writer.bits(value & ((1usize << width) - 1), width);
    }
    let mut candidate = vec![1, width as u8];
    candidate.extend(writer.finish());
    Some(candidate)
}

fn class_blobs(values: &[usize]) -> Result<Vec<(u8, Vec<u8>)>, String> {
    let mut classes = Vec::with_capacity(values.len());
    for &value in values {
        let shifted = value.checked_add(1).ok_or("histogram integer overflow")?;
        classes.push((usize::BITS - shifted.leading_zeros() - 1) as u8);
    }
    Ok(vec![
        (2, rank::encode_type_class(&classes)?),
        (3, encode_class_bytes(&classes)?),
    ])
}

fn class_candidate(mode: u8, class_blob: Vec<u8>, values: &[usize]) -> Result<Vec<u8>, String> {
    let mut writer = BitWriter::new();
    for &value in values {
        let shifted = value.checked_add(1).ok_or("histogram integer overflow")?;
        let width = (usize::BITS - shifted.leading_zeros() - 1) as usize;
        writer.bits(shifted - (1usize << width), width);
    }
    let mut candidate = vec![mode];
    put_v(class_blob.len(), &mut candidate);
    candidate.extend(class_blob);
    candidate.extend(writer.finish());
    Ok(candidate)
}

/// Return every frozen count-vector candidate with its stable selector name.
pub fn encode_candidates(counts: &[usize; 256]) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
    let total = counts
        .iter()
        .try_fold(0usize, |sum, &value| sum.checked_add(value))
        .ok_or("histogram total overflow")?;
    if total == 0 {
        return Err("experimental count vector requires non-empty block".into());
    }
    let active: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(symbol, &count)| (count > 0).then_some(symbol))
        .collect();
    let active_counts: Vec<usize> = active.iter().map(|&symbol| counts[symbol]).collect();
    let prefix = active_prefix(&active);
    let mut candidates = Vec::with_capacity(6);

    let active_uint: Vec<usize> = active_counts[..active_counts.len() - 1]
        .iter()
        .map(|count| count - 1)
        .collect();
    let mut candidate = vec![VERSION, 0];
    candidate.extend_from_slice(&prefix);
    candidate.extend(encode_uints(&active_uint)?);
    candidates.push(("active-uint", candidate));

    let mut previous = 0i128;
    let mut deltas = Vec::with_capacity(active_counts.len().saturating_sub(1));
    for &count in &active_counts[..active_counts.len() - 1] {
        deltas.push(zig(count as i128 - previous)?);
        previous = count as i128;
    }
    let mut candidate = vec![VERSION, 1];
    candidate.extend_from_slice(&prefix);
    candidate.extend(encode_uints(&deltas)?);
    candidates.push(("active-delta", candidate));

    let dense = &counts[..255];
    let mut candidate = vec![VERSION, 2];
    candidate.extend(encode_uints(dense)?);
    candidates.push(("dense-uint", candidate));

    previous = 0;
    deltas.clear();
    for &count in dense {
        deltas.push(zig(count as i128 - previous)?);
        previous = count as i128;
    }
    let mut candidate = vec![VERSION, 3];
    candidate.extend(encode_uints(&deltas)?);
    candidates.push(("dense-delta", candidate));

    let active_center = (total / active_counts.len()) as i128;
    let centered: Vec<usize> = active_counts[..active_counts.len() - 1]
        .iter()
        .map(|&count| zig(count as i128 - active_center))
        .collect::<Result<_, _>>()?;
    let mut candidate = vec![VERSION, 4];
    candidate.extend_from_slice(&prefix);
    candidate.extend(encode_uints(&centered)?);
    candidates.push(("active-centered", candidate));

    let dense_center = (total / 256) as i128;
    let centered: Vec<usize> = dense
        .iter()
        .map(|&count| zig(count as i128 - dense_center))
        .collect::<Result<_, _>>()?;
    let mut candidate = vec![VERSION, 5];
    candidate.extend(encode_uints(&centered)?);
    candidates.push(("dense-centered", candidate));
    Ok(candidates)
}

/// Select the smallest complete count-vector representation, breaking physical
/// size ties by the frozen candidate name.
pub fn encode_best(counts: &[usize; 256]) -> Result<(Vec<u8>, &'static str), String> {
    encode_candidates(counts)?
        .into_iter()
        .min_by(|(name_a, data_a), (name_b, data_b)| {
            (data_a.len(), *name_a).cmp(&(data_b.len(), *name_b))
        })
        .map(|(name, data)| (data, name))
        .ok_or("no histogram candidates".into())
}

/// Replace the incumbent type-class histogram only when the complete mode-20
/// representation is physically smaller under the frozen eligibility gates.
pub fn type_class_candidate(data: &[u8], incumbent: &[u8]) -> Result<Option<Vec<u8>>, String> {
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
    header_size +=
        fixed_width(&(choose(data.len() - 1, active.len() - 1) - BigUint::one())).div_ceil(8);
    if header_size < 64 {
        return Ok(None);
    }
    if header_size > incumbent.len() {
        return Err("incumbent type-class header is truncated".into());
    }
    let (histogram, _) = encode_best(&counts)?;
    let mut prefix = Vec::new();
    put_v(histogram.len(), &mut prefix);
    prefix.extend(histogram);
    if prefix.len() >= header_size {
        return Ok(None);
    }
    prefix.extend_from_slice(&incumbent[header_size..]);
    Ok(Some(prefix))
}

fn decode_class_bytes(blob: &[u8], count: usize) -> Result<Vec<u8>, String> {
    let mode = *blob.first().ok_or("empty histogram byte substream")?;
    match mode {
        0 => {
            let mut pos = 1;
            let values = rank::decode_type_class(blob, &mut pos, count)?;
            if pos != blob.len() {
                return Err("trailing histogram class bytes".into());
            }
            Ok(values)
        }
        4 => wavelet::decode(&blob[1..], count),
        other => Err(format!(
            "unreachable histogram class substream mode {other}"
        )),
    }
}

fn decode_uints(blob: &[u8], count: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mode = *blob.first().ok_or("empty histogram integer stream")?;
    let mut pos = 1;
    match mode {
        0 => decode_varints(blob, &mut pos, count),
        1 => decode_rice(blob, pos, count),
        2 | 3 => decode_classed_uints(blob, &mut pos, count, mode),
        other => Err(format!("unknown histogram integer stream mode {other}")),
    }
}

fn decode_varints(blob: &[u8], pos: &mut usize, count: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_v(blob, pos)?);
    }
    if *pos != blob.len() {
        return Err("trailing histogram varints".into());
    }
    Ok(out)
}

fn decode_rice(blob: &[u8], pos: usize, count: usize) -> Result<Vec<usize>, String> {
    let width = *blob.get(pos).ok_or("missing histogram Rice parameter")? as usize;
    if width > 16 {
        return Err("invalid histogram Rice parameter".into());
    }
    let mut reader = BitReader {
        data: &blob[pos + 1..],
        pos: 0,
    };
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut quotient = 0usize;
        while reader.bit()? != 0 {
            quotient = quotient.checked_add(1).ok_or("Rice quotient overflow")?;
        }
        if quotient > (usize::MAX >> width) {
            return Err("Rice value overflow".into());
        }
        let low = reader.bits(width)?;
        out.push(
            quotient
                .checked_shl(width as u32)
                .and_then(|value| value.checked_add(low))
                .ok_or("Rice value overflow")?,
        );
    }
    Ok(out)
}

fn decode_classed_uints(
    blob: &[u8],
    pos: &mut usize,
    count: usize,
    mode: u8,
) -> Result<Vec<usize>, String> {
    let class_len = read_v(blob, pos)?;
    let end = pos
        .checked_add(class_len)
        .ok_or("histogram class length overflow")?;
    if end > blob.len() {
        return Err("truncated histogram classes".into());
    }
    let classes = if mode == 2 {
        let mut class_pos = 0;
        let values = rank::decode_type_class(&blob[*pos..end], &mut class_pos, count)?;
        if class_pos != class_len {
            return Err("trailing histogram class data".into());
        }
        values
    } else {
        decode_class_bytes(&blob[*pos..end], count)?
    };
    let mut reader = BitReader {
        data: &blob[end..],
        pos: 0,
    };
    classes
        .into_iter()
        .map(|class| decode_classed_value(&mut reader, class))
        .collect()
}

fn decode_classed_value(reader: &mut BitReader<'_>, class: u8) -> Result<usize, String> {
    let width = class as usize;
    if width >= usize::BITS as usize {
        return Err("histogram integer class outside machine range".into());
    }
    (1usize << width)
        .checked_add(reader.bits(width)?)
        .and_then(|value| value.checked_sub(1))
        .ok_or("histogram integer overflow".into())
}

/// Decode a standalone frozen count-vector payload.
pub fn decode_counts(blob: &[u8], total: usize) -> Result<[usize; 256], String> {
    if blob.len() < 2 || blob[0] != VERSION {
        return Err("unknown count-vector version".into());
    }
    if total == 0 {
        return Err("invalid count-vector total".into());
    }
    let mode = blob[1];
    match mode {
        0 | 1 | 4 => decode_active_counts(blob, total, mode),
        2 | 3 | 5 => decode_dense_counts(blob, total, mode),
        _ => Err(format!("unknown count-vector mode {mode}")),
    }
}

fn checked_count(value: i128, positive: bool, message: &'static str) -> Result<usize, String> {
    if value < 0 || positive && value == 0 {
        return Err(message.into());
    }
    usize::try_from(value).map_err(|_| message.into())
}

fn active_values(
    encoded: Vec<usize>,
    mode: u8,
    total: usize,
    active_count: usize,
) -> Result<Vec<usize>, String> {
    match mode {
        0 => encoded
            .into_iter()
            .map(|value| value.checked_add(1).ok_or("active count overflow".into()))
            .collect(),
        1 => {
            let mut previous = 0i128;
            encoded
                .into_iter()
                .map(|value| {
                    previous += unzig(value);
                    checked_count(previous, true, "non-positive active count")
                })
                .collect()
        }
        4 => encoded
            .into_iter()
            .map(|value| {
                checked_count(
                    (total / active_count) as i128 + unzig(value),
                    true,
                    "non-positive centered active count",
                )
            })
            .collect(),
        _ => unreachable!(),
    }
}

fn decode_active_counts(blob: &[u8], total: usize, mode: u8) -> Result<[usize; 256], String> {
    let mut pos = 2;
    let active_count = read_v(blob, &mut pos)?;
    if active_count == 0 || active_count > 256 || active_count > total {
        return Err("invalid active count alphabet".into());
    }
    let active = if active_count == 256 {
        (0..256).collect()
    } else {
        rank::decode_colex_positions(blob, &mut pos, 256, active_count)?
    };
    let mut first = active_values(
        decode_uints(&blob[pos..], active_count - 1)?,
        mode,
        total,
        active_count,
    )?;
    let first_total = first
        .iter()
        .try_fold(0usize, |sum, &value| sum.checked_add(value))
        .ok_or("active histogram total overflow")?;
    first.push(
        total
            .checked_sub(first_total)
            .filter(|&value| value > 0)
            .ok_or("invalid implied active count")?,
    );
    let mut counts = [0usize; 256];
    for (symbol, count) in active.into_iter().zip(first) {
        counts[symbol] = count;
    }
    Ok(counts)
}

fn dense_values(encoded: Vec<usize>, mode: u8, total: usize) -> Result<Vec<usize>, String> {
    match mode {
        2 => Ok(encoded),
        3 => {
            let mut previous = 0i128;
            encoded
                .into_iter()
                .map(|value| {
                    previous += unzig(value);
                    checked_count(previous, false, "negative dense count")
                })
                .collect()
        }
        5 => encoded
            .into_iter()
            .map(|value| {
                checked_count(
                    (total / 256) as i128 + unzig(value),
                    false,
                    "negative centered dense count",
                )
            })
            .collect(),
        _ => unreachable!(),
    }
}

fn decode_dense_counts(blob: &[u8], total: usize, mode: u8) -> Result<[usize; 256], String> {
    let first = dense_values(decode_uints(&blob[2..], 255)?, mode, total)?;
    let first_total = first
        .iter()
        .try_fold(0usize, |sum, &value| sum.checked_add(value))
        .ok_or("dense histogram total overflow")?;
    let mut counts = [0usize; 256];
    counts[..255].copy_from_slice(&first);
    counts[255] = total
        .checked_sub(first_total)
        .ok_or("invalid implied dense count")?;
    Ok(counts)
}

pub fn decode_type_class(blob: &[u8], total: usize) -> Result<Vec<u8>, String> {
    let mut pos = 0;
    let length = read_v(blob, &mut pos)?;
    let end = pos.checked_add(length).ok_or("histogram length overflow")?;
    if end > blob.len() {
        return Err("truncated CIX histogram".into());
    }
    let counts = decode_counts(&blob[pos..end], total)?;
    rank::decode_type_class_with_counts(&blob[end..], total, &counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_table_candidates_round_trip() {
        let mut counts = [0usize; 256];
        counts[0] = 19;
        counts[1] = 2;
        counts[7] = 31;
        counts[129] = 1;
        counts[255] = 11;
        let total = counts.iter().sum();

        for (name, encoded) in encode_candidates(&counts).unwrap() {
            assert_eq!(decode_counts(&encoded, total).unwrap(), counts, "{name}");
        }
    }

    #[test]
    fn count_table_rejects_malformed_prefix_and_truncation() {
        let counts = {
            let mut counts = [0usize; 256];
            counts[3] = 13;
            counts[42] = 7;
            counts
        };
        let total = counts.iter().sum();
        let (encoded, _) = encode_best(&counts).unwrap();
        assert!(decode_counts(&[], total).is_err());
        assert!(decode_counts(&[VERSION + 1, 0], total).is_err());
        assert!(decode_counts(&encoded[..encoded.len() - 1], total).is_err());
    }
}
