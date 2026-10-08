//! Historical CIXI1 JPEG-LS/JPEG 2000 spatial-image envelopes.
//!
//! This is the native counterpart of
//! `runtime/cix_runtime/legacy/spatial_image_codec.py`.  CIXI1 carries the
//! original length, region offset, region byte count, metadata length and
//! codec payload length, but deliberately has no stored image shape.  Shape is
//! therefore recovered only with the bounded native codestream probe.

use crate::full_engine::arithmetic::{vdecode, vencode};
use crate::full_engine::spatial_provider::{SpatialCodec, SpatialProvider};
use crate::full_engine::volume_lifting::DicomRegion;
use crc32fast::hash as crc32;

pub const SPATIAL_CIXI1_MAGIC: &[u8; 5] = b"CIXI\x01";

/// Metadata compression is injected so this framing remains independent of a
/// particular package bridge. Implementations must reject trailing or truncated
/// streams and must return exactly `expected` bytes on decode.
pub trait SpatialMetadataBackend {
    /// The returned buffer must be at most `max_output`; `memory` is the
    /// complete budget available to this call, including its source/output and
    /// codec working state, but excluding caller-retained CIX buffers.
    fn compress(&self, source: &[u8], max_output: usize, memory: usize) -> Result<Vec<u8>, String>;
    /// `memory` has already had all retained CIX buffers subtracted from it;
    /// this call must still charge its source/output/working state.
    fn decompress(&self, source: &[u8], expected: usize, memory: usize) -> Result<Vec<u8>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpatialFrameHeader {
    pub original_bytes: usize,
    pub region_offset: usize,
    pub region_bytes: usize,
    pub metadata_bytes: usize,
    pub payload_bytes: usize,
    pub checksum: u32,
    pub codec: SpatialCodec,
}

struct ParsedFrame<'a> {
    header: SpatialFrameHeader,
    metadata: &'a [u8],
    payload: &'a [u8],
}

fn checked_region(data: &[u8], region: &DicomRegion) -> Result<(), String> {
    let pixels = region
        .width
        .checked_mul(region.height)
        .ok_or("spatial dimensions overflow")?;
    let bytes = pixels.checked_mul(2).ok_or("spatial bytes overflow")?;
    if region.width == 0
        || region.height == 0
        || region.length != bytes
        || region
            .offset
            .checked_add(region.length)
            .is_none_or(|end| end > data.len())
    {
        return Err("invalid spatial region".into());
    }
    Ok(())
}

fn source_bits(codec: SpatialCodec, pixels: &[u8]) -> Result<u32, String> {
    if !pixels.len().is_multiple_of(2) {
        return Err("unaligned spatial samples".into());
    }
    // Historical imagecodecs JPEG-LS was passed a uint16 ndarray without an
    // explicit bitspersample argument, so its frame precision is 16. JPEG 2000
    // was explicitly given max(1, max_sample.bit_length()).
    if codec == SpatialCodec::JpegLs {
        return Ok(16);
    }
    let maximum = pixels
        .as_chunks::<2>()
        .0
        .iter()
        .map(|sample| u16::from_le_bytes([sample[0], sample[1]]))
        .max()
        .unwrap_or(0);
    Ok(u32::max(1, 16 - maximum.leading_zeros()))
}

fn mode_codec(mode: u8) -> Result<SpatialCodec, String> {
    match mode {
        2 => Ok(SpatialCodec::Jpeg2000),
        3 => Ok(SpatialCodec::JpegLs),
        _ => Err("unsupported spatial representation".into()),
    }
}

fn checked_add(left: usize, right: usize, error: &'static str) -> Result<usize, String> {
    left.checked_add(right).ok_or_else(|| error.into())
}

fn checked_sum(values: &[usize], error: &'static str) -> Result<usize, String> {
    values
        .iter()
        .try_fold(0usize, |total, value| total.checked_add(*value))
        .ok_or_else(|| error.into())
}

const FRAME_HEADER_MAX: usize = 5 + 1 + 5 * 10 + 4;

fn parse(frame: &[u8]) -> Result<ParsedFrame<'_>, String> {
    if frame.get(..5) != Some(SPATIAL_CIXI1_MAGIC) || frame.len() < 6 {
        return Err("invalid spatial frame".into());
    }
    let codec = mode_codec(frame[5])?;
    let mut pos = 6;
    let mut fields = [0usize; 5];
    for field in &mut fields {
        let (value, next) = vdecode(frame, pos)?;
        *field = usize::try_from(value).map_err(|_| "spatial field overflow")?;
        pos = next;
    }
    let checksum_at = pos;
    let metadata_at = checked_add(checksum_at, 4, "spatial frame overflow")?;
    let payload_at = checked_add(metadata_at, fields[3], "spatial frame overflow")?;
    let end = checked_add(payload_at, fields[4], "spatial frame overflow")?;
    if end != frame.len() {
        return Err("spatial framing mismatch".into());
    }
    let checksum = u32::from_be_bytes(
        frame
            .get(checksum_at..metadata_at)
            .ok_or("truncated spatial checksum")?
            .try_into()
            .map_err(|_| "truncated spatial checksum")?,
    );
    Ok(ParsedFrame {
        header: SpatialFrameHeader {
            original_bytes: fields[0],
            region_offset: fields[1],
            region_bytes: fields[2],
            metadata_bytes: fields[3],
            payload_bytes: fields[4],
            checksum,
            codec,
        },
        metadata: frame
            .get(metadata_at..payload_at)
            .ok_or("truncated spatial metadata")?,
        payload: frame
            .get(payload_at..end)
            .ok_or("truncated spatial payload")?,
    })
}

fn validate_decode_header(
    frame_bytes: usize,
    header: SpatialFrameHeader,
    maximum: usize,
    memory: usize,
) -> Result<(), String> {
    if header.original_bytes > maximum || header.original_bytes > memory {
        return Err("spatial output exceeds limit".into());
    }
    if header.region_bytes == 0 || !header.region_bytes.is_multiple_of(2) {
        return Err("invalid spatial region byte count".into());
    }
    if header
        .region_offset
        .checked_add(header.region_bytes)
        .is_none_or(|end| end > header.original_bytes)
    {
        return Err("spatial region exceeds original".into());
    }
    if header.metadata_bytes == 0 && header.original_bytes != header.region_bytes {
        return Err("missing spatial metadata".into());
    }
    // Before metadata decompression starts, its complete output and the
    // retained archive must fit. The metadata backend receives only the
    // remaining budget for its own source/output/working allocation.
    if checked_sum(
        &[frame_bytes, header.original_bytes - header.region_bytes],
        "spatial resource estimate overflow",
    )? > memory
    {
        return Err("spatial resource estimate exceeds limit".into());
    }
    Ok(())
}

/// Encode a representation-2 JPEG 2000 or representation-3 JPEG-LS CIXI1
/// envelope with exact historical field order and big-endian CRC32.
pub fn encode_spatial_frame(
    data: &[u8],
    region: &DicomRegion,
    codec: SpatialCodec,
    provider: &SpatialProvider,
    metadata: &impl SpatialMetadataBackend,
    max_frame_bytes: usize,
    memory: usize,
) -> Result<Vec<u8>, String> {
    checked_region(data, region)?;
    let metadata_source_len = data.len() - region.length;
    let payload_cap = max_frame_bytes
        .checked_sub(FRAME_HEADER_MAX)
        .ok_or("spatial frame output cap is too small")?
        .min(provider.max_output_bytes());
    // `data` remains caller-retained throughout this API. It is deliberately
    // charged here and in later stages, in addition to the provider's native
    // workspace/output contract.
    if data.len() > memory {
        return Err("spatial encode resource estimate exceeds limit".into());
    }
    let provider_memory = memory
        .checked_sub(data.len())
        .ok_or("spatial encode resource estimate exceeds limit")?;
    let pixels = &data[region.offset..region.offset + region.length];
    let bits = source_bits(codec, pixels)?;
    let payload = provider
        .encode_with_output_limit(
            codec,
            pixels,
            region.width,
            region.height,
            bits,
            payload_cap,
            provider_memory,
        )
        .map_err(|error| format!("spatial {codec:?} encode: {error}"))?;
    let metadata_cap = max_frame_bytes
        .checked_sub(FRAME_HEADER_MAX)
        .and_then(|n| n.checked_sub(payload.len()))
        .ok_or("spatial payload exceeds frame output cap")?;
    if metadata_source_len != 0 && metadata_cap == 0 {
        return Err("spatial metadata cannot fit frame output cap".into());
    }
    // Metadata compression begins only after native encode has released its
    // native output. Its source, payload Vec capacity and caller data coexist.
    let metadata_stage = checked_sum(
        &[
            data.len(),
            payload.capacity(),
            metadata_source_len,
            metadata_cap,
        ],
        "spatial encode budget overflow",
    )?;
    if metadata_stage > memory {
        return Err("spatial encode resource estimate exceeds limit".into());
    }
    let mut metadata_source = Vec::new();
    metadata_source
        .try_reserve_exact(metadata_source_len)
        .map_err(|_| "spatial metadata allocation failed")?;
    if checked_sum(
        &[
            data.len(),
            payload.capacity(),
            metadata_source.capacity(),
            metadata_cap,
        ],
        "spatial encode budget overflow",
    )? > memory
    {
        return Err("spatial encode resource estimate exceeds limit".into());
    }
    metadata_source.extend_from_slice(&data[..region.offset]);
    metadata_source.extend_from_slice(&data[region.offset + region.length..]);
    let metadata_memory = memory
        .checked_sub(checked_sum(
            &[data.len(), payload.capacity()],
            "spatial encode budget overflow",
        )?)
        .ok_or("spatial encode resource estimate exceeds limit")?;
    let metadata = metadata.compress(&metadata_source, metadata_cap, metadata_memory)?;
    if metadata.len() > metadata_cap || metadata.capacity() > metadata_cap {
        return Err("spatial metadata exceeds frame output cap".into());
    }
    drop(metadata_source);
    let frame_len = checked_sum(
        &[FRAME_HEADER_MAX, metadata.len(), payload.len()],
        "spatial frame output overflow",
    )?;
    if frame_len > max_frame_bytes
        || checked_sum(
            &[
                data.len(),
                metadata.capacity(),
                payload.capacity(),
                frame_len,
            ],
            "spatial encode resource estimate overflow",
        )? > memory
    {
        return Err("spatial frame output exceeds limit".into());
    }
    let mode = codec as u32 as u8;
    let mut frame = Vec::new();
    frame
        .try_reserve_exact(frame_len)
        .map_err(|_| "spatial frame allocation failed")?;
    if frame.capacity() > max_frame_bytes
        || checked_sum(
            &[
                data.len(),
                metadata.capacity(),
                payload.capacity(),
                frame.capacity(),
            ],
            "spatial encode resource estimate overflow",
        )? > memory
    {
        return Err("spatial frame output exceeds limit".into());
    }
    frame.extend_from_slice(SPATIAL_CIXI1_MAGIC);
    frame.push(mode);
    for field in [
        data.len(),
        region.offset,
        region.length,
        metadata.len(),
        payload.len(),
    ] {
        frame.extend_from_slice(&vencode(field as u64));
    }
    frame.extend_from_slice(&crc32(data).to_be_bytes());
    frame.extend_from_slice(&metadata);
    frame.extend_from_slice(&payload);
    if frame.len() > max_frame_bytes || frame.capacity() > max_frame_bytes {
        return Err("spatial frame output exceeds limit".into());
    }
    Ok(frame)
}

/// Decode representation 2 or 3 without guessing image dimensions. The
/// codestream's bounded probe must produce exactly the paid canonical u16 byte
/// count before native decode receives an output buffer.
pub fn decode_spatial_frame(
    frame: &[u8],
    maximum: usize,
    memory: usize,
    provider: &SpatialProvider,
    metadata: &impl SpatialMetadataBackend,
) -> Result<Vec<u8>, String> {
    let parsed = parse(frame)?;
    validate_decode_header(frame.len(), parsed.header, maximum, memory)?;
    let expected_metadata = parsed.header.original_bytes - parsed.header.region_bytes;
    let metadata_memory = memory
        .checked_sub(frame.len())
        .ok_or("spatial resource estimate exceeds limit")?;
    let restored_metadata =
        metadata.decompress(parsed.metadata, expected_metadata, metadata_memory)?;
    if restored_metadata.len() != expected_metadata {
        return Err("spatial metadata mismatch".into());
    }
    let info = provider
        .probe_embedded(
            parsed.header.codec,
            parsed.payload,
            parsed.header.region_bytes,
        )
        .map_err(|error| format!("spatial {:?} probe: {error}", parsed.header.codec))?;
    let expected_pixels = parsed.header.region_bytes / 2;
    if info.width == 0
        || info.height == 0
        || info
            .width
            .checked_mul(info.height)
            .is_none_or(|pixels| pixels != expected_pixels)
    {
        return Err("spatial codestream shape does not match paid region".into());
    }
    // The frame and restored metadata remain live throughout native decode.
    let provider_memory = memory
        .checked_sub(checked_sum(
            &[frame.len(), restored_metadata.capacity()],
            "spatial resource estimate overflow",
        )?)
        .ok_or("spatial resource estimate exceeds limit")?;
    let raw = provider
        .decode(
            parsed.header.codec,
            parsed.payload,
            info.width,
            info.height,
            info.bits_per_sample,
            provider_memory,
        )
        .map_err(|error| format!("spatial {:?} decode: {error}", parsed.header.codec))?;
    if raw.len() != parsed.header.region_bytes {
        return Err("spatial decoded byte count mismatch".into());
    }
    let final_live = checked_sum(
        &[
            frame.len(),
            restored_metadata.capacity(),
            raw.capacity(),
            parsed.header.original_bytes,
        ],
        "spatial resource estimate overflow",
    )?;
    if final_live > memory {
        return Err("spatial resource estimate exceeds limit".into());
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(parsed.header.original_bytes)
        .map_err(|_| "spatial output allocation failed")?;
    if checked_sum(
        &[
            frame.len(),
            restored_metadata.capacity(),
            raw.capacity(),
            output.capacity(),
        ],
        "spatial resource estimate overflow",
    )? > memory
    {
        return Err("spatial resource estimate exceeds limit".into());
    }
    output.extend_from_slice(&restored_metadata[..parsed.header.region_offset]);
    output.extend_from_slice(&raw);
    output.extend_from_slice(&restored_metadata[parsed.header.region_offset..]);
    if output.len() != parsed.header.original_bytes || crc32(&output) != parsed.header.checksum {
        return Err("spatial checksum mismatch".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_field_order_and_modes_parse_without_shape() {
        let mut frame = SPATIAL_CIXI1_MAGIC.to_vec();
        frame.push(2);
        for field in [8_u64, 2, 4, 1, 3] {
            frame.extend_from_slice(&vencode(field));
        }
        frame.extend_from_slice(&0x1020_3040_u32.to_be_bytes());
        frame.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);
        let parsed = parse(&frame).expect("historical frame");
        assert_eq!(parsed.header.codec, SpatialCodec::Jpeg2000);
        assert_eq!(parsed.header.original_bytes, 8);
        assert_eq!(parsed.header.region_offset, 2);
        assert_eq!(parsed.header.region_bytes, 4);
        assert_eq!(parsed.metadata, &[0xaa]);
        assert_eq!(parsed.payload, &[0xbb, 0xcc, 0xdd]);
    }

    #[test]
    fn framing_rejects_trailing_and_overflowed_region() {
        let mut frame = SPATIAL_CIXI1_MAGIC.to_vec();
        frame.push(3);
        for field in [4_u64, 3, 2, 0, 0] {
            frame.extend_from_slice(&vencode(field));
        }
        frame.extend_from_slice(&0_u32.to_be_bytes());
        assert!(parse(&frame).is_ok());
        let parsed = parse(&frame).unwrap();
        assert!(validate_decode_header(frame.len(), parsed.header, 4, 1 << 30).is_err());
        frame.push(0);
        assert!(parse(&frame).is_err());
    }

    #[test]
    fn source_precision_matches_historical_backends() {
        assert_eq!(
            source_bits(SpatialCodec::JpegLs, &[0, 0, 1, 0]).unwrap(),
            16
        );
        assert_eq!(
            source_bits(SpatialCodec::Jpeg2000, &[0, 0, 1, 0]).unwrap(),
            1
        );
        assert_eq!(
            source_bits(SpatialCodec::Jpeg2000, &[0xff, 0x0f]).unwrap(),
            12
        );
    }

    #[test]
    fn frozen_python_cixi1_members_retain_modes_and_container_bytes() {
        let jls = parse(include_bytes!(
            "../../tests/fixtures/spatial_frames/jls.cix"
        ))
        .expect("frozen JPEG-LS CIXI1 frame");
        assert_eq!(jls.header.codec, SpatialCodec::JpegLs);
        assert!(jls.payload.starts_with(&[0xff, 0xd8]));

        let j2k = parse(include_bytes!(
            "../../tests/fixtures/spatial_frames/j2k.cix"
        ))
        .expect("frozen JPEG 2000 CIXI1 frame");
        assert_eq!(j2k.header.codec, SpatialCodec::Jpeg2000);
        assert!(j2k
            .payload
            .starts_with(&[0, 0, 0, 12, b'j', b'P', b' ', b' ']));
    }

    #[test]
    fn packaged_bridge_crossdecodes_frozen_jls_and_jp2_regions() {
        let Some(path) = std::env::var_os("CIX_TEST_SPATIAL_BRIDGE") else {
            return;
        };
        let provider = SpatialProvider::load_package_bridge(std::path::Path::new(&path), 8 << 20)
            .expect("CIX_TEST_SPATIAL_BRIDGE must name the staged spatial bridge");
        let jls_frame = include_bytes!("../../tests/fixtures/spatial_frames/jls.cix").as_slice();
        let j2k_frame = include_bytes!("../../tests/fixtures/spatial_frames/j2k.cix").as_slice();
        let jls_source =
            include_bytes!("../../tests/fixtures/spatial_frames/jls-input.bin").as_slice();
        let j2k_source =
            include_bytes!("../../tests/fixtures/spatial_frames/j2k-input.bin").as_slice();
        let jls = parse(jls_frame).expect("frozen JLS CIXI1 frame");
        let j2k = parse(j2k_frame).expect("frozen JP2 CIXI1 frame");
        let raw_j2k = jp2c(j2k.payload).expect("frozen JP2 codestream box");
        for (codec, payload, source, offset, length) in [
            (
                SpatialCodec::JpegLs,
                jls.payload,
                jls_source,
                jls.header.region_offset,
                jls.header.region_bytes,
            ),
            (
                SpatialCodec::Jpeg2000,
                j2k.payload,
                j2k_source,
                j2k.header.region_offset,
                j2k.header.region_bytes,
            ),
            (
                SpatialCodec::Jpeg2000,
                raw_j2k,
                j2k_source,
                j2k.header.region_offset,
                j2k.header.region_bytes,
            ),
        ] {
            let info = provider
                .probe_embedded(codec, payload, length)
                .expect("bounded native shape probe");
            let raw = provider
                .decode(
                    codec,
                    payload,
                    info.width,
                    info.height,
                    info.bits_per_sample,
                    128 << 20,
                )
                .expect("native frozen payload decode");
            let end = offset + length;
            assert_eq!(raw, source[offset..end]);
        }
    }

    fn jp2c(payload: &[u8]) -> Option<&[u8]> {
        let mut at = 0usize;
        while at.checked_add(8)? <= payload.len() {
            let length = u32::from_be_bytes(payload.get(at..at + 4)?.try_into().ok()?) as usize;
            let kind = payload.get(at + 4..at + 8)?;
            if length < 8 || at.checked_add(length)? > payload.len() {
                return None;
            }
            if kind == b"jp2c" {
                return payload.get(at + 8..at + length);
            }
            at += length;
        }
        None
    }
}
