//! Deterministic fixed-point three-family bit mixer for CIXG1 route 9.
use crate::{
    rank::{ArithmeticDecoder, ArithmeticEncoder},
    zpaq_context::{Config as RecurrenceConfig, Predictor as RecurrencePredictor},
};
use std::{collections::HashMap, sync::OnceLock};

const SCALE: i64 = 4096;
const LOG_SCALE: i64 = 4096;
const WEIGHT_SCALE: i64 = 1024;
const CLIP_LOG: i64 = 12 * LOG_SCALE;
const COUNT_LIMIT: i32 = 2048;
const HISTORY_MAX: usize = 65536;
const ETA: [u8; 3] = [5, 6, 7];
const PATTERNS: &[&[usize]] = &[
    &[],
    &[1],
    &[1, 2],
    &[1, 2, 3],
    &[1, 2, 3, 4],
    &[1, 3],
    &[1, 4],
    &[1, 5],
    &[2, 3],
    &[2, 4],
    &[3, 6],
    &[4, 8],
    &[1, 4, 8],
];
const LAGS: &[usize] = &[1, 2, 4, 8, 16, 32];
const PATTERN_EXPERTS: usize = 13;
const LAG_EXPERTS: usize = 6;
const EXPERTS: usize = PATTERN_EXPERTS + LAG_EXPERTS + 1;

fn floor_div(a: i64, b: i64) -> i64 {
    let q = a / b;
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}
fn fixed_log2(value: u32) -> i32 {
    assert!(value > 0);
    let integer = 31 - value.leading_zeros();
    let one = 1u128 << 32;
    let mut y = ((value as u128) << 32) >> integer;
    let mut fraction = 0u32;
    for i in 0..12 {
        y = (y * y) >> 32;
        if y >= 2 * one {
            y >>= 1;
            fraction |= 1 << (11 - i);
        }
    }
    (integer * 4096 + fraction) as i32
}
/// The stretch values are a pure function of fixed constants.  Build them once
/// per process rather than rebuilding and allocating the same 16 KiB table for
/// every encoded or decoded block.  The model state deliberately remains local
/// to each stream; only this immutable lookup table is shared.
static STRETCH_TABLE: OnceLock<[i32; 4096]> = OnceLock::new();

fn stretch_table() -> &'static [i32; 4096] {
    STRETCH_TABLE.get_or_init(|| {
        let mut table = [0; 4096];
        for (p, entry) in table.iter_mut().enumerate().skip(1) {
            *entry = fixed_log2(p as u32) - fixed_log2((4096 - p) as u32);
        }
        table
    })
}
fn squash(v: i64, t: &[i32]) -> i32 {
    if v <= t[1] as i64 {
        return 1;
    }
    if v >= t[4095] as i64 {
        return 4095;
    }
    let mut lo = 1usize;
    let mut hi = 4095;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if t[mid] as i64 >= v {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    if (t[lo] as i64 - v).abs() < (v - t[lo - 1] as i64).abs() {
        lo as i32
    } else {
        (lo - 1) as i32
    }
}
type CtxKey = u64;
type RecurrencePrediction = (i32, usize, u8, i32);
type ProbabilityDetails = (
    i32,
    [i32; EXPERTS],
    [Option<CtxKey>; PATTERN_EXPERTS],
    Option<RecurrencePrediction>,
);

/// Last `HISTORY_MAX` bytes in logical insertion order.  Unlike the legacy
/// Vec/remove(0) implementation, eviction is O(1); `get` preserves every
/// index relationship used by version-1 and version-2 prediction paths.
struct History {
    bytes: Vec<u8>,
    start: usize,
}
impl History {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(HISTORY_MAX),
            start: 0,
        }
    }
    fn len(&self) -> usize {
        self.bytes.len()
    }
    fn get(&self, logical: usize) -> u8 {
        debug_assert!(logical < self.bytes.len());
        self.bytes[(self.start + logical) % self.bytes.len()]
    }
    fn last(&self) -> Option<u8> {
        self.len().checked_sub(1).map(|index| self.get(index))
    }
    fn push(&mut self, byte: u8) {
        if self.bytes.len() < HISTORY_MAX {
            self.bytes.push(byte);
        } else {
            self.bytes[self.start] = byte;
            self.start = (self.start + 1) % HISTORY_MAX;
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RecurrenceMode {
    LegacyV2,
    CorrectV3,
}

struct State {
    history: History,
    tables: Vec<HashMap<CtxKey, [i32; 2]>>,
    weights: Vec<[i32; EXPERTS]>,
    bias: [i32; 8],
    eta: u8,
    lag_rel: Vec<[[i32; 2]; 8]>,
    match_rel: [[i32; 2]; 8],
    matches: HashMap<[u8; 4], u8>,
    // Version 2 only.  The feature extractor itself is fixed-size; the
    // reliability table is bounded by the serialized bucket count.
    recurrence: Option<RecurrencePredictor>,
    recurrence_rel: Vec<[[i32; 2]; 8]>,
    recurrence_weights: [i32; 8],
    recurrence_mode: Option<RecurrenceMode>,
    // Correct v3 semantics use the feature returned while observing the
    // previous byte, not a lookup after it has been inserted.
    previous_recurrence_feature: Option<crate::zpaq_context::Feature>,
}
impl State {
    fn new(eta: u8) -> Self {
        let initial = (WEIGHT_SCALE as i32 / EXPERTS as i32).max(1);
        Self {
            history: History::new(),
            tables: (0..PATTERNS.len()).map(|_| HashMap::new()).collect(),
            weights: vec![[initial; EXPERTS]; 8],
            bias: [0; 8],
            eta,
            lag_rel: vec![[[1, 1]; 8]; LAGS.len()],
            match_rel: [[1, 1]; 8],
            matches: HashMap::new(),
            recurrence: None,
            recurrence_rel: Vec::new(),
            recurrence_weights: [0; 8],
            recurrence_mode: None,
            previous_recurrence_feature: None,
        }
    }
    fn with_recurrence(
        eta: u8,
        config: RecurrenceConfig,
        mode: RecurrenceMode,
    ) -> Result<Self, String> {
        let mut state = Self::new(eta);
        let buckets = 1usize << config.bucket_bits;
        state.recurrence = Some(match mode {
            RecurrenceMode::LegacyV2 => RecurrencePredictor::new(config)?,
            RecurrenceMode::CorrectV3 => RecurrencePredictor::new_saturated(config)?,
        });
        state.recurrence_rel = vec![[[1, 1]; 8]; buckets];
        state.recurrence_weights = [WEIGHT_SCALE as i32 / EXPERTS as i32; 8];
        state.recurrence_mode = Some(mode);
        Ok(state)
    }
    fn context(&self, p: &[usize], bit: usize, prefix: u8) -> Option<CtxKey> {
        if p.iter().any(|&o| self.history.len() < o) {
            return None;
        }
        let mut key = 0u64;
        for &o in p {
            key = (key << 8) | self.history.get(self.history.len() - o) as u64;
        }
        key = (key << 16) | ((bit as u64) << 8) | prefix as u64;
        Some(key)
    }
    fn context_prob(counts: Option<&[i32; 2]>) -> i32 {
        if let Some(c) = counts {
            (((2 * c[1] + 1) * 4096) / (2 * (c[0] + c[1] + 1))).clamp(1, 4095)
        } else {
            2048
        }
    }
    fn rel_prob(pred: Option<u8>, rel: &[i32; 2], bit: usize) -> i32 {
        if let Some(p) = pred {
            let conf = (((2 * rel[0] + 1) * 4096) / (2 * (rel[0] + rel[1] + 1))).clamp(2048, 4095);
            if ((p >> (7 - bit)) & 1) != 0 {
                conf
            } else {
                4096 - conf
            }
        } else {
            2048
        }
    }
    fn predictions(&self) -> ([Option<u8>; LAG_EXPERTS], Option<[u8; 4]>) {
        let lags = std::array::from_fn(|i| {
            let n = LAGS[i];
            if self.history.len() >= n {
                Some(self.history.get(self.history.len() - n))
            } else {
                None
            }
        });
        let key = if self.history.len() >= 4 {
            Some(std::array::from_fn(|i| {
                self.history.get(self.history.len() - 4 + i)
            }))
        } else {
            None
        };
        (lags, key)
    }
    /// Predict the next bit from the most recently reconstructed byte and the
    /// causal recurrence-distance feature of that byte.  The feature is
    /// derived before the current symbol is observed, so both encoder and
    /// decoder have identical state.  This deliberately does not attempt the
    /// full ZPAQ periodic/MATCH/ISSE/SSE component graph.
    fn recurrence_prob(&self, bit: usize, t: &[i32]) -> Option<RecurrencePrediction> {
        let predictor = self.recurrence.as_ref()?;
        let previous = self.history.last()?;
        let feature = match self.recurrence_mode? {
            // Preserve the v2 decoder's historical lookup-after-observe
            // behavior, even though it collapses every feature to distance 1.
            RecurrenceMode::LegacyV2 => predictor.feature(previous),
            RecurrenceMode::CorrectV3 => self.previous_recurrence_feature?,
        };
        if !feature.repeated {
            return None;
        }
        let bucket = feature.distance_bucket as usize;
        let rel = self.recurrence_rel.get(bucket)?[bit];
        let confidence =
            (((2 * rel[0] + 1) * 4096) / (2 * (rel[0] + rel[1] + 1))).clamp(2048, 4095);
        let predicted = (previous >> (7 - bit)) & 1;
        let probability = if predicted != 0 {
            confidence
        } else {
            4096 - confidence
        };
        Some((probability, bucket, predicted, t[probability as usize]))
    }
    fn probabilities(&self, bit: usize, prefix: u8, t: &[i32]) -> ProbabilityDetails {
        let mut probabilities = [0; EXPERTS];
        let mut keys = [None; PATTERN_EXPERTS];
        for (i, p) in PATTERNS.iter().enumerate() {
            let k = self.context(p, bit, prefix);
            let cnt = k.as_ref().and_then(|x| self.tables[i].get(x));
            probabilities[i] = Self::context_prob(cnt);
            keys[i] = k;
        }
        let (lags, match_key) = self.predictions();
        for (i, p) in lags.iter().enumerate() {
            probabilities[PATTERN_EXPERTS + i] = Self::rel_prob(*p, &self.lag_rel[i][bit], bit);
        }
        let mp = match_key.and_then(|k| self.matches.get(&k).copied());
        probabilities[EXPERTS - 1] = Self::rel_prob(mp, &self.match_rel[bit], bit);
        let stretches = std::array::from_fn(|i| t[probabilities[i] as usize]);
        let sum: i64 = self.weights[bit]
            .iter()
            .zip(&stretches)
            .map(|(&w, &s)| w as i64 * s as i64)
            .sum();
        let recurrence = self.recurrence_prob(bit, t);
        let recurrence_sum = recurrence
            .map(|(_, _, _, stretch)| self.recurrence_weights[bit] as i64 * stretch as i64)
            .unwrap_or(0);
        let mixed = self.bias[bit] as i64 + floor_div(sum + recurrence_sum, WEIGHT_SCALE);
        (squash(mixed, t), stretches, keys, recurrence)
    }
    fn update(
        &mut self,
        bit: usize,
        actual: u8,
        p: i32,
        stretches: &[i32],
        keys: &[Option<CtxKey>],
        recurrence: Option<(i32, usize, u8, i32)>,
    ) {
        let error = if actual != 0 {
            SCALE - p as i64
        } else {
            -(p as i64)
        };
        let denom = SCALE * LOG_SCALE * (1i64 << self.eta);
        self.update_bias_and_weights(bit, error, stretches, denom);
        self.update_recurrence(bit, actual, error, recurrence, denom);
        self.update_pattern_counts(actual, keys);
        self.update_lag_relations(bit, actual);
        self.update_match_relation(bit, actual);
    }

    fn update_bias_and_weights(&mut self, bit: usize, error: i64, stretches: &[i32], denom: i64) {
        let bd = floor_div(error * LOG_SCALE, SCALE * (1i64 << self.eta));
        self.bias[bit] = (self.bias[bit] as i64 + bd).clamp(-CLIP_LOG, CLIP_LOG) as i32;
        for (i, &s) in stretches.iter().enumerate() {
            let d = floor_div(error * s as i64 * WEIGHT_SCALE, denom);
            self.weights[bit][i] =
                (self.weights[bit][i] as i64 + d).clamp(-4 * WEIGHT_SCALE, 4 * WEIGHT_SCALE) as i32;
        }
    }

    fn update_recurrence(
        &mut self,
        bit: usize,
        actual: u8,
        error: i64,
        recurrence: Option<(i32, usize, u8, i32)>,
        denom: i64,
    ) {
        if let Some((_probability, bucket, predicted, stretch)) = recurrence {
            let d = floor_div(error * stretch as i64 * WEIGHT_SCALE, denom);
            self.recurrence_weights[bit] = (self.recurrence_weights[bit] as i64 + d)
                .clamp(-4 * WEIGHT_SCALE, 4 * WEIGHT_SCALE)
                as i32;
            observe_relation(&mut self.recurrence_rel[bucket][bit], predicted, actual);
        }
    }

    fn update_pattern_counts(&mut self, actual: u8, keys: &[Option<CtxKey>]) {
        for (i, key) in keys.iter().enumerate() {
            if let Some(k) = key {
                let c = self.tables[i].entry(*k).or_insert([0, 0]);
                c[actual as usize] += 1;
                if c[0] + c[1] > COUNT_LIMIT {
                    c[0] = (c[0] + 1) / 2;
                    c[1] = (c[1] + 1) / 2;
                }
            }
        }
    }

    fn update_lag_relations(&mut self, bit: usize, actual: u8) {
        for (i, &lag) in LAGS.iter().enumerate() {
            if self.history.len() >= lag {
                let pred = (self.history.get(self.history.len() - lag) >> (7 - bit)) & 1;
                observe_relation(&mut self.lag_rel[i][bit], pred, actual);
            }
        }
    }

    fn update_match_relation(&mut self, bit: usize, actual: u8) {
        if self.history.len() >= 4 {
            let k: [u8; 4] = std::array::from_fn(|i| self.history.get(self.history.len() - 4 + i));
            if let Some(&v) = self.matches.get(&k) {
                let pred = (v >> (7 - bit)) & 1;
                observe_relation(&mut self.match_rel[bit], pred, actual);
            }
        }
    }
    fn finish_byte(&mut self, b: u8) {
        if self.history.len() >= 4 {
            let k: [u8; 4] = std::array::from_fn(|i| self.history.get(self.history.len() - 4 + i));
            self.matches.insert(k, b);
        }
        self.history.push(b);
        if let Some(predictor) = &mut self.recurrence {
            let feature = predictor.observe(b);
            if self.recurrence_mode == Some(RecurrenceMode::CorrectV3) {
                self.previous_recurrence_feature = Some(feature);
            }
        }
    }
}

fn observe_relation(relation: &mut [i32; 2], predicted: u8, actual: u8) {
    relation[if predicted == actual { 0 } else { 1 }] += 1;
    if relation[0] + relation[1] > COUNT_LIMIT {
        relation[0] = (relation[0] + 1) / 2;
        relation[1] = (relation[1] + 1) / 2;
    }
}
fn observe_byte(state: &mut State, byte: u8, t: &[i32]) {
    let mut prefix = 0;
    for bit in 0..8 {
        let actual = (byte >> (7 - bit)) & 1;
        let (probability, stretches, keys, recurrence) = state.probabilities(bit, prefix, t);
        state.update(bit, actual, probability, &stretches, &keys, recurrence);
        prefix = (prefix << 1) | actual;
    }
    state.finish_byte(byte);
}
fn prime_unchecked(prefix_history: &[u8], block_length: usize, eta: u8, t: &[i32]) -> State {
    let mut state = State::new(eta);
    let training = prefix_history
        .len()
        .min(HISTORY_MAX)
        .min(4096usize.max(block_length.saturating_mul(2)));
    for &byte in &prefix_history[prefix_history.len() - training..] {
        observe_byte(&mut state, byte, t);
    }
    state
}
fn prime_checked(
    prefix_history: &[u8],
    block_length: usize,
    eta: u8,
    t: &[i32],
) -> Result<State, String> {
    let mut state = State::new(eta);
    let training = prefix_history
        .len()
        .min(HISTORY_MAX)
        .min(4096usize.max(block_length.saturating_mul(2)));
    for (index, &byte) in prefix_history[prefix_history.len() - training..]
        .iter()
        .enumerate()
    {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        observe_byte(&mut state, byte, t);
    }
    Ok(state)
}
fn prime_with_recurrence(
    prefix_history: &[u8],
    block_length: usize,
    eta: u8,
    t: &[i32],
    config: RecurrenceConfig,
    mode: RecurrenceMode,
) -> Result<State, String> {
    let mut state = State::with_recurrence(eta, config, mode)?;
    let training = prefix_history
        .len()
        .min(HISTORY_MAX)
        .min(4096usize.max(block_length.saturating_mul(2)));
    for (index, &byte) in prefix_history[prefix_history.len() - training..]
        .iter()
        .enumerate()
    {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        observe_byte(&mut state, byte, t);
    }
    Ok(state)
}
fn encode_eta_unchecked(
    data: &[u8],
    eta: u8,
    t: &[i32],
    prefix_history: &[u8],
) -> (Vec<u8>, usize) {
    let mut state = prime_unchecked(prefix_history, data.len(), eta, t);
    let mut coder = ArithmeticEncoder::new();
    for &b in data {
        let mut prefix = 0;
        for bit in 0..8 {
            let actual = (b >> (7 - bit)) & 1;
            let (p, s, k, recurrence) = state.probabilities(bit, prefix, t);
            let zero = 4096 - p as usize;
            if actual == 1 {
                coder.encode(zero, 4096, 4096)
            } else {
                coder.encode(0, zero, 4096)
            }
            state.update(bit, actual, p, &s, &k, recurrence);
            prefix = (prefix << 1) | actual;
        }
        state.finish_byte(b);
    }
    coder.finish()
}
fn encode_eta_checked(
    data: &[u8],
    eta: u8,
    t: &[i32],
    prefix_history: &[u8],
) -> Result<(Vec<u8>, usize), String> {
    let mut state = prime_checked(prefix_history, data.len(), eta, t)?;
    let mut coder = ArithmeticEncoder::new();
    for (index, &b) in data.iter().enumerate() {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        let mut prefix = 0;
        for bit in 0..8 {
            let actual = (b >> (7 - bit)) & 1;
            let (p, s, k, recurrence) = state.probabilities(bit, prefix, t);
            let zero = 4096 - p as usize;
            if actual == 1 {
                coder.encode(zero, 4096, 4096)
            } else {
                coder.encode(0, zero, 4096)
            }
            state.update(bit, actual, p, &s, &k, recurrence);
            prefix = (prefix << 1) | actual;
        }
        state.finish_byte(b);
    }
    Ok(coder.finish())
}
fn encode_eta_with_recurrence(
    data: &[u8],
    eta: u8,
    t: &[i32],
    prefix_history: &[u8],
    config: RecurrenceConfig,
    mode: RecurrenceMode,
) -> Result<(Vec<u8>, usize), String> {
    let mut state = prime_with_recurrence(prefix_history, data.len(), eta, t, config, mode)?;
    let mut coder = ArithmeticEncoder::new();
    for (index, &b) in data.iter().enumerate() {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        let mut prefix = 0;
        for bit in 0..8 {
            let actual = (b >> (7 - bit)) & 1;
            let (p, s, k, recurrence) = state.probabilities(bit, prefix, t);
            let zero = 4096 - p as usize;
            if actual == 1 {
                coder.encode(zero, 4096, 4096)
            } else {
                coder.encode(0, zero, 4096)
            }
            state.update(bit, actual, p, &s, &k, recurrence);
            prefix = (prefix << 1) | actual;
        }
        state.finish_byte(b);
    }
    Ok(coder.finish())
}
fn put_v(mut x: usize, o: &mut Vec<u8>) {
    while x >= 128 {
        o.push((x as u8 & 127) | 128);
        x >>= 7;
    }
    o.push(x as u8);
}
fn get_v(b: &[u8], p: &mut usize) -> Result<usize, String> {
    let mut x = 0usize;
    for s in (0..usize::BITS as usize).step_by(7) {
        let v = *b.get(*p).ok_or("truncated fixed-mix length")?;
        *p += 1;
        let low = (v & 127) as usize;
        if low > (usize::MAX >> s) {
            return Err("fixed-mix length overflow".into());
        }
        x |= low
            .checked_shl(s as u32)
            .ok_or("fixed-mix length overflow")?;
        if v < 128 {
            return Ok(x);
        }
    }
    Err("oversized fixed-mix length".into())
}
pub fn encode_with_history_eta(
    data: &[u8],
    prefix_history: &[u8],
    eta_shift: u8,
) -> Result<Vec<u8>, String> {
    let id = ETA
        .iter()
        .position(|&eta| eta == eta_shift)
        .ok_or("unsupported fixed-mix learning rate")?;
    let t = stretch_table();
    let (payload, bits) = encode_eta_checked(data, eta_shift, t, prefix_history)?;
    let mut out = vec![1, id as u8];
    put_v(bits, &mut out);
    out.extend(payload);
    Ok(out)
}
/// Experimental fixed-mix payload version 3. Its header is:
/// `3, eta-id, recurrence-config-v2[7], arithmetic-bit-count-varint, payload`.
/// The v2 recurrence config uses saturated buckets and the encoder saves the
/// feature returned while observing the previous byte. Versions 1 and 2 keep
/// their original decoder semantics for archive compatibility.
pub fn encode_with_recurrence_eta(
    data: &[u8],
    prefix_history: &[u8],
    eta_shift: u8,
    recurrence: RecurrenceConfig,
) -> Result<Vec<u8>, String> {
    let id = ETA
        .iter()
        .position(|&eta| eta == eta_shift)
        .ok_or("unsupported fixed-mix learning rate")?;
    let recurrence_bytes = recurrence.to_saturated_bytes()?;
    let t = stretch_table();
    let (payload, bits) = encode_eta_with_recurrence(
        data,
        eta_shift,
        t,
        prefix_history,
        recurrence,
        RecurrenceMode::CorrectV3,
    )?;
    let mut out = vec![3, id as u8];
    out.extend(recurrence_bytes);
    put_v(bits, &mut out);
    out.extend(payload);
    Ok(out)
}
/// Legacy v2 emission for reproducing historic experimental payloads. New
/// callers should use `encode_with_recurrence_eta`, which emits corrected v3.
pub fn encode_with_legacy_recurrence_eta(
    data: &[u8],
    prefix_history: &[u8],
    eta_shift: u8,
    recurrence: RecurrenceConfig,
) -> Result<Vec<u8>, String> {
    let id = ETA
        .iter()
        .position(|&eta| eta == eta_shift)
        .ok_or("unsupported fixed-mix learning rate")?;
    let recurrence_bytes = recurrence.to_bytes()?;
    let t = stretch_table();
    let (payload, bits) = encode_eta_with_recurrence(
        data,
        eta_shift,
        t,
        prefix_history,
        recurrence,
        RecurrenceMode::LegacyV2,
    )?;
    let mut out = vec![2, id as u8];
    out.extend(recurrence_bytes);
    put_v(bits, &mut out);
    out.extend(payload);
    Ok(out)
}
pub fn encode_with_history(data: &[u8], prefix_history: &[u8]) -> Vec<u8> {
    let t = stretch_table();
    let mut best = None;
    for (id, &eta) in ETA.iter().enumerate() {
        let (payload, bits) = encode_eta_unchecked(data, eta, t, prefix_history);
        let mut out = vec![1, id as u8];
        put_v(bits, &mut out);
        out.extend(payload);
        if best.as_ref().is_none_or(|x: &Vec<u8>| out.len() < x.len()) {
            best = Some(out)
        }
    }
    best.unwrap()
}
pub fn encode(data: &[u8]) -> Vec<u8> {
    encode_with_history(data, &[])
}
pub fn decode(blob: &[u8], n: usize) -> Result<Vec<u8>, String> {
    decode_with_history(blob, n, &[])
}

fn decoder_state(
    blob: &[u8],
    n: usize,
    prefix_history: &[u8],
) -> Result<(usize, usize, State, &'static [i32; 4096]), String> {
    if blob.len() < 3 || !matches!(blob[0], 1..=3) || blob[1] > 2 {
        return Err("invalid fixed-mix version or learning rate".into());
    }
    let mut pos: usize = 2;
    let recurrence = if blob[0] >= 2 {
        let end = pos
            .checked_add(7)
            .ok_or("fixed-mix recurrence header overflow")?;
        let bytes = blob
            .get(pos..end)
            .ok_or("truncated fixed-mix recurrence configuration")?;
        pos = end;
        Some(match blob[0] {
            2 => (
                RecurrenceConfig::from_bytes(bytes)?,
                RecurrenceMode::LegacyV2,
            ),
            3 => (
                RecurrenceConfig::from_saturated_bytes(bytes)?,
                RecurrenceMode::CorrectV3,
            ),
            _ => unreachable!(),
        })
    } else {
        None
    };
    let bits = get_v(blob, &mut pos)?;
    if pos.checked_add(bits.div_ceil(8)) != Some(blob.len()) {
        return Err("fixed-mix payload length mismatch".into());
    }
    let table = stretch_table();
    let state = match recurrence {
        Some((config, mode)) => prime_with_recurrence(
            prefix_history,
            n,
            ETA[blob[1] as usize],
            table,
            config,
            mode,
        )?,
        None => prime_checked(prefix_history, n, ETA[blob[1] as usize], table)?,
    };
    Ok((pos, bits, state, table))
}

fn decode_byte(state: &mut State, coder: &mut ArithmeticDecoder<'_>, table: &[i32]) -> u8 {
    let mut byte = 0;
    let mut prefix = 0;
    for bit in 0..8 {
        let (probability, stretches, keys, recurrence) = state.probabilities(bit, prefix, table);
        let zero = 4096 - probability as usize;
        let actual = u8::from(coder.target(4096) >= zero);
        if actual == 0 {
            coder.update(0, zero, 4096);
        } else {
            coder.update(zero, 4096, 4096);
        }
        state.update(bit, actual, probability, &stretches, &keys, recurrence);
        byte = (byte << 1) | actual;
        prefix = (prefix << 1) | actual;
    }
    state.finish_byte(byte);
    byte
}

pub fn decode_with_history(
    blob: &[u8],
    n: usize,
    prefix_history: &[u8],
) -> Result<Vec<u8>, String> {
    let (p, bits, mut state, table) = decoder_state(blob, n, prefix_history)?;
    let mut coder = ArithmeticDecoder::new(&blob[p..], bits);
    let mut out = Vec::with_capacity(n);
    for index in 0..n {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        out.push(decode_byte(&mut state, &mut coder, table));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_stretch_table() -> [i32; 4096] {
        let mut table = [0; 4096];
        for (p, entry) in table.iter_mut().enumerate().skip(1) {
            *entry = fixed_log2(p as u32) - fixed_log2((4096 - p) as u32);
        }
        table
    }

    fn fixture() -> Vec<u8> {
        (0..4096)
            .map(|index| ((index * 37 + (index >> 3) * 11) & 255) as u8)
            .collect()
    }

    #[test]
    fn shared_stretch_table_matches_legacy_construction_exactly() {
        assert_eq!(*stretch_table(), legacy_stretch_table());
    }

    #[test]
    fn shared_stretch_table_keeps_fixedmix_versions_and_history_exact() {
        let input = fixture();
        let (history, payload_input) = input.split_at(512);
        let config = RecurrenceConfig {
            bucket_bits: 4,
            max_distance: 1024,
        };
        let payloads = [
            encode_with_history_eta(payload_input, history, 6).unwrap(),
            encode_with_legacy_recurrence_eta(payload_input, history, 6, config).unwrap(),
            encode_with_recurrence_eta(payload_input, history, 6, config).unwrap(),
        ];
        for payload in payloads {
            assert_eq!(
                decode_with_history(&payload, payload_input.len(), history).unwrap(),
                payload_input
            );
            assert!(decode_with_history(
                &payload[..payload.len() - 1],
                payload_input.len(),
                history
            )
            .is_err());
        }
    }

    #[test]
    fn shared_stretch_table_initializes_once_across_threads() {
        let local = stretch_table().as_ptr() as usize;
        let joined = std::thread::spawn(|| stretch_table().as_ptr() as usize)
            .join()
            .unwrap();
        assert_eq!(local, joined);
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
        assert_eq!(get_v(&encoded, &mut 0).unwrap(), usize::MAX);
        let groups = (usize::BITS as usize).div_ceil(7);
        let mut malformed = vec![128; groups - 1];
        malformed.push(1u8 << ((usize::BITS as usize - 1) % 7 + 1));
        assert!(get_v(&malformed, &mut 0).is_err());
    }
}
