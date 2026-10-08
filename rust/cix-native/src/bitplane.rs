//! Legacy CIXM6 bit-plane block codec (exact-rank v1 decode and arithmetic v2).
use crate::rank::{self, ArithmeticDecoder, ArithmeticEncoder};

fn read_v(data: &[u8], p: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..usize::BITS as usize).step_by(7) {
        let byte = *data.get(*p).ok_or("truncated bit-plane varint")?;
        *p += 1;
        let low = (byte & 0x7f) as usize;
        let remaining_bits = usize::BITS as usize - shift;
        if remaining_bits < 7 && low >> remaining_bits != 0 {
            return Err("bit-plane varint overflow".into());
        }
        value |= low
            .checked_shl(shift as u32)
            .ok_or("bit-plane varint overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized bit-plane varint".into())
}

fn put_v(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Encode the canonical version-2 bit-plane payload used by the frozen Python
/// reference. Effort is fixed: eight counting and eight coding passes over the
/// caller's already bounded block, with no candidate search or retained model.
pub fn encode(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut out = Vec::with_capacity(18);
    out.push(2);
    let mut plane_counts = [0usize; 8];

    for (plane, shift) in (0..8).rev().enumerate() {
        let ones = data
            .iter()
            .map(|byte| ((byte >> shift) & 1) as usize)
            .sum::<usize>();
        let majority = ones > n / 2;
        out.push(majority as u8);
        put_v(if majority { n - ones } else { ones }, &mut out);
        plane_counts[plane] = ones;
    }

    let mut encoder = ArithmeticEncoder::new();
    for (plane, shift) in (0..8).rev().enumerate() {
        let mut remaining_ones = plane_counts[plane];
        let mut remaining_zeros = n - remaining_ones;
        for &byte in data {
            let total = remaining_zeros + remaining_ones;
            if (byte >> shift) & 1 != 0 {
                encoder.encode(remaining_zeros, total, total);
                remaining_ones -= 1;
            } else {
                encoder.encode(0, remaining_zeros, total);
                remaining_zeros -= 1;
            }
        }
    }
    let (payload, bit_length) = encoder.finish();
    put_v(bit_length, &mut out);
    out.extend_from_slice(&payload);
    out
}

pub fn decode(data: &[u8], n: usize) -> Result<Vec<u8>, String> {
    if data.is_empty() || !matches!(data[0], 1 | 2) {
        return Err("unknown bit-plane version".into());
    }
    let version = data[0];
    if version == 1 {
        return decode_rank_planes(data, n);
    }
    decode_arithmetic_planes(data, n)
}

fn plane_header(data: &[u8], p: &mut usize, n: usize) -> Result<(u8, usize), String> {
    let majority = *data.get(*p).ok_or("truncated bit-plane header")?;
    *p += 1;
    if majority > 1 {
        return Err("invalid bit-plane majority".into());
    }
    let minority = read_v(data, p)?;
    if minority > n {
        return Err("invalid bit-plane minority".into());
    }
    Ok((majority, minority))
}

fn decode_rank_planes(data: &[u8], n: usize) -> Result<Vec<u8>, String> {
    let mut p = 1;
    let mut out = vec![0u8; n];
    for shift in (0..8).rev() {
        let (majority, k) = plane_header(data, &mut p, n)?;
        let positions = rank::decode_colex_positions(data, &mut p, n, k)?;
        apply_rank_plane(&mut out, shift, majority, positions);
    }
    if p != data.len() {
        return Err("trailing bit-plane data".into());
    }
    Ok(out)
}

fn apply_rank_plane(out: &mut [u8], shift: usize, majority: u8, positions: Vec<usize>) {
    if majority == 1 {
        for byte in out.iter_mut() {
            *byte |= 1 << shift;
        }
        for index in positions {
            out[index] &= !(1 << shift);
        }
    } else {
        for index in positions {
            out[index] |= 1 << shift;
        }
    }
}

fn decode_arithmetic_planes(data: &[u8], n: usize) -> Result<Vec<u8>, String> {
    let mut p = 1;
    let mut ones = Vec::with_capacity(8);
    for _ in 0..8 {
        let (majority, minority) = plane_header(data, &mut p, n)?;
        ones.push(if majority == 1 {
            n - minority
        } else {
            minority
        });
    }
    let bit_length = read_v(data, &mut p)?;
    let payload_bytes = bit_length.div_ceil(8);
    let payload_end = p
        .checked_add(payload_bytes)
        .ok_or("bit-plane arithmetic payload length overflow")?;
    if payload_end != data.len() {
        return Err("bit-plane arithmetic payload length mismatch".into());
    }
    let mut decoder = ArithmeticDecoder::new(&data[p..], bit_length);
    let mut out = vec![0u8; n];
    for (plane, &one_count) in ones.iter().enumerate() {
        decode_arithmetic_plane(&mut decoder, &mut out, plane, one_count);
    }
    Ok(out)
}

fn decode_arithmetic_plane(
    decoder: &mut ArithmeticDecoder<'_>,
    out: &mut [u8],
    plane: usize,
    one_count: usize,
) {
    let (mut one, mut zero) = (one_count, out.len() - one_count);
    for byte in out {
        let total = one + zero;
        if decoder.target(total) < zero {
            decoder.update(0, zero, total);
            zero -= 1;
        } else {
            decoder.update(zero, total, total);
            one -= 1;
            *byte |= 1 << (7 - plane);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_planes_round_trip_and_reject_truncation() {
        let input: Vec<u8> = (0..513).map(|index| (index * 37) as u8).collect();
        let encoded = encode(&input);
        assert_eq!(decode(&encoded, input.len()).unwrap(), input);
        assert!(decode(&encoded[..encoded.len() - 1], input.len()).is_err());
    }

    #[test]
    fn maximal_arithmetic_bit_length_is_rejected_without_overflow() {
        let mut encoded = vec![2];
        // Eight valid zero-minority plane headers, followed by usize::MAX as
        // the arithmetic bit length. This exercises the ceiling division at
        // the host maximum: the prior `bit_length + 7` expression overflowed
        // before the decoder could reject the malformed payload.
        encoded.extend([0, 0].repeat(8));
        put_v(usize::MAX, &mut encoded);
        assert_eq!(
            decode(&encoded, 0).unwrap_err(),
            "bit-plane arithmetic payload length mismatch"
        );
    }

    #[test]
    fn varint_maximum_round_trips_canonically() {
        let mut encoded = Vec::new();
        put_v(usize::MAX, &mut encoded);
        let mut position = 0;
        assert_eq!(read_v(&encoded, &mut position).unwrap(), usize::MAX);
        assert_eq!(position, encoded.len());
    }

    #[test]
    fn varint_rejects_high_bits_in_final_group() {
        let groups = (usize::BITS as usize).div_ceil(7);
        let final_bits = usize::BITS as usize - 7 * (groups - 1);
        let mut encoded = vec![0xff; groups - 1];
        encoded.push(1 << final_bits);
        let mut position = 0;
        assert_eq!(
            read_v(&encoded, &mut position).unwrap_err(),
            "bit-plane varint overflow"
        );
    }
}
