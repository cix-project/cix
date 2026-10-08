//! CIXG1 adaptive multinomial byte substream (order 0, bucket bits 8).
use crate::rank::{ArithmeticDecoder, ArithmeticEncoder};

struct Model {
    counts: [u32; 256],
    tree: [u32; 257],
    seen: u32,
}
impl Model {
    fn new() -> Self {
        Self {
            counts: [0; 256],
            tree: [0; 257],
            seen: 0,
        }
    }
    fn prefix(&self, symbol: usize) -> u32 {
        let mut i = symbol;
        let mut sum = 0;
        while i > 0 {
            sum += self.tree[i];
            i &= i - 1;
        }
        sum
    }
    fn interval(&self, symbol: usize) -> (usize, usize, usize) {
        let low = 2 * self.prefix(symbol) as usize + symbol;
        (
            low,
            low + 2 * self.counts[symbol] as usize + 1,
            2 * self.seen as usize + 256,
        )
    }
    fn add(&mut self, symbol: usize) {
        self.counts[symbol] += 1;
        self.seen += 1;
        let mut i = symbol + 1;
        while i <= 256 {
            self.tree[i] += 1;
            i += i.isolate_lowest_one();
        }
    }
    fn select(&self, target: usize) -> Result<(usize, usize, usize, usize), String> {
        let mut lo = 0usize;
        let mut hi = 255usize;
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if 2 * self.prefix(mid) as usize + mid <= target {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let (a, b, t) = self.interval(lo);
        if !(a <= target && target < b) {
            return Err("adaptive target outside symbol interval".into());
        }
        Ok((lo, a, b, t))
    }
}
fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}
fn get_varint(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..usize::BITS as usize).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated adaptive bit length")?;
        *pos += 1;
        let low = (byte & 127) as usize;
        if low > (usize::MAX >> shift) {
            return Err("adaptive varint overflow".into());
        }
        value |= low
            .checked_shl(shift as u32)
            .ok_or("adaptive varint overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized adaptive varint".into())
}

fn bucket(tail: &[u8], order: usize, bucket_bits: usize) -> usize {
    if order == 0 {
        0
    } else if order == 1 && bucket_bits >= 8 {
        tail.last().copied().unwrap_or(0) as usize
    } else {
        let mut hash = 1469598103934665603u64;
        for &byte in &tail[tail.len().saturating_sub(order)..] {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        (hash & ((1u64 << bucket_bits) - 1)) as usize
    }
}

fn validate_parameters(order: usize, bucket_bits: usize) -> Result<(), String> {
    if order > 16 || !(1..=16).contains(&bucket_bits) {
        return Err("invalid adaptive model parameters".into());
    }
    Ok(())
}

fn initial_tail(prefix_history: &[u8], order: usize) -> Vec<u8> {
    if order == 0 {
        Vec::new()
    } else {
        prefix_history[prefix_history.len().saturating_sub(order)..].to_vec()
    }
}

fn extend_tail(tail: &mut Vec<u8>, symbol: u8, order: usize) {
    if order != 0 {
        tail.push(symbol);
        if tail.len() > order {
            tail.remove(0);
        }
    }
}

fn model_for(
    models: &mut std::collections::HashMap<usize, Model>,
    bucket: usize,
    model_limit: usize,
) -> Result<&mut Model, String> {
    if !models.contains_key(&bucket) && models.len() >= model_limit {
        return Err("adaptive model count exceeds --memory budget".into());
    }
    Ok(models.entry(bucket).or_insert_with(Model::new))
}

fn payload_bounds(blob: &[u8]) -> Result<(usize, usize), String> {
    let mut pos = 3;
    let bits = get_varint(blob, &mut pos)?;
    let bytes = bits.div_ceil(8);
    if pos.checked_add(bytes) != Some(blob.len()) {
        return Err("adaptive payload length mismatch".into());
    }
    Ok((pos, bits))
}

/// Encode the frozen general CIX adaptive payload with bounded context storage.
pub fn encode_general(
    data: &[u8],
    order: usize,
    bucket_bits: usize,
    prefix_history: &[u8],
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    validate_parameters(order, bucket_bits)?;
    let mut tail = initial_tail(prefix_history, order);
    let mut models = std::collections::HashMap::<usize, Model>::new();
    let mut coder = ArithmeticEncoder::new();
    for (index, &symbol) in data.iter().enumerate() {
        if index & 15 == 0 {
            crate::limits::check()?;
        }
        let bucket = bucket(&tail, order, bucket_bits);
        let model = model_for(&mut models, bucket, model_limit)?;
        let (low, high, total) = model.interval(symbol as usize);
        coder.encode(low, high, total);
        model.add(symbol as usize);
        extend_tail(&mut tail, symbol, order);
    }
    let (payload, bits) = coder.finish();
    let mut out = vec![1, order as u8, bucket_bits as u8];
    put_varint(bits, &mut out);
    out.extend(payload);
    Ok(out)
}

pub fn encode(data: &[u8]) -> Vec<u8> {
    encode_general(data, 0, 8, &[], 1).expect("order-0 adaptive encoding is bounded")
}
pub fn decode(blob: &[u8], n: usize) -> Result<Vec<u8>, String> {
    if blob.len() < 4 || blob[0] != 1 || blob[1] != 0 || blob[2] != 8 {
        return Err("unsupported adaptive substream model".into());
    }
    let (pos, bits) = payload_bounds(blob)?;
    let mut model = Model::new();
    let mut coder = ArithmeticDecoder::new(&blob[pos..], bits);
    let mut out = Vec::with_capacity(n);
    for index in 0..n {
        if index & 15 == 0 {
            crate::limits::check()?;
        }
        let target = coder.target(2 * model.seen as usize + 256);
        let (symbol, low, high, total) = model.select(target)?;
        coder.update(low, high, total);
        model.add(symbol);
        out.push(symbol as u8);
    }
    Ok(out)
}

/// Decode the frozen general CIX adaptive block format, including context buckets.
pub fn decode_general(
    blob: &[u8],
    n: usize,
    prefix_history: &[u8],
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    if blob.len() < 4 || blob[0] != 1 {
        return Err("unknown adaptive multinomial version".into());
    }
    let order = blob[1] as usize;
    let bucket_bits = blob[2] as usize;
    validate_parameters(order, bucket_bits)?;
    let (pos, bits) = payload_bounds(blob)?;
    let mut tail = initial_tail(prefix_history, order);
    let mut models = std::collections::HashMap::<usize, Model>::new();
    let mut decoder = ArithmeticDecoder::new(&blob[pos..], bits);
    let mut out = Vec::with_capacity(n);
    for index in 0..n {
        if index & 15 == 0 {
            crate::limits::check()?;
        }
        let bucket = bucket(&tail, order, bucket_bits);
        let model = model_for(&mut models, bucket, model_limit)?;
        let target = decoder.target(2 * model.seen as usize + 256);
        let (symbol, low, high, total) = model.select(target)?;
        decoder.update(low, high, total);
        model.add(symbol);
        out.push(symbol as u8);
        extend_tail(&mut tail, symbol as u8, order);
    }
    Ok(out)
}

#[cfg(test)]
mod integer_overflow_tests {
    use super::*;
    #[test]
    fn varint_accepts_max_rejects_high_bits() {
        let mut value = usize::MAX;
        let mut encoded = Vec::new();
        while value >= 128 {
            encoded.push((value as u8 & 127) | 128);
            value >>= 7;
        }
        encoded.push(value as u8);
        assert_eq!(get_varint(&encoded, &mut 0).unwrap(), usize::MAX);
        let groups = (usize::BITS as usize).div_ceil(7);
        let mut malformed = vec![128; groups - 1];
        malformed.push(1u8 << ((usize::BITS as usize - 1) % 7 + 1));
        assert!(get_varint(&malformed, &mut 0).is_err());
    }
}
