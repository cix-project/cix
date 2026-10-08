//! A deliberately small, generic LZ parser for the CIXG1 route-3 grammar.
//!
//! This is a new Rust implementation, not an encoder for either Snappy or
//! Gipfeli streams.  It borrows only the broadly published fast-parser ideas:
//! a fixed hash table with one recent candidate (Snappy), and avoiding a full
//! insertion pass across a long match (Gipfeli).  Useful source references
//! are Google's Snappy `snappy.cc` and Gipfeli `gipfeli-internal.cc`; neither
//! their token format nor their framing is used here.
//!
//! CIX route 3 has four independent streams:
//! * tag `0` plus one literal byte;
//! * tag `1` plus unsigned LEB128 `(length - 4)` and unsigned LEB128 distance.
//!
//! The parser intentionally uses no input identity, filename, dictionary, or
//! ISA dispatch.  Its 64 KiB window and fixed table make its memory bounded by
//! the supplied block (which is itself bounded by the CIXG1 block limit).

use crate::limits;

const MIN_MATCH: usize = 4;
const WINDOW: usize = 65_536;
const HASH_BITS: u32 = 16;
const HASH_SIZE: usize = 1 << HASH_BITS;
const NO_POSITION: u32 = u32::MAX;
const POLL_MASK: usize = 0x0fff;

struct Streams {
    tags: Vec<u8>,
    literals: Vec<u8>,
    lengths: Vec<u8>,
    distances: Vec<u8>,
}

impl Streams {
    fn new(input_len: usize) -> Self {
        Self {
            tags: Vec::with_capacity(input_len),
            literals: Vec::with_capacity(input_len),
            lengths: Vec::new(),
            distances: Vec::new(),
        }
    }

    fn literal(&mut self, value: u8) {
        self.tags.push(0);
        self.literals.push(value);
    }

    fn matched(&mut self, length: usize, distance: usize) {
        self.tags.push(1);
        put_leb128(length - MIN_MATCH, &mut self.lengths);
        put_leb128(distance, &mut self.distances);
    }

    fn finish(self) -> [Vec<u8>; 4] {
        [self.tags, self.literals, self.lengths, self.distances]
    }
}

fn match_at(
    data: &[u8],
    table: &mut [u32],
    position: usize,
    probe: bool,
) -> Option<(usize, usize)> {
    if !probe || position + MIN_MATCH > data.len() {
        return None;
    }
    let hash = hash4(data, position);
    let prior = table[hash];
    table[hash] = position as u32;
    let candidate = (prior != NO_POSITION).then_some(prior as usize)?;
    let distance = position.saturating_sub(candidate);
    let prefix_matches = distance != 0
        && distance <= WINDOW
        && candidate + MIN_MATCH <= data.len()
        && data[candidate..candidate + MIN_MATCH] == data[position..position + MIN_MATCH];
    prefix_matches
        .then(|| (match_length(data, candidate, position), distance))
        .filter(|(length, _)| *length >= MIN_MATCH)
}

fn next_probe(skip_probes: &mut usize) -> bool {
    let probe = *skip_probes == 0;
    if !probe {
        *skip_probes -= 1;
    }
    probe
}

fn advance_after_miss(misses: &mut usize, skip_probes: &mut usize, probe: bool) {
    *misses = misses.saturating_add(1);
    if probe && *misses >= 16 {
        *skip_probes = ((*misses - 16) >> 4).min(8);
    }
}

/// Parse one CIXG1 LZ block into its uncompressed route-3 substreams.
///
/// `data` must be a CIXG1 block, hence no larger than 64 KiB.  The returned
/// streams are `[tags, literals, lengths, distances]`; callers apply their
/// selected nested coder afterwards.
pub fn parse(data: &[u8]) -> Result<[Vec<u8>; 4], String> {
    if data.len() > WINDOW {
        return Err(format!(
            "fast LZ block exceeds CIXG1 64 KiB limit: {} bytes",
            data.len()
        ));
    }

    // A u32 slot is a fixed-position table with an explicit sentinel.  It is
    // intentionally not a hash map: collision behaviour is deterministic,
    // cache-friendly, and its 256 KiB cost is independent of input content.
    let mut table = vec![NO_POSITION; HASH_SIZE];
    let mut streams = Streams::new(data.len());
    let mut position = 0usize;
    let mut misses = 0usize;
    let mut skip_probes = 0usize;

    while position < data.len() {
        if position & POLL_MASK == 0 {
            limits::check()?;
        }

        // Once a region has repeatedly failed to match, spend fewer hash
        // probes there.  This affects only search work: every skipped source
        // byte is still emitted as a literal, so it cannot lose data.
        let probe = next_probe(&mut skip_probes);
        let matched = match_at(data, &mut table, position, probe);

        if let Some((length, distance)) = matched {
            streams.matched(length, distance);

            let stop = position + length;
            // Insert only bounded boundary positions instead of every byte of
            // a long match.  This retains useful anchors for adjacent matches
            // while preventing match length from becoming parser work.
            insert_boundaries(&mut table, data, position, stop);
            position = stop;
            misses = 0;
            skip_probes = 0;
        } else {
            streams.literal(data[position]);
            position += 1;
            // The bounded skip is only activated after enough demonstrated
            // misses and is capped, preserving a cheap opportunity to resume
            // matching soon after an incompressible section.
            // Only a failed *probe* may schedule more skipped probes.  If a
            // skipped literal reset this counter too, a long random prefix
            // could perpetually re-arm a one-position skip and never notice
            // the first match in a later compressible region.
            advance_after_miss(&mut misses, &mut skip_probes, probe);
        }
    }
    limits::check()?;
    Ok(streams.finish())
}

#[inline]
fn hash4(data: &[u8], position: usize) -> usize {
    let word = u32::from_le_bytes(data[position..position + 4].try_into().expect("four bytes"));
    ((word.wrapping_mul(0x1e35_a7bd)) >> (32 - HASH_BITS)) as usize
}

#[inline]
fn put_leb128(mut value: usize, dst: &mut Vec<u8>) {
    while value >= 0x80 {
        dst.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    dst.push(value as u8);
}

/// Finds the full match length using safe, little-endian 64-bit loads.  The
/// first differing byte in a word is `trailing_zeros / 8` on little-endian
/// words; a byte tail covers an incomplete final word.
#[inline]
fn match_length(data: &[u8], candidate: usize, position: usize) -> usize {
    let limit = data.len() - position;
    let mut length = MIN_MATCH;
    while length + 8 <= limit {
        let left = u64::from_le_bytes(
            data[candidate + length..candidate + length + 8]
                .try_into()
                .expect("checked 64-bit source load"),
        );
        let right = u64::from_le_bytes(
            data[position + length..position + length + 8]
                .try_into()
                .expect("checked 64-bit target load"),
        );
        let diff = left ^ right;
        if diff != 0 {
            return length + (diff.trailing_zeros() as usize >> 3);
        }
        length += 8;
    }
    while length < limit && data[candidate + length] == data[position + length] {
        length += 1;
    }
    length
}

fn insert_boundaries(table: &mut [u32], data: &[u8], start: usize, stop: usize) {
    // Four anchors are enough to cover the beginning and end of a match; all
    // are bounded independently of match length.  Duplicates are harmless.
    let positions = [
        start,
        start.saturating_add(1),
        stop.saturating_sub(3),
        stop.saturating_sub(2),
    ];
    for position in positions {
        if position + MIN_MATCH <= data.len() {
            table[hash4(data, position)] = position as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_leb128(source: &[u8], at: &mut usize) -> usize {
        let mut shift = 0u32;
        let mut value = 0usize;
        loop {
            let byte = source[*at];
            *at += 1;
            value |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
            shift += 7;
        }
    }

    fn reconstruct(parts: &[Vec<u8>; 4]) -> Vec<u8> {
        let (mut literal, mut length, mut distance) = (0usize, 0usize, 0usize);
        let mut output = Vec::new();
        for &tag in &parts[0] {
            match tag {
                0 => {
                    output.push(parts[1][literal]);
                    literal += 1;
                }
                1 => {
                    let count = get_leb128(&parts[2], &mut length) + MIN_MATCH;
                    let offset = get_leb128(&parts[3], &mut distance);
                    assert!(offset > 0 && offset <= output.len().min(WINDOW));
                    for _ in 0..count {
                        let byte = output[output.len() - offset];
                        output.push(byte);
                    }
                }
                _ => panic!("bad tag"),
            }
        }
        assert_eq!(literal, parts[1].len());
        assert_eq!(length, parts[2].len());
        assert_eq!(distance, parts[3].len());
        output
    }

    fn round_trip(data: &[u8]) {
        let parts = parse(data).expect("parse");
        assert_eq!(reconstruct(&parts), data);
    }

    #[test]
    fn tiny_inputs_reconstruct_exactly() {
        for data in [b"".as_slice(), b"a", b"abc", b"abcd", b"abcde"] {
            round_trip(data);
        }
    }

    #[test]
    fn repeated_data_reconstructs_exactly() {
        let data = b"the quick brown fox jumps over the quick brown fox jumps over ".repeat(1000);
        round_trip(&data);
    }

    #[test]
    fn runs_reconstruct_exactly() {
        let mut data = vec![0u8; 32_768];
        data.extend(std::iter::repeat_n(0xff, 32_768));
        round_trip(&data);
    }

    #[test]
    fn deterministic_random_reconstructs_exactly() {
        let mut state = 0x1234_5678u32;
        let mut data = Vec::with_capacity(65_536);
        for _ in 0..65_536 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            data.push((state >> 24) as u8);
        }
        round_trip(&data);
    }

    #[test]
    fn matching_resumes_after_a_random_prefix() {
        let mut state = 0x7f4a_7c15u32;
        let mut data = Vec::with_capacity(65_536);
        for _ in 0..16_384 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            data.push((state >> 24) as u8);
        }
        data.extend(b"CIX fast parser resumes matching after random data. ".repeat(900));
        let parts = parse(&data).expect("parse");
        assert!(
            parts[0].contains(&1),
            "expected a match after random prefix"
        );
        assert_eq!(reconstruct(&parts), data);
    }
}
