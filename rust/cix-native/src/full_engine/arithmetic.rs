//! Exact bounded arithmetic primitives shared by native specialist ports.
//!
//! These functions retain the byte-level conventions of the extracted Python
//! reference closure while rejecting malformed inputs before they can overflow
//! a native integer or address an invalid Fenwick entry.
use num_bigint::BigUint;
use num_traits::{One, Zero};

const ARITH_MAX: u64 = u32::MAX as u64;
const ARITH_HALF: u64 = 0x8000_0000;
const ARITH_Q1: u64 = 0x4000_0000;
const ARITH_Q3: u64 = 0xc000_0000;

pub fn vencode(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

/// Decodes the canonical, unsigned LEB128 representation used by the native
/// closure.  Ten-byte values may use only the low bit of their last payload.
pub fn vdecode(data: &[u8], mut pos: usize) -> Result<(u64, usize), String> {
    let start = pos;
    let mut value = 0u64;
    for index in 0..10 {
        let byte = *data.get(pos).ok_or("truncated varint")?;
        pos += 1;
        let payload = byte & 0x7f;
        if index == 9 && (byte & 0x80 != 0 || payload > 1) {
            return Err("varint overflow".into());
        }
        value |= u64::from(payload) << (index * 7);
        if byte & 0x80 == 0 {
            if vencode(value).len() != pos - start {
                return Err("noncanonical varint".into());
            }
            return Ok((value, pos));
        }
    }
    Err("overlong varint".into())
}

pub fn choose(n: usize, k: usize) -> BigUint {
    if k > n {
        return BigUint::zero();
    }
    let k = k.min(n - k);
    let mut result = BigUint::one();
    for index in 0..k {
        result *= BigUint::from(n - index);
        result /= BigUint::from(index + 1);
    }
    result
}

pub fn colex_rank(values: &[usize]) -> Result<BigUint, String> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("invalid ordered subset".into());
    }
    Ok(values
        .iter()
        .enumerate()
        .fold(BigUint::zero(), |rank, (index, value)| {
            rank + choose(*value, index + 1)
        }))
}

pub fn colex_unrank(rank: &BigUint, count: usize, universe: usize) -> Result<Vec<usize>, String> {
    if count > universe || rank >= &choose(universe, count) {
        return Err("combination rank outside range".into());
    }
    let mut remaining_rank = rank.clone();
    let mut upper = universe;
    let mut values = Vec::with_capacity(count);
    for width in (1..=count).rev() {
        while upper > 0 && choose(upper - 1, width) > remaining_rank {
            upper -= 1;
        }
        if upper == 0 {
            return Err("invalid combination".into());
        }
        values.push(upper - 1);
        remaining_rank -= choose(upper - 1, width);
        upper -= 1;
    }
    values.reverse();
    Ok(values)
}

pub fn fixed_be(value: &BigUint, bits: usize) -> Result<Vec<u8>, String> {
    let bytes = bits.checked_add(7).ok_or("fixed width overflow")? / 8;
    if bytes == 0 {
        if value.is_zero() {
            return Ok(Vec::new());
        }
        return Err("fixed-width integer overflow".into());
    }
    let raw = value.to_bytes_be();
    if raw.len() > bytes {
        return Err("fixed-width integer overflow".into());
    }
    let mut out = vec![0; bytes - raw.len()];
    out.extend(raw);
    Ok(out)
}

pub fn read_fixed(data: &[u8], pos: usize, bits: usize) -> Result<(BigUint, usize), String> {
    let bytes = bits.checked_add(7).ok_or("fixed width overflow")? / 8;
    let end = pos.checked_add(bytes).ok_or("fixed width overflow")?;
    let slice = data.get(pos..end).ok_or("truncated fixed-width integer")?;
    Ok((BigUint::from_bytes_be(slice), end))
}

#[derive(Clone)]
pub struct Fenwick {
    tree: Vec<i128>,
    values: Vec<i128>,
    n: usize,
}

impl Fenwick {
    pub fn new(counts: &[usize]) -> Result<Self, String> {
        let capacity = counts.len().checked_add(1).ok_or("Fenwick size overflow")?;
        let mut fenwick = Self {
            tree: vec![0; capacity],
            values: vec![0; counts.len()],
            n: counts.len(),
        };
        for (index, count) in counts.iter().enumerate() {
            if *count != 0 {
                fenwick.add(
                    index,
                    i128::try_from(*count).map_err(|_| "Fenwick count overflow")?,
                )?;
            }
        }
        Ok(fenwick)
    }

    pub fn add(&mut self, index: usize, delta: i128) -> Result<(), String> {
        let mut tree_index = index.checked_add(1).ok_or("Fenwick index overflow")?;
        if tree_index > self.n {
            return Err("Fenwick index outside range".into());
        }
        let next_value = self.values[index]
            .checked_add(delta)
            .ok_or("Fenwick count overflow")?;
        if next_value < 0 {
            return Err("negative Fenwick count".into());
        }
        // Validate the complete update before mutating any tree node.
        let mut check_index = tree_index;
        while check_index <= self.n {
            let updated = self.tree[check_index]
                .checked_add(delta)
                .ok_or("Fenwick count overflow")?;
            if updated < 0 {
                return Err("negative Fenwick count".into());
            }
            let step = check_index.isolate_lowest_one();
            check_index = check_index
                .checked_add(step)
                .ok_or("Fenwick index overflow")?;
        }
        self.values[index] = next_value;
        while tree_index <= self.n {
            self.tree[tree_index] += delta;
            let step = tree_index.isolate_lowest_one();
            tree_index += step;
        }
        Ok(())
    }

    pub fn prefix(&self, mut end: usize) -> Result<i128, String> {
        if end > self.n {
            return Err("Fenwick prefix outside range".into());
        }
        let mut sum = 0i128;
        while end != 0 {
            sum = sum
                .checked_add(self.tree[end])
                .ok_or("Fenwick count overflow")?;
            end &= end - 1;
        }
        Ok(sum)
    }

    pub fn total(&self) -> Result<i128, String> {
        self.prefix(self.n)
    }

    pub fn select(&self, mut order: i128) -> Result<usize, String> {
        let total = self.total()?;
        if self.n == 0 || order < 0 || order >= total {
            return Err("Fenwick select outside range".into());
        }
        let mut index = 0usize;
        let mut bit = 1usize << (usize::BITS - self.n.leading_zeros() - 1);
        while bit != 0 {
            let next = index.checked_add(bit).ok_or("Fenwick index overflow")?;
            if next <= self.n && self.tree[next] <= order {
                order -= self.tree[next];
                index = next;
            }
            bit >>= 1;
        }
        if index >= self.n {
            Err("Fenwick select outside range".into())
        } else {
            Ok(index)
        }
    }
}

fn arithmetic_bounds(
    low: u64,
    high: u64,
    lo: u64,
    hi: u64,
    total: u64,
) -> Result<(u64, u64), String> {
    if low > high || high > ARITH_MAX || lo >= hi || hi > total || total == 0 {
        return Err("invalid arithmetic interval".into());
    }
    let range = high - low + 1;
    let next_high = u128::from(low) + u128::from(range) * u128::from(hi) / u128::from(total) - 1;
    let next_low = u128::from(low) + u128::from(range) * u128::from(lo) / u128::from(total);
    if next_low > next_high || next_high > u128::from(ARITH_MAX) {
        return Err("zero arithmetic interval".into());
    }
    Ok((next_low as u64, next_high as u64))
}

pub struct ArithmeticEncoder {
    low: u64,
    high: u64,
    pending: usize,
    bytes: Vec<u8>,
    current: u8,
    used: u8,
}

impl ArithmeticEncoder {
    pub fn new() -> Self {
        Self {
            low: 0,
            high: ARITH_MAX,
            pending: 0,
            bytes: Vec::new(),
            current: 0,
            used: 0,
        }
    }

    fn bit(&mut self, bit: u8) {
        self.current = (self.current << 1) | bit;
        self.used += 1;
        if self.used == 8 {
            self.bytes.push(self.current);
            self.current = 0;
            self.used = 0;
        }
    }

    fn emit(&mut self, bit: u8) {
        self.bit(bit);
        while self.pending != 0 {
            self.bit(bit ^ 1);
            self.pending -= 1;
        }
    }

    pub fn encode(&mut self, lo: u64, hi: u64, total: u64) -> Result<(), String> {
        (self.low, self.high) = arithmetic_bounds(self.low, self.high, lo, hi, total)?;
        loop {
            if self.high < ARITH_HALF {
                self.emit(0);
            } else if self.low >= ARITH_HALF {
                self.emit(1);
                self.low -= ARITH_HALF;
                self.high -= ARITH_HALF;
            } else if self.low >= ARITH_Q1 && self.high < ARITH_Q3 {
                self.pending = self
                    .pending
                    .checked_add(1)
                    .ok_or("arithmetic pending overflow")?;
                self.low -= ARITH_Q1;
                self.high -= ARITH_Q1;
            } else {
                break;
            }
            self.low <<= 1;
            self.high = (self.high << 1) + 1;
        }
        Ok(())
    }

    pub fn finish(mut self) -> (Vec<u8>, usize) {
        // The reference encoder's final two emitted bits cannot make `pending`
        // overflow in a finite stream; use saturation only for that terminal count.
        self.pending = self.pending.saturating_add(1);
        self.emit(if self.low < ARITH_Q1 { 0 } else { 1 });
        let bit_length = self.bytes.len() * 8 + usize::from(self.used);
        if self.used != 0 {
            self.bytes.push(self.current << (8 - self.used));
        }
        (self.bytes, bit_length)
    }
}

impl Default for ArithmeticEncoder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ArithmeticDecoder<'a> {
    data: &'a [u8],
    bits: usize,
    position: usize,
    low: u64,
    high: u64,
    value: u64,
}

impl<'a> ArithmeticDecoder<'a> {
    pub fn new(data: &'a [u8], bits: usize) -> Result<Self, String> {
        if bits
            > data
                .len()
                .checked_mul(8)
                .ok_or("arithmetic bit length overflow")?
        {
            return Err("truncated arithmetic stream".into());
        }
        let mut decoder = Self {
            data,
            bits,
            position: 0,
            low: 0,
            high: ARITH_MAX,
            value: 0,
        };
        for _ in 0..32 {
            decoder.value = (decoder.value << 1) | decoder.bit();
        }
        Ok(decoder)
    }

    fn bit(&mut self) -> u64 {
        if self.position >= self.bits {
            self.position = self.position.saturating_add(1);
            return 0;
        }
        let bit = (self.data[self.position / 8] >> (7 - self.position % 8)) & 1;
        self.position += 1;
        u64::from(bit)
    }

    pub fn target(&self, total: u64) -> Result<u64, String> {
        if total == 0 || self.low > self.high || self.value < self.low || self.value > self.high {
            return Err("invalid arithmetic state".into());
        }
        let range = self.high - self.low + 1;
        let numerator = u128::from(self.value - self.low + 1) * u128::from(total) - 1;
        let target = numerator / u128::from(range);
        u64::try_from(target).map_err(|_| "arithmetic target overflow".into())
    }

    pub fn update(&mut self, lo: u64, hi: u64, total: u64) -> Result<(), String> {
        let (next_low, next_high) = arithmetic_bounds(self.low, self.high, lo, hi, total)?;
        if self.value < next_low || self.value > next_high {
            return Err("arithmetic symbol does not contain decoder state".into());
        }
        self.low = next_low;
        self.high = next_high;
        loop {
            if self.high < ARITH_HALF {
                // No offset in the lower half.
            } else if self.low >= ARITH_HALF {
                self.low -= ARITH_HALF;
                self.high -= ARITH_HALF;
                self.value = self
                    .value
                    .checked_sub(ARITH_HALF)
                    .ok_or("invalid arithmetic state")?;
            } else if self.low >= ARITH_Q1 && self.high < ARITH_Q3 {
                self.low -= ARITH_Q1;
                self.high -= ARITH_Q1;
                self.value = self
                    .value
                    .checked_sub(ARITH_Q1)
                    .ok_or("invalid arithmetic state")?;
            } else {
                break;
            }
            self.low <<= 1;
            self.high = (self.high << 1) + 1;
            self.value = (self.value << 1) | self.bit();
        }
        Ok(())
    }
}

fn validate_subset(values: &[usize], universe: usize) -> Result<(), String> {
    if universe > (1 << 30)
        || values.len() > universe
        || values.iter().any(|value| *value >= universe)
    {
        return Err("invalid subset dimensions".into());
    }
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("invalid ordered subset".into());
    }
    Ok(())
}

pub fn ordered_subset_encode_with_method(
    values: &[usize],
    universe: usize,
    method: u8,
) -> Result<Vec<u8>, String> {
    validate_subset(values, universe)?;
    match method {
        0 => {
            let rank = colex_rank(values)?;
            let possibilities = choose(universe, values.len());
            fixed_be(&rank, (&possibilities - BigUint::one()).bits() as usize)
        }
        1 => {
            let mut coder = ArithmeticEncoder::new();
            let mut minimum = 0usize;
            for (index, value) in values.iter().enumerate() {
                arithmetic_subset_position(
                    Some(*value),
                    universe,
                    values.len() - index,
                    minimum,
                    &mut coder,
                )?;
                minimum = value.checked_add(1).ok_or("subset index overflow")?;
            }
            let (payload, bits) = coder.finish();
            let mut out = vencode(u64::try_from(bits).map_err(|_| "subset bit length overflow")?);
            out.extend(payload);
            Ok(out)
        }
        _ => Err("invalid subset method".into()),
    }
}

pub fn ordered_subset_encode(values: &[usize], universe: usize) -> Result<Vec<u8>, String> {
    ordered_subset_encode_with_method(values, universe, 0)
}

fn arithmetic_subset_position(
    value: Option<usize>,
    universe: usize,
    remaining: usize,
    minimum: usize,
    coder: &mut ArithmeticEncoder,
) -> Result<usize, String> {
    let mut low = minimum;
    let mut high = universe
        .checked_sub(remaining)
        .ok_or("invalid subset dimensions")?;
    while low < high {
        let mid = low + (high - low) / 2;
        let left = choose(universe - low, remaining) - choose(universe - mid - 1, remaining);
        let right = choose(universe - mid - 1, remaining) - choose(universe - high - 1, remaining);
        let total = &left + &right;
        if total.is_zero() {
            return Err("invalid subset arithmetic state".into());
        }
        let scale = BigUint::from(1u64 << 20);
        let rounded = (&left * &scale + (&total >> 1usize)) / &total;
        let frequency = rounded
            .to_u64_digits()
            .first()
            .copied()
            .unwrap_or(0)
            .clamp(1, (1 << 20) - 1);
        let take_left = value.map_or_else(|| false, |selected| selected <= mid);
        let (start, end) = if take_left {
            (0, frequency)
        } else {
            (frequency, 1 << 20)
        };
        coder.encode(start, end, 1 << 20)?;
        if take_left {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    if let Some(selected) = value {
        if selected != low {
            return Err("invalid ordered subset".into());
        }
    }
    Ok(low)
}

fn arithmetic_subset_decode_position(
    universe: usize,
    remaining: usize,
    minimum: usize,
    coder: &mut ArithmeticDecoder<'_>,
) -> Result<usize, String> {
    let mut low = minimum;
    let mut high = universe
        .checked_sub(remaining)
        .ok_or("invalid subset dimensions")?;
    while low < high {
        let mid = low + (high - low) / 2;
        let left = choose(universe - low, remaining) - choose(universe - mid - 1, remaining);
        let right = choose(universe - mid - 1, remaining) - choose(universe - high - 1, remaining);
        let total = &left + &right;
        if total.is_zero() {
            return Err("invalid subset arithmetic state".into());
        }
        let scale = BigUint::from(1u64 << 20);
        let rounded = (&left * &scale + (&total >> 1usize)) / &total;
        let frequency = rounded
            .to_u64_digits()
            .first()
            .copied()
            .unwrap_or(0)
            .clamp(1, (1 << 20) - 1);
        let take_left = coder.target(1 << 20)? < frequency;
        let (start, end) = if take_left {
            (0, frequency)
        } else {
            (frequency, 1 << 20)
        };
        coder.update(start, end, 1 << 20)?;
        if take_left {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    Ok(low)
}

pub fn ordered_subset_decode_with_method(
    blob: &[u8],
    pos: usize,
    universe: usize,
    count: usize,
    method: u8,
) -> Result<(Vec<usize>, usize), String> {
    if universe > (1 << 30) || count > universe {
        return Err("invalid subset dimensions".into());
    }
    match method {
        0 => {
            let possibilities = choose(universe, count);
            let width = (&possibilities - BigUint::one()).bits() as usize;
            let (rank, end) = read_fixed(blob, pos, width)?;
            if rank >= possibilities {
                return Err("subset rank outside class".into());
            }
            Ok((colex_unrank(&rank, count, universe)?, end))
        }
        1 => {
            let (bits, payload_pos) = vdecode(blob, pos)?;
            let bits = usize::try_from(bits).map_err(|_| "subset bit length overflow")?;
            let size = bits.checked_add(7).ok_or("subset bit length overflow")? / 8;
            let end = payload_pos
                .checked_add(size)
                .ok_or("subset payload overflow")?;
            let payload = blob
                .get(payload_pos..end)
                .ok_or("truncated subset arithmetic")?;
            let mut coder = ArithmeticDecoder::new(payload, bits)?;
            let mut values = Vec::with_capacity(count);
            let mut minimum = 0usize;
            for index in 0..count {
                let selected = arithmetic_subset_decode_position(
                    universe,
                    count - index,
                    minimum,
                    &mut coder,
                )?;
                minimum = selected.checked_add(1).ok_or("subset index overflow")?;
                values.push(selected);
            }
            Ok((values, end))
        }
        _ => Err("invalid subset method".into()),
    }
}

pub fn ordered_subset_decode(
    blob: &[u8],
    pos: usize,
    universe: usize,
    count: usize,
) -> Result<(Vec<usize>, usize), String> {
    ordered_subset_decode_with_method(blob, pos, universe, count, 0)
}

pub fn residual_encode(value: u64, previous: u64, bits: u32) -> Result<u64, String> {
    if bits == 0 || bits > 64 {
        return Err("invalid residual width".into());
    }
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let difference = value.wrapping_sub(previous) & mask;
    let negative = difference & (1u64 << (bits - 1)) != 0;
    if negative {
        let magnitude = difference.wrapping_neg() & mask;
        Ok(magnitude.wrapping_mul(2).wrapping_sub(1))
    } else {
        Ok(difference.wrapping_mul(2))
    }
}

pub fn residual_decode(value: u64, previous: u64, bits: u32) -> Result<u64, String> {
    if bits == 0 || bits > 64 {
        return Err("invalid residual width".into());
    }
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let difference = if value & 1 == 0 {
        value / 2
    } else {
        (value / 2 + 1).wrapping_neg()
    };
    Ok(previous.wrapping_add(difference) & mask)
}

pub fn checked_frame(magic: &[u8], mode: u8, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(magic.len() + 16);
    out.extend_from_slice(magic);
    out.push(mode);
    out.extend(vencode(data.len() as u64));
    out.extend(crc32fast::hash(data).to_be_bytes());
    out
}

pub fn parse_checked_frame(
    frame: &[u8],
    magic: &[u8],
    modes: &[u8],
    maximum: usize,
) -> Result<(u8, usize, u32, usize), String> {
    if !frame.starts_with(magic) {
        return Err("invalid legacy frame".into());
    }
    let mode = *frame.get(magic.len()).ok_or("invalid legacy frame")?;
    if !modes.contains(&mode) {
        return Err("invalid legacy mode".into());
    }
    let (size, pos) = vdecode(
        frame,
        magic.len().checked_add(1).ok_or("legacy frame overflow")?,
    )?;
    let size = usize::try_from(size).map_err(|_| "output size overflow")?;
    if size > maximum {
        return Err("legacy output exceeds limit".into());
    }
    let end = pos.checked_add(4).ok_or("truncated legacy checksum")?;
    let checksum = frame.get(pos..end).ok_or("truncated legacy checksum")?;
    let checksum = u32::from_be_bytes([checksum[0], checksum[1], checksum[2], checksum[3]]);
    Ok((mode, size, checksum, end))
}

pub fn verify_frame(data: &[u8], size: usize, checksum: u32) -> Result<(), String> {
    if data.len() != size || crc32fast::hash(data) != checksum {
        return Err("legacy source checksum mismatch".into());
    }
    Ok(())
}

pub fn multinomial_cardinality(counts: &[usize]) -> Result<BigUint, String> {
    let mut remaining = counts.iter().try_fold(0usize, |total, count| {
        total.checked_add(*count).ok_or("count overflow")
    })?;
    let mut total = BigUint::one();
    for count in counts {
        if *count != 0 {
            total *= choose(remaining, *count);
            remaining -= *count;
        }
    }
    Ok(total)
}

pub fn composition_rank(parts: &[usize]) -> Result<BigUint, String> {
    if parts.is_empty() || parts.contains(&0) {
        return Err("composition parts must be positive".into());
    }
    if parts.len() == 1 {
        return Ok(BigUint::zero());
    }
    let mut total = 0usize;
    let mut cuts = Vec::with_capacity(parts.len() - 1);
    for part in &parts[..parts.len() - 1] {
        total = total.checked_add(*part).ok_or("composition overflow")?;
        cuts.push(total - 1);
    }
    colex_rank(&cuts)
}

pub fn composition_unrank(
    rank: &BigUint,
    total: usize,
    count: usize,
) -> Result<Vec<usize>, String> {
    if count == 0 {
        return if total == 0 {
            Ok(Vec::new())
        } else {
            Err("invalid composition".into())
        };
    }
    if total == 0 {
        return Err("invalid composition".into());
    }
    if count == 1 {
        return if rank.is_zero() {
            Ok(vec![total])
        } else {
            Err("composition rank outside range".into())
        };
    }
    let cuts = colex_unrank(
        rank,
        count - 1,
        total.checked_sub(1).ok_or("invalid composition")?,
    )?;
    let mut previous = 0usize;
    let mut parts = Vec::with_capacity(count);
    for cut in cuts {
        let next = cut.checked_add(1).ok_or("composition overflow")?;
        parts.push(next.checked_sub(previous).ok_or("invalid composition")?);
        previous = next;
    }
    parts.push(total.checked_sub(previous).ok_or("invalid composition")?);
    if parts.contains(&0) {
        return Err("invalid composition".into());
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_reference_varint_and_frame_vectors() {
        assert_eq!(vencode(0), vec![0]);
        assert_eq!(vencode(127), vec![127]);
        assert_eq!(vencode(128), vec![128, 1]);
        assert_eq!(vencode(300), vec![172, 2]);
        assert_eq!(
            vencode(u64::MAX),
            vec![255, 255, 255, 255, 255, 255, 255, 255, 255, 1]
        );
        assert!(vdecode(&[128, 0], 0).is_err());
        assert!(vdecode(&[255, 255, 255, 255, 255, 255, 255, 255, 255, 2], 0).is_err());
        let header = checked_frame(b"TEST", 2, b"abc");
        assert_eq!(header, b"TEST\x02\x03\x35\x24\x41\xc2");
        let (mode, size, checksum, end) = parse_checked_frame(&header, b"TEST", &[2], 3).unwrap();
        assert_eq!(
            (mode, size, checksum, end),
            (2, 3, 0x3524_41c2, header.len())
        );
        verify_frame(b"abc", size, checksum).unwrap();
    }

    #[test]
    fn exhaustive_small_combinatorics_and_compositions() {
        for universe in 0..=8 {
            for count in 0..=universe {
                let possibilities = choose(universe, count);
                let mut rank = BigUint::zero();
                while rank < possibilities {
                    let values = colex_unrank(&rank, count, universe).unwrap();
                    assert_eq!(colex_rank(&values).unwrap(), rank);
                    let blob = ordered_subset_encode(&values, universe).unwrap();
                    assert_eq!(
                        ordered_subset_decode(&blob, 0, universe, count).unwrap().0,
                        values
                    );
                    rank += BigUint::one();
                }
            }
        }
        assert_eq!(composition_unrank(&BigUint::zero(), 5, 1).unwrap(), vec![5]);
        assert!(composition_unrank(&BigUint::one(), 5, 1).is_err());
        assert_eq!(composition_rank(&[2, 3, 1]).unwrap(), BigUint::from(7u8));
    }

    #[test]
    fn ordered_subset_method_one_and_residual_vectors() {
        let values = [1, 3, 7];
        // Python ordered_subset.encode([1, 3, 7], 8, method=0) is rank 39 in one byte.
        assert_eq!(ordered_subset_encode(&values, 8).unwrap(), vec![39]);
        let method_one = ordered_subset_encode_with_method(&values, 8, 1).unwrap();
        assert_eq!(
            ordered_subset_decode_with_method(&method_one, 0, 8, 3, 1)
                .unwrap()
                .0,
            values
        );
        assert_eq!(residual_encode(5, 2, 64).unwrap(), 6);
        assert_eq!(residual_encode(1, 5, 64).unwrap(), 7);
        assert_eq!(residual_decode(7, 5, 64).unwrap(), 1);
        assert_eq!(residual_encode(0, 255, 8).unwrap(), 2);
    }

    #[test]
    fn arithmetic_and_fenwick_reject_invalid_state() {
        let mut encoder = ArithmeticEncoder::new();
        assert!(encoder.encode(1, 1, 2).is_err());
        encoder.encode(0, 1, 3).unwrap();
        encoder.encode(1, 3, 3).unwrap();
        let (payload, bits) = encoder.finish();
        let mut decoder = ArithmeticDecoder::new(&payload, bits).unwrap();
        assert_eq!(decoder.target(3).unwrap(), 0);
        decoder.update(0, 1, 3).unwrap();
        assert!(decoder.target(3).unwrap() >= 1);
        let mut tree = Fenwick::new(&[2, 1, 0]).unwrap();
        assert_eq!(tree.prefix(2).unwrap(), 3);
        assert_eq!(tree.select(2).unwrap(), 1);
        assert!(tree.add(3, 1).is_err());
        assert!(tree.select(3).is_err());
    }
}
