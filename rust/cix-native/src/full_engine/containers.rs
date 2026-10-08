//! Byte-exact retained outer containers, independent of installed providers.
//!
//! Allocation limits here bound the returned buffer. The full-engine scheduler
//! separately reserves retained input and codec working state. No codec search,
//! filesystem lookup, environment setting, or child process occurs here.

use crate::paq_bridge::PaqVariant;
use sha2::{Digest, Sha256};

pub const RAW_MAGIC: &[u8; 5] = b"CIXR1";
pub const PAQ_FRAME_MAGIC: &[u8; 5] = b"CIXP\x01";
const PAQ_MEMBER_MAGIC: &[u8; 6] = b"paq8px";
const RAW_HEADER: usize = 5 + 8 + 32;

fn allocate(bytes: usize, limit: usize) -> Result<Vec<u8>, String> {
    if bytes > limit || bytes > isize::MAX as usize {
        return Err("outer-container allocation limit".into());
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(bytes)
        .map_err(|_| "outer-container allocation failed")?;
    Ok(result)
}

/// Encode the retained Python-runtime stored frame, including its SHA-256.
pub fn encode_raw(source: &[u8], allocation_limit: usize) -> Result<Vec<u8>, String> {
    let size = source
        .len()
        .checked_add(RAW_HEADER)
        .ok_or("raw frame size overflow")?;
    let length = u64::try_from(source.len()).map_err(|_| "raw source length overflow")?;
    let mut frame = allocate(size, allocation_limit)?;
    frame.extend_from_slice(RAW_MAGIC);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(&Sha256::digest(source));
    frame.extend_from_slice(source);
    Ok(frame)
}

/// Validate complete length and checksum before allocating a restored buffer.
pub fn decode_raw(
    frame: &[u8],
    output_limit: usize,
    allocation_limit: usize,
) -> Result<Vec<u8>, String> {
    if frame.len() < RAW_HEADER || !frame.starts_with(RAW_MAGIC) {
        return Err("invalid stored CIXR1 frame".into());
    }
    let length = usize::try_from(u64::from_le_bytes(frame[5..13].try_into().unwrap()))
        .map_err(|_| "stored CIXR1 length overflow")?;
    if length > output_limit || length > allocation_limit {
        return Err("stored CIXR1 output limit".into());
    }
    if length.checked_add(RAW_HEADER) != Some(frame.len()) {
        return Err("truncated or trailing stored CIXR1 bytes".into());
    }
    let source = &frame[RAW_HEADER..];
    if Sha256::digest(source).as_slice() != &frame[13..RAW_HEADER] {
        return Err("stored CIXR1 checksum mismatch".into());
    }
    let mut result = allocate(length, allocation_limit)?;
    result.extend_from_slice(source);
    Ok(result)
}

/// Replace the PAQ member signature using the existing v215/v216 wire IDs.
/// Joint-discount and store-state have distinct specialist envelopes; they must
/// not acquire invented CIXP IDs here.
pub fn wrap_paq(
    member: &[u8],
    variant: PaqVariant,
    allocation_limit: usize,
) -> Result<Vec<u8>, String> {
    let id = match variant {
        PaqVariant::V215 => 1,
        PaqVariant::V216 => 2,
        _ => return Err("variant has no historical CIXP identifier".into()),
    };
    if !member.starts_with(PAQ_MEMBER_MAGIC) {
        return Err("invalid PAQ member signature".into());
    }
    let mut frame = allocate(member.len(), allocation_limit)?;
    frame.extend_from_slice(PAQ_FRAME_MAGIC);
    frame.push(id);
    frame.extend_from_slice(&member[PAQ_MEMBER_MAGIC.len()..]);
    Ok(frame)
}

/// Restore the PAQ signature. The selected provider remains responsible for
/// validating and decoding its stream under worker memory/output limits.
pub fn unwrap_paq(frame: &[u8], allocation_limit: usize) -> Result<(PaqVariant, Vec<u8>), String> {
    if frame.len() < 6 || !frame.starts_with(PAQ_FRAME_MAGIC) {
        return Err("invalid CIXP frame".into());
    }
    let variant = match frame[5] {
        1 => PaqVariant::V215,
        2 => PaqVariant::V216,
        _ => return Err("unsupported CIXP backend identifier".into()),
    };
    let mut member = allocate(frame.len(), allocation_limit)?;
    member.extend_from_slice(PAQ_MEMBER_MAGIC);
    member.extend_from_slice(&frame[6..]);
    Ok((variant, member))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_historical_empty_frame_has_known_digest() {
        let frame = encode_raw(b"", RAW_HEADER).unwrap();
        assert_eq!(&frame[..13], b"CIXR1\0\0\0\0\0\0\0\0");
        assert_eq!(
            &frame[13..],
            &[
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
                0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
                0x78, 0x52, 0xb8, 0x55
            ]
        );
        assert_eq!(decode_raw(&frame, 0, 0).unwrap(), b"");
    }

    #[test]
    fn stored_frame_checks_length_checksum_and_limits_before_allocation() {
        let source = b"independently framed source";
        let frame = encode_raw(source, 1024).unwrap();
        assert_eq!(decode_raw(&frame, 1024, 1024).unwrap(), source);
        assert!(decode_raw(&frame, source.len() - 1, 1024).is_err());
        assert!(decode_raw(&frame, 1024, source.len() - 1).is_err());
        assert!(encode_raw(source, frame.len() - 1).is_err());
        let mut bad = frame.clone();
        bad[RAW_HEADER] ^= 1;
        assert!(decode_raw(&bad, 1024, 1024).is_err());
        bad = frame.clone();
        bad.push(0);
        assert!(decode_raw(&bad, 1024, 1024).is_err());
        assert!(decode_raw(&frame[..frame.len() - 1], 1024, 1024).is_err());
        bad = frame;
        bad[5..13].fill(255);
        assert!(decode_raw(&bad, 1024, 1024).is_err());
    }

    #[test]
    fn paq_identifiers_match_historical_wrapping_and_reject_unassigned_ids() {
        for (variant, id) in [(PaqVariant::V215, 1), (PaqVariant::V216, 2)] {
            let frame = wrap_paq(b"paq8px\x00\xfe\x01", variant, 9).unwrap();
            let mut expected = b"CIXP\x01".to_vec();
            expected.extend_from_slice(&[id, 0, 254, 1]);
            assert_eq!(frame, expected);
            assert_eq!(
                unwrap_paq(&frame, 9).unwrap(),
                (variant, b"paq8px\x00\xfe\x01".to_vec())
            );
            assert!(unwrap_paq(&frame, 8).is_err());
        }
        assert!(unwrap_paq(b"CIXP\x01\x03x", 32).is_err());
        assert!(wrap_paq(b"paq8pxx", PaqVariant::JointDiscount, 32).is_err());
        assert!(wrap_paq(b"broken", PaqVariant::V216, 32).is_err());
    }
}
