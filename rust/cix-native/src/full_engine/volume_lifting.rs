//! CIX-owned DICOM discovery and reversible CIXV1 volume layout transforms.
//!
//! JPEG XL and LZMA are deliberately injected at the archive boundary.  This
//! keeps parsing, geometry and framing native while a pinned codec adapter owns
//! its library lifetime and resource limits.

use crate::full_engine::arithmetic::{vdecode, vencode};
use crc32fast::hash as crc32;
use sha2::{Digest, Sha256};

pub const SPATIAL_MAGIC: &[u8; 5] = b"CIXI\x01";
pub const VOLUME_MAGIC: &[u8; 5] = b"CIXV\x01";
pub const MULTISCALE_MAGIC: &[u8; 5] = b"CIXI\x02";
const CODEC_OVERHEAD: usize = 64 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DicomRegion {
    pub offset: usize,
    pub length: usize,
    pub width: usize,
    pub height: usize,
    pub slice_height: usize,
}

/// The complete archive chosen between the equivalent mode-3 CIXV1 and
/// zero-lifting CIXI2 envelopes. The shared JPEG XL payload is encoded once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedIdentityArchive {
    pub archive: Vec<u8>,
    pub report: SharedIdentityReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedIdentityFormat {
    CixV1,
    CixI2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedIdentityReport {
    pub cixv1_logical_bytes: usize,
    pub cixi2_logical_bytes: usize,
    pub selected_format: SharedIdentityFormat,
    pub payload_sha256: [u8; 32],
    pub shared_current_encode: bool,
}

fn le16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn le32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// Recognize the bounded implicit-VR DICOM subset used by the historical
/// image routes. Invalid/truncated structures are a non-match, never an error.
pub fn dicom_regions(data: &[u8]) -> Vec<DicomRegion> {
    let mut pos = 0usize;
    let mut rows = 0usize;
    let mut columns = 0usize;
    let mut bits = 0usize;
    while pos.checked_add(8).is_some_and(|end| end <= data.len()) && pos < (1 << 20) {
        let Some(group) = le16(data, pos) else { break };
        let Some(element) = le16(data, pos + 2) else {
            break;
        };
        let Some(length) = le32(data, pos + 4) else {
            break;
        };
        let start = pos + 8;
        let Ok(length) = usize::try_from(length) else {
            break;
        };
        let Some(end) = start.checked_add(length) else {
            break;
        };
        if length == u32::MAX as usize || end > data.len() {
            break;
        }
        match (group, element) {
            (0x0028, 0x0010) if length == 2 => rows = le16(data, start).unwrap_or(0) as usize,
            (0x0028, 0x0011) if length == 2 => columns = le16(data, start).unwrap_or(0) as usize,
            (0x0028, 0x0100) if length == 2 => bits = le16(data, start).unwrap_or(0) as usize,
            (0x7fe0, 0x0010) => {
                let pixels_per_slice = rows.checked_mul(columns).and_then(|n| n.checked_mul(2));
                if bits == 16
                    && rows > 0
                    && columns > 0
                    && pixels_per_slice.is_some_and(|n| length.is_multiple_of(n))
                {
                    return vec![DicomRegion {
                        offset: start,
                        length,
                        width: columns,
                        height: length / (columns * 2),
                        slice_height: rows,
                    }];
                }
                break;
            }
            _ => {}
        }
        pos = end;
    }
    Vec::new()
}

fn samples(bytes: &[u8]) -> Result<Vec<u16>, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("unaligned u16 image region".into());
    }
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect())
}

fn sample_bytes(values: &[u16]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn index(depth: usize, row: usize, column: usize, height: usize, width: usize) -> usize {
    (depth * height + row) * width + column
}

fn checked_shape_count(shape: [usize; 3], label: &str) -> Result<usize, String> {
    if shape.contains(&0) {
        return Err(format!("{label} has a zero dimension"));
    }
    shape[0]
        .checked_mul(shape[1])
        .and_then(|count| count.checked_mul(shape[2]))
        .ok_or_else(|| format!("{label} dimensions overflow"))
}

fn split_shapes(shape: [usize; 3], axis: usize) -> Result<([usize; 3], [usize; 3]), String> {
    if axis >= shape.len() {
        return Err("multiscale axis out of range".into());
    }
    checked_shape_count(shape, "multiscale shape")?;
    let mut low_shape = shape;
    low_shape[axis] = shape[axis]
        .checked_add(1)
        .ok_or("multiscale dimensions overflow")?
        / 2;
    let mut high_shape = shape;
    high_shape[axis] = shape[axis] / 2;
    Ok((low_shape, high_shape))
}

fn zigzag(value: i64) -> u16 {
    // Interpret the residual modulo 2^16 as signed before applying zigzag.
    let signed = i64::from(value as u16 as i16);
    ((signed << 1) ^ (signed >> 15)) as u16
}
fn unzigzag(value: u16) -> i64 {
    let value = value as i64;
    (value >> 1) ^ -(value & 1)
}

/// Exact CIXV1 transform output, serialized as little-endian u16 values.
pub fn volume_transform(
    source: &[u8],
    depth: usize,
    height: usize,
    width: usize,
    mode: u8,
) -> Result<Vec<u8>, String> {
    let values = samples(source)?;
    let count = depth
        .checked_mul(height)
        .and_then(|n| n.checked_mul(width))
        .ok_or("volume dimensions overflow")?;
    if values.len() != count {
        return Err("volume length mismatch".into());
    }
    let mut out = vec![0u16; count];
    match mode {
        0 | 3 => out.copy_from_slice(&values),
        1 => {
            for row in 0..height {
                for depth_index in 0..depth {
                    for column in 0..width {
                        out[(row * depth + depth_index) * width + column] =
                            values[index(depth_index, row, column, height, width)];
                    }
                }
            }
        }
        2 => {
            for row in 0..height {
                for column in 0..width {
                    for depth_index in 0..depth {
                        out[(row * width + column) * depth + depth_index] =
                            values[index(depth_index, row, column, height, width)];
                    }
                }
            }
        }
        4 => {
            for depth_index in 0..depth {
                for row in 0..height {
                    for column in 0..width {
                        let at = index(depth_index, row, column, height, width);
                        let previous = if depth_index == 0 {
                            0
                        } else {
                            values[index(depth_index - 1, row, column, height, width)] as i64
                        };
                        out[at] = zigzag(values[at] as i64 - previous);
                    }
                }
            }
        }
        5 => {
            let mut work: Vec<i64> = values.iter().map(|&v| v as i64).collect();
            for axis in [0usize, 1, 2] {
                let prior = work.clone();
                for z in 0..depth {
                    for y in 0..height {
                        for x in 0..width {
                            let at = index(z, y, x, height, width);
                            let before = match axis {
                                0 if z > 0 => prior[index(z - 1, y, x, height, width)],
                                1 if y > 0 => prior[index(z, y - 1, x, height, width)],
                                2 if x > 0 => prior[index(z, y, x - 1, height, width)],
                                _ => 0,
                            };
                            work[at] = prior[at] - before;
                        }
                    }
                }
            }
            for (dst, value) in out.iter_mut().zip(work) {
                *dst = zigzag(value);
            }
        }
        _ => return Err("unknown volume mode".into()),
    }
    Ok(sample_bytes(&out))
}

/// Inverse of `volume_transform`; input uses the corresponding flattened mode
/// layout and output is canonical depth, row, column little-endian u16 bytes.
pub fn volume_inverse(
    encoded: &[u8],
    depth: usize,
    height: usize,
    width: usize,
    mode: u8,
) -> Result<Vec<u8>, String> {
    let values = samples(encoded)?;
    let count = depth
        .checked_mul(height)
        .and_then(|n| n.checked_mul(width))
        .ok_or("volume dimensions overflow")?;
    if values.len() != count {
        return Err("decoded volume length mismatch".into());
    }
    let mut out = vec![0u16; count];
    match mode {
        0 | 3 => out.copy_from_slice(&values),
        1 => {
            for row in 0..height {
                for z in 0..depth {
                    for x in 0..width {
                        out[index(z, row, x, height, width)] =
                            values[(row * depth + z) * width + x];
                    }
                }
            }
        }
        2 => {
            for row in 0..height {
                for x in 0..width {
                    for z in 0..depth {
                        out[index(z, row, x, height, width)] =
                            values[(row * width + x) * depth + z];
                    }
                }
            }
        }
        4 | 5 => {
            let mut work: Vec<i64> = values.into_iter().map(unzigzag).collect();
            for axis in if mode == 4 {
                vec![0usize]
            } else {
                vec![2usize, 1, 0]
            } {
                for z in 0..depth {
                    for y in 0..height {
                        for x in 0..width {
                            let at = index(z, y, x, height, width);
                            let before = match axis {
                                0 if z > 0 => work[index(z - 1, y, x, height, width)],
                                1 if y > 0 => work[index(z, y - 1, x, height, width)],
                                2 if x > 0 => work[index(z, y, x - 1, height, width)],
                                _ => 0,
                            };
                            work[at] += before;
                        }
                    }
                }
            }
            for (dst, value) in out.iter_mut().zip(work) {
                *dst = (value & 65535) as u16;
            }
        }
        _ => return Err("unknown volume mode".into()),
    }
    Ok(sample_bytes(&out))
}

/// Dimensions recovered from a bounded JXL basic-info probe. `ImageBackend`
/// implementations return canonical little-endian u16 CIX samples from
/// `jpegxl_decode`, even when the encoded plane used UINT8 samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JpegxlImageInfo {
    pub depth: usize,
    pub height: usize,
    pub width: usize,
}

pub trait ImageBackend {
    fn lzma_compress(&self, source: &[u8]) -> Result<Vec<u8>, String>;
    fn lzma_decompress(
        &self,
        source: &[u8],
        expected: usize,
        memory: usize,
    ) -> Result<Vec<u8>, String>;
    fn jpegxl_encode(
        &self,
        pixels: &[u8],
        depth: usize,
        height: usize,
        width: usize,
        planar: bool,
        effort: u8,
    ) -> Result<Vec<u8>, String>;
    /// Inspect a gray still JXL payload without an image output allocation.
    /// `expected_pixels` comes from paid CIX framing and must be checked by the
    /// provider before it returns dimensions.
    fn jpegxl_probe(
        &self,
        payload: &[u8],
        expected_pixels: usize,
        memory: usize,
    ) -> Result<JpegxlImageInfo, String>;
    fn jpegxl_decode(
        &self,
        payload: &[u8],
        expected: usize,
        depth: usize,
        height: usize,
        width: usize,
        memory: usize,
    ) -> Result<Vec<u8>, String>;
}

fn volume_coded_shape(
    depth: usize,
    height: usize,
    width: usize,
    mode: u8,
) -> Result<[usize; 3], String> {
    match mode {
        0 | 1 | 4 | 5 => Ok([
            1,
            depth
                .checked_mul(height)
                .ok_or("volume dimensions overflow")?,
            width,
        ]),
        2 => Ok([
            1,
            height,
            depth
                .checked_mul(width)
                .ok_or("volume dimensions overflow")?,
        ]),
        3 => Ok([depth, height, width]),
        _ => Err("unknown volume mode".into()),
    }
}

pub fn encode_volume<B: ImageBackend>(
    data: &[u8],
    region: &DicomRegion,
    mode: u8,
    effort: u8,
    backend: &B,
) -> Result<Vec<u8>, String> {
    let slice_bytes = region
        .slice_height
        .checked_mul(region.width)
        .and_then(|n| n.checked_mul(2))
        .ok_or("volume dimensions overflow")?;
    if slice_bytes == 0
        || region.length == 0
        || !region.length.is_multiple_of(slice_bytes)
        || region
            .offset
            .checked_add(region.length)
            .is_none_or(|end| end > data.len())
    {
        return Err("invalid volume region".into());
    }
    let depth = region.length / slice_bytes;
    let transformed = volume_transform(
        &data[region.offset..region.offset + region.length],
        depth,
        region.slice_height,
        region.width,
        mode,
    )?;
    let [coded_depth, coded_height, coded_width] =
        volume_coded_shape(depth, region.slice_height, region.width, mode)?;
    let pixels = backend.jpegxl_encode(
        &transformed,
        coded_depth,
        coded_height,
        coded_width,
        mode == 3,
        effort,
    )?;
    let metadata = backend.lzma_compress(
        &[
            &data[..region.offset],
            &data[region.offset + region.length..],
        ]
        .concat(),
    )?;
    let mut frame = VOLUME_MAGIC.to_vec();
    frame.push(mode);
    for value in [
        data.len(),
        region.offset,
        region.length,
        depth,
        region.slice_height,
        region.width,
        metadata.len(),
        pixels.len(),
    ] {
        frame.extend(vencode(value as u64));
    }
    frame.extend(crc32(data).to_be_bytes());
    frame.extend(metadata);
    frame.extend(pixels);
    Ok(frame)
}

/// Encode the current bytes once as CIXV1 mode 3 at effort 10, then construct
/// the equivalent CIXI2 axes=0/levels=0/predictor=0 envelope around that same
/// metadata and JPEG XL payload. This is framing selection, not a payload
/// cache: each call encodes the supplied bytes exactly once.
pub fn encode_volume_shared_identity<B: ImageBackend>(
    data: &[u8],
    region: &DicomRegion,
    backend: &B,
) -> Result<SharedIdentityArchive, String> {
    let cixv1 = encode_volume(data, region, 3, 10, backend)?;
    let mut pos = 6usize;
    if cixv1.get(..5) != Some(VOLUME_MAGIC) || cixv1.get(5) != Some(&3) {
        return Err("unexpected mode-3 volume frame".into());
    }
    let mut fields = [0usize; 8];
    for field in &mut fields {
        let (value, next) = vdecode(&cixv1, pos)?;
        *field = usize::try_from(value).map_err(|_| "mode-3 field overflow")?;
        pos = next;
    }
    let [original, offset, length, depth, height, width, metadata_len, payload_len] = fields;
    let pixel_bytes = depth
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(width))
        .and_then(|pixels| pixels.checked_mul(2))
        .ok_or("mode-3 dimensions overflow")?;
    let checksum_end = pos.checked_add(4).ok_or("mode-3 frame overflow")?;
    let metadata_end = checksum_end
        .checked_add(metadata_len)
        .ok_or("mode-3 frame overflow")?;
    let payload_end = metadata_end
        .checked_add(payload_len)
        .ok_or("mode-3 frame overflow")?;
    if length != pixel_bytes
        || offset.checked_add(length).is_none_or(|end| end > original)
        || payload_end != cixv1.len()
    {
        return Err("invalid mode-3 frame bounds".into());
    }
    let checksum = &cixv1[pos..checksum_end];
    let metadata = &cixv1[checksum_end..metadata_end];
    let payload = &cixv1[metadata_end..payload_end];

    let mut cixi2 = MULTISCALE_MAGIC.to_vec();
    cixi2.extend([0, 0, 0]);
    for value in [original, offset, depth, height, width, metadata_len] {
        cixi2.extend(vencode(value as u64));
    }
    cixi2.extend_from_slice(checksum);
    cixi2.extend_from_slice(metadata);
    cixi2.extend(vencode(payload_len as u64));
    cixi2.extend_from_slice(payload);

    let cixv1_logical_bytes = cixv1.len();
    let cixi2_logical_bytes = cixi2.len();
    let payload_sha256: [u8; 32] = Sha256::digest(payload).into();
    let selected_format = if cixi2.len() < cixv1.len() {
        SharedIdentityFormat::CixI2
    } else {
        SharedIdentityFormat::CixV1
    };
    let archive = match selected_format {
        SharedIdentityFormat::CixV1 => cixv1,
        SharedIdentityFormat::CixI2 => cixi2,
    };
    Ok(SharedIdentityArchive {
        archive,
        report: SharedIdentityReport {
            cixv1_logical_bytes,
            cixi2_logical_bytes,
            selected_format,
            payload_sha256,
            shared_current_encode: true,
        },
    })
}

pub fn decode_volume<B: ImageBackend>(
    frame: &[u8],
    maximum: usize,
    memory: usize,
    backend: &B,
) -> Result<Vec<u8>, String> {
    if frame.get(..5) != Some(VOLUME_MAGIC) || frame.len() < 6 {
        return Err("invalid volume frame".into());
    }
    let mode = frame[5];
    let mut pos = 6;
    let mut fields = [0usize; 8];
    for field in &mut fields {
        let (value, next) = vdecode(frame, pos)?;
        *field = usize::try_from(value).map_err(|_| "volume field overflow")?;
        pos = next;
    }
    let [original, offset, length, depth, height, width, metadata_len, pixels_len] = fields;
    if original > maximum || original > memory {
        return Err("volume output exceeds limit".into());
    }
    if length == 0
        || original.checked_add(length).is_none_or(|n| {
            n.checked_add(CODEC_OVERHEAD)
                .is_none_or(|total| total > memory)
        })
    {
        return Err("volume resource estimate exceeds limit".into());
    }
    let dimensions = depth
        .checked_mul(height)
        .and_then(|n| n.checked_mul(width))
        .ok_or("volume dimensions overflow")?;
    let payload_at = pos
        .checked_add(4)
        .and_then(|n| n.checked_add(metadata_len))
        .ok_or("volume frame overflow")?;
    if dimensions == 0
        || dimensions.checked_mul(2) != Some(length)
        || offset.checked_add(length).is_none_or(|end| end > original)
        || payload_at.checked_add(pixels_len) != Some(frame.len())
    {
        return Err("volume framing mismatch".into());
    }
    let checksum = u32::from_be_bytes(
        frame[pos..pos + 4]
            .try_into()
            .map_err(|_| "truncated volume checksum")?,
    );
    let metadata =
        backend.lzma_decompress(&frame[pos + 4..payload_at], original - length, memory)?;
    if metadata.len() != original - length {
        return Err("volume metadata mismatch".into());
    }
    let [coded_depth, coded_height, coded_width] = volume_coded_shape(depth, height, width, mode)?;
    let pixels = backend.jpegxl_decode(
        &frame[payload_at..],
        length,
        coded_depth,
        coded_height,
        coded_width,
        memory,
    )?;
    if pixels.len() != length {
        return Err("volume JPEG XL output mismatch".into());
    }
    let raw = volume_inverse(&pixels, depth, height, width, mode)?;
    let mut output = Vec::with_capacity(original);
    output.extend_from_slice(&metadata[..offset]);
    output.extend_from_slice(&raw);
    output.extend_from_slice(&metadata[offset..]);
    if output.len() != original || crc32(&output) != checksum {
        return Err("volume checksum mismatch".into());
    }
    Ok(output)
}

/// Encode the retained CIXI1 grayscale-JXL envelope. The CIX framing owns
/// dimensions at encode time; the JXL provider receives canonical u16 region
/// samples and chooses the frozen UINT8/UINT16 transport representation.
pub fn encode_spatial<B: ImageBackend>(
    data: &[u8],
    region: &DicomRegion,
    backend: &B,
) -> Result<Vec<u8>, String> {
    encode_spatial_with_effort(data, region, 7, backend)
}

/// Encode the retained CIXI1 grayscale-JXL envelope at an explicitly selected
/// frozen JPEG XL effort. The historical `encode_spatial` entry point remains
/// the exact effort-7 form.
pub fn encode_spatial_with_effort<B: ImageBackend>(
    data: &[u8],
    region: &DicomRegion,
    effort: u8,
    backend: &B,
) -> Result<Vec<u8>, String> {
    if !(1..=10).contains(&effort) {
        return Err("spatial JPEG XL effort must be in 1..=10".into());
    }
    let pixels = region
        .width
        .checked_mul(region.height)
        .ok_or("spatial dimensions overflow")?;
    let length = pixels.checked_mul(2).ok_or("spatial bytes overflow")?;
    if region.width == 0
        || region.height == 0
        || region.length != length
        || region
            .offset
            .checked_add(region.length)
            .is_none_or(|end| end > data.len())
    {
        return Err("invalid spatial region".into());
    }
    let payload = backend.jpegxl_encode(
        &data[region.offset..region.offset + region.length],
        1,
        region.height,
        region.width,
        false,
        effort,
    )?;
    let metadata = backend.lzma_compress(
        &[
            &data[..region.offset],
            &data[region.offset + region.length..],
        ]
        .concat(),
    )?;
    let mut frame = SPATIAL_MAGIC.to_vec();
    frame.push(1); // Frozen CIXI1 JXL representation.
    for value in [
        data.len(),
        region.offset,
        region.length,
        metadata.len(),
        payload.len(),
    ] {
        frame.extend(vencode(value as u64));
    }
    frame.extend(crc32(data).to_be_bytes());
    frame.extend(metadata);
    frame.extend(payload);
    Ok(frame)
}

/// Restore a bounded CIXI1 JXL envelope. Historical CIXI1 stores the paid
/// region byte length but no image shape, so it first performs a JXL basic-info
/// probe constrained by `length / 2`. The probe must not install an output
/// buffer; only its checked dimensions are passed to the decode call.
pub fn decode_spatial<B: ImageBackend>(
    frame: &[u8],
    maximum: usize,
    memory: usize,
    backend: &B,
) -> Result<Vec<u8>, String> {
    if frame.get(..5) != Some(SPATIAL_MAGIC) || frame.len() < 6 {
        return Err("invalid spatial frame".into());
    }
    if frame[5] != 1 {
        return Err("unsupported spatial representation".into());
    }
    let mut pos = 6;
    let mut fields = [0usize; 5];
    for field in &mut fields {
        let (value, next) = vdecode(frame, pos)?;
        *field = usize::try_from(value).map_err(|_| "spatial field overflow")?;
        pos = next;
    }
    let [original, offset, length, metadata_len, payload_len] = fields;
    if original > maximum || original > memory {
        return Err("spatial output exceeds limit".into());
    }
    if length == 0
        || length % 2 != 0
        || original.checked_add(length).is_none_or(|sum| {
            sum.checked_add(CODEC_OVERHEAD)
                .is_none_or(|total| total > memory)
        })
    {
        return Err("spatial resource estimate exceeds limit".into());
    }
    let checksum_at = pos;
    let metadata_at = checksum_at.checked_add(4).ok_or("spatial frame overflow")?;
    let payload_at = metadata_at
        .checked_add(metadata_len)
        .ok_or("spatial frame overflow")?;
    if offset.checked_add(length).is_none_or(|end| end > original)
        || payload_at.checked_add(payload_len) != Some(frame.len())
    {
        return Err("spatial framing mismatch".into());
    }
    let checksum = u32::from_be_bytes(
        frame
            .get(checksum_at..metadata_at)
            .ok_or("truncated spatial checksum")?
            .try_into()
            .map_err(|_| "truncated spatial checksum")?,
    );
    let metadata = backend.lzma_decompress(
        frame
            .get(metadata_at..payload_at)
            .ok_or("truncated spatial metadata")?,
        original - length,
        memory,
    )?;
    if metadata.len() != original - length {
        return Err("spatial metadata mismatch".into());
    }
    let payload = frame.get(payload_at..).ok_or("truncated spatial payload")?;
    let expected_pixels = length / 2;
    let info = backend.jpegxl_probe(payload, expected_pixels, memory)?;
    if info.depth != 1
        || info.width == 0
        || info.height == 0
        || info
            .width
            .checked_mul(info.height)
            .is_none_or(|pixels| pixels != expected_pixels)
    {
        return Err("spatial JPEG XL basic info mismatch".into());
    }
    let raw =
        backend.jpegxl_decode(payload, length, info.depth, info.height, info.width, memory)?;
    if raw.len() != length {
        return Err("spatial JPEG XL output mismatch".into());
    }
    let mut output = Vec::with_capacity(original);
    output.extend_from_slice(&metadata[..offset]);
    output.extend_from_slice(&raw);
    output.extend_from_slice(&metadata[offset..]);
    if output.len() != original || crc32(&output) != checksum {
        return Err("spatial checksum mismatch".into());
    }
    Ok(output)
}

/// Split a depth/row/column u16 volume on one axis using the exact CIXI2
/// reversible lifting rule. The returned shapes follow ceil/floor splitting.
type MultiscaleBands = (Vec<u16>, [usize; 3], Vec<u16>, [usize; 3]);

pub fn multiscale_split(
    values: &[u16],
    shape: [usize; 3],
    axis: usize,
    predictor: u8,
) -> Result<MultiscaleBands, String> {
    if predictor > 1 {
        return Err("unsupported multiscale predictor".into());
    }
    let count = checked_shape_count(shape, "multiscale shape")?;
    if values.len() != count {
        return Err("multiscale length mismatch".into());
    }
    let (low_shape, high_shape) = split_shapes(shape, axis)?;
    let low_count = checked_shape_count(low_shape, "multiscale low band")?;
    let high_count = if high_shape[axis] == 0 {
        0
    } else {
        checked_shape_count(high_shape, "multiscale high band")?
    };
    let mut low = vec![0u16; low_count];
    let mut high = vec![0u16; high_count];
    for z in 0..shape[0] {
        for y in 0..shape[1] {
            for x in 0..shape[2] {
                let coordinates = [z, y, x];
                if coordinates[axis] & 1 == 0 {
                    let mut target = coordinates;
                    target[axis] /= 2;
                    low[index(target[0], target[1], target[2], low_shape[1], low_shape[2])] =
                        values[index(z, y, x, shape[1], shape[2])];
                }
            }
        }
    }
    for z in 0..shape[0] {
        for y in 0..shape[1] {
            for x in 0..shape[2] {
                let coordinates = [z, y, x];
                let source = values[index(z, y, x, shape[1], shape[2])];
                let pair = coordinates[axis] / 2;
                let odd = coordinates[axis] & 1 != 0;
                let mut target = coordinates;
                target[axis] = pair;
                if !odd {
                    continue;
                }
                let mut neighbor = target;
                neighbor[axis] = (pair + 1).min(low_shape[axis] - 1);
                let predicted = if predictor == 0 {
                    low[index(target[0], target[1], target[2], low_shape[1], low_shape[2])] as i64
                } else {
                    (low[index(target[0], target[1], target[2], low_shape[1], low_shape[2])] as i64
                        + low[index(
                            neighbor[0],
                            neighbor[1],
                            neighbor[2],
                            low_shape[1],
                            low_shape[2],
                        )] as i64)
                        / 2
                };
                high[index(
                    target[0],
                    target[1],
                    target[2],
                    high_shape[1],
                    high_shape[2],
                )] = zigzag(source as i64 - predicted);
            }
        }
    }
    Ok((low, low_shape, high, high_shape))
}

pub fn multiscale_join(
    low: &[u16],
    low_shape: [usize; 3],
    high: &[u16],
    high_shape: [usize; 3],
    axis: usize,
    predictor: u8,
) -> Result<(Vec<u16>, [usize; 3]), String> {
    if axis >= low_shape.len() || predictor > 1 {
        return Err("invalid multiscale parameters".into());
    }
    let low_count = checked_shape_count(low_shape, "multiscale low band")?;
    if high_shape
        .iter()
        .enumerate()
        .any(|(at, &value)| (at != axis && value == 0) || (at != axis && value != low_shape[at]))
    {
        return Err("invalid multiscale bands".into());
    }
    let expected_high = low_shape[axis].saturating_sub(1);
    if high_shape[axis] != expected_high && high_shape[axis] != low_shape[axis] {
        return Err("invalid multiscale bands".into());
    }
    let high_count = if high_shape[axis] == 0 {
        0
    } else {
        checked_shape_count(high_shape, "multiscale high band")?
    };
    let mut shape = low_shape;
    shape[axis] = low_shape[axis]
        .checked_add(high_shape[axis])
        .ok_or("multiscale dimensions overflow")?;
    let count = checked_shape_count(shape, "multiscale reconstructed shape")?;
    if low.len() != low_count || high.len() != high_count {
        return Err("multiscale band length mismatch".into());
    }
    let mut output = vec![0u16; count];
    for z in 0..low_shape[0] {
        for y in 0..low_shape[1] {
            for x in 0..low_shape[2] {
                let mut out = [z, y, x];
                out[axis] *= 2;
                output[index(out[0], out[1], out[2], shape[1], shape[2])] =
                    low[index(z, y, x, low_shape[1], low_shape[2])];
            }
        }
    }
    for z in 0..high_shape[0] {
        for y in 0..high_shape[1] {
            for x in 0..high_shape[2] {
                let here = [z, y, x];
                let mut out = here;
                out[axis] = out[axis] * 2 + 1;
                let mut next = here;
                next[axis] = (next[axis] + 1).min(low_shape[axis] - 1);
                let base = low[index(here[0], here[1], here[2], low_shape[1], low_shape[2])] as i64;
                let predicted = if predictor == 0 {
                    base
                } else {
                    (base
                        + low[index(next[0], next[1], next[2], low_shape[1], low_shape[2])] as i64)
                        / 2
                };
                output[index(out[0], out[1], out[2], shape[1], shape[2])] =
                    ((predicted + unzigzag(high[index(z, y, x, high_shape[1], high_shape[2])]))
                        & 65535) as u16;
            }
        }
    }
    Ok((output, shape))
}

fn multiscale_plan(axes: u8, levels: u8) -> Vec<usize> {
    let mut plan = Vec::new();
    for _ in 0..levels {
        for axis in 0..3 {
            if axes & (1 << axis) != 0 {
                plan.push(axis);
            }
        }
    }
    plan
}

// Recursive bands retain their shape, plan, predictor, backend, and output state.
#[allow(clippy::too_many_arguments)]
fn encode_multiscale_tree<B: ImageBackend>(
    values: &[u16],
    shape: [usize; 3],
    plan: &[usize],
    step: usize,
    predictor: u8,
    effort: u8,
    backend: &B,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let mut step = step;
    while step < plan.len() && shape[plan[step]] < 2 {
        step += 1;
    }
    if step == plan.len() {
        let pixels = sample_bytes(values);
        let payload =
            backend.jpegxl_encode(&pixels, shape[0], shape[1], shape[2], shape[0] > 1, effort)?;
        out.extend(vencode(payload.len() as u64));
        out.extend(payload);
        return Ok(());
    }
    let (low, low_shape, high, high_shape) =
        multiscale_split(values, shape, plan[step], predictor)?;
    encode_multiscale_tree(
        &low,
        low_shape,
        plan,
        step + 1,
        predictor,
        effort,
        backend,
        out,
    )?;
    let pixels = sample_bytes(&high);
    let payload = backend.jpegxl_encode(
        &pixels,
        high_shape[0],
        high_shape[1],
        high_shape[2],
        high_shape[0] > 1,
        effort,
    )?;
    out.extend(vencode(payload.len() as u64));
    out.extend(payload);
    Ok(())
}

fn decode_multiscale_band<B: ImageBackend>(
    frame: &[u8],
    pos: usize,
    shape: [usize; 3],
    memory: usize,
    backend: &B,
) -> Result<(Vec<u16>, usize), String> {
    let (size, pos) = vdecode(frame, pos)?;
    let size = usize::try_from(size).map_err(|_| "multiscale band size overflow")?;
    let end = pos.checked_add(size).ok_or("multiscale band overflow")?;
    let payload = frame.get(pos..end).ok_or("truncated multiscale band")?;
    let sample_count = shape[0]
        .checked_mul(shape[1])
        .and_then(|n| n.checked_mul(shape[2]))
        .ok_or("multiscale dimensions overflow")?;
    let bytes = sample_count
        .checked_mul(2)
        .ok_or("multiscale bytes overflow")?;
    let decoded = backend.jpegxl_decode(payload, bytes, shape[0], shape[1], shape[2], memory)?;
    if decoded.len() != bytes {
        return Err("multiscale JPEG XL output mismatch".into());
    }
    Ok((samples(&decoded)?, end))
}

// Recursive decode needs the same independently bounded band state.
#[allow(clippy::too_many_arguments)]
fn decode_multiscale_tree<B: ImageBackend>(
    frame: &[u8],
    pos: usize,
    shape: [usize; 3],
    plan: &[usize],
    step: usize,
    predictor: u8,
    memory: usize,
    backend: &B,
) -> Result<(Vec<u16>, usize), String> {
    let mut step = step;
    while step < plan.len() && shape[plan[step]] < 2 {
        step += 1;
    }
    if step == plan.len() {
        return decode_multiscale_band(frame, pos, shape, memory, backend);
    }
    let axis = plan[step];
    let (low_shape, high_shape) = split_shapes(shape, axis)?;
    let (low, pos) = decode_multiscale_tree(
        frame,
        pos,
        low_shape,
        plan,
        step + 1,
        predictor,
        memory,
        backend,
    )?;
    let (high, pos) = decode_multiscale_band(frame, pos, high_shape, memory, backend)?;
    let (joined, joined_shape) =
        multiscale_join(&low, low_shape, &high, high_shape, axis, predictor)?;
    if joined_shape != shape {
        return Err("multiscale reconstructed shape mismatch".into());
    }
    Ok((joined, pos))
}

/// Encode the historical CIXI2 multiscale envelope.  The supplied backend owns
/// the pinned LZMA/JXL ABI; this routine owns all CIX framing and lifting.
pub fn encode_multiscale<B: ImageBackend>(
    data: &[u8],
    region: &DicomRegion,
    axes: u8,
    levels: u8,
    predictor: u8,
    effort: u8,
    backend: &B,
) -> Result<Vec<u8>, String> {
    if axes > 7 || levels > 8 || predictor > 1 {
        return Err("unsupported multiscale parameters".into());
    }
    let slice_bytes = region
        .slice_height
        .checked_mul(region.width)
        .and_then(|n| n.checked_mul(2))
        .ok_or("multiscale dimensions overflow")?;
    if slice_bytes == 0
        || region.length == 0
        || !region.length.is_multiple_of(slice_bytes)
        || region
            .offset
            .checked_add(region.length)
            .is_none_or(|end| end > data.len())
    {
        return Err("invalid multiscale region".into());
    }
    let depth = region.length / slice_bytes;
    let values = samples(&data[region.offset..region.offset + region.length])?;
    let metadata = backend.lzma_compress(
        &[
            &data[..region.offset],
            &data[region.offset + region.length..],
        ]
        .concat(),
    )?;
    let mut frame = MULTISCALE_MAGIC.to_vec();
    frame.extend([axes, levels, predictor]);
    for value in [
        data.len(),
        region.offset,
        depth,
        region.slice_height,
        region.width,
        metadata.len(),
    ] {
        frame.extend(vencode(value as u64));
    }
    frame.extend(crc32(data).to_be_bytes());
    frame.extend(metadata);
    encode_multiscale_tree(
        &values,
        [depth, region.slice_height, region.width],
        &multiscale_plan(axes, levels),
        0,
        predictor,
        effort,
        backend,
        &mut frame,
    )?;
    Ok(frame)
}

pub fn decode_multiscale<B: ImageBackend>(
    frame: &[u8],
    maximum: usize,
    memory: usize,
    backend: &B,
) -> Result<Vec<u8>, String> {
    if frame.get(..5) != Some(MULTISCALE_MAGIC) || frame.len() < 8 {
        return Err("invalid multiscale frame".into());
    }
    let (axes, levels, predictor) = (frame[5], frame[6], frame[7]);
    if axes > 7 || levels > 8 || predictor > 1 {
        return Err("unsupported multiscale parameters".into());
    }
    let mut pos = 8;
    let mut fields = [0usize; 6];
    for field in &mut fields {
        let (value, next) = vdecode(frame, pos)?;
        *field = usize::try_from(value).map_err(|_| "multiscale field overflow")?;
        pos = next;
    }
    let [original, offset, depth, height, width, metadata_len] = fields;
    let pixels = depth
        .checked_mul(height)
        .and_then(|n| n.checked_mul(width))
        .and_then(|n| n.checked_mul(2))
        .ok_or("multiscale dimensions overflow")?;
    if original > maximum
        || original > memory
        || original
            .checked_add(pixels)
            .is_none_or(|n| n.checked_add(CODEC_OVERHEAD).is_none_or(|n| n > memory))
    {
        return Err("multiscale output exceeds limit".into());
    }
    let metadata_at = pos.checked_add(4).ok_or("multiscale frame overflow")?;
    let bands_at = metadata_at
        .checked_add(metadata_len)
        .ok_or("multiscale frame overflow")?;
    if pixels == 0
        || offset.checked_add(pixels).is_none_or(|end| end > original)
        || bands_at > frame.len()
    {
        return Err("invalid multiscale dimensions".into());
    }
    let checksum = u32::from_be_bytes(
        frame[pos..metadata_at]
            .try_into()
            .map_err(|_| "truncated multiscale checksum")?,
    );
    let metadata =
        backend.lzma_decompress(&frame[metadata_at..bands_at], original - pixels, memory)?;
    if metadata.len() != original - pixels {
        return Err("multiscale metadata mismatch".into());
    }
    let (values, end) = decode_multiscale_tree(
        frame,
        bands_at,
        [depth, height, width],
        &multiscale_plan(axes, levels),
        0,
        predictor,
        memory,
        backend,
    )?;
    if end != frame.len() {
        return Err("trailing multiscale bytes".into());
    }
    let raw = sample_bytes(&values);
    let mut output = Vec::with_capacity(original);
    output.extend_from_slice(&metadata[..offset]);
    output.extend_from_slice(&raw);
    output.extend_from_slice(&metadata[offset..]);
    if output.len() != original || crc32(&output) != checksum {
        return Err("multiscale checksum mismatch".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn signed_wrap_matches_reference_and_all_volume_modes_restore() {
        // Python reference zigzag first wraps into [-32768, 32767].
        assert_eq!(
            [zigzag(65535), zigzag(-65535), zigzag(32768), zigzag(-32768)],
            [1, 2, 65535, 65535]
        );
        let values = [0, 65535, 32768, 32767, 1, 65534, 50000, 12345];
        let bytes = sample_bytes(&values);
        for mode in 0..=5 {
            let encoded = volume_transform(&bytes, 2, 2, 2, mode).unwrap();
            assert_eq!(volume_inverse(&encoded, 2, 2, 2, mode).unwrap(), bytes);
        }
        assert_eq!(volume_coded_shape(3, 5, 7, 2).unwrap(), [1, 5, 21]);
        assert_eq!(volume_coded_shape(3, 5, 7, 3).unwrap(), [3, 5, 7]);
    }

    #[test]
    fn multiscale_lifting_round_trips_every_axis() {
        let values: Vec<u16> = (0..24).map(|value| (value * 271) as u16).collect();
        for axis in 0..3 {
            for predictor in 0..=1 {
                let (low, low_shape, high, high_shape) =
                    multiscale_split(&values, [2, 3, 4], axis, predictor).unwrap();
                let (joined, shape) =
                    multiscale_join(&low, low_shape, &high, high_shape, axis, predictor).unwrap();
                assert_eq!(shape, [2, 3, 4]);
                assert_eq!(joined, values);
            }
        }
    }

    #[test]
    fn multiscale_shape_validation_rejects_malformed_inputs_without_panicking() {
        assert!(multiscale_split(&[], [0, 1, 1], 0, 0).is_err());
        assert!(multiscale_split(&[1], [1, 1, 1], 3, 0).is_err());
        assert!(multiscale_split(&[], [usize::MAX, 2, 1], 0, 0).is_err());

        // A one-sample axis is a legitimate no-detail split.  Its zero-length
        // high band must not trigger the ``low_shape - 1`` neighbour path.
        let (low, low_shape, high, high_shape) = multiscale_split(&[42], [1, 1, 1], 0, 1).unwrap();
        let (joined, shape) = multiscale_join(&low, low_shape, &high, high_shape, 0, 1).unwrap();
        assert_eq!(shape, [1, 1, 1]);
        assert_eq!(joined, [42]);

        assert!(multiscale_join(&[], [0, 1, 1], &[], [0, 1, 1], 0, 0).is_err());
        assert!(multiscale_join(&[1], [1, 1, 1], &[], [0, 0, 1], 0, 0).is_err());
        assert!(multiscale_join(&[], [usize::MAX, 1, 1], &[], [usize::MAX, 1, 1], 0, 0,).is_err());
    }

    struct CopySpatialBackend;

    impl ImageBackend for CopySpatialBackend {
        fn lzma_compress(&self, source: &[u8]) -> Result<Vec<u8>, String> {
            Ok(source.to_vec())
        }

        fn lzma_decompress(
            &self,
            source: &[u8],
            expected: usize,
            _memory: usize,
        ) -> Result<Vec<u8>, String> {
            if source.len() != expected {
                return Err("mock metadata length mismatch".into());
            }
            Ok(source.to_vec())
        }

        fn jpegxl_encode(
            &self,
            pixels: &[u8],
            depth: usize,
            _height: usize,
            _width: usize,
            planar: bool,
            effort: u8,
        ) -> Result<Vec<u8>, String> {
            if depth != 1 || planar || effort != 7 {
                return Err("mock spatial encode contract mismatch".into());
            }
            Ok(pixels.to_vec())
        }

        fn jpegxl_probe(
            &self,
            payload: &[u8],
            expected_pixels: usize,
            _memory: usize,
        ) -> Result<JpegxlImageInfo, String> {
            if payload.len() != expected_pixels.checked_mul(2).ok_or("overflow")? {
                return Err("mock paid pixel mismatch".into());
            }
            Ok(JpegxlImageInfo {
                depth: 1,
                height: 1,
                width: expected_pixels,
            })
        }

        fn jpegxl_decode(
            &self,
            payload: &[u8],
            expected: usize,
            depth: usize,
            height: usize,
            width: usize,
            _memory: usize,
        ) -> Result<Vec<u8>, String> {
            if depth != 1
                || height != 1
                || width.checked_mul(height).and_then(|n| n.checked_mul(2)) != Some(expected)
                || payload.len() != expected
            {
                return Err("mock spatial decode contract mismatch".into());
            }
            Ok(payload.to_vec())
        }
    }

    struct IdentityBackend {
        encodes: Cell<usize>,
        last_effort: Cell<u8>,
    }

    impl IdentityBackend {
        fn new() -> Self {
            Self {
                encodes: Cell::new(0),
                last_effort: Cell::new(0),
            }
        }
    }

    impl ImageBackend for IdentityBackend {
        fn lzma_compress(&self, source: &[u8]) -> Result<Vec<u8>, String> {
            Ok(source.to_vec())
        }

        fn lzma_decompress(
            &self,
            source: &[u8],
            expected: usize,
            _memory: usize,
        ) -> Result<Vec<u8>, String> {
            if source.len() != expected {
                return Err("identity metadata length mismatch".into());
            }
            Ok(source.to_vec())
        }

        fn jpegxl_encode(
            &self,
            pixels: &[u8],
            _depth: usize,
            _height: usize,
            _width: usize,
            _planar: bool,
            effort: u8,
        ) -> Result<Vec<u8>, String> {
            self.encodes.set(self.encodes.get() + 1);
            self.last_effort.set(effort);
            Ok(pixels.to_vec())
        }

        fn jpegxl_probe(
            &self,
            payload: &[u8],
            expected_pixels: usize,
            _memory: usize,
        ) -> Result<JpegxlImageInfo, String> {
            if payload.len() != expected_pixels.checked_mul(2).ok_or("overflow")? {
                return Err("identity paid pixel mismatch".into());
            }
            Ok(JpegxlImageInfo {
                depth: 1,
                height: 1,
                width: expected_pixels,
            })
        }

        fn jpegxl_decode(
            &self,
            payload: &[u8],
            expected: usize,
            depth: usize,
            height: usize,
            width: usize,
            _memory: usize,
        ) -> Result<Vec<u8>, String> {
            if depth
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(width))
                .and_then(|pixels| pixels.checked_mul(2))
                != Some(expected)
                || payload.len() != expected
            {
                return Err("identity JPEG XL shape mismatch".into());
            }
            Ok(payload.to_vec())
        }
    }

    #[test]
    fn spatial_jxl_round_trip_probes_paid_pixels_before_decode() {
        let data = [9_u8, 8, 1, 0, 2, 0, 3, 0, 4, 0, 7, 6];
        let region = DicomRegion {
            offset: 2,
            length: 8,
            width: 2,
            height: 2,
            slice_height: 2,
        };
        let backend = CopySpatialBackend;
        let frame = encode_spatial(&data, &region, &backend).unwrap();
        assert_eq!(
            // The retained decoder reserves CODEC_OVERHEAD in addition to the
            // reconstructed bytes, matching the CIXV1 and CIXI2 contracts.
            decode_spatial(&frame, data.len(), 128 << 20, &backend).unwrap(),
            data
        );
    }

    #[test]
    fn spatial_effort_wrapper_preserves_default_and_accepts_effort_ten() {
        let data = [9_u8, 8, 1, 0, 2, 0, 3, 0, 4, 0, 7, 6];
        let region = DicomRegion {
            offset: 2,
            length: 8,
            width: 2,
            height: 2,
            slice_height: 2,
        };
        let backend = IdentityBackend::new();
        encode_spatial(&data, &region, &backend).unwrap();
        assert_eq!(backend.last_effort.get(), 7);
        encode_spatial_with_effort(&data, &region, 10, &backend).unwrap();
        assert_eq!(backend.last_effort.get(), 10);
        assert!(encode_spatial_with_effort(&data, &region, 0, &backend).is_err());
        assert_eq!(backend.encodes.get(), 2);
    }

    #[test]
    fn shared_mode3_identity_encodes_once_and_restores_zero_lifting_cixi2() {
        let region = DicomRegion {
            offset: 3,
            // A three-byte paid region length makes the zero-lifting CIXI2
            // header strictly smaller than its CIXV1 mode-3 counterpart.
            length: 32_768,
            width: 128,
            height: 128,
            slice_height: 64,
        };
        let mut data = b"pre".to_vec();
        for value in 0..(region.length / 2) {
            data.extend_from_slice(&(value as u16).to_le_bytes());
        }
        data.extend_from_slice(b"post");
        let backend = IdentityBackend::new();
        let shared = encode_volume_shared_identity(&data, &region, &backend).unwrap();
        assert_eq!(backend.encodes.get(), 1);
        assert_eq!(backend.last_effort.get(), 10);
        assert!(shared.report.shared_current_encode);
        assert_eq!(
            shared.report.payload_sha256,
            <[u8; 32]>::from(Sha256::digest(&data[3..3 + region.length]))
        );
        assert!(shared.report.cixi2_logical_bytes < shared.report.cixv1_logical_bytes);
        assert_eq!(shared.report.selected_format, SharedIdentityFormat::CixI2);
        let restored = match shared.report.selected_format {
            SharedIdentityFormat::CixV1 => {
                decode_volume(&shared.archive, data.len(), 128 << 20, &backend).unwrap()
            }
            SharedIdentityFormat::CixI2 => {
                decode_multiscale(&shared.archive, data.len(), 128 << 20, &backend).unwrap()
            }
        };
        assert_eq!(restored, data);
    }

    #[test]
    fn dicom_recognizer_rejects_truncated_pixel_member() {
        let mut data = Vec::new();
        for (group, element, value) in [
            (0x0028u16, 0x0010u16, 2u16.to_le_bytes().to_vec()),
            (0x0028, 0x0011, 2u16.to_le_bytes().to_vec()),
            (0x0028, 0x0100, 16u16.to_le_bytes().to_vec()),
            (0x7fe0, 0x0010, vec![0; 7]),
        ] {
            data.extend(group.to_le_bytes());
            data.extend(element.to_le_bytes());
            data.extend((value.len() as u32).to_le_bytes());
            data.extend(value);
        }
        assert!(dicom_regions(&data).is_empty());
    }
}
