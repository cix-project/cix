//! Historic chemical-specialist admission and CIXG/CIXQ envelopes.
//!
//! These envelopes deliberately do not identify a backend by an invented
//! numeric ID: the payload is the historic raw `PAQ_MAGIC` member supplied by
//! the caller.  The full engine selects a provider only after it has proved it
//! can reproduce that member byte-for-byte.

use super::arithmetic::{vdecode, vencode, verify_frame};

pub const GEOMETRY_MAGIC: &[u8] = b"CIXG\x01";
pub const HYDROGEN_MAGIC: &[u8] = b"CIXQ\x01";
pub const PAQ_MAGIC: &[u8] = b"paq8px";
pub const PAQ_V215_8_BASE_MEMORY: usize = 4_500_000_000;

pub type EncodeBackend<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a;
pub type DecodeBackend<'a> = dyn Fn(&[u8], usize) -> Result<Vec<u8>, String> + 'a;

/// A marker is only an inexpensive admission preflight.  The transform must
/// still preserve malformed or unsupported records as literal records.
pub fn admit_sdf_geometry(data: &[u8]) -> bool {
    data.windows(5).any(|v| v == b"V2000") && data.windows(6).any(|v| v == b"M  END")
}
pub fn admit_sdf_hydrogen(data: &[u8]) -> bool {
    admit_sdf_geometry(data)
}

pub fn paq_memory_requirement(source_size: usize) -> Result<usize, String> {
    PAQ_V215_8_BASE_MEMORY
        .checked_add(
            source_size
                .checked_mul(8)
                .ok_or("PAQ source size overflow")?,
        )
        .ok_or_else(|| "PAQ memory requirement overflow".into())
}

fn pack(
    magic: &[u8],
    source: &[u8],
    transformed: &[u8],
    encode: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    let payload = encode(transformed)?;
    if !payload.starts_with(PAQ_MAGIC) {
        return Err("chemical backend did not return a historic paq8px member".into());
    }
    let mut out = Vec::with_capacity(magic.len() + 16 + payload.len());
    out.extend_from_slice(magic);
    out.extend(vencode(source.len() as u64));
    out.extend_from_slice(&crc32fast::hash(source).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

fn unpack(
    magic: &[u8],
    frame: &[u8],
    limit: usize,
    memory: usize,
    decode: &DecodeBackend<'_>,
) -> Result<(usize, Vec<u8>), String> {
    if !frame.starts_with(magic) || memory == 0 {
        return Err("invalid legacy specialist frame".into());
    }
    let (size, at) = vdecode(frame, magic.len())?;
    let size = usize::try_from(size).map_err(|_| "legacy source size overflow")?;
    if size > limit || size > memory {
        return Err("legacy specialist output exceeds admission limit".into());
    }
    if memory < paq_memory_requirement(size)? {
        return Err("PAQ decode memory admission limit".into());
    }
    let checksum = frame
        .get(at..at + 4)
        .ok_or("truncated legacy specialist frame")?;
    let checksum = u32::from_be_bytes(checksum.try_into().unwrap());
    let payload = frame
        .get(at + 4..)
        .ok_or("truncated legacy specialist payload")?;
    if !payload.starts_with(PAQ_MAGIC) {
        return Err("invalid historic PAQ payload".into());
    }
    Ok((size, {
        let raw = decode(payload, memory)?;
        if raw.len() > memory {
            return Err("backend ignored transformed-output limit".into());
        }
        let _ = checksum;
        raw
    }))
}

/// Encode a transformed geometry member with the exact historic CIXG01 layout.
pub fn pack_geometry(
    source: &[u8],
    transformed: &[u8],
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    pack(GEOMETRY_MAGIC, source, transformed, backend)
}
pub fn encode_geometry(
    source: &[u8],
    transformed: &[u8],
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    pack_geometry(source, transformed, backend)
}
pub fn encode_hydrogen(
    source: &[u8],
    transformed: &[u8],
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    pack(HYDROGEN_MAGIC, source, transformed, backend)
}

/// Decode CIXG01 after the caller selects the exact SDF geometry inverse.
pub fn decode_geometry(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
    inverse: impl FnOnce(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    let (size, transformed) = unpack(GEOMETRY_MAGIC, frame, limit, memory, backend)?;
    let (_, at) = vdecode(frame, GEOMETRY_MAGIC.len())?;
    let checksum = u32::from_be_bytes(frame[at..at + 4].try_into().unwrap());
    let restored = inverse(&transformed)?;
    verify_frame(&restored, size, checksum)?;
    Ok(restored)
}
/// Decode CIXQ01 after the caller selects the exact SDF hydrogen inverse.
pub fn decode_hydrogen(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
    inverse: impl FnOnce(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    let (size, transformed) = unpack(HYDROGEN_MAGIC, frame, limit, memory, backend)?;
    let (_, at) = vdecode(frame, HYDROGEN_MAGIC.len())?;
    let checksum = u32::from_be_bytes(frame[at..at + 4].try_into().unwrap());
    let restored = inverse(&transformed)?;
    verify_frame(&restored, size, checksum)?;
    Ok(restored)
}
