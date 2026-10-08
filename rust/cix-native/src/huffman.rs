//! Deterministic canonical Huffman substream framing.
//!
//! This module deliberately owns a complete substream: it never consumes
//! bytes beyond its declared bit length, and a decoder rejects both a
//! truncated payload and bytes appended after the payload.  It is not wired
//! into an archive route yet.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use num_bigint::BigUint;
use num_traits::{One, Zero};

pub const VERSION: u8 = 1;
const FIXED_HEADER: usize = 19; // version, symbol count, output length, bit length

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuffmanError(pub String);

impl std::fmt::Display for HuffmanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HuffmanError {}

type Result<T> = std::result::Result<T, HuffmanError>;

#[derive(Clone, Debug)]
struct TreeNode {
    left: Option<usize>,
    right: Option<usize>,
    symbol: Option<u8>,
}

struct FrameHeader<'a> {
    decoded_len: u64,
    bit_len: u64,
    metadata: &'a [u8],
    payload: &'a [u8],
}

fn parse_frame_header(frame: &[u8]) -> Result<FrameHeader<'_>> {
    if frame.len() < FIXED_HEADER {
        return Err(HuffmanError("truncated Huffman header".into()));
    }
    if frame[0] != VERSION {
        return Err(HuffmanError("unsupported Huffman version".into()));
    }
    let symbol_count = u16::from_le_bytes([frame[1], frame[2]]) as usize;
    if symbol_count > 256 {
        return Err(HuffmanError("symbol count exceeds byte alphabet".into()));
    }
    let decoded_len = u64::from_le_bytes(frame[3..11].try_into().unwrap());
    let bit_len = u64::from_le_bytes(frame[11..19].try_into().unwrap());
    let metadata_end = FIXED_HEADER
        .checked_add(
            symbol_count
                .checked_mul(2)
                .ok_or_else(|| HuffmanError("metadata size overflow".into()))?,
        )
        .ok_or_else(|| HuffmanError("metadata size overflow".into()))?;
    if frame.len() < metadata_end {
        return Err(HuffmanError("truncated Huffman code lengths".into()));
    }
    let payload_len = usize::try_from(bit_len.div_ceil(8))
        .map_err(|_| HuffmanError("declared payload does not fit this platform".into()))?;
    let expected_len = metadata_end
        .checked_add(payload_len)
        .ok_or_else(|| HuffmanError("declared frame size overflow".into()))?;
    if frame.len() != expected_len {
        return Err(HuffmanError(
            if frame.len() < expected_len {
                "truncated Huffman payload"
            } else {
                "trailing bytes after Huffman payload"
            }
            .into(),
        ));
    }
    Ok(FrameHeader {
        decoded_len,
        bit_len,
        metadata: &frame[FIXED_HEADER..metadata_end],
        payload: &frame[metadata_end..],
    })
}

fn read_lengths(metadata: &[u8]) -> Result<[u8; 256]> {
    let mut lengths = [0u8; 256];
    let mut previous = None;
    let (pairs, remainder) = metadata.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(HuffmanError("truncated Huffman code lengths".into()));
    }
    for pair in pairs {
        let (symbol, length) = (pair[0], pair[1]);
        if length == 0 {
            return Err(HuffmanError("zero-length code is invalid".into()));
        }
        if previous.is_some_and(|last| symbol <= last) {
            return Err(HuffmanError("code lengths are not symbol-sorted".into()));
        }
        previous = Some(symbol);
        lengths[symbol as usize] = length;
    }
    Ok(lengths)
}

fn validate_padding(payload: &[u8], bit_len: u64) -> Result<()> {
    if !bit_len.is_multiple_of(8) && !payload.is_empty() {
        let unused = 8 - (bit_len % 8) as u8;
        if payload[payload.len() - 1] & ((1u8 << unused) - 1) != 0 {
            return Err(HuffmanError("nonzero Huffman padding bits".into()));
        }
    }
    Ok(())
}

fn decode_payload(
    payload: &[u8],
    bit_len: u64,
    trie: &[TreeNode],
    output_len: usize,
) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(output_len);
    let mut node = 0usize;
    for bit_index in 0..bit_len {
        let byte = payload[(bit_index / 8) as usize];
        let bit = (byte >> (7 - (bit_index % 8) as u8)) & 1;
        node = if bit == 0 {
            trie[node].left
        } else {
            trie[node].right
        }
        .ok_or_else(|| HuffmanError("bit stream does not match Huffman table".into()))?;
        if let Some(symbol) = trie[node].symbol {
            if output.len() == output_len {
                return Err(HuffmanError("decoded more bytes than declared".into()));
            }
            output.push(symbol);
            node = 0;
        }
    }
    if node != 0 {
        return Err(HuffmanError("Huffman bit stream ends mid-code".into()));
    }
    if output.len() != output_len {
        return Err(HuffmanError(
            "decoded byte count differs from declared length".into(),
        ));
    }
    Ok(output)
}

/// Encodes one complete, self-delimiting Huffman substream.
///
/// Wire format, all integers little-endian:
/// `version:u8, symbols:u16, decoded_bytes:u64, encoded_bits:u64,`
/// followed by `symbols` sorted `(symbol:u8, code_length:u8)` pairs and
/// exactly `ceil(encoded_bits / 8)` bytes.  The unused low bits of the final
/// byte are zero.  Codewords are canonical, MSB first.
pub fn encode(input: &[u8]) -> Result<Vec<u8>> {
    let mut frequency = [0u64; 256];
    for &byte in input {
        frequency[byte as usize] = frequency[byte as usize]
            .checked_add(1)
            .ok_or_else(|| HuffmanError("symbol frequency overflow".into()))?;
    }
    let lengths = build_lengths(&frequency)?;
    let table = canonical_codes(&lengths)?;

    let symbols: Vec<(u8, u8)> = (0u16..=255)
        .filter_map(|symbol| {
            let length = lengths[symbol as usize];
            (length != 0).then_some((symbol as u8, length))
        })
        .collect();
    let mut bit_len = 0u64;
    for &byte in input {
        bit_len = bit_len
            .checked_add(lengths[byte as usize] as u64)
            .ok_or_else(|| HuffmanError("encoded bit length overflow".into()))?;
    }

    let payload_bytes = usize::try_from(bit_len.div_ceil(8))
        .map_err(|_| HuffmanError("encoded payload does not fit this platform".into()))?;
    let header_bytes = FIXED_HEADER
        .checked_add(
            symbols
                .len()
                .checked_mul(2)
                .ok_or_else(|| HuffmanError("header size overflow".into()))?,
        )
        .ok_or_else(|| HuffmanError("header size overflow".into()))?;
    let total_bytes = header_bytes
        .checked_add(payload_bytes)
        .ok_or_else(|| HuffmanError("encoded stream size overflow".into()))?;
    let mut out = Vec::with_capacity(total_bytes);
    out.push(VERSION);
    out.extend_from_slice(&(symbols.len() as u16).to_le_bytes());
    out.extend_from_slice(&(input.len() as u64).to_le_bytes());
    out.extend_from_slice(&bit_len.to_le_bytes());
    for &(symbol, length) in &symbols {
        out.extend_from_slice(&[symbol, length]);
    }

    let mut writer = BitWriter::default();
    for &byte in input {
        let code = &table[byte as usize];
        for &bit in code {
            writer.push(bit);
        }
    }
    debug_assert_eq!(writer.bit_len, bit_len);
    out.extend_from_slice(&writer.finish());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_and_trailing_frames() {
        let encoded = encode(b"canonical huffman frame with several symbols").unwrap();
        assert!(decode(&encoded[..encoded.len() - 1]).is_err());
        let mut trailing = encoded;
        trailing.push(0);
        assert!(decode(&trailing).is_err());
    }
}

/// Decodes one complete Huffman substream and rejects malformed framing.
pub fn decode(frame: &[u8]) -> Result<Vec<u8>> {
    let header = parse_frame_header(frame)?;
    let lengths = read_lengths(header.metadata)?;
    validate_table(
        &lengths,
        header.metadata.len() / 2,
        header.decoded_len,
        header.bit_len,
    )?;
    let codes = canonical_codes(&lengths)?;
    let trie = build_trie(&codes)?;
    validate_padding(header.payload, header.bit_len)?;
    let output_len = usize::try_from(header.decoded_len)
        .map_err(|_| HuffmanError("declared output does not fit this platform".into()))?;
    decode_payload(header.payload, header.bit_len, &trie, output_len)
}

fn build_lengths(frequency: &[u64; 256]) -> Result<[u8; 256]> {
    let mut nodes = Vec::new();
    let mut heap = BinaryHeap::new();
    for (symbol, &weight) in frequency.iter().enumerate() {
        if weight != 0 {
            let index = nodes.len();
            nodes.push(TreeNode {
                left: None,
                right: None,
                symbol: Some(symbol as u8),
            });
            heap.push(Reverse((weight, symbol as u16, index)));
        }
    }
    let mut lengths = [0u8; 256];
    if heap.is_empty() {
        return Ok(lengths);
    }
    if heap.len() == 1 {
        let Reverse((_, _, only)) = heap.pop().unwrap();
        lengths[nodes[only].symbol.unwrap() as usize] = 1;
        return Ok(lengths);
    }
    let mut ordinal = 256u16;
    while heap.len() > 1 {
        let Reverse((left_weight, _, left)) = heap.pop().unwrap();
        let Reverse((right_weight, _, right)) = heap.pop().unwrap();
        let weight = left_weight
            .checked_add(right_weight)
            .ok_or_else(|| HuffmanError("Huffman tree weight overflow".into()))?;
        let index = nodes.len();
        nodes.push(TreeNode {
            left: Some(left),
            right: Some(right),
            symbol: None,
        });
        heap.push(Reverse((weight, ordinal, index)));
        ordinal = ordinal
            .checked_add(1)
            .ok_or_else(|| HuffmanError("Huffman tie ordinal overflow".into()))?;
    }
    let root = heap.pop().unwrap().0 .2;
    let mut stack = vec![(root, 0u16)];
    while let Some((index, depth)) = stack.pop() {
        let item = &nodes[index];
        if let Some(symbol) = item.symbol {
            if depth == 0 || depth > u8::MAX as u16 {
                return Err(HuffmanError("invalid Huffman code depth".into()));
            }
            lengths[symbol as usize] = depth as u8;
        } else {
            let next = depth
                .checked_add(1)
                .ok_or_else(|| HuffmanError("Huffman depth overflow".into()))?;
            stack.push((item.right.unwrap(), next));
            stack.push((item.left.unwrap(), next));
        }
    }
    Ok(lengths)
}

fn validate_table(
    lengths: &[u8; 256],
    declared_symbols: usize,
    decoded_len: u64,
    bit_len: u64,
) -> Result<()> {
    let present: Vec<u8> = lengths
        .iter()
        .copied()
        .filter(|&length| length != 0)
        .collect();
    if present.len() != declared_symbols {
        return Err(HuffmanError(
            "declared symbol count disagrees with lengths".into(),
        ));
    }
    if present.is_empty() {
        if decoded_len != 0 || bit_len != 0 {
            return Err(HuffmanError("empty table cannot encode data".into()));
        }
        return Ok(());
    }
    if decoded_len == 0 {
        return Err(HuffmanError("nonempty table for empty output".into()));
    }
    if present.len() == 1 {
        if present[0] != 1 || bit_len != decoded_len {
            return Err(HuffmanError("invalid single-symbol Huffman stream".into()));
        }
        return Ok(());
    }
    let max_length = *present.iter().max().unwrap();
    let mut used = BigUint::zero();
    for &length in &present {
        used += BigUint::one() << (max_length - length) as usize;
    }
    let capacity = BigUint::one() << max_length as usize;
    if used > capacity {
        return Err(HuffmanError("oversubscribed Huffman table".into()));
    }
    if used < capacity {
        return Err(HuffmanError("incomplete Huffman table".into()));
    }
    let min_length = *present.iter().min().unwrap() as u64;
    let max_length = max_length as u64;
    if bit_len
        < decoded_len
            .checked_mul(min_length)
            .ok_or_else(|| HuffmanError("declared output length overflow".into()))?
        || bit_len
            > decoded_len
                .checked_mul(max_length)
                .ok_or_else(|| HuffmanError("declared output length overflow".into()))?
    {
        return Err(HuffmanError(
            "bit length is impossible for declared output".into(),
        ));
    }
    Ok(())
}

fn canonical_codes(lengths: &[u8; 256]) -> Result<Vec<Vec<bool>>> {
    let mut entries: Vec<(u8, u8)> = (0u16..=255)
        .filter_map(|symbol| {
            let length = lengths[symbol as usize];
            (length != 0).then_some((symbol as u8, length))
        })
        .collect();
    entries.sort_unstable_by_key(|&(symbol, length)| (length, symbol));
    let mut result = vec![Vec::new(); 256];
    let mut current: Vec<bool> = Vec::new();
    let mut previous_length = 0u8;
    for (position, (symbol, length)) in entries.into_iter().enumerate() {
        if position == 0 {
            current.resize(length as usize, false);
        } else {
            increment_bits(&mut current)?;
            if length < previous_length {
                return Err(HuffmanError("nonmonotonic canonical code lengths".into()));
            }
            current.resize(length as usize, false);
        }
        result[symbol as usize] = current.clone();
        previous_length = length;
    }
    Ok(result)
}

fn increment_bits(bits: &mut [bool]) -> Result<()> {
    for bit in bits.iter_mut().rev() {
        if !*bit {
            *bit = true;
            return Ok(());
        }
        *bit = false;
    }
    Err(HuffmanError(
        "oversubscribed canonical Huffman codes".into(),
    ))
}

fn build_trie(codes: &[Vec<bool>]) -> Result<Vec<TreeNode>> {
    let mut nodes = vec![TreeNode {
        left: None,
        right: None,
        symbol: None,
    }];
    for (symbol, code) in codes.iter().enumerate() {
        if code.is_empty() {
            continue;
        }
        insert_code(&mut nodes, code, symbol as u8)?;
    }
    Ok(nodes)
}

fn insert_code(nodes: &mut Vec<TreeNode>, code: &[bool], symbol: u8) -> Result<()> {
    let mut index = 0;
    for (position, &bit) in code.iter().enumerate() {
        if nodes[index].symbol.is_some() {
            return Err(HuffmanError(
                "Huffman code is a prefix of another code".into(),
            ));
        }
        let next = child_or_insert(nodes, index, bit);
        index = next;
        if position + 1 == code.len() {
            set_symbol(nodes, index, symbol)?;
        }
    }
    Ok(())
}

fn child_or_insert(nodes: &mut Vec<TreeNode>, index: usize, bit: bool) -> usize {
    let child = if bit {
        nodes[index].right
    } else {
        nodes[index].left
    };
    child.unwrap_or_else(|| {
        let next = nodes.len();
        nodes.push(TreeNode {
            left: None,
            right: None,
            symbol: None,
        });
        if bit {
            nodes[index].right = Some(next);
        } else {
            nodes[index].left = Some(next);
        }
        next
    })
}

fn set_symbol(nodes: &mut [TreeNode], index: usize, symbol: u8) -> Result<()> {
    let node = &mut nodes[index];
    if node.left.is_some() || node.right.is_some() || node.symbol.is_some() {
        return Err(HuffmanError("duplicate or prefix Huffman code".into()));
    }
    node.symbol = Some(symbol);
    Ok(())
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    partial: u8,
    used: u8,
    bit_len: u64,
}

impl BitWriter {
    fn push(&mut self, bit: bool) {
        self.partial = (self.partial << 1) | bit as u8;
        self.used += 1;
        self.bit_len += 1;
        if self.used == 8 {
            self.bytes.push(self.partial);
            self.partial = 0;
            self.used = 0;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.used != 0 {
            self.bytes.push(self.partial << (8 - self.used));
        }
        self.bytes
    }
}
