//! Frozen CIXM5/CIXM6 mode 13 persistent adaptive coder.
//!
//! All three models observe every reconstructed block.  A mode 13 block is
//! encoded or decoded from a private working copy of the selected model; the
//! caller must then call [`PersistentAdaptiveStates::observe`] once, just as
//! it does for every other block mode.

use crate::rank::{ArithmeticDecoder, ArithmeticEncoder};

const STATE_LIMIT: u32 = 16_384;
const SPECS: [(usize, usize); 3] = [(0, 8), (1, 8), (2, 10)];
const COUNTS_BYTES: usize = 256 * std::mem::size_of::<u32>();
const TREE_BYTES: usize = 257 * std::mem::size_of::<u32>();

/// Conservative accounting unit for one private per-block arithmetic model.
pub const WORKING_BUCKET_BYTES: usize = COUNTS_BYTES + TREE_BYTES + 32;

/// Maximum retained state for all frozen model specifications, including a
/// conservative allowance for bucket tables and allocation metadata.
pub const RETAINED_MEMORY_BOUND: usize = (1 + 256 + 1024) * (COUNTS_BYTES + 32);

/// Maximum private decode state when every bucket in one model is touched.
pub const WORKING_MEMORY_BOUND: usize = 1024 * WORKING_BUCKET_BYTES + 16 * 1024;

/// Number of frozen persistent model choices, in canonical selection order.
pub const MODEL_COUNT: usize = SPECS.len();

#[derive(Clone)]
struct Model {
    counts: [u32; 256],
    tree: [u32; 257],
    seen: u32,
}

impl Model {
    fn from_counts(counts: &[u32; 256]) -> Result<Self, String> {
        let seen64: u64 = counts.iter().map(|&n| u64::from(n)).sum();
        if seen64 > u64::from((u32::MAX - 256) / 2) {
            return Err("persistent adaptive model total overflow".into());
        }
        let mut model = Self {
            counts: *counts,
            tree: [0; 257],
            seen: seen64 as u32,
        };
        for (symbol, &count) in counts.iter().enumerate() {
            if count != 0 {
                model.tree_add(symbol, count)?;
            }
        }
        Ok(model)
    }

    fn tree_add(&mut self, symbol: usize, count: u32) -> Result<(), String> {
        let mut i = symbol + 1;
        while i <= 256 {
            self.tree[i] = self.tree[i]
                .checked_add(count)
                .ok_or("persistent adaptive Fenwick overflow")?;
            i += i.isolate_lowest_one();
        }
        Ok(())
    }

    fn prefix(&self, symbol: usize) -> u32 {
        let mut i = symbol;
        let mut sum = 0u32;
        while i > 0 {
            sum += self.tree[i];
            i &= i - 1;
        }
        sum
    }

    fn total(&self) -> usize {
        2 * self.seen as usize + 256
    }

    fn interval(&self, symbol: usize) -> (usize, usize, usize) {
        let low = 2 * self.prefix(symbol) as usize + symbol;
        (
            low,
            low + 2 * self.counts[symbol] as usize + 1,
            self.total(),
        )
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
        let (low, high, total) = self.interval(lo);
        if !(low <= target && target < high) {
            return Err("persistent adaptive target outside symbol interval".into());
        }
        Ok((lo, low, high, total))
    }

    fn add(&mut self, symbol: usize) -> Result<(), String> {
        self.counts[symbol] = self.counts[symbol]
            .checked_add(1)
            .ok_or("persistent adaptive symbol count overflow")?;
        self.seen = self
            .seen
            .checked_add(1)
            .ok_or("persistent adaptive model total overflow")?;
        self.tree_add(symbol, 1)
    }
}

struct PersistentAdaptiveState {
    order: usize,
    bucket_bits: usize,
    buckets: Vec<Option<Box<[u32; 256]>>>,
}

impl PersistentAdaptiveState {
    fn new(order: usize, bucket_bits: usize) -> Self {
        let bucket_count = if order == 0 { 1 } else { 1usize << bucket_bits };
        Self {
            order,
            bucket_bits,
            buckets: std::iter::repeat_with(|| None).take(bucket_count).collect(),
        }
    }

    fn bucket(&self, tail: &[u8]) -> usize {
        if self.order == 0 {
            return 0;
        }
        let context = &tail[tail.len().saturating_sub(self.order)..];
        if self.order == 1 && self.bucket_bits >= 8 {
            return context.last().copied().unwrap_or(0) as usize;
        }
        let mut hash = 1_469_598_103_934_665_603u64;
        for &byte in context {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(1_099_511_628_211);
        }
        (hash & ((1u64 << self.bucket_bits) - 1)) as usize
    }

    fn initial_tail(&self, prefix_history: &[u8]) -> Vec<u8> {
        if self.order == 0 {
            Vec::new()
        } else {
            prefix_history[prefix_history.len().saturating_sub(self.order)..].to_vec()
        }
    }

    fn advance_tail(&self, tail: &mut Vec<u8>, symbol: u8) {
        if self.order == 0 {
            return;
        }
        tail.push(symbol);
        if tail.len() > self.order {
            tail.remove(0);
        }
    }

    fn working_bucket_limit(
        &self,
        source_length: usize,
        memory_budget: usize,
    ) -> Result<usize, String> {
        let table_bytes = self
            .buckets
            .len()
            .saturating_mul(std::mem::size_of::<Option<Box<Model>>>());
        if source_length != 0 && memory_budget < table_bytes + WORKING_BUCKET_BYTES {
            return Err("persistent adaptive working state exceeds --memory".into());
        }
        Ok(memory_budget
            .saturating_sub(table_bytes)
            .checked_div(WORKING_BUCKET_BYTES)
            .unwrap_or(0)
            .min(self.buckets.len()))
    }

    fn new_working_models(&self) -> Vec<Option<Box<Model>>> {
        std::iter::repeat_with(|| None)
            .take(self.buckets.len())
            .collect()
    }

    fn working_model<'a>(
        &'a self,
        working: &'a mut [Option<Box<Model>>],
        working_count: &mut usize,
        bucket: usize,
        bucket_limit: usize,
    ) -> Result<&'a mut Model, String> {
        if working[bucket].is_none() {
            if *working_count >= bucket_limit {
                return Err("persistent adaptive working state exceeds --memory".into());
            }
            let zero = [0u32; 256];
            let counts = self.buckets[bucket].as_deref().unwrap_or(&zero);
            working[bucket] = Some(Box::new(Model::from_counts(counts)?));
            *working_count += 1;
        }
        Ok(working[bucket].as_deref_mut().unwrap())
    }

    fn encode(
        &self,
        data: &[u8],
        prefix_history: &[u8],
        memory_budget: usize,
    ) -> Result<Vec<u8>, String> {
        let bucket_limit = self.working_bucket_limit(data.len(), memory_budget)?;
        let mut working = self.new_working_models();
        let mut working_count = 0usize;
        let mut tail = self.initial_tail(prefix_history);
        let mut encoder = ArithmeticEncoder::new();

        for &symbol in data {
            let bucket = self.bucket(&tail);
            let model =
                self.working_model(&mut working, &mut working_count, bucket, bucket_limit)?;
            let (low, high, total) = model.interval(symbol as usize);
            encoder.encode(low, high, total);
            model.add(symbol as usize)?;
            self.advance_tail(&mut tail, symbol);
        }

        let (coded, bit_length) = encoder.finish();
        let mut payload = Vec::with_capacity(varint_len(bit_length) + coded.len());
        write_varint(bit_length, &mut payload);
        payload.extend_from_slice(&coded);
        Ok(payload)
    }

    fn decode(
        &self,
        blob: &[u8],
        source_length: usize,
        prefix_history: &[u8],
        memory_budget: usize,
    ) -> Result<Vec<u8>, String> {
        let mut pos = 0usize;
        let bit_length = read_varint(blob, &mut pos)?;
        let byte_length = bit_length
            .checked_add(7)
            .ok_or("persistent adaptive bit length overflow")?
            / 8;
        if pos.checked_add(byte_length) != Some(blob.len()) {
            return Err("persistent adaptive payload length mismatch".into());
        }

        let bucket_limit = self.working_bucket_limit(source_length, memory_budget)?;
        let mut working = self.new_working_models();
        let mut working_count = 0usize;
        let mut tail = self.initial_tail(prefix_history);
        let mut decoder = ArithmeticDecoder::new(&blob[pos..], bit_length);
        let mut out = Vec::with_capacity(source_length);

        for _ in 0..source_length {
            let bucket = self.bucket(&tail);
            let model =
                self.working_model(&mut working, &mut working_count, bucket, bucket_limit)?;
            let target = decoder.target(model.total());
            let (symbol, low, high, total) = model.select(target)?;
            decoder.update(low, high, total);
            model.add(symbol)?;
            out.push(symbol as u8);
            self.advance_tail(&mut tail, symbol as u8);
        }
        Ok(out)
    }

    fn observe(&mut self, data: &[u8], prefix_history: &[u8]) -> Result<(), String> {
        let mut tail = self.initial_tail(prefix_history);
        let mut touched = vec![false; self.buckets.len()];
        for &symbol in data {
            let bucket = self.bucket(&tail);
            let counts = self.buckets[bucket].get_or_insert_with(|| Box::new([0; 256]));
            counts[symbol as usize] = counts[symbol as usize]
                .checked_add(1)
                .ok_or("persistent adaptive symbol count overflow")?;
            touched[bucket] = true;
            self.advance_tail(&mut tail, symbol);
        }
        for (bucket, was_touched) in touched.into_iter().enumerate() {
            if !was_touched {
                continue;
            }
            let counts = self.buckets[bucket].as_deref_mut().unwrap();
            let total: u64 = counts.iter().map(|&n| u64::from(n)).sum();
            if total > u64::from(STATE_LIMIT) {
                for count in counts.iter_mut() {
                    *count /= 2;
                }
            }
        }
        Ok(())
    }
}

/// Persistent model collection shared by an entire CIXM5/CIXM6 stream.
pub struct PersistentAdaptiveStates {
    states: Vec<PersistentAdaptiveState>,
}

impl Default for PersistentAdaptiveStates {
    fn default() -> Self {
        Self::new()
    }
}

impl PersistentAdaptiveStates {
    pub fn new() -> Self {
        Self {
            states: SPECS
                .iter()
                .map(|&(order, bucket_bits)| PersistentAdaptiveState::new(order, bucket_bits))
                .collect(),
        }
    }

    /// Encode a complete mode 13 payload, including its leading model ID.
    /// This does not commit the block: call [`Self::observe`] exactly once
    /// after the winning block representation has been chosen.
    pub fn encode(
        &self,
        model_id: usize,
        data: &[u8],
        prefix_history: &[u8],
        memory_budget: usize,
    ) -> Result<Vec<u8>, String> {
        let state = self
            .states
            .get(model_id)
            .ok_or("unknown persistent adaptive model ID")?;
        let coded = state.encode(data, prefix_history, memory_budget)?;
        let mut payload = Vec::with_capacity(1 + coded.len());
        payload.push(model_id as u8);
        payload.extend_from_slice(&coded);
        Ok(payload)
    }

    /// Encode all frozen model choices and return the shortest complete mode
    /// 13 payload. Equal lengths retain the lower model ID, matching Python's
    /// strict-improvement candidate loop.
    pub fn encode_best(
        &self,
        data: &[u8],
        prefix_history: &[u8],
        memory_budget: usize,
    ) -> Result<(usize, Vec<u8>), String> {
        let mut best: Option<(usize, Vec<u8>)> = None;
        for model_id in 0..MODEL_COUNT {
            let payload = self.encode(model_id, data, prefix_history, memory_budget)?;
            if best
                .as_ref()
                .is_none_or(|(_, current)| payload.len() < current.len())
            {
                best = Some((model_id, payload));
            }
        }
        best.ok_or_else(|| "no persistent adaptive models configured".into())
    }

    /// Decode a complete mode 13 payload, including its leading model ID.
    /// `memory_budget` covers the private working models for this block.
    pub fn decode(
        &self,
        payload: &[u8],
        source_length: usize,
        prefix_history: &[u8],
        memory_budget: usize,
    ) -> Result<Vec<u8>, String> {
        let (&model_id, blob) = payload
            .split_first()
            .ok_or("missing persistent adaptive model ID")?;
        let state = self
            .states
            .get(model_id as usize)
            .ok_or("unknown persistent adaptive model ID")?;
        state.decode(blob, source_length, prefix_history, memory_budget)
    }

    /// Commit one reconstructed block to every persistent model.
    pub fn observe(&mut self, data: &[u8], prefix_history: &[u8]) -> Result<(), String> {
        for state in &mut self.states {
            state.observe(data, prefix_history)?;
        }
        Ok(())
    }
}

fn varint_len(mut value: usize) -> usize {
    let mut len = 1usize;
    while value >= 128 {
        value >>= 7;
        len += 1;
    }
    len
}

fn write_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    let mut shift = 0usize;
    loop {
        let byte = *data
            .get(*pos)
            .ok_or("truncated persistent adaptive bit length")?;
        *pos += 1;
        let low = (byte & 0x7f) as usize;
        if low > (usize::MAX >> shift) {
            return Err("persistent adaptive bit length overflow".into());
        }
        let part = low
            .checked_shl(shift as u32)
            .ok_or("persistent adaptive bit length overflow")?;
        value = value
            .checked_add(part)
            .ok_or("persistent adaptive bit length overflow")?;
        if byte < 128 {
            return Ok(value);
        }
        shift = shift
            .checked_add(7)
            .ok_or("persistent adaptive bit length overflow")?;
        if shift >= usize::BITS as usize {
            return Err("persistent adaptive bit length overflow".into());
        }
    }
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
        assert_eq!(read_varint(&encoded, &mut 0).unwrap(), usize::MAX);
        let groups = (usize::BITS as usize).div_ceil(7);
        let mut malformed = vec![128; groups - 1];
        malformed.push(1u8 << ((usize::BITS as usize - 1) % 7 + 1));
        assert!(read_varint(&malformed, &mut 0).is_err());
    }

    #[test]
    fn persistent_models_restore_across_observed_blocks() {
        let first = b"persistent model state must survive a block boundary";
        let second = b"and decode with exactly the same accumulated state";
        let budget = WORKING_MEMORY_BOUND;
        let mut encoder = PersistentAdaptiveStates::new();
        let mut decoder = PersistentAdaptiveStates::new();

        let (_, first_payload) = encoder.encode_best(first, &[], budget).unwrap();
        assert_eq!(
            decoder
                .decode(&first_payload, first.len(), &[], budget)
                .unwrap(),
            first
        );
        encoder.observe(first, &[]).unwrap();
        decoder.observe(first, &[]).unwrap();

        let (_, second_payload) = encoder.encode_best(second, first, budget).unwrap();
        assert_eq!(
            decoder
                .decode(&second_payload, second.len(), first, budget)
                .unwrap(),
            second
        );
    }
}
