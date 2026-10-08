//! Exact CIX type-class rank coding used by CIXG1 composition substreams.
//!
//! Combinatorial ranks are arbitrary precision. The 1024-symbol tiles used by
//! CIXG1 can produce ranks thousands of bits wide; converting them to machine
//! integers would silently corrupt archives.

use crate::combinatorics;
use num_bigint::BigUint;
use num_traits::{One, Zero};

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
    let mut out = vec![0; size.saturating_sub(bytes.len())];
    out.extend_from_slice(&bytes);
    out
}

fn read_fixed(data: &[u8], pos: &mut usize, width_bits: usize) -> Result<BigUint, String> {
    let size = width_bits.div_ceil(8);
    let end = pos.checked_add(size).ok_or("rank offset overflow")?;
    if end > data.len() {
        return Err("truncated CIX rank".into());
    }
    let value = BigUint::from_bytes_be(&data[*pos..end]);
    if !width_bits.is_multiple_of(8) && size > 0 {
        let excess = 8 - width_bits % 8;
        if data[*pos] >> (8 - excess) != 0 {
            return Err("nonzero padding in CIX rank".into());
        }
    }
    *pos = end;
    Ok(value)
}

fn varint(value: usize, out: &mut Vec<u8>) {
    let mut v = value;
    while v >= 128 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn read_varint(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..35).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated CIX varint")?;
        *pos += 1;
        value |= ((byte & 0x7f) as usize)
            .checked_shl(shift)
            .ok_or("oversized CIX varint")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized CIX varint".into())
}

struct BitWriter {
    data: Vec<u8>,
    current: u8,
    used: u8,
    bit_length: usize,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            current: 0,
            used: 0,
            bit_length: 0,
        }
    }
    fn write(&mut self, bit: u8) {
        self.current = (self.current << 1) | (bit & 1);
        self.used += 1;
        self.bit_length += 1;
        if self.used == 8 {
            self.data.push(self.current);
            self.current = 0;
            self.used = 0;
        }
    }
    fn finish(mut self) -> (Vec<u8>, usize) {
        if self.used > 0 {
            self.data.push(self.current << (8 - self.used));
        }
        (self.data, self.bit_length)
    }
}

pub struct ArithmeticEncoder {
    low: u64,
    high: u64,
    pending: usize,
    writer: BitWriter,
}

impl Default for ArithmeticEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ArithmeticEncoder {
    const HALF: u64 = 1 << 31;
    const Q1: u64 = 1 << 30;
    const Q3: u64 = 3 << 30;
    const MAX: u64 = (1 << 32) - 1;

    pub fn new() -> Self {
        Self {
            low: 0,
            high: Self::MAX,
            pending: 0,
            writer: BitWriter::new(),
        }
    }
    fn emit(&mut self, bit: u8) {
        self.writer.write(bit);
        while self.pending > 0 {
            self.writer.write(bit ^ 1);
            self.pending -= 1;
        }
    }
    pub fn encode(&mut self, low_count: usize, high_count: usize, total: usize) {
        let interval = self.high - self.low + 1;
        self.high = self.low + (interval * high_count as u64 / total as u64) - 1;
        self.low += interval * low_count as u64 / total as u64;
        loop {
            if self.high < Self::HALF {
                self.emit(0);
            } else if self.low >= Self::HALF {
                self.emit(1);
                self.low -= Self::HALF;
                self.high -= Self::HALF;
            } else if self.low >= Self::Q1 && self.high < Self::Q3 {
                self.pending += 1;
                self.low -= Self::Q1;
                self.high -= Self::Q1;
            } else {
                break;
            }
            self.low <<= 1;
            self.high = (self.high << 1) + 1;
        }
    }
    pub fn finish(mut self) -> (Vec<u8>, usize) {
        self.pending += 1;
        self.emit(if self.low < Self::Q1 { 0 } else { 1 });
        self.writer.finish()
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_length: usize,
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn read(&mut self) -> u8 {
        if self.pos >= self.bit_length {
            self.pos += 1;
            return 0;
        }
        let bit = (self.data[self.pos >> 3] >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        bit
    }
}

pub struct ArithmeticDecoder<'a> {
    reader: BitReader<'a>,
    low: u64,
    high: u64,
    value: u64,
}

impl<'a> ArithmeticDecoder<'a> {
    pub fn new(data: &'a [u8], bit_length: usize) -> Self {
        let mut result = Self {
            reader: BitReader {
                data,
                bit_length,
                pos: 0,
            },
            low: 0,
            high: ArithmeticEncoder::MAX,
            value: 0,
        };
        for _ in 0..32 {
            result.value = (result.value << 1) | result.reader.read() as u64;
        }
        result
    }
    pub fn target(&self, total: usize) -> usize {
        let interval = self.high - self.low + 1;
        (((self.value - self.low + 1) * total as u64 - 1) / interval) as usize
    }
    pub fn update(&mut self, low_count: usize, high_count: usize, total: usize) {
        let interval = self.high - self.low + 1;
        self.high = self.low + interval * high_count as u64 / total as u64 - 1;
        self.low += interval * low_count as u64 / total as u64;
        loop {
            if self.high < ArithmeticEncoder::HALF {
            } else if self.low >= ArithmeticEncoder::HALF {
                self.low -= ArithmeticEncoder::HALF;
                self.high -= ArithmeticEncoder::HALF;
                self.value -= ArithmeticEncoder::HALF;
            } else if self.low >= ArithmeticEncoder::Q1 && self.high < ArithmeticEncoder::Q3 {
                self.low -= ArithmeticEncoder::Q1;
                self.high -= ArithmeticEncoder::Q1;
                self.value -= ArithmeticEncoder::Q1;
            } else {
                break;
            }
            self.low <<= 1;
            self.high = (self.high << 1) + 1;
            self.value = (self.value << 1) | self.reader.read() as u64;
        }
    }
}

fn encode_sequence_range(data: &[u8], counts: &[usize]) -> Result<(Vec<u8>, usize), String> {
    let mut mutable = counts.to_vec();
    let mut fenwick = Fenwick::new(&mutable);
    let mut remaining = data.len();
    let mut encoder = ArithmeticEncoder::new();
    for (index, &symbol) in data.iter().enumerate() {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        let i = symbol as usize;
        let low = fenwick.prefix(i);
        let high = low + mutable[i];
        encoder.encode(low, high, remaining);
        mutable[i] -= 1;
        fenwick.add(i, -1);
        remaining -= 1;
    }
    Ok(encoder.finish())
}

/// Python CIXG1's matched-information count-range control (coder 5).
pub fn encode_count_range(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (tile_index, tile) in data.chunks(1024).enumerate() {
        if tile_index & 3 == 0 {
            crate::limits::check()?;
        }
        let counts = type_class_counts(tile)?;
        // Count-range has its own sequence stream.  Calling
        // `encode_type_class` here used to first materialize the canonical
        // permutation rank only to discard it.  Build the identical count
        // header directly instead, keeping the format and emitted bytes
        // unchanged while avoiding the arbitrary-precision rank work.
        let mut encoded = encode_type_class_header(tile.len(), &counts)?;
        let (payload, bit_length) = encode_sequence_range(tile, &counts)?;
        encoded.push(1);
        varint(bit_length, &mut encoded);
        encoded.extend_from_slice(&payload);
        varint(encoded.len(), &mut out);
        out.extend_from_slice(&encoded);
    }
    Ok(out)
}

fn decode_sequence_range(
    payload: &[u8],
    bit_length: usize,
    counts: &[usize],
) -> Result<Vec<u8>, String> {
    let mut mutable = counts.to_vec();
    let mut fenwick = Fenwick::new(&mutable);
    let mut remaining: usize = counts.iter().sum();
    let mut decoder = ArithmeticDecoder::new(payload, bit_length);
    let mut out = Vec::with_capacity(remaining);
    while remaining > 0 {
        if remaining & 255 == 0 {
            crate::limits::check()?;
        }
        let target = decoder.target(remaining);
        let symbol = fenwick.select(target)?;
        let low = fenwick.prefix(symbol);
        let high = low + mutable[symbol];
        decoder.update(low, high, remaining);
        out.push(symbol as u8);
        mutable[symbol] -= 1;
        fenwick.add(symbol, -1);
        remaining -= 1;
    }
    Ok(out)
}

fn colex_rank(values: &[usize]) -> Result<BigUint, String> {
    combinatorics::colex_rank_checked(values)
}

fn colex_unrank(mut rank: BigUint, k: usize, universe: usize) -> Result<Vec<usize>, String> {
    let limit = choose(universe, k);
    if k > universe || rank >= limit {
        return Err("combination rank outside range".into());
    }
    let mut result = Vec::with_capacity(k);
    let mut x = universe.saturating_sub(1);
    let mut polls = 0usize;
    for i in (1..=k).rev() {
        while choose(x, i) > rank {
            polls += 1;
            if polls & 255 == 0 {
                crate::limits::check()?;
            }
            if x == 0 {
                return Err("invalid combination rank".into());
            }
            x -= 1;
        }
        rank -= choose(x, i);
        result.push(x);
        x = x.saturating_sub(1);
    }
    result.reverse();
    Ok(result)
}

fn composition_rank(parts: &[usize]) -> Result<BigUint, String> {
    if parts.is_empty() || parts.contains(&0) {
        return Err("invalid CIX composition".into());
    }
    let mut total = 0usize;
    let mut cuts = Vec::with_capacity(parts.len().saturating_sub(1));
    for &part in &parts[..parts.len() - 1] {
        total = total.checked_add(part).ok_or("composition overflow")?;
        cuts.push(total - 1);
    }
    colex_rank(&cuts)
}

fn composition_unrank(rank: BigUint, total: usize, count: usize) -> Result<Vec<usize>, String> {
    if count == 0 || total == 0 || count > total {
        return Err("invalid CIX composition dimensions".into());
    }
    if count == 1 {
        return if rank.is_zero() {
            Ok(vec![total])
        } else {
            Err("invalid one-part composition rank".into())
        };
    }
    let cuts = colex_unrank(rank, count - 1, total - 1)?
        .into_iter()
        .map(|x| x + 1);
    let mut previous = 0;
    let mut parts = Vec::with_capacity(count);
    for cut in cuts {
        parts.push(cut - previous);
        previous = cut;
    }
    parts.push(total - previous);
    Ok(parts)
}

#[derive(Clone)]
struct Fenwick {
    tree: Vec<usize>,
}

impl Fenwick {
    fn new(counts: &[usize]) -> Self {
        let mut result = Self {
            tree: vec![0; counts.len() + 1],
        };
        for (i, &count) in counts.iter().enumerate() {
            if count > 0 {
                result.add(i, count as isize);
            }
        }
        result
    }

    fn add(&mut self, index: usize, delta: isize) {
        let mut i = index + 1;
        while i < self.tree.len() {
            if delta >= 0 {
                self.tree[i] += delta as usize;
            } else {
                self.tree[i] -= (-delta) as usize;
            }
            i += i.isolate_lowest_one();
        }
    }

    fn prefix(&self, end: usize) -> usize {
        let mut i = end;
        let mut sum = 0;
        while i > 0 {
            sum += self.tree[i];
            i &= i - 1;
        }
        sum
    }

    fn select(&self, mut order: usize) -> Result<usize, String> {
        let n = self.tree.len() - 1;
        let mut index = 0;
        let mut bit = 1usize << (usize::BITS - n.leading_zeros() - 1);
        while bit > 0 {
            let next = index + bit;
            if next <= n && self.tree[next] <= order {
                order -= self.tree[next];
                index = next;
            }
            bit >>= 1;
        }
        if index >= n {
            Err("CIX rank selected outside active alphabet".into())
        } else {
            Ok(index)
        }
    }
}

fn cardinality(counts: &[usize]) -> BigUint {
    let mut remaining: usize = counts.iter().sum();
    let mut total = BigUint::one();
    for &count in counts {
        if count > 0 {
            total *= choose(remaining, count);
            remaining -= count;
        }
    }
    total
}

fn active_symbol_count(counts: &[usize]) -> usize {
    counts.iter().filter(|&&count| count > 0).count()
}

// `ways` is the already computed cardinality of `counts`.  Keeping that
// arbitrary-precision value avoids a second choose/divide pass in rank mode.
fn permutation_rank(data: &[u8], counts: &[usize], mut ways: BigUint) -> Result<BigUint, String> {
    let mut mutable = counts.to_vec();
    let mut fenwick = Fenwick::new(&mutable);
    let mut remaining = data.len();
    let mut rank = BigUint::zero();
    for (index, &symbol) in data.iter().enumerate() {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        let i = symbol as usize;
        let before = fenwick.prefix(i);
        if before > 0 {
            rank += (&ways * before) / remaining;
        }
        let occurrences = mutable[i];
        if occurrences == 0 || remaining == 0 {
            return Err("symbol absent from CIX histogram".into());
        }
        ways = (ways * occurrences) / remaining;
        mutable[i] -= 1;
        fenwick.add(i, -1);
        remaining -= 1;
    }
    Ok(rank)
}

fn permutation_unrank(
    mut rank: BigUint,
    counts: &[usize],
    mut ways: BigUint,
) -> Result<Vec<u8>, String> {
    let mut mutable = counts.to_vec();
    let mut fenwick = Fenwick::new(&mutable);
    let mut remaining: usize = counts.iter().sum();
    if rank >= ways {
        return Err("permutation rank outside type class".into());
    }
    let mut output = Vec::with_capacity(remaining);
    while remaining > 0 {
        if remaining & 255 == 0 {
            crate::limits::check()?;
        }
        let order = ((&rank * remaining) / &ways)
            .try_into()
            .map_err(|_| "CIX rank selection overflow")?;
        let symbol = fenwick.select(order)?;
        let before = fenwick.prefix(symbol);
        rank -= (&ways * before) / remaining;
        let occurrences = mutable[symbol];
        ways = (ways * occurrences) / remaining;
        mutable[symbol] -= 1;
        fenwick.add(symbol, -1);
        remaining -= 1;
        output.push(symbol as u8);
    }
    Ok(output)
}

fn type_class_counts(data: &[u8]) -> Result<[usize; 256], String> {
    let mut counts = [0usize; 256];
    for (index, &byte) in data.iter().enumerate() {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        counts[byte as usize] += 1;
    }
    Ok(counts)
}

/// Serialize the count portion of a type-class payload, excluding its
/// sequence mode and sequence bytes.  The byte layout deliberately matches
/// the prefix emitted by `encode_type_class`.
fn encode_type_class_header(n: usize, counts: &[usize; 256]) -> Result<Vec<u8>, String> {
    let active: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(symbol, &count)| (count > 0).then_some(symbol))
        .collect();
    let m = active.len();
    let mut out = Vec::new();
    varint(m, &mut out);
    if n == 0 {
        return Ok(out);
    }
    if m == 0 || m > n {
        return Err("invalid CIX type-class counts".into());
    }
    if m < 256 {
        let width = fixed_width(&(choose(256, m) - BigUint::one()));
        out.extend(fixed_bytes(&colex_rank(&active)?, width));
    }
    if m > 1 {
        let parts: Vec<usize> = active.iter().map(|&s| counts[s]).collect();
        let width = fixed_width(&(choose(n - 1, m - 1) - BigUint::one()));
        out.extend(fixed_bytes(&composition_rank(&parts)?, width));
    }
    Ok(out)
}

pub fn encode_type_class(data: &[u8]) -> Result<Vec<u8>, String> {
    let counts = type_class_counts(data)?;
    let mut out = encode_type_class_header(data.len(), &counts)?;
    if data.is_empty() {
        return Ok(out);
    }
    // For a nonempty histogram, more than one active symbol is exactly the
    // condition cardinality(counts) > 1.  Remaining-count range mode needs
    // neither the cardinality nor its fixed rank width, so avoid BigUint
    // choose/divide work on this large-tile path.
    if data.len() >= 2048 && active_symbol_count(&counts) >= 2 {
        let (payload, bit_length) = encode_sequence_range(data, &counts)?;
        out.push(1);
        varint(bit_length, &mut out);
        out.extend_from_slice(&payload);
    } else {
        let ways = cardinality(&counts);
        let width = fixed_width(&(&ways - BigUint::one()));
        out.push(0); // canonical rank mode
        out.extend(fixed_bytes(&permutation_rank(data, &counts, ways)?, width));
    }
    Ok(out)
}

pub fn decode_type_class(data: &[u8], pos: &mut usize, n: usize) -> Result<Vec<u8>, String> {
    let m = read_varint(data, pos)?;
    if n == 0 {
        if m != 0 {
            return Err("active symbols in empty CIX type class".into());
        }
        return Ok(Vec::new());
    }
    if m == 0 || m > 256 || m > n {
        return Err("invalid CIX active alphabet".into());
    }
    let active = if m == 256 {
        (0..256).collect()
    } else {
        let width = fixed_width(&(choose(256, m) - BigUint::one()));
        colex_unrank(read_fixed(data, pos, width)?, m, 256)?
    };
    let parts = if m == 1 {
        vec![n]
    } else {
        let width = fixed_width(&(choose(n - 1, m - 1) - BigUint::one()));
        composition_unrank(read_fixed(data, pos, width)?, n, m)?
    };
    let mut counts = vec![0usize; 256];
    for (&symbol, count) in active.iter().zip(parts) {
        counts[symbol] = count;
    }
    let mode = *data.get(*pos).ok_or("missing CIX sequence mode")?;
    *pos += 1;
    match mode {
        0 => {
            // Only canonical rank mode consumes a cardinality-derived width.
            let ways = cardinality(&counts);
            let width = fixed_width(&(&ways - BigUint::one()));
            let rank = read_fixed(data, pos, width)?;
            permutation_unrank(rank, &counts, ways)
        }
        1 => {
            let bit_length = read_varint(data, pos)?;
            let byte_length = bit_length.div_ceil(8);
            let end = pos
                .checked_add(byte_length)
                .ok_or("range stream length overflow")?;
            if end > data.len() {
                return Err("truncated CIX range stream".into());
            }
            let result = decode_sequence_range(&data[*pos..end], bit_length, &counts)?;
            *pos = end;
            Ok(result)
        }
        _ => Err(format!("unsupported CIX type-class sequence mode {mode}")),
    }
}

pub fn encode_composition_tiles(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (tile_index, tile) in data.chunks(1024).enumerate() {
        if tile_index & 3 == 0 {
            crate::limits::check()?;
        }
        let encoded = encode_type_class(tile)?;
        varint(encoded.len(), &mut out);
        out.extend_from_slice(&encoded);
    }
    Ok(out)
}

pub fn decode_composition_tiles(data: &[u8], n: usize) -> Result<Vec<u8>, String> {
    let mut pos = 0;
    let mut output = Vec::with_capacity(n);
    while output.len() < n {
        crate::limits::check()?;
        let size = read_varint(data, &mut pos)?;
        let end = pos.checked_add(size).ok_or("CIX tile length overflow")?;
        if end > data.len() {
            return Err("truncated CIX composition tile".into());
        }
        let mut tile_pos = 0;
        let tile_n = (n - output.len()).min(1024);
        let decoded = decode_type_class(&data[pos..end], &mut tile_pos, tile_n)?;
        if tile_pos != size {
            return Err("trailing bytes in CIX composition tile".into());
        }
        output.extend(decoded);
        pos = end;
    }
    if pos != data.len() {
        return Err("trailing CIX composition data".into());
    }
    Ok(output)
}

/// Decode the legacy CIX sparse dominant-symbol representation.
pub fn decode_sparse_blob(data: &[u8], n: usize) -> Result<Vec<u8>, String> {
    if data.len() < 2 || data[0] != 1 {
        return Err("unknown sparse block version".into());
    }
    let dominant = data[1];
    let mut pos = 2;
    let k = read_varint(data, &mut pos)?;
    if k > n {
        return Err("too many sparse exceptions".into());
    }
    let width = fixed_width(&(choose(n, k) - BigUint::one()));
    let ranks = colex_unrank(read_fixed(data, &mut pos, width)?, k, n)?;
    let mut value_pos = pos;
    let values = decode_type_class(data, &mut value_pos, k)?;
    if value_pos != data.len() {
        return Err("unused sparse payload bytes".into());
    }
    let mut out = vec![dominant; n];
    for (index, value) in ranks.into_iter().zip(values) {
        out[index] = value;
    }
    Ok(out)
}

/// Read a fixed-width combinadic rank and unrank its colex positions.
pub fn decode_colex_positions(
    data: &[u8],
    pos: &mut usize,
    universe: usize,
    count: usize,
) -> Result<Vec<usize>, String> {
    let width = fixed_width(&(choose(universe, count) - BigUint::one()));
    colex_unrank(read_fixed(data, pos, width)?, count, universe)
}

/// Decode an existing type-class sequence after rebuilding its explicit count header.
pub fn decode_type_class_with_counts(
    sequence: &[u8],
    n: usize,
    counts: &[usize],
) -> Result<Vec<u8>, String> {
    if counts.len() != 256 || counts.iter().sum::<usize>() != n {
        return Err("invalid supplied type-class counts".into());
    }
    let active: Vec<usize> = counts
        .iter()
        .enumerate()
        .filter_map(|(s, &c)| (c > 0).then_some(s))
        .collect();
    let m = active.len();
    let mut header = Vec::new();
    varint(m, &mut header);
    if m < 256 {
        let width = fixed_width(&(choose(256, m) - BigUint::one()));
        header.extend(fixed_bytes(&colex_rank(&active)?, width));
    }
    if m > 1 {
        let parts: Vec<usize> = active.iter().map(|&s| counts[s]).collect();
        let width = fixed_width(&(choose(n - 1, m - 1) - BigUint::one()));
        header.extend(fixed_bytes(&composition_rank(&parts)?, width));
    }
    header.extend_from_slice(sequence);
    let mut pos = 0;
    let out = decode_type_class(&header, &mut pos, n)?;
    if pos != header.len() {
        return Err("unused histogram sequence bytes".into());
    }
    Ok(out)
}

fn context_tail(prefix_history: &[u8], order: usize) -> Vec<u8> {
    if order <= prefix_history.len() {
        prefix_history[prefix_history.len() - order..].to_vec()
    } else {
        prefix_history.to_vec()
    }
}

fn context_bucket(tail: &[u8], order: usize, bucket_bits: usize) -> usize {
    if order == 1 && bucket_bits >= 8 {
        return tail.last().copied().unwrap_or(0) as usize;
    }
    let mut hash = 1469598103934665603u64;
    for &byte in tail {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    (hash & ((1u64 << bucket_bits) - 1)) as usize
}

fn advance_context_tail(tail: &mut Vec<u8>, value: u8, order: usize) {
    tail.push(value);
    if tail.len() > order {
        tail.remove(0);
    }
}

fn decode_context_lanes(
    data: &[u8],
    pos: &mut usize,
    lengths: Vec<usize>,
) -> Result<Vec<Vec<u8>>, String> {
    let mut lanes = Vec::with_capacity(lengths.len());
    for count in lengths {
        lanes.push(decode_type_class(data, pos, count)?);
    }
    Ok(lanes)
}

fn validate_context_cursors(cursors: &[usize], lanes: &[Vec<u8>]) -> Result<(), String> {
    if cursors
        .iter()
        .zip(lanes)
        .any(|(&used, lane)| used != lane.len())
    {
        return Err("unused context lane symbols".into());
    }
    Ok(())
}

fn context_layout(
    data: &[u8],
    pos: &mut usize,
    n: usize,
    bucket_count: usize,
) -> Result<(Vec<usize>, Vec<usize>), String> {
    let q = read_varint(data, pos)?;
    if q > bucket_count.min(n) || (q == 0 && n != 0) {
        return Err("invalid active context count".into());
    }
    if q == 0 {
        if *pos != data.len() {
            return Err("trailing context type-class bytes".into());
        }
        return Ok((Vec::new(), Vec::new()));
    }
    let ids = if q == bucket_count {
        (0..bucket_count).collect()
    } else {
        let width = fixed_width(&(choose(bucket_count, q) - BigUint::one()));
        colex_unrank(read_fixed(data, pos, width)?, q, bucket_count)?
    };
    let lengths = if q == 1 {
        vec![n]
    } else {
        let width = fixed_width(&(choose(n - 1, q - 1) - BigUint::one()));
        composition_unrank(read_fixed(data, pos, width)?, n, q)?
    };
    Ok((ids, lengths))
}

/// Decode CIXM6 context-conditioned type classes (mode 2).
pub fn decode_context_type_classes(
    data: &[u8],
    n: usize,
    prefix_history: &[u8],
) -> Result<Vec<u8>, String> {
    if data.len() < 3 {
        return Err("truncated context type-class header".into());
    }
    let order = data[0] as usize;
    let bucket_bits = data[1] as usize;
    if !(1..=16).contains(&order) || !(1..=16).contains(&bucket_bits) {
        return Err("invalid context type-class parameters".into());
    }
    let bucket_count = 1usize << bucket_bits;
    let mut pos = 2;
    let (ids, lengths) = context_layout(data, &mut pos, n, bucket_count)?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let q = ids.len();
    let lanes = decode_context_lanes(data, &mut pos, lengths)?;
    if pos != data.len() {
        return Err("trailing context type-class bytes".into());
    }
    let lookup: std::collections::HashMap<usize, usize> =
        ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut cursors = vec![0usize; q];
    let mut tail = context_tail(prefix_history, order);
    let mut out = Vec::with_capacity(n);
    for index in 0..n {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        let bucket = context_bucket(&tail, order, bucket_bits);
        let lane = *lookup
            .get(&bucket)
            .ok_or("context bucket absent from stream")?;
        let value = *lanes[lane]
            .get(cursors[lane])
            .ok_or("context lane exhausted")?;
        cursors[lane] += 1;
        out.push(value);
        advance_context_tail(&mut tail, value, order);
    }
    validate_context_cursors(&cursors, &lanes)?;
    Ok(out)
}

/// Encode the frozen CIXM6 context-conditioned type-class payload (mode 2).
///
/// `prefix_history` seeds the causal context for the first symbol, but is not
/// included in the payload. The caller must retain and supply the same history
/// to the decoder. Every combinatorial rank remains arbitrary precision.
pub fn encode_context_type_classes(
    data: &[u8],
    order: usize,
    bucket_bits: usize,
    prefix_history: &[u8],
) -> Result<Vec<u8>, String> {
    if !(1..=16).contains(&order) {
        return Err("context order outside supported range".into());
    }
    if !(1..=16).contains(&bucket_bits) {
        return Err("context bucket bits outside supported range".into());
    }

    let bucket_count = 1usize << bucket_bits;
    let mut buckets = std::collections::BTreeMap::<usize, Vec<u8>>::new();
    let mut tail = context_tail(prefix_history, order);

    for (index, &symbol) in data.iter().enumerate() {
        if index & 255 == 0 {
            crate::limits::check()?;
        }
        let bucket = context_bucket(&tail, order, bucket_bits);
        buckets.entry(bucket).or_default().push(symbol);
        advance_context_tail(&mut tail, symbol, order);
    }

    let ids: Vec<usize> = buckets.keys().copied().collect();
    let q = ids.len();
    let mut out = Vec::new();
    out.push(order as u8);
    out.push(bucket_bits as u8);
    varint(q, &mut out);

    if q != 0 && q < bucket_count {
        let width = fixed_width(&(choose(bucket_count, q) - BigUint::one()));
        out.extend(fixed_bytes(&colex_rank(&ids)?, width));
    }

    if q > 1 {
        let lengths: Vec<usize> = ids.iter().map(|id| buckets[id].len()).collect();
        let width = fixed_width(&(choose(data.len() - 1, q - 1) - BigUint::one()));
        out.extend(fixed_bytes(&composition_rank(&lengths)?, width));
    }

    for id in ids {
        out.extend(encode_type_class(&buckets[&id])?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_arithmetic_encoder_preserves_constructor_bytes() {
        let mut explicit = ArithmeticEncoder::new();
        let mut default = ArithmeticEncoder::default();
        for symbol in [0, 2, 1, 2, 0, 1] {
            explicit.encode(symbol, symbol + 1, 3);
            default.encode(symbol, symbol + 1, 3);
        }
        assert_eq!(explicit.finish(), default.finish());
    }

    #[test]
    fn lowest_bit_preserves_fenwick_step() {
        for value in [0usize, 1, 2, 3, usize::MAX, 1 << (usize::BITS - 1)] {
            let expected = if value == 0 {
                0
            } else {
                1usize << value.trailing_zeros()
            };
            assert_eq!(value.isolate_lowest_one(), expected);
        }
    }

    // This is the pre-refactor count-range assembly.  It is test-only so the
    // production path cannot accidentally retain the discarded rank work.
    fn legacy_count_range(data: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        for tile in data.chunks(1024) {
            let exact = encode_type_class(tile)?;
            let counts = type_class_counts(tile)?;
            let ways = cardinality(&counts);
            let width = fixed_width(&(&ways - BigUint::one()));
            let sequence_bytes = width.div_ceil(8);
            let header_end = exact
                .len()
                .checked_sub(sequence_bytes + 1)
                .ok_or("invalid exact-rank header")?;
            let (payload, bit_length) = encode_sequence_range(tile, &counts)?;
            let mut encoded = exact[..header_end].to_vec();
            encoded.push(1);
            varint(bit_length, &mut encoded);
            encoded.extend_from_slice(&payload);
            varint(encoded.len(), &mut out);
            out.extend_from_slice(&encoded);
        }
        Ok(out)
    }

    // Literal pre-optimization definition retained only to lock emitted
    // type-class bytes.  It deliberately materializes cardinality before the
    // large remaining-count-range decision.
    fn legacy_encode_type_class(data: &[u8]) -> Result<Vec<u8>, String> {
        let counts = type_class_counts(data)?;
        let mut out = encode_type_class_header(data.len(), &counts)?;
        if data.is_empty() {
            return Ok(out);
        }
        let ways = cardinality(&counts);
        let width = fixed_width(&(&ways - BigUint::one()));
        if data.len() >= 2048 && ways > BigUint::one() {
            let (payload, bit_length) = encode_sequence_range(data, &counts)?;
            out.push(1);
            varint(bit_length, &mut out);
            out.extend_from_slice(&payload);
        } else {
            out.push(0);
            // The former rank path computed cardinality once here and once
            // inside permutation_rank.  Preserve that test-only work so this
            // helper is a definition-level parity control.
            out.extend(fixed_bytes(
                &permutation_rank(data, &counts, cardinality(&counts))?,
                width,
            ));
        }
        Ok(out)
    }

    fn legacy_decode_type_class(data: &[u8], pos: &mut usize, n: usize) -> Result<Vec<u8>, String> {
        let m = read_varint(data, pos)?;
        if n == 0 {
            if m != 0 {
                return Err("active symbols in empty CIX type class".into());
            }
            return Ok(Vec::new());
        }
        if m == 0 || m > 256 || m > n {
            return Err("invalid CIX active alphabet".into());
        }
        let active = if m == 256 {
            (0..256).collect()
        } else {
            let width = fixed_width(&(choose(256, m) - BigUint::one()));
            colex_unrank(read_fixed(data, pos, width)?, m, 256)?
        };
        let parts = if m == 1 {
            vec![n]
        } else {
            let width = fixed_width(&(choose(n - 1, m - 1) - BigUint::one()));
            composition_unrank(read_fixed(data, pos, width)?, n, m)?
        };
        let mut counts = vec![0usize; 256];
        for (&symbol, count) in active.iter().zip(parts) {
            counts[symbol] = count;
        }
        let ways = cardinality(&counts);
        let width = fixed_width(&(&ways - BigUint::one()));
        let mode = *data.get(*pos).ok_or("missing CIX sequence mode")?;
        *pos += 1;
        match mode {
            0 => permutation_unrank(read_fixed(data, pos, width)?, &counts, ways),
            1 => {
                let bit_length = read_varint(data, pos)?;
                let byte_length = bit_length.div_ceil(8);
                let end = pos
                    .checked_add(byte_length)
                    .ok_or("range stream length overflow")?;
                if end > data.len() {
                    return Err("truncated CIX range stream".into());
                }
                let result = decode_sequence_range(&data[*pos..end], bit_length, &counts)?;
                *pos = end;
                Ok(result)
            }
            _ => Err(format!("unsupported CIX type-class sequence mode {mode}")),
        }
    }

    fn deterministic_bytes(length: usize) -> Vec<u8> {
        let mut state = 0xD1CE_BA5Eu32;
        (0..length)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn type_class_shortcut_preserves_prechange_exact_bytes_and_malformed_rejection() {
        let mut cases = vec![Vec::new(), vec![7], vec![7; 2048]];
        cases.push((0..2047).map(|i| (i & 1) as u8).collect());
        cases.push((0..2048).map(|i| (i & 1) as u8).collect());
        cases.push(deterministic_bytes(8193));
        for input in cases {
            let legacy = legacy_encode_type_class(&input).unwrap();
            let current = encode_type_class(&input).unwrap();
            assert_eq!(current, legacy);
            let mut old_pos = 0;
            let mut new_pos = 0;
            assert_eq!(
                legacy_decode_type_class(&legacy, &mut old_pos, input.len()).unwrap(),
                input
            );
            assert_eq!(
                decode_type_class(&current, &mut new_pos, input.len()).unwrap(),
                input
            );
            assert_eq!(old_pos, legacy.len());
            assert_eq!(new_pos, current.len());
        }

        let malformed_input: Vec<u8> = (0..2048).map(|i| (i & 1) as u8).collect();
        let counts = type_class_counts(&malformed_input).unwrap();
        let header = encode_type_class_header(malformed_input.len(), &counts).unwrap();
        let mut mode_zero = header.clone();
        mode_zero.push(0); // canonical rank payload is deliberately absent.
        let mut mode_one = header;
        mode_one.push(1);
        varint(1, &mut mode_one); // declared range bit has no backing byte.
        for malformed in [mode_zero, mode_one] {
            let mut old_pos = 0;
            let mut new_pos = 0;
            assert!(
                legacy_decode_type_class(&malformed, &mut old_pos, malformed_input.len()).is_err()
            );
            assert!(decode_type_class(&malformed, &mut new_pos, malformed_input.len()).is_err());
        }
    }

    #[test]
    fn count_range_keeps_pre_refactor_bytes_and_round_trips() {
        let mut cases = vec![Vec::new(), vec![7], vec![0; 1024]];
        cases.push((0..1024).map(|i| (i & 255) as u8).collect());
        cases.push(deterministic_bytes(2_049));
        cases.push(
            (0..3_217)
                .map(|i| if i % 17 == 0 { 0xFF } else { (i % 9) as u8 })
                .collect(),
        );

        for input in cases {
            let encoded = encode_count_range(&input).unwrap();
            assert_eq!(encoded, legacy_count_range(&input).unwrap());
            assert_eq!(
                decode_composition_tiles(&encoded, input.len()).unwrap(),
                input
            );
        }
    }

    #[test]
    fn count_range_preserves_large_exact_rank_type_classes() {
        let input = deterministic_bytes(1024);
        let counts = type_class_counts(&input).unwrap();
        assert!(cardinality(&counts).bits() > 128);
        let encoded = encode_count_range(&input).unwrap();
        assert_eq!(
            decode_composition_tiles(&encoded, input.len()).unwrap(),
            input
        );
    }

    #[test]
    fn count_range_rejects_truncated_tile() {
        let input = deterministic_bytes(1024);
        let mut encoded = encode_count_range(&input).unwrap();
        encoded.pop();
        assert!(decode_composition_tiles(&encoded, input.len()).is_err());
    }

    #[test]
    fn context_layouts_round_trip_and_reject_trailing_bytes() {
        let cases = [
            (
                b"aaaaabbbbccccddddeeeeffff".as_slice(),
                1,
                1,
                b"".as_slice(),
            ),
            (
                b"context lanes preserve canonical ordering across blocks".as_slice(),
                3,
                4,
                b"prior block context".as_slice(),
            ),
        ];
        for (input, order, bucket_bits, history) in cases {
            let encoded = encode_context_type_classes(input, order, bucket_bits, history).unwrap();
            assert_eq!(
                decode_context_type_classes(&encoded, input.len(), history).unwrap(),
                input
            );
            let mut trailing = encoded;
            trailing.push(0);
            assert!(decode_context_type_classes(&trailing, input.len(), history).is_err());
        }
    }

    #[test]
    fn context_layout_rejects_malformed_prefix() {
        assert!(decode_context_type_classes(&[1, 1], 1, b"").is_err());
        assert!(decode_context_type_classes(&[1, 1, 0], 1, b"").is_err());
    }
}
