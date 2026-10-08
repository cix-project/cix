//! Hierarchical Colour-Index codec for legacy CIXM6 mode 18.
use crate::rank::{ArithmeticDecoder, ArithmeticEncoder};

fn read_v(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..usize::BITS as usize).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated wavelet varint")?;
        *pos += 1;
        let low = (byte & 0x7f) as usize;
        if low > (usize::MAX >> shift) {
            return Err("wavelet varint overflow".into());
        }
        value |= low
            .checked_shl(shift as u32)
            .ok_or("wavelet varint overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized wavelet varint".into())
}

fn put_v(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn encode_node(values: &[u8], depth: usize, encoder: &mut ArithmeticEncoder) {
    if values.is_empty() || depth == 8 {
        return;
    }
    let shift = 7 - depth;
    let ones = values
        .iter()
        .filter(|&&value| ((value >> shift) & 1) != 0)
        .count();

    // The child population is one of exactly n + 1 possibilities.
    encoder.encode(ones, ones + 1, values.len() + 1);

    let mut zeros = values.len() - ones;
    let mut remaining = values.len();
    for &value in values {
        if ((value >> shift) & 1) != 0 {
            encoder.encode(zeros, remaining, remaining);
        } else {
            encoder.encode(0, zeros, remaining);
            zeros -= 1;
        }
        remaining -= 1;
    }

    if ones == 0 || ones == values.len() {
        encode_node(values, depth + 1, encoder);
    } else {
        let mut zero_values = Vec::with_capacity(values.len() - ones);
        let mut one_values = Vec::with_capacity(ones);
        for &value in values {
            if ((value >> shift) & 1) == 0 {
                zero_values.push(value);
            } else {
                one_values.push(value);
            }
        }
        encode_node(&zero_values, depth + 1, encoder);
        encode_node(&one_values, depth + 1, encoder);
    }
}

/// Encode the canonical version-1 hierarchical Colour-Index payload.
///
/// The route has no internal candidate search. Work is bounded by eight tree
/// levels and temporary stable partitions are proportional to the input block.
pub fn encode(data: &[u8]) -> Vec<u8> {
    let mut encoder = ArithmeticEncoder::new();
    encode_node(data, 0, &mut encoder);
    let (payload, bit_length) = encoder.finish();
    let mut out = vec![1];
    put_v(bit_length, &mut out);
    out.extend_from_slice(&payload);
    out
}

fn node(
    n: usize,
    depth: usize,
    decoder: &mut ArithmeticDecoder<'_>,
    prefix: u8,
) -> Result<Vec<u8>, String> {
    if n == 0 {
        return Ok(Vec::new());
    }
    if depth == 8 {
        return Ok(vec![prefix; n]);
    }
    let ones = decode_child_count(n, decoder)?;
    let original_ones = ones;
    let original_zeros = n - ones;
    let (mut zero, mut one, mut remaining) = (original_zeros, original_ones, n);
    let mut bits = Vec::with_capacity(n);
    for _ in 0..n {
        bits.push(decode_bit(decoder, &mut zero, &mut one, &mut remaining));
    }
    let z = node(original_zeros, depth + 1, decoder, prefix << 1)?;
    let o = node(original_ones, depth + 1, decoder, (prefix << 1) | 1)?;
    Ok(interleave(bits, z, o, n))
}

fn decode_child_count(n: usize, decoder: &mut ArithmeticDecoder<'_>) -> Result<usize, String> {
    let total = n + 1;
    let ones = decoder.target(total);
    if ones > n {
        return Err("wavelet child count out of range".into());
    }
    decoder.update(ones, ones + 1, total);
    Ok(ones)
}

fn decode_bit(
    decoder: &mut ArithmeticDecoder<'_>,
    zero: &mut usize,
    one: &mut usize,
    remaining: &mut usize,
) -> u8 {
    let bit = if *zero == 0 {
        decoder.update(0, *one, *remaining);
        *one -= 1;
        1
    } else if *one == 0 || decoder.target(*remaining) < *zero {
        decoder.update(0, *zero, *remaining);
        *zero -= 1;
        0
    } else {
        decoder.update(*zero, *remaining, *remaining);
        *one -= 1;
        1
    };
    *remaining -= 1;
    bit
}

fn interleave(bits: Vec<u8>, zeros: Vec<u8>, ones: Vec<u8>, n: usize) -> Vec<u8> {
    let (mut zi, mut oi) = (0, 0);
    let mut out = Vec::with_capacity(n);
    for bit in bits {
        if bit == 1 {
            out.push(ones[oi]);
            oi += 1;
        } else {
            out.push(zeros[zi]);
            zi += 1;
        }
    }
    out
}

pub fn decode(data: &[u8], n: usize) -> Result<Vec<u8>, String> {
    if data.is_empty() || data[0] != 1 {
        return Err("unknown wavelet version".into());
    }
    let mut p = 1;
    let bits = read_v(data, &mut p)?;
    if p.checked_add(bits.div_ceil(8)) != Some(data.len()) {
        return Err("wavelet payload length mismatch".into());
    }
    let mut decoder = ArithmeticDecoder::new(&data[p..], bits);
    node(n, 0, &mut decoder, 0)
}

#[cfg(test)]
mod integer_overflow_tests {
    use super::*;

    #[test]
    fn canonical_payload_round_trips() {
        let input: Vec<u8> = (0..513).map(|index| (index * 37) as u8).collect();
        assert_eq!(decode(&encode(&input), input.len()).unwrap(), input);
    }

    #[test]
    fn varint_accepts_max_rejects_high_bits() {
        let mut value = usize::MAX;
        let mut encoded = Vec::new();
        while value >= 128 {
            encoded.push((value as u8 & 127) | 128);
            value >>= 7;
        }
        encoded.push(value as u8);
        assert_eq!(read_v(&encoded, &mut 0).unwrap(), usize::MAX);
        let groups = (usize::BITS as usize).div_ceil(7);
        let mut malformed = vec![128; groups - 1];
        malformed.push(1u8 << ((usize::BITS as usize - 1) % 7 + 1));
        assert!(read_v(&malformed, &mut 0).is_err());
    }
}
