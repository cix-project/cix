//! Native admission and historic framing for executable-specialist streams.
//!
//! This module deliberately separates three things which were coupled in the
//! reference implementation: byte-layout discovery, reversible CIX-owned
//! transforms, and the backend which compresses a transformed byte stream.
//! The backend is supplied by the full engine.  A decoder therefore never
//! consults Python, a program on `PATH`, a corpus name, or an external object
//! file in order to restore an archive.

use crate::full_engine::arithmetic::{vdecode, vencode, ArithmeticDecoder, ArithmeticEncoder};
use crc32fast::Hasher;
use num_bigint::BigUint;
use num_traits::{One, Zero};
use std::collections::{BTreeMap, BTreeSet};

pub const JOINT_MAGIC: &[u8; 5] = b"CIXB\x1d";
pub const FILTER_MAGIC: &[u8; 5] = b"CIXB\x13";
pub const GOT_MAGIC: &[u8; 5] = b"CIXB\x03";
pub const RELOCATED_MAGIC: &[u8; 5] = b"CIXB\x0a";
pub const POINTER_MAGIC: &[u8; 5] = b"CIXB\x0b";
pub const SECTION_MAGIC: &[u8; 5] = b"CIXB\x0c";

const ECOFF_HEADER: usize = 104;
const ECOFF_SECTION: usize = 64;
const ECOFF_OPTIONAL_HEADER: u16 = 80;
const CONSTRAINT_TRAILER: &[u8; 5] = b"ECFX1";
const FIELDS_TRAILER: &[u8; 5] = b"ECFC1";
const GOT_TRAILER: &[u8; 5] = b"ECGT1";
const RELOCATED_POINTER_TRAILER: &[u8; 5] = b"ECRP1";
const POINTER_DELTA_TRAILER: &[u8; 5] = b"ECRD1";
const SECTION_POINTER_TRAILER: &[u8; 5] = b"ECSP1";
const SYMBOL_LOAD_TRAILER: &[u8; 5] = b"ECSL1";
// The single-table pdata representation is retained as a fixture vector only.
// Production historical frames use the combined pdata/symbol representation.
#[cfg(test)]
const PDATA_TRAILER: &[u8; 5] = b"ECPD1";
const PDATA_SYMBOL_TRAILER: &[u8; 5] = b"ECPD2";
const GOT_BLOCK: usize = 1024;
// Address inverse chains retain the backend raw stream, then rebuild several
// nested skeletons (pointer/GOT/fields/constraints).  Six declared-source
// buffers cover those simultaneously owned vectors before any inverse starts.
const ADDRESS_INVERSE_BUFFERS: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendVariant {
    PaqV215,
    PaqV216,
    PaqJointDiscount,
    PaqStoreState,
    Brotli,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendRequest {
    pub variant: BackendVariant,
    /// Native backend options are explicit archive policy, never environment.
    pub options: String,
    /// A frame can have several compressed streams.  The historic executable
    /// frames currently use stream zero; retaining this field prevents a later
    /// multistream format from being accidentally treated as a single stream.
    pub stream: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transform {
    Constraints { mode: u8 },
    Fields { mode: u8 },
    Got { mode: u8 },
    RelocatedPointers { mode: u8, shared: bool },
    PointerDeltas { mode: u8 },
    SectionPointers { mode: u8 },
    PdataSymbols { mode: u8 },
    Identity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoricalFrame {
    Got,
    Relocated,
    Pointer,
    Section,
    JointDiscount,
    Filtered,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectSpan {
    pub offset: usize,
    pub length: usize,
    pub tar_member: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutableKind {
    Elf,
    Ecoff,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableRegion {
    pub kind: ExecutableKind,
    pub span: ObjectSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Section {
    pub name: Vec<u8>,
    pub offset: usize,
    pub size: usize,
    pub virtual_address: u64,
}

#[derive(Clone, Debug)]
pub struct EcoffObject {
    pub span: ObjectSpan,
    pub sections: BTreeMap<Vec<u8>, Section>,
}

/// Backend callbacks are intentionally narrow.  The engine owns concrete
/// library integration and can refuse unavailable historic PAQ variants.
pub type EncodeBackend<'a> = dyn FnMut(&BackendRequest, &[u8]) -> Result<Vec<u8>, String> + 'a;
pub type DecodeBackend<'a> =
    dyn FnMut(&BackendRequest, &[u8], usize) -> Result<Vec<u8>, String> + 'a;

fn le_u16(data: &[u8], at: usize) -> Result<u16, String> {
    let bytes = data
        .get(at..at.checked_add(2).ok_or("offset overflow")?)
        .ok_or("truncated ECOFF word")?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn le_u32(data: &[u8], at: usize) -> Result<u32, String> {
    let bytes = data
        .get(at..at.checked_add(4).ok_or("offset overflow")?)
        .ok_or("truncated ECOFF word")?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn le_u64(data: &[u8], at: usize) -> Result<u64, String> {
    let bytes = data
        .get(at..at.checked_add(8).ok_or("offset overflow")?)
        .ok_or("truncated ECOFF word")?;
    Ok(u64::from_le_bytes(
        bytes.try_into().map_err(|_| "invalid ECOFF word")?,
    ))
}

fn checked_end(start: usize, count: usize, limit: usize) -> Result<usize, String> {
    let end = start.checked_add(count).ok_or("ECOFF range overflow")?;
    if end > limit {
        Err("ECOFF range outside input".into())
    } else {
        Ok(end)
    }
}

fn trim_nul(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .copied()
        .take_while(|byte| *byte != 0)
        .collect()
}

/// Parse only the verified Alpha ECOFF layout accepted by the reference port.
/// Unknown executable bytes remain ordinary input and are never partially
/// rewritten.
pub fn parse_ecoff_object(bytes: &[u8], span: ObjectSpan) -> Result<EcoffObject, String> {
    if bytes.len() < ECOFF_HEADER || bytes.get(0..2) != Some(&b"\x83\x01"[..]) {
        return Err("not Alpha ECOFF".into());
    }
    let count = le_u16(bytes, 2)? as usize;
    let optional = le_u16(bytes, 20)?;
    let flags = le_u16(bytes, 22)?;
    let table_end = checked_end(
        ECOFF_HEADER,
        count
            .checked_mul(ECOFF_SECTION)
            .ok_or("section count overflow")?,
        bytes.len(),
    )?;
    if optional != ECOFF_OPTIONAL_HEADER
        || !(1..=256).contains(&count)
        || !matches!(flags, 0x2005 | 0x2007 | 0x3007)
    {
        return Err("unsupported ECOFF header".into());
    }
    let mut sections = BTreeMap::new();
    let mut ranges = Vec::new();
    for index in 0..count {
        let start = ECOFF_HEADER + index * ECOFF_SECTION;
        let name = trim_nul(
            bytes
                .get(start..start + 8)
                .ok_or("truncated ECOFF section")?,
        );
        let virtual_address = le_u64(bytes, start + 16)?;
        let size_u64 = le_u64(bytes, start + 24)?;
        let offset_u64 = le_u64(bytes, start + 32)?;
        let size = usize::try_from(size_u64).map_err(|_| "ECOFF section too large")?;
        let offset = usize::try_from(offset_u64).map_err(|_| "ECOFF section offset too large")?;
        if offset != 0 {
            if offset < table_end {
                return Err("ECOFF section overlaps header".into());
            }
            checked_end(offset, size, bytes.len())?;
            if size != 0 {
                ranges.push((offset, offset + size));
            }
        }
        // ECOFF's zero file offset denotes an absent section.  It is not a
        // usable mapping and duplicate absent names do not make the object
        // ambiguous; this mirrors the legacy parser exactly.
        if offset != 0
            && sections
                .insert(
                    name.clone(),
                    Section {
                        name,
                        offset,
                        size,
                        virtual_address,
                    },
                )
                .is_some()
        {
            return Err("duplicate ECOFF section".into());
        }
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("overlapping ECOFF sections".into());
    }
    Ok(EcoffObject { span, sections })
}

fn tar_number(field: &[u8]) -> Option<usize> {
    if field.first().copied()? & 0x80 != 0 {
        return None;
    }
    let text = field
        .iter()
        .copied()
        .take_while(|byte| *byte != 0 && *byte != b' ')
        .collect::<Vec<_>>();
    if text.is_empty() {
        return Some(0);
    }
    std::str::from_utf8(&text)
        .ok()
        .and_then(|value| usize::from_str_radix(value.trim(), 8).ok())
}

/// Discover canonical standalone ECOFF or regular-file ECOFF members in a
/// plain ustar-compatible TAR.  Nonzero member offsets are retained in spans.
/// Malformed TAR is simply not an executable admission candidate.
pub fn discover_ecoff_objects(data: &[u8]) -> Vec<EcoffObject> {
    let standalone = ObjectSpan {
        offset: 0,
        length: data.len(),
        tar_member: false,
    };
    if let Ok(object) = parse_ecoff_object(data, standalone) {
        return vec![object];
    }
    let mut found = Vec::new();
    let mut position = 0usize;
    while position
        .checked_add(512)
        .is_some_and(|end| end <= data.len())
    {
        let header = &data[position..position + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let Some(size) = tar_number(&header[124..136]) else {
            break;
        };
        let kind = header[156];
        let payload = position + 512;
        let Some(end) = payload.checked_add(size) else {
            break;
        };
        if end > data.len() {
            break;
        }
        if kind == 0 || kind == b'0' {
            let span = ObjectSpan {
                offset: payload,
                length: size,
                tar_member: true,
            };
            if let Ok(object) = parse_ecoff_object(&data[payload..end], span) {
                found.push(object);
            }
        }
        let blocks = size.checked_add(511).map(|value| value / 512);
        let Some(next) = blocks
            .and_then(|value| value.checked_mul(512))
            .and_then(|value| payload.checked_add(value))
        else {
            break;
        };
        position = next;
    }
    found
}

fn is_elf(data: &[u8]) -> bool {
    if data.get(..4) != Some(&b"\x7fELF"[..]) || data.len() < 0x34 {
        return false;
    }
    let class = data[4];
    if !matches!(class, 1 | 2) || data[5] != 1 || data[6] != 1 {
        return false;
    }
    let header_size_at = if class == 1 { 0x28 } else { 0x34 };
    let header_size = match le_u16(data, header_size_at) {
        Ok(value) => value as usize,
        Err(_) => return false,
    };
    header_size >= (if class == 1 { 52 } else { 64 }) && header_size <= data.len()
}

/// Find plain ELF/ECOFF files and valid regular TAR members without assuming a
/// filename or container offset.  ECOFF-specific transforms use
/// [`discover_ecoff_objects`]; ELF remains eligible for its own native path.
pub fn discover_executable_regions(data: &[u8]) -> Vec<ExecutableRegion> {
    let whole = ObjectSpan {
        offset: 0,
        length: data.len(),
        tar_member: false,
    };
    if parse_ecoff_object(data, whole.clone()).is_ok() {
        return vec![ExecutableRegion {
            kind: ExecutableKind::Ecoff,
            span: whole,
        }];
    }
    if is_elf(data) {
        return vec![ExecutableRegion {
            kind: ExecutableKind::Elf,
            span: whole,
        }];
    }
    let mut output = Vec::new();
    let mut position = 0usize;
    while position
        .checked_add(512)
        .is_some_and(|end| end <= data.len())
    {
        let header = &data[position..position + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let Some(size) = tar_number(&header[124..136]) else {
            break;
        };
        let payload = position + 512;
        let Some(end) = payload.checked_add(size) else {
            break;
        };
        if end > data.len() {
            break;
        }
        if header[156] == 0 || header[156] == b'0' {
            let span = ObjectSpan {
                offset: payload,
                length: size,
                tar_member: true,
            };
            let member = &data[payload..end];
            if parse_ecoff_object(member, span.clone()).is_ok() {
                output.push(ExecutableRegion {
                    kind: ExecutableKind::Ecoff,
                    span,
                });
            } else if is_elf(member) {
                output.push(ExecutableRegion {
                    kind: ExecutableKind::Elf,
                    span,
                });
            }
        }
        let Some(next) = size
            .checked_add(511)
            .map(|value| value / 512)
            .and_then(|blocks| blocks.checked_mul(512))
            .and_then(|padding| payload.checked_add(padding))
        else {
            break;
        };
        position = next;
    }
    output
}

fn crc32(data: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

fn verify(data: Vec<u8>, size: usize, checksum: u32) -> Result<Vec<u8>, String> {
    if data.len() != size || crc32(&data) != checksum {
        return Err("executable source checksum mismatch".into());
    }
    Ok(data)
}

fn address_inverse_workspace(declared: usize, memory: usize) -> Result<usize, String> {
    let reserve = declared
        .checked_mul(ADDRESS_INVERSE_BUFFERS)
        .ok_or("executable inverse workspace overflow")?;
    memory
        .checked_sub(reserve)
        .ok_or_else(|| "executable inverse workspace exceeds memory limit".to_string())
}

fn backend_selector(variant: BackendVariant) -> Result<u8, String> {
    match variant {
        BackendVariant::PaqV215 => Ok(1),
        BackendVariant::Brotli => Ok(0),
        _ => Err("frame has no historical selector for requested backend".into()),
    }
}

fn selected_backend(selector: u8) -> Result<BackendVariant, String> {
    match selector {
        0 => Ok(BackendVariant::Brotli),
        1 => Ok(BackendVariant::PaqV215),
        _ => Err("invalid executable backend selector".into()),
    }
}

/// CIX-owned constraints transform.  Only tables recomputed exactly from bytes
/// in the object are zeroed, and each object/mask is paid in the trailer.
pub fn constraints_transform(source: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    if mode > 3 {
        return Err("invalid ECOFF constraint policy".into());
    }
    let objects = discover_ecoff_objects(source);
    let mut out = source.to_vec();
    let mut changes = Vec::new();
    for object in objects {
        let original = &source[object.span.offset..object.span.offset + object.span.length];
        let predictions = predicted_constraints(original, &object.sections)?;
        let mut mask = 0u8;
        for (bit, at, expected) in predictions {
            if mode & bit != 0 && original.get(at..at + expected.len()) == Some(expected.as_slice())
            {
                let destination = object.span.offset + at;
                out[destination..destination + expected.len()].fill(0);
                mask |= bit;
            }
        }
        if mask != 0 {
            changes.push((object.span.offset, object.span.length, mask));
        }
    }
    let mut metadata = vencode(changes.len() as u64);
    let mut previous_end = 0usize;
    for (offset, length, mask) in changes {
        if offset < previous_end {
            return Err("non-monotonic ECOFF object spans".into());
        }
        metadata.extend_from_slice(&vencode((offset - previous_end) as u64));
        metadata.extend_from_slice(&vencode(length as u64));
        metadata.push(mask);
        previous_end = offset
            .checked_add(length)
            .ok_or("constraint span overflow")?;
    }
    out.extend_from_slice(&metadata);
    out.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    out.extend_from_slice(CONSTRAINT_TRAILER);
    Ok(out)
}

pub fn constraints_inverse(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (mut output, metadata) = split_trailer(raw, CONSTRAINT_TRAILER, 1)?;
    let (count, mut position) = vdecode(metadata, 0)?;
    if count > (output.len() / ECOFF_HEADER) as u64 {
        return Err("excessive ECOFF object count".into());
    }
    let mut previous_end = 0usize;
    for _ in 0..count {
        let (delta, after_delta) = vdecode(metadata, position)?;
        let (length, after_length) = vdecode(metadata, after_delta)?;
        let offset = previous_end
            .checked_add(usize::try_from(delta).map_err(|_| "constraint offset too large")?)
            .ok_or("constraint offset overflow")?;
        let length = usize::try_from(length).map_err(|_| "constraint length too large")?;
        let mask = *metadata
            .get(after_length)
            .ok_or("missing ECOFF constraint mask")?;
        position = after_length + 1;
        if !(1..=3).contains(&mask) || checked_end(offset, length, output.len()).is_err() {
            return Err("invalid ECOFF constraint descriptor".into());
        }
        let local = output[offset..offset + length].to_vec();
        let object = parse_ecoff_object(
            &local,
            ObjectSpan {
                offset,
                length,
                tar_member: false,
            },
        )?;
        let predictions = predicted_constraints(&local, &object.sections)?;
        let mut seen = 0u8;
        for (bit, at, expected) in predictions {
            if mask & bit != 0 {
                let target = checked_end(offset + at, expected.len(), output.len())?;
                if output[offset + at..target].iter().any(|byte| *byte != 0) {
                    return Err("derived ECOFF field is not zero".into());
                }
                output[offset + at..target].copy_from_slice(&expected);
                seen |= bit;
            }
        }
        if seen != mask {
            return Err("ECOFF dependencies missing".into());
        }
        previous_end = offset + length;
    }
    if position != metadata.len() {
        return Err("unused ECOFF constraint metadata".into());
    }
    Ok(output)
}

fn split_trailer<'a>(
    raw: &'a [u8],
    trailer: &[u8; 5],
    minimum: usize,
) -> Result<(Vec<u8>, &'a [u8]), String> {
    if raw.len() < 13 + minimum || !raw.ends_with(trailer) {
        return Err("invalid ECOFF transform trailer".into());
    }
    let metadata_length = u64::from_le_bytes(
        raw[raw.len() - 13..raw.len() - 5]
            .try_into()
            .map_err(|_| "invalid trailer")?,
    );
    let metadata_length =
        usize::try_from(metadata_length).map_err(|_| "ECOFF metadata too large")?;
    if metadata_length < minimum {
        return Err("ECOFF metadata too short".into());
    }
    let trailer_length = metadata_length
        .checked_add(13)
        .ok_or("ECOFF metadata length overflow")?;
    let start = raw
        .len()
        .checked_sub(trailer_length)
        .ok_or("ECOFF metadata outside frame")?;
    Ok((raw[..start].to_vec(), &raw[start..raw.len() - 13]))
}

fn predicted_constraints(
    data: &[u8],
    sections: &BTreeMap<Vec<u8>, Section>,
) -> Result<Vec<(u8, usize, Vec<u8>)>, String> {
    let (hash, dynsym, dynstr) = match (
        sections.get(b".hash" as &[u8]),
        sections.get(b".dynsym" as &[u8]),
        sections.get(b".dynstr" as &[u8]),
    ) {
        (Some(hash), Some(dynsym), Some(dynstr)) => (hash, dynsym, dynstr),
        _ => return Ok(Vec::new()),
    };
    if hash.size < 8 {
        return Ok(Vec::new());
    }
    let buckets = le_u32(data, hash.offset)? as usize;
    let count = le_u32(data, hash.offset + 4)? as usize;
    if buckets == 0
        || count == 0
        || 8usize
            .checked_add(
                4usize
                    .checked_mul(buckets.checked_add(count).ok_or("hash count overflow")?)
                    .ok_or("hash size overflow")?,
            )
            .ok_or("hash size overflow")?
            > hash.size
        || count.checked_mul(24).ok_or("symbol count overflow")? > dynsym.size
    {
        return Ok(Vec::new());
    }
    let mut hashes = Vec::with_capacity(count);
    for index in 0..count {
        let name_offset = le_u32(data, dynsym.offset + index * 24)? as usize;
        if name_offset >= dynstr.size {
            return Ok(Vec::new());
        }
        let start = dynstr.offset + name_offset;
        let end_limit = dynstr.offset + dynstr.size;
        let Some(relative) = data[start..end_limit].iter().position(|byte| *byte == 0) else {
            return Ok(Vec::new());
        };
        hashes.push(name_hash(&data[start..start + relative]));
    }
    let mut heads = vec![0u32; buckets];
    let mut chains = vec![0u32; count];
    for index in (1..count).rev() {
        let bucket = hashes[index] as usize % buckets;
        chains[index] = heads[bucket];
        heads[bucket] = index as u32;
    }
    let mut table = Vec::with_capacity(4 * (buckets + count));
    for value in heads.into_iter().chain(chains) {
        table.extend_from_slice(&value.to_le_bytes());
    }
    let mut result = vec![(1, hash.offset + 8, table)];
    if let (Some(msym), Some(rel)) = (
        sections.get(b".msym" as &[u8]),
        sections.get(b".rel.dyn" as &[u8]),
    ) {
        if msym.size >= count * 8 && rel.size % 16 == 0 {
            let mut first = vec![0u32; count];
            for index in 1..rel.size / 16 {
                let info = le_u32(data, rel.offset + index * 16 + 8)?;
                let symbol = (info >> 8) as usize;
                if info != 0 && symbol < count && first[symbol] == 0 {
                    first[symbol] = index as u32;
                }
            }
            if first.iter().all(|value| *value < (1 << 24)) {
                let mut table = Vec::with_capacity(count * 8);
                for (hash, first) in hashes.into_iter().zip(first) {
                    table.extend_from_slice(&hash.to_le_bytes());
                    table.extend_from_slice(&(first << 8).to_le_bytes());
                }
                result.push((2, msym.offset, table));
            }
        }
    }
    Ok(result)
}

fn name_hash(name: &[u8]) -> u32 {
    let mut value = 0u32;
    for byte in name {
        value = value.wrapping_shl(4).wrapping_add(*byte as u32);
        let high = value & 0xf000_0000;
        value ^= high >> 24;
        value &= !high;
    }
    value
}

fn zigzag_delta(value: u64, previous: u64, bits: u32, inverse: bool) -> u64 {
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    if inverse {
        let signed = if value & 1 == 0 {
            (value / 2) as i128
        } else {
            -((value / 2) as i128) - 1
        };
        ((previous as i128 + signed) as u64) & mask
    } else {
        let delta = value.wrapping_sub(previous) & mask;
        let signed = if bits < 64 && delta & (1u64 << (bits - 1)) != 0 {
            delta as i128 - (1i128 << bits)
        } else if bits == 64 && delta & (1u64 << 63) != 0 {
            delta as i128 - (1i128 << 64)
        } else {
            delta as i128
        };
        if signed >= 0 {
            (signed as u64) << 1
        } else {
            // Keep the intermediate wide: -2^63 maps exactly to u64::MAX.
            (((-signed) << 1) - 1) as u64
        }
    }
}

fn transpose(data: &mut [u8], width: usize, inverse: bool) -> Result<(), String> {
    if width == 0 || !data.len().is_multiple_of(width) {
        return Err("partial ECOFF field matrix".into());
    }
    let rows = data.len() / width;
    let original = data.to_vec();
    if inverse {
        for column in 0..width {
            data[column..]
                .iter_mut()
                .step_by(width)
                .zip(&original[column * rows..(column + 1) * rows])
                .for_each(|(out, value)| *out = *value);
        }
    } else {
        for column in 0..width {
            for row in 0..rows {
                data[column * rows + row] = original[row * width + column];
            }
        }
    }
    Ok(())
}

fn write_u32(data: &mut [u8], at: usize, value: u32) -> Result<(), String> {
    let end = checked_end(at, 4, data.len())?;
    data[at..end].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u64(data: &mut [u8], at: usize, value: u64) -> Result<(), String> {
    let end = checked_end(at, 8, data.len())?;
    data[at..end].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn fields_for_object(
    data: &mut [u8],
    sections: &BTreeMap<Vec<u8>, Section>,
    mode: u8,
    inverse: bool,
) -> Result<(), String> {
    let (symbols, strings, hash) = match (
        sections.get(b".dynsym" as &[u8]),
        sections.get(b".dynstr" as &[u8]),
        sections.get(b".hash" as &[u8]),
    ) {
        (Some(symbols), Some(strings), Some(hash)) => (symbols, strings, hash),
        _ => return Err("missing ECOFF field dependencies".into()),
    };
    if hash.size < 8 {
        return Err("short ECOFF hash header".into());
    }
    let count = le_u32(data, hash.offset + 4)? as usize;
    if count == 0 || count.checked_mul(24).ok_or("symbol count overflow")? > symbols.size {
        return Err("symbol count outside table".into());
    }
    let symbols_end = checked_end(symbols.offset, count * 24, data.len())?;
    if inverse && mode >= 2 {
        transpose(&mut data[symbols.offset..symbols_end], 24, true)?;
    }
    let mut previous_name_end = 0u64;
    let mut values: BTreeMap<(u8, u16), u64> = BTreeMap::new();
    for index in 0..count {
        let row = symbols.offset + index * 24;
        let encoded_name = le_u32(data, row)? as u64;
        let padding = le_u32(data, row + 4)?;
        let encoded_value = le_u64(data, row + 8)?;
        let size = le_u32(data, row + 16)?;
        let info = *data.get(row + 20).ok_or("truncated ECOFF symbol")?;
        let other = *data.get(row + 21).ok_or("truncated ECOFF symbol")?;
        let shndx = le_u16(data, row + 22)?;
        let name = if inverse {
            zigzag_delta(encoded_name, previous_name_end, 32, true)
        } else {
            encoded_name
        };
        let name = usize::try_from(name).map_err(|_| "symbol name offset too large")?;
        if name >= strings.size {
            return Err("symbol name outside strings".into());
        }
        let string_start = strings.offset + name;
        let string_end = strings.offset + strings.size;
        let Some(length) = data[string_start..string_end]
            .iter()
            .position(|byte| *byte == 0)
        else {
            return Err("unterminated symbol name".into());
        };
        let next_name_end = (name + length + 1) as u64;
        let key = (info, shndx);
        let old = *values.get(&key).unwrap_or(&0);
        let value = if inverse && mode >= 2 {
            zigzag_delta(encoded_value, old, 64, true)
        } else {
            encoded_value
        };
        let stored_name = if inverse {
            name as u64
        } else {
            zigzag_delta(name as u64, previous_name_end, 32, false)
        };
        let stored_value = if inverse || mode < 2 {
            value
        } else {
            zigzag_delta(value, old, 64, false)
        };
        write_u32(data, row, stored_name as u32)?;
        write_u32(data, row + 4, padding)?;
        write_u64(data, row + 8, stored_value)?;
        write_u32(data, row + 16, size)?;
        data[row + 20] = info;
        data[row + 21] = other;
        data[row + 22..row + 24].copy_from_slice(&shndx.to_le_bytes());
        previous_name_end = next_name_end;
        values.insert(key, value);
    }
    if !inverse && mode >= 2 {
        transpose(&mut data[symbols.offset..symbols_end], 24, false)?;
    }
    if mode < 3 {
        return Ok(());
    }
    let Some(relocations) = sections.get(b".rel.dyn" as &[u8]) else {
        return Ok(());
    };
    let relocation_length = relocations.size / 16 * 16;
    let relocation_end = checked_end(relocations.offset, relocation_length, data.len())?;
    if inverse {
        transpose(&mut data[relocations.offset..relocation_end], 16, true)?;
    }
    let mut previous_address = 0u64;
    let mut symbols_by_kind: BTreeMap<u8, u64> = BTreeMap::new();
    for position in (relocations.offset..relocation_end).step_by(16) {
        let encoded_address = le_u64(data, position)?;
        let info = le_u32(data, position + 8)?;
        let reserved = le_u32(data, position + 12)?;
        let kind = (info & 255) as u8;
        let encoded_symbol = (info >> 8) as u64;
        let old = *symbols_by_kind.get(&kind).unwrap_or(&0);
        let address = if inverse {
            zigzag_delta(encoded_address, previous_address, 64, true)
        } else {
            encoded_address
        };
        let symbol = if inverse {
            zigzag_delta(encoded_symbol, old, 24, true)
        } else {
            encoded_symbol
        };
        write_u64(
            data,
            position,
            if inverse {
                address
            } else {
                zigzag_delta(address, previous_address, 64, false)
            },
        )?;
        write_u32(
            data,
            position + 8,
            (((if inverse {
                symbol
            } else {
                zigzag_delta(symbol, old, 24, false)
            }) as u32)
                << 8)
                | kind as u32,
        )?;
        write_u32(data, position + 12, reserved)?;
        previous_address = address;
        symbols_by_kind.insert(kind, symbol);
    }
    if !inverse {
        transpose(&mut data[relocations.offset..relocation_end], 16, false)?;
    }
    Ok(())
}

pub fn fields_transform(source: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    if mode > 3 {
        return Err("invalid ECOFF field mode".into());
    }
    let mut output = constraints_transform(source, 3)?;
    let payload_length = output.len()
        - 13
        - usize::try_from(u64::from_le_bytes(
            output[output.len() - 13..output.len() - 5]
                .try_into()
                .map_err(|_| "invalid constraints trailer")?,
        ))
        .map_err(|_| "constraints metadata too large")?;
    let objects = discover_ecoff_objects(source);
    let mut changes = Vec::new();
    if mode != 0 {
        for object in objects {
            let end = object.span.offset + object.span.length;
            if end > payload_length {
                return Err("ECOFF object outside constraint payload".into());
            }
            let before = output[object.span.offset..end].to_vec();
            if fields_for_object(
                &mut output[object.span.offset..end],
                &object.sections,
                mode,
                false,
            )
            .is_ok()
            {
                changes.push((object.span.offset, object.span.length));
            } else {
                output[object.span.offset..end].copy_from_slice(&before);
            }
        }
    }
    let mut metadata = vec![mode];
    metadata.extend_from_slice(&vencode(changes.len() as u64));
    let mut previous_end = 0usize;
    for (offset, length) in changes {
        metadata.extend_from_slice(&vencode((offset - previous_end) as u64));
        metadata.extend_from_slice(&vencode(length as u64));
        previous_end = offset + length;
    }
    output.extend_from_slice(&metadata);
    output.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    output.extend_from_slice(FIELDS_TRAILER);
    Ok(output)
}

pub fn fields_inverse(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (mut output, metadata) = split_trailer(raw, FIELDS_TRAILER, 2)?;
    let mode = metadata[0];
    if mode > 3 {
        return Err("invalid ECOFF field mode".into());
    }
    let (count, mut position) = vdecode(metadata, 1)?;
    if (mode == 0 && count != 0) || count > (output.len() / ECOFF_HEADER) as u64 {
        return Err("invalid ECOFF field count".into());
    }
    let mut previous_end = 0usize;
    for _ in 0..count {
        let (gap, after_gap) = vdecode(metadata, position)?;
        let (length, after_length) = vdecode(metadata, after_gap)?;
        position = after_length;
        let offset = previous_end
            .checked_add(usize::try_from(gap).map_err(|_| "field offset too large")?)
            .ok_or("field offset overflow")?;
        let length = usize::try_from(length).map_err(|_| "field length too large")?;
        checked_end(offset, length, output.len())?;
        let local = output[offset..offset + length].to_vec();
        let object = parse_ecoff_object(
            &local,
            ObjectSpan {
                offset,
                length,
                tar_member: false,
            },
        )?;
        fields_for_object(
            &mut output[offset..offset + length],
            &object.sections,
            mode,
            true,
        )?;
        previous_end = offset + length;
    }
    if position != metadata.len() {
        return Err("unused ECOFF field metadata".into());
    }
    constraints_inverse(&output)
}

#[derive(Clone, Debug)]
struct GotField {
    where_: usize,
    options: Vec<u64>,
    context: u8,
}

#[derive(Clone, Debug)]
struct SymbolTable {
    span: ObjectSpan,
    sections: BTreeMap<Vec<u8>, Section>,
    symbols: Vec<(Vec<u8>, u64)>,
}

fn object_symbols(source: &[u8], object: EcoffObject) -> Option<SymbolTable> {
    let blob =
        source.get(object.span.offset..object.span.offset.checked_add(object.span.length)?)?;
    let symbols = object.sections.get(b".dynsym" as &[u8])?;
    let strings = object.sections.get(b".dynstr" as &[u8])?;
    let hash = object.sections.get(b".hash" as &[u8])?;
    if hash.size < 8 {
        return None;
    }
    let count = le_u32(blob, hash.offset + 4).ok()? as usize;
    let symbol_bytes = count.checked_mul(24)?;
    if symbol_bytes > symbols.size
        || symbols.offset.checked_add(symbol_bytes)? > blob.len()
        || strings.offset.checked_add(strings.size)? > blob.len()
    {
        return None;
    }
    // Grow only after every on-input table bound has been checked.  An
    // untrusted count never requests a speculative giant allocation.
    let mut table = Vec::new();
    for index in 0..count {
        let row = symbols.offset.checked_add(index.checked_mul(24)?)?;
        let start = le_u32(blob, row).ok()? as usize;
        if start >= strings.size {
            return None;
        }
        let left = strings.offset.checked_add(start)?;
        let right = strings.offset.checked_add(strings.size)?;
        let name_len = blob.get(left..right)?.iter().position(|byte| *byte == 0)?;
        table.push((
            blob[left..left + name_len].to_vec(),
            le_u64(blob, row + 8).ok()?,
        ));
    }
    Some(SymbolTable {
        span: object.span,
        sections: object.sections,
        symbols: table,
    })
}

fn symbol_tables(source: &[u8]) -> Vec<SymbolTable> {
    discover_ecoff_objects(source)
        .into_iter()
        .filter_map(|object| object_symbols(source, object))
        .collect()
}

fn global_symbol_index(tables: &[SymbolTable]) -> BTreeMap<Vec<u8>, BTreeMap<u64, usize>> {
    let mut index = BTreeMap::new();
    for table in tables {
        for (name, value) in &table.symbols {
            if *value != 0 {
                *index
                    .entry(name.clone())
                    .or_insert_with(BTreeMap::new)
                    .entry(*value)
                    .or_insert(0) += 1;
            }
        }
    }
    index
}

fn dynamic_tags(blob: &[u8], section: &Section) -> Result<BTreeMap<u32, Vec<u32>>, String> {
    let mut tags = BTreeMap::new();
    let end = checked_end(section.offset, section.size / 16 * 16, blob.len())?;
    for position in (section.offset..end).step_by(16) {
        let tag = le_u32(blob, position)?;
        if tag == 0 {
            break;
        }
        tags.entry(tag)
            .or_insert_with(Vec::new)
            .push(le_u64(blob, position + 8)? as u32);
    }
    Ok(tags)
}

fn tag_ranges(tags: &BTreeMap<u32, Vec<u32>>, symbol_count: usize) -> Option<(Vec<u32>, Vec<u32>)> {
    let starts = tags.get(&0x7000_000a)?.clone();
    let symbol_starts = tags.get(&0x7000_0013)?.clone();
    if starts.is_empty()
        || starts.len() != symbol_starts.len()
        || tags.get(&0x7000_0011) != Some(&vec![symbol_count as u32])
    {
        return None;
    }
    if starts.windows(2).any(|pair| pair[0] >= pair[1])
        || symbol_starts.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return None;
    }
    Some((starts, symbol_starts))
}

fn got_candidates(source: &[u8], global_symbols: bool) -> Result<Vec<GotField>, String> {
    let tables = symbol_tables(source);
    let global = if global_symbols {
        global_symbol_index(&tables)
    } else {
        BTreeMap::new()
    };
    let mut fields = Vec::new();
    for table in tables {
        let blob = &source[table.span.offset..table.span.offset + table.span.length];
        let (Some(dynamic), Some(got)) = (
            table.sections.get(b".dynamic" as &[u8]),
            table.sections.get(b".got" as &[u8]),
        ) else {
            continue;
        };
        let Some((starts, symbol_starts)) =
            tag_ranges(&dynamic_tags(blob, dynamic)?, table.symbols.len())
        else {
            continue;
        };
        let mut previous = None;
        let mut local = Vec::new();
        let mut valid = true;
        for group in 0..starts.len() {
            let begin = symbol_starts[group] as usize;
            let end = if group + 1 == starts.len() {
                table.symbols.len()
            } else {
                symbol_starts[group + 1] as usize
            };
            if begin > end || end > table.symbols.len() {
                valid = false;
                break;
            }
            for symbol_index in begin..end {
                let got_index = starts[group]
                    .checked_add((symbol_index - begin) as u32)
                    .ok_or("GOT index overflow")? as usize;
                if previous.is_some_and(|old| got_index <= old)
                    || got_index
                        .checked_mul(8)
                        .and_then(|at| at.checked_add(8))
                        .is_none_or(|end| end > got.size)
                {
                    valid = false;
                    break;
                }
                previous = Some(got_index);
                let (name, value) = &table.symbols[symbol_index];
                let mut options = vec![*value];
                if global_symbols {
                    if let Some(values) = global.get(name) {
                        let mut ranked = values
                            .iter()
                            .map(|(value, count)| (*value, *count))
                            .collect::<Vec<_>>();
                        ranked
                            .sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
                        options.extend(
                            ranked
                                .into_iter()
                                .map(|(candidate, _)| candidate)
                                .filter(|candidate| *candidate != *value),
                        );
                    }
                }
                local.push(GotField {
                    where_: table.span.offset + got.offset + got_index * 8,
                    options,
                    context: u8::from(*value == 0),
                });
            }
            if !valid {
                break;
            }
        }
        if valid {
            fields.extend(local);
        }
    }
    fields.sort_by_key(|field| field.where_);
    if fields.windows(2).any(|pair| {
        pair[0]
            .where_
            .checked_add(8)
            .is_none_or(|end| end > pair[1].where_)
    }) {
        return Err("overlapping GOT fields".into());
    }
    Ok(fields)
}

fn pack(blob: &[u8]) -> Vec<u8> {
    let mut output = vencode(blob.len() as u64);
    output.extend_from_slice(blob);
    output
}

fn read_packed(blob: &[u8], position: usize) -> Result<(&[u8], usize), String> {
    let (size, start) = vdecode(blob, position)?;
    let size = usize::try_from(size).map_err(|_| "GOT substream too large")?;
    let end = checked_end(start, size, blob.len()).map_err(|_| "truncated GOT substream")?;
    Ok((&blob[start..end], end))
}

fn rank_byte_width(product: &BigUint) -> Result<usize, String> {
    if product.is_zero() {
        return Err("zero GOT rank product".into());
    }
    let maximum = product - BigUint::one();
    let bits = usize::try_from(maximum.bits()).map_err(|_| "GOT rank width too large")?;
    bits.checked_add(7)
        .ok_or_else(|| "GOT rank width overflow".to_string())
        .map(|width| width / 8)
}

fn encode_choices(
    digits: &[usize],
    alphabets: &[usize],
    contexts: &[u8],
    mode: u8,
) -> Result<Vec<u8>, String> {
    if digits.len() != alphabets.len() || digits.len() != contexts.len() || mode > 3 {
        return Err("invalid GOT choice stream".into());
    }
    if digits
        .iter()
        .zip(alphabets)
        .any(|(digit, alphabet)| *alphabet == 0 || *digit >= *alphabet)
    {
        return Err("GOT choice outside alphabet".into());
    }
    if mode == 0 {
        return Ok(digits
            .iter()
            .flat_map(|digit| vencode(*digit as u64))
            .collect());
    }
    if mode == 1 {
        let mut out = Vec::new();
        for start in (0..digits.len()).step_by(GOT_BLOCK) {
            let end = (start + GOT_BLOCK).min(digits.len());
            let mut rank = BigUint::zero();
            let mut product = BigUint::one();
            for index in start..end {
                rank += BigUint::from(digits[index]) * &product;
                product *= BigUint::from(alphabets[index]);
            }
            let width = rank_byte_width(&product)?;
            let mut bytes = rank.to_bytes_le();
            bytes.resize(width, 0);
            out.extend_from_slice(&bytes);
        }
        return Ok(out);
    }
    let mut coder = ArithmeticEncoder::new();
    let mut counts: BTreeMap<(usize, u8), Vec<u64>> = BTreeMap::new();
    for index in 0..digits.len() {
        let key = (alphabets[index], contexts[index]);
        let model = counts
            .entry(key)
            .or_insert_with(|| vec![1; alphabets[index]]);
        let low = model[..digits[index]].iter().sum::<u64>();
        let total = model.iter().sum::<u64>();
        coder.encode(low, low + model[digits[index]], total)?;
        if mode == 3 {
            model[digits[index]] = model[digits[index]]
                .checked_add(1)
                .ok_or("GOT count overflow")?;
            if model.iter().sum::<u64>() > 4096 {
                for count in model {
                    *count = (*count).div_ceil(2);
                }
            }
        }
    }
    let (payload, bits) = coder.finish();
    let mut out = vencode(bits as u64);
    out.extend_from_slice(&payload);
    Ok(out)
}

fn decode_choices(
    payload: &[u8],
    alphabets: &[usize],
    contexts: &[u8],
    mode: u8,
) -> Result<Vec<usize>, String> {
    if alphabets.len() != contexts.len() || mode > 3 || alphabets.contains(&0) {
        return Err("invalid GOT choice stream".into());
    }
    if mode == 0 {
        let mut output = Vec::with_capacity(alphabets.len());
        let mut position = 0;
        for alphabet in alphabets {
            let (digit, next) = vdecode(payload, position)?;
            let digit = usize::try_from(digit).map_err(|_| "GOT choice outside alphabet")?;
            if digit >= *alphabet {
                return Err("GOT choice outside alphabet".into());
            }
            output.push(digit);
            position = next;
        }
        if position != payload.len() {
            return Err("unused GOT choice bytes".into());
        }
        return Ok(output);
    }
    if mode == 1 {
        let mut output = Vec::with_capacity(alphabets.len());
        let mut position = 0;
        for start in (0..alphabets.len()).step_by(GOT_BLOCK) {
            let end = (start + GOT_BLOCK).min(alphabets.len());
            let mut product = BigUint::one();
            for alphabet in &alphabets[start..end] {
                product *= BigUint::from(*alphabet);
            }
            let width = rank_byte_width(&product)?;
            let next =
                checked_end(position, width, payload.len()).map_err(|_| "truncated GOT rank")?;
            let mut rank = BigUint::from_bytes_le(&payload[position..next]);
            if rank >= product {
                return Err("GOT rank outside state space".into());
            }
            for alphabet in &alphabets[start..end] {
                let divisor = BigUint::from(*alphabet);
                let digit = (&rank % &divisor)
                    .to_u64_digits()
                    .first()
                    .copied()
                    .unwrap_or(0) as usize;
                output.push(digit);
                rank /= divisor;
            }
            position = next;
        }
        if position != payload.len() {
            return Err("unused GOT choice bytes".into());
        }
        return Ok(output);
    }
    let (bits, position) = vdecode(payload, 0)?;
    let bits = usize::try_from(bits).map_err(|_| "GOT arithmetic length too large")?;
    let byte_length = bits
        .checked_add(7)
        .ok_or("GOT arithmetic length overflow")?
        / 8;
    let expected = checked_end(position, byte_length, payload.len())?;
    if expected != payload.len() {
        return Err("invalid GOT arithmetic framing".into());
    }
    let mut decoder = ArithmeticDecoder::new(&payload[position..], bits)?;
    let mut counts: BTreeMap<(usize, u8), Vec<u64>> = BTreeMap::new();
    let mut output = Vec::with_capacity(alphabets.len());
    for index in 0..alphabets.len() {
        let model = counts
            .entry((alphabets[index], contexts[index]))
            .or_insert_with(|| vec![1; alphabets[index]]);
        let total = model.iter().sum::<u64>();
        let target = decoder.target(total)?;
        let mut low = 0u64;
        let mut selected = None;
        for (digit, count) in model.iter().enumerate() {
            if target < low + *count {
                selected = Some((digit, low, *count));
                break;
            }
            low += *count;
        }
        let (digit, low, width) = selected.ok_or("invalid GOT arithmetic symbol")?;
        decoder.update(low, low + width, total)?;
        output.push(digit);
        if mode == 3 {
            model[digit] = model[digit].checked_add(1).ok_or("GOT count overflow")?;
            if model.iter().sum::<u64>() > 4096 {
                for count in model {
                    *count = (*count).div_ceil(2);
                }
            }
        }
    }
    Ok(output)
}

pub fn got_transform(source: &[u8], mode: u8, global_symbols: bool) -> Result<Vec<u8>, String> {
    if mode > 3 {
        return Err("invalid GOT representation".into());
    }
    let fields = got_candidates(source, global_symbols)?;
    let mut output = source.to_vec();
    let mut digits = Vec::with_capacity(fields.len());
    let mut alphabets = Vec::with_capacity(fields.len());
    let mut contexts = Vec::with_capacity(fields.len());
    let mut escapes = Vec::new();
    for field in &fields {
        let value = le_u64(source, field.where_)?;
        let digit = field
            .options
            .iter()
            .position(|candidate| *candidate == value)
            .unwrap_or(field.options.len());
        if digit == field.options.len() {
            escapes.extend_from_slice(&source[field.where_..field.where_ + 8]);
        }
        output[field.where_..field.where_ + 8].fill(0);
        digits.push(digit);
        alphabets.push(field.options.len() + 1);
        contexts.push(field.context);
    }
    let choices = encode_choices(&digits, &alphabets, &contexts, mode)?;
    let skeleton = fields_transform(&output, 1)?;
    let mut metadata = vec![mode, u8::from(global_symbols)];
    metadata.extend_from_slice(&pack(&choices));
    metadata.extend_from_slice(&escapes);
    let mut raw = skeleton;
    raw.extend_from_slice(&metadata);
    raw.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    raw.extend_from_slice(GOT_TRAILER);
    Ok(raw)
}

pub fn got_inverse(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (skeleton, metadata) = split_trailer(raw, GOT_TRAILER, 3)?;
    let mode = metadata[0];
    let global_symbols = metadata[1];
    if mode > 3 || global_symbols > 1 {
        return Err("invalid GOT representation".into());
    }
    let (choices, mut position) = read_packed(metadata, 2)?;
    let data = fields_inverse(&skeleton)?;
    let fields = got_candidates(&data, global_symbols != 0)?;
    let alphabets = fields
        .iter()
        .map(|field| field.options.len() + 1)
        .collect::<Vec<_>>();
    let contexts = fields.iter().map(|field| field.context).collect::<Vec<_>>();
    let digits = decode_choices(choices, &alphabets, &contexts, mode)?;
    let mut output = data;
    for (field, digit) in fields.iter().zip(digits) {
        if output[field.where_..field.where_ + 8]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err("GOT skeleton field is not zero".into());
        }
        if digit == field.options.len() {
            let end =
                checked_end(position, 8, metadata.len()).map_err(|_| "truncated GOT exception")?;
            output[field.where_..field.where_ + 8].copy_from_slice(&metadata[position..end]);
            position = end;
        } else {
            write_u64(&mut output, field.where_, field.options[digit])?;
        }
    }
    if position != metadata.len() {
        return Err("unused GOT exceptions".into());
    }
    Ok(output)
}

#[derive(Clone, Debug)]
struct RelocatedPointerField {
    where_: usize,
    options: Vec<u64>,
    key: usize,
    context: u8,
}

#[derive(Clone, Debug)]
enum SectionAlphabet {
    Functions(Vec<u64>),
    Section { virtual_address: u64, count: u64 },
}

#[derive(Clone, Debug)]
struct SectionPointerField {
    where_: usize,
    declared: u64,
    alphabet: SectionAlphabet,
}

fn section_pointer_candidates(source: &[u8], mode: u8) -> Result<Vec<SectionPointerField>, String> {
    if mode != 4 {
        return Err("unsupported section-pointer model".into());
    }
    let mut fields = Vec::new();
    for table in symbol_tables(source) {
        let Some(relocations) = table.sections.get(b".rel.dyn" as &[u8]) else {
            continue;
        };
        let blob = source
            .get(
                table.span.offset
                    ..table
                        .span
                        .offset
                        .checked_add(table.span.length)
                        .ok_or("section object overflow")?,
            )
            .ok_or("section object outside input")?;
        let mut functions = BTreeSet::new();
        for (_, value) in &table.symbols {
            functions.insert(*value);
        }
        if let Some(pdata) = table.sections.get(b".pdata" as &[u8]) {
            let end = checked_end(pdata.offset, pdata.size / 8 * 8, blob.len())?;
            for position in (pdata.offset..end).step_by(8) {
                let delta = le_u32(blob, position)? as i32 as i64;
                functions.insert(pdata.virtual_address.wrapping_add((delta as u64) & !3));
            }
        }
        let mut pools = BTreeMap::new();
        for name in [b".text" as &[u8], b".init", b".fini"] {
            if let Some(section) = table.sections.get(name) {
                let end = section
                    .virtual_address
                    .checked_add(section.size as u64)
                    .ok_or("section range overflow")?;
                let pool = functions
                    .iter()
                    .copied()
                    .filter(|value| {
                        *value >= section.virtual_address
                            && *value < end
                            && (*value - section.virtual_address) % 4 == 0
                    })
                    .collect::<Vec<_>>();
                if !pool.is_empty() && pool.len() <= 65_535 {
                    pools.insert(name.to_vec(), pool);
                }
            }
        }
        let end = checked_end(relocations.offset, relocations.size / 16 * 16, blob.len())?;
        for position in (relocations.offset..end).step_by(16) {
            let address = le_u64(blob, position)?;
            let info = le_u32(blob, position + 8)?;
            let symbol = (info >> 8) as usize;
            if info & 255 != 2 || symbol >= table.symbols.len() {
                continue;
            }
            let (name, declared) = &table.symbols[symbol];
            let Some(target) = table.sections.get(name.as_slice()) else {
                continue;
            };
            let mut location = None;
            for name in [b".rdata" as &[u8], b".data", b".sdata"] {
                if let Some(data) = table.sections.get(name) {
                    let Some(end) = data.virtual_address.checked_add(data.size as u64) else {
                        continue;
                    };
                    if data.virtual_address <= address
                        && address.checked_add(8).is_some_and(|right| right <= end)
                    {
                        let relative = usize::try_from(address - data.virtual_address)
                            .map_err(|_| "section pointer location too large")?;
                        let candidate = data
                            .offset
                            .checked_add(relative)
                            .ok_or("section pointer location overflow")?;
                        if location.replace(candidate).is_some() {
                            location = None;
                            break;
                        }
                    }
                }
            }
            let Some(location) = location else { continue };
            let alphabet = if let Some(pool) = pools.get(name) {
                SectionAlphabet::Functions(pool.clone())
            } else {
                let count = (target.size as u64)
                    .checked_add(3)
                    .ok_or("section pointer size overflow")?
                    / 4;
                if count == 0 {
                    continue;
                }
                SectionAlphabet::Section {
                    virtual_address: target.virtual_address,
                    count,
                }
            };
            fields.push(SectionPointerField {
                where_: table
                    .span
                    .offset
                    .checked_add(location)
                    .ok_or("section pointer field overflow")?,
                declared: *declared,
                alphabet,
            });
        }
    }
    fields.sort_by_key(|field| field.where_);
    let mut blocked = BTreeSet::new();
    for (index, pair) in fields.windows(2).enumerate() {
        if pair[0]
            .where_
            .checked_add(8)
            .is_none_or(|end| end > pair[1].where_)
        {
            blocked.insert(index);
            blocked.insert(index + 1);
        }
    }
    Ok(fields
        .into_iter()
        .enumerate()
        .filter_map(|(index, field)| (!blocked.contains(&index)).then_some(field))
        .collect())
}

fn section_pointer_digit(value: u64, alphabet: &SectionAlphabet) -> Option<u64> {
    match alphabet {
        SectionAlphabet::Functions(pool) => pool
            .iter()
            .position(|candidate| *candidate == value)
            .map(|index| index as u64),
        SectionAlphabet::Section {
            virtual_address,
            count,
        } => value
            .checked_sub(*virtual_address)
            .filter(|delta| delta % 4 == 0)
            .map(|delta| delta / 4)
            .filter(|digit| *digit < *count),
    }
}

fn section_pointer_value(digit: u64, alphabet: &SectionAlphabet) -> Result<u64, String> {
    match alphabet {
        SectionAlphabet::Functions(pool) => pool
            .get(usize::try_from(digit).map_err(|_| "function index outside alphabet")?)
            .copied()
            .ok_or("function index outside alphabet".into()),
        SectionAlphabet::Section {
            virtual_address,
            count,
        } => {
            if digit >= *count {
                Err("section index outside interval".into())
            } else {
                virtual_address
                    .checked_add(digit.checked_mul(4).ok_or("section pointer overflow")?)
                    .ok_or("section pointer overflow".into())
            }
        }
    }
}

fn section_pointer_transform(source: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    let fields = section_pointer_candidates(source, mode)?;
    let mut output = source.to_vec();
    let mut flags = vec![
        0u8;
        fields
            .len()
            .checked_add(7)
            .ok_or("section pointer field count overflow")?
            / 8
    ];
    for (index, field) in fields.iter().enumerate() {
        let value = le_u64(source, field.where_)?;
        if let Some(digit) = section_pointer_digit(value, &field.alphabet) {
            flags[index / 8] |= 1 << (index % 8);
            write_u64(
                &mut output,
                field.where_,
                field.declared.wrapping_add(digit),
            )?;
        }
    }
    let skeleton = pointer_delta_transform(&output, 5)?;
    let mut metadata = vec![mode];
    metadata.extend_from_slice(&pack(&flags));
    metadata.extend_from_slice(&pack(&[]));
    let mut raw = skeleton;
    raw.extend_from_slice(&metadata);
    raw.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    raw.extend_from_slice(SECTION_POINTER_TRAILER);
    Ok(raw)
}

fn section_pointer_inverse(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (skeleton, metadata) = split_trailer(raw, SECTION_POINTER_TRAILER, 3)?;
    let mode = metadata[0];
    let (flags, position) = read_packed(metadata, 1)?;
    let (choices, position) = read_packed(metadata, position)?;
    if mode != 4 || position != metadata.len() || !choices.is_empty() {
        return Err("invalid section-pointer metadata".into());
    }
    let mut output = pointer_delta_inverse(&skeleton)?;
    let fields = section_pointer_candidates(&output, mode)?;
    if flags.len()
        != fields
            .len()
            .checked_add(7)
            .ok_or("section pointer field count overflow")?
            / 8
    {
        return Err("section-pointer membership mismatch".into());
    }
    for (index, field) in fields.iter().enumerate() {
        if flags[index / 8] & (1 << (index % 8)) != 0 {
            let encoded = le_u64(&output, field.where_)?;
            let digit = encoded.wrapping_sub(field.declared);
            write_u64(
                &mut output,
                field.where_,
                section_pointer_value(digit, &field.alphabet)?,
            )?;
        }
    }
    Ok(output)
}

#[derive(Clone)]
struct SymbolLoadField {
    where_: usize,
    forward: BTreeMap<u16, u16>,
    reverse: BTreeMap<u16, u16>,
}

fn symbol_load_fields(source: &[u8]) -> Result<Vec<SymbolLoadField>, String> {
    let tables = symbol_tables(source);
    let mut frequency = BTreeMap::<Vec<u8>, usize>::new();
    for table in &tables {
        for (name, _) in &table.symbols {
            if !name.is_empty() {
                *frequency.entry(name.clone()).or_default() += 1;
            }
        }
    }
    let mut names = frequency.into_iter().collect::<Vec<_>>();
    names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let global = names
        .into_iter()
        .take(8192)
        .enumerate()
        .map(|(i, (name, _))| (name, i as u16))
        .collect::<BTreeMap<_, _>>();
    let mut output = Vec::new();
    for table in tables {
        let (Some(text), Some(got), Some(dynamic)) = (
            table.sections.get(b".text" as &[u8]),
            table.sections.get(b".got" as &[u8]),
            table.sections.get(b".dynamic" as &[u8]),
        ) else {
            continue;
        };
        let blob = &source[table.span.offset..table.span.offset + table.span.length];
        let tags = dynamic_tags(blob, dynamic)?;
        let Some((starts, symbol_starts)) = tag_ranges(&tags, table.symbols.len()) else {
            continue;
        };
        let got_virtual = got.virtual_address;
        let gp = le_u64(blob, 96)?;
        let mut slots = BTreeMap::new();
        let mut previous = None;
        let mut valid = true;
        for group in 0..starts.len() {
            let begin = symbol_starts[group] as usize;
            let end = if group + 1 == starts.len() {
                table.symbols.len()
            } else {
                symbol_starts[group + 1] as usize
            };
            if begin > end || end > table.symbols.len() {
                valid = false;
                break;
            };
            for symbol in begin..end {
                let index = starts[group]
                    .checked_add((symbol - begin) as u32)
                    .ok_or("symbol slot overflow")?;
                if previous.is_some_and(|old| index <= old)
                    || usize::try_from(index)
                        .ok()
                        .and_then(|i| i.checked_mul(8))
                        .and_then(|i| i.checked_add(8))
                        .is_none_or(|end| end > got.size)
                {
                    valid = false;
                    break;
                };
                previous = Some(index);
                let displacement = got_virtual
                    .wrapping_add(u64::from(index) * 8)
                    .wrapping_sub(gp) as u16;
                slots.insert(displacement, table.symbols[symbol].0.clone());
            }
        }
        if !valid {
            continue;
        }
        let mut grouped = BTreeMap::<Vec<u8>, Vec<u16>>::new();
        for (disp, name) in slots {
            if !name.is_empty() {
                grouped.entry(name).or_default().push(disp);
            }
        }
        let mut forward = BTreeMap::new();
        for (name, values) in grouped {
            if values.len() == 1 {
                if let Some(index) = global.get(&name) {
                    forward.insert(values[0], index.saturating_mul(8));
                }
            }
        }
        let reverse: BTreeMap<u16, u16> = forward.iter().map(|(a, b)| (*b, *a)).collect();
        let end = checked_end(text.offset, text.size / 4 * 4, blob.len())?;
        for position in (text.offset..end).step_by(4) {
            let word = le_u32(blob, position)?;
            if word >> 26 == 0x29 && ((word >> 16) & 31) == 29 {
                output.push(SymbolLoadField {
                    where_: table.span.offset + position,
                    forward: forward.clone(),
                    reverse: reverse.clone(),
                });
            }
        }
    }
    Ok(output)
}

fn symbol_load_transform(source: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    if mode != 2 {
        return Err("unsupported symbol-load model".into());
    }
    let fields = symbol_load_fields(source)?;
    let mut output = source.to_vec();
    let mut flags = vec![
        0;
        fields
            .len()
            .checked_add(7)
            .ok_or("symbol-load count overflow")?
            / 8
    ];
    for (i, field) in fields.iter().enumerate() {
        let value = le_u16(source, field.where_)?;
        if let Some(encoded) = field.forward.get(&value) {
            flags[i / 8] |= 1 << (i % 8);
            output[field.where_..field.where_ + 2].copy_from_slice(&encoded.to_le_bytes());
        }
    }
    let skeleton = section_pointer_transform(&output, 4)?;
    let mut metadata = vec![mode];
    metadata.extend_from_slice(&pack(&flags));
    let mut raw = skeleton;
    raw.extend_from_slice(&metadata);
    raw.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    raw.extend_from_slice(SYMBOL_LOAD_TRAILER);
    Ok(raw)
}
fn symbol_load_inverse(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (skeleton, metadata) = split_trailer(raw, SYMBOL_LOAD_TRAILER, 2)?;
    if metadata[0] != 2 {
        return Err("invalid symbol-load model".into());
    }
    let (flags, pos) = read_packed(metadata, 1)?;
    if pos != metadata.len() {
        return Err("unused symbol-load metadata".into());
    }
    let mut output = section_pointer_inverse(&skeleton)?;
    let fields = symbol_load_fields(&output)?;
    if flags.len()
        != fields
            .len()
            .checked_add(7)
            .ok_or("symbol-load count overflow")?
            / 8
    {
        return Err("symbol-load flag count mismatch".into());
    }
    for (i, field) in fields.iter().enumerate() {
        if flags[i / 8] & (1 << (i % 8)) != 0 {
            let value = le_u16(&output, field.where_)?;
            let original = field
                .reverse
                .get(&value)
                .ok_or("symbol outside reachable load alphabet")?;
            output[field.where_..field.where_ + 2].copy_from_slice(&original.to_le_bytes());
        }
    }
    Ok(output)
}

fn pdata_tables(source: &[u8]) -> Vec<(usize, usize)> {
    discover_ecoff_objects(source)
        .into_iter()
        .filter_map(|object| {
            object
                .sections
                .get(b".pdata" as &[u8])
                .filter(|s| s.size >= 8)
                .map(|s| (object.span.offset + s.offset, s.size / 8))
        })
        .collect()
}
#[cfg(test)]
fn pdata_transform(source: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    if mode != 1 {
        return Err("unsupported procedure model".into());
    }
    let skeleton = section_pointer_transform(source, 4)?;
    let mut output = skeleton;
    let tables = pdata_tables(&output);
    for (start, count) in tables {
        let mut previous = 0u32;
        for i in 0..count {
            let at = start + i * 8;
            let word = le_u32(&output, at)?;
            write_u32(&mut output, at, word.wrapping_sub(previous))?;
            previous = word;
        }
    }
    let metadata = [mode];
    output.extend_from_slice(&metadata);
    output.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    output.extend_from_slice(PDATA_TRAILER);
    Ok(output)
}
#[cfg(test)]
fn pdata_inverse(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (mut output, metadata) = split_trailer(raw, PDATA_TRAILER, 1)?;
    if metadata != [1] {
        return Err("invalid procedure metadata".into());
    }
    for (start, count) in pdata_tables(&output) {
        let mut previous = 0u32;
        for i in 0..count {
            let at = start + i * 8;
            let word = le_u32(&output, at)?.wrapping_add(previous);
            write_u32(&mut output, at, word)?;
            previous = word;
        }
    }
    section_pointer_inverse(&output)
}
fn pdata_symbols_transform(source: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    if mode != 1 {
        return Err("invalid combined procedure mode".into());
    }
    let skeleton = symbol_load_transform(source, 2)?;
    let mut output = skeleton;
    for (start, count) in pdata_tables(&output) {
        let mut previous = 0u32;
        for i in 0..count {
            let at = start + i * 8;
            let word = le_u32(&output, at)?;
            write_u32(&mut output, at, word.wrapping_sub(previous))?;
            previous = word;
        }
    }
    output.push(mode);
    output.extend_from_slice(PDATA_SYMBOL_TRAILER);
    Ok(output)
}
fn pdata_symbols_inverse(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.len() < 6 || !raw.ends_with(PDATA_SYMBOL_TRAILER) || raw[raw.len() - 6] != 1 {
        return Err("invalid combined procedure trailer".into());
    }
    let mut output = raw[..raw.len() - 6].to_vec();
    for (start, count) in pdata_tables(&output) {
        let mut previous = 0u32;
        for i in 0..count {
            let at = start + i * 8;
            let word = le_u32(&output, at)?.wrapping_add(previous);
            write_u32(&mut output, at, word)?;
            previous = word;
        }
    }
    symbol_load_inverse(&output)
}

/// Content-only eligibility for the historical GOT route; no filename, hash,
/// or external object identity participates in this decision.
pub fn eligible_got(source: &[u8]) -> bool {
    got_candidates(source, true).is_ok_and(|fields| !fields.is_empty())
}

/// Content-only eligibility for relocated-pointer and pointer-delta candidates.
pub fn eligible_pointer_deltas(source: &[u8]) -> bool {
    relocated_pointer_candidates(source, true).is_ok_and(|fields| !fields.is_empty())
}

/// Content-only eligibility for the mode-4 section-pointer transform.
pub fn eligible_section_pointers(source: &[u8]) -> bool {
    section_pointer_candidates(source, 4).is_ok_and(|fields| !fields.is_empty())
}

/// Content-only eligibility for filtered pdata/symbol relations.  At least one
/// physical pdata table is required; symbol-load membership is evaluated by
/// the transform itself and can legitimately have zero hits.
pub fn eligible_filtered_pdata(source: &[u8]) -> bool {
    !pdata_tables(source).is_empty()
}

/// The immediate `ecoff_relocated_pointers` dependency.  Every alphabet is
/// derived from symbols carried in these object bytes; `key` preserves the
/// reference implementation's per-symbol predictor identity even where two
/// alphabets happen to contain the same numeric values.
fn relocated_pointer_candidates(
    source: &[u8],
    shared: bool,
) -> Result<Vec<RelocatedPointerField>, String> {
    let tables = symbol_tables(source);
    let definitions = if shared {
        global_symbol_index(&tables)
    } else {
        BTreeMap::new()
    };
    let mut fields = Vec::new();
    let mut next_key = 0usize;
    for table in tables {
        let Some(relocations) = table.sections.get(b".rel.dyn" as &[u8]) else {
            continue;
        };
        let mut regions = Vec::new();
        for name in [b".rdata" as &[u8], b".data", b".sdata"] {
            if let Some(section) = table.sections.get(name) {
                regions.push((section.virtual_address, section.size, section.offset));
            }
        }
        let blob = &source[table.span.offset..table.span.offset + table.span.length];
        let end = checked_end(relocations.offset, relocations.size / 16 * 16, blob.len())?;
        let mut alphabets: BTreeMap<usize, (usize, Vec<u64>)> = BTreeMap::new();
        for position in (relocations.offset..end).step_by(16) {
            let address = le_u64(blob, position)?;
            let info = le_u32(blob, position + 8)?;
            let symbol = (info >> 8) as usize;
            if info & 255 != 2 || symbol >= table.symbols.len() {
                continue;
            }
            let mut location = None;
            for (virtual_address, size, offset) in &regions {
                let section_end = virtual_address.checked_add(*size as u64);
                if *virtual_address <= address
                    && section_end
                        .is_some_and(|end| address.checked_add(8).is_some_and(|right| right <= end))
                {
                    let relative = usize::try_from(address - *virtual_address)
                        .map_err(|_| "pointer location too large")?;
                    let candidate = offset
                        .checked_add(relative)
                        .ok_or("pointer location overflow")?;
                    if location.replace(candidate).is_some() {
                        location = None;
                        break;
                    }
                }
            }
            let Some(location) = location else { continue };
            let (key, options) = if let Some(entry) = alphabets.get(&symbol) {
                (entry.0, entry.1.clone())
            } else {
                let (name, value) = &table.symbols[symbol];
                let mut options = vec![*value];
                if shared {
                    if let Some(values) = definitions.get(name) {
                        let mut ranked = values
                            .iter()
                            .map(|(candidate, count)| (*candidate, *count))
                            .collect::<Vec<_>>();
                        ranked
                            .sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
                        options.extend(
                            ranked
                                .into_iter()
                                .map(|(candidate, _)| candidate)
                                .filter(|candidate| *candidate != *value),
                        );
                    }
                }
                let key = next_key;
                next_key = next_key.checked_add(1).ok_or("pointer alphabet overflow")?;
                alphabets.insert(symbol, (key, options.clone()));
                (key, options)
            };
            fields.push(RelocatedPointerField {
                where_: table
                    .span
                    .offset
                    .checked_add(location)
                    .ok_or("pointer field overflow")?,
                options,
                key,
                context: u8::from(table.symbols[symbol].1 == 0),
            });
        }
    }
    fields.sort_by_key(|field| field.where_);
    let mut blocked = BTreeSet::new();
    for (index, pair) in fields.windows(2).enumerate() {
        if pair[0]
            .where_
            .checked_add(8)
            .is_none_or(|end| end > pair[1].where_)
        {
            blocked.insert(index);
            blocked.insert(index + 1);
        }
    }
    Ok(fields
        .into_iter()
        .enumerate()
        .filter_map(|(index, field)| (!blocked.contains(&index)).then_some(field))
        .collect())
}

/// Historic ECRP1 representation. Every candidate value is reconstructed
/// from the input's own symbol tables; misses are carried as literal bytes.
pub fn relocated_pointer_transform(
    source: &[u8],
    mode: u8,
    shared: bool,
) -> Result<Vec<u8>, String> {
    if mode > 5 {
        return Err("invalid relocated-pointer representation".into());
    }
    let fields = relocated_pointer_candidates(source, shared)?;
    let mut output = source.to_vec();
    let mut flags = if mode == 1 {
        vec![0u8; fields.len().div_ceil(8)]
    } else {
        Vec::new()
    };
    let mut digits = Vec::new();
    let mut alphabets = Vec::new();
    let mut contexts = Vec::new();
    let mut exceptions = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        let value = le_u64(source, field.where_)?;
        let digit = field
            .options
            .iter()
            .position(|candidate| *candidate == value)
            .unwrap_or(field.options.len());
        if mode == 1 && digit < field.options.len() {
            flags[index / 8] |= 1 << (index % 8);
            write_u64(&mut output, field.where_, digit as u64)?;
        } else if mode >= 2 {
            let end = checked_end(field.where_, 8, output.len())?;
            output[field.where_..end].fill(0);
            digits.push(digit);
            alphabets.push(
                field
                    .options
                    .len()
                    .checked_add(1)
                    .ok_or("pointer alphabet overflow")?,
            );
            contexts.push(field.context);
            if digit == field.options.len() {
                exceptions.extend_from_slice(&source[field.where_..end]);
            }
        }
    }
    let choices = if mode >= 2 {
        encode_choices(&digits, &alphabets, &contexts, mode - 2)?
    } else {
        Vec::new()
    };
    // The nested GOT model always uses shared symbols, independently of the
    // ECRP1 local/shared flag. This is part of the existing reference wire.
    let mut raw = got_transform(&output, 3, true)?;
    let mut metadata = vec![mode, u8::from(shared)];
    metadata.extend_from_slice(&pack(&flags));
    metadata.extend_from_slice(&pack(&choices));
    metadata.extend_from_slice(&exceptions);
    raw.extend_from_slice(&metadata);
    raw.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    raw.extend_from_slice(RELOCATED_POINTER_TRAILER);
    Ok(raw)
}

/// Restore ECRP1 using only its paid skeleton, symbol tables and metadata.
pub fn relocated_pointer_inverse(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (skeleton, metadata) = split_trailer(raw, RELOCATED_POINTER_TRAILER, 4)?;
    let (mode, shared) = (metadata[0], metadata[1]);
    if mode > 5 || shared > 1 {
        return Err("invalid relocated-pointer model".into());
    }
    let (flags, next) = read_packed(metadata, 2)?;
    let (choices, mut position) = read_packed(metadata, next)?;
    let mut output = got_inverse(&skeleton)?;
    let fields = relocated_pointer_candidates(&output, shared != 0)?;
    let expected_flags = if mode == 1 {
        fields.len().div_ceil(8)
    } else {
        0
    };
    if flags.len() != expected_flags {
        return Err("pointer membership length mismatch".into());
    }
    if mode < 2 && !choices.is_empty() {
        return Err("unexpected pointer choices".into());
    }
    let digits = if mode >= 2 {
        let alphabets = fields
            .iter()
            .map(|field| {
                field
                    .options
                    .len()
                    .checked_add(1)
                    .ok_or_else(|| "pointer alphabet overflow".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let contexts = fields.iter().map(|field| field.context).collect::<Vec<_>>();
        decode_choices(choices, &alphabets, &contexts, mode - 2)?
    } else {
        Vec::new()
    };
    for (index, field) in fields.iter().enumerate() {
        if mode == 1 && flags[index / 8] & (1 << (index % 8)) != 0 {
            let digit = usize::try_from(le_u64(&output, field.where_)?)
                .map_err(|_| "pointer index too large")?;
            let value = *field
                .options
                .get(digit)
                .ok_or("pointer index outside alphabet")?;
            write_u64(&mut output, field.where_, value)?;
        } else if mode >= 2 {
            let end = checked_end(field.where_, 8, output.len())?;
            if output[field.where_..end].iter().any(|byte| *byte != 0) {
                return Err("pointer skeleton is not zero".into());
            }
            let digit = digits[index];
            if digit == field.options.len() {
                let next = checked_end(position, 8, metadata.len())
                    .map_err(|_| "truncated pointer exception")?;
                output[field.where_..end].copy_from_slice(&metadata[position..next]);
                position = next;
            } else {
                let value = *field
                    .options
                    .get(digit)
                    .ok_or("pointer index outside alphabet")?;
                write_u64(&mut output, field.where_, value)?;
            }
        }
    }
    if position != metadata.len() {
        return Err("unused pointer metadata".into());
    }
    Ok(output)
}

fn pointer_prediction(
    mode: u8,
    last: u64,
    by_key: &BTreeMap<usize, u64>,
    field: &RelocatedPointerField,
) -> u64 {
    match mode {
        2 | 5 => *by_key.get(&field.key).unwrap_or(&field.options[0]),
        3 => field.options[0],
        _ => last,
    }
}

fn pointer_delta_process(
    source: &[u8],
    mode: u8,
    inverse: bool,
    residuals: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), String> {
    if mode > 5 {
        return Err("invalid pointer-delta model".into());
    }
    let fields = relocated_pointer_candidates(source, true)?;
    let mut output = source.to_vec();
    let mut side = Vec::new();
    let mut side_position = 0usize;
    let mut last = 0u64;
    let mut by_key = BTreeMap::new();
    for field in fields {
        let prediction = pointer_prediction(mode, last, &by_key, &field);
        let value = if inverse {
            let residual = if mode < 4 {
                le_u64(&output, field.where_)?
            } else {
                if output[field.where_..field.where_ + 8]
                    .iter()
                    .any(|byte| *byte != 0)
                {
                    return Err("pointer-delta skeleton is not zero".into());
                }
                let (residual, next) = vdecode(residuals, side_position)?;
                side_position = next;
                residual
            };
            let value = if mode == 0 {
                residual
            } else {
                zigzag_delta(residual, prediction, 64, true)
            };
            write_u64(&mut output, field.where_, value)?;
            value
        } else {
            let value = le_u64(source, field.where_)?;
            let residual = if mode == 0 {
                value
            } else {
                zigzag_delta(value, prediction, 64, false)
            };
            if mode >= 4 {
                output[field.where_..field.where_ + 8].fill(0);
                side.extend_from_slice(&vencode(residual));
            } else if mode != 0 {
                write_u64(&mut output, field.where_, residual)?;
            }
            value
        };
        last = value;
        by_key.insert(field.key, value);
    }
    if inverse && side_position != residuals.len() {
        return Err("unused pointer delta bytes".into());
    }
    Ok((output, side))
}

pub fn pointer_delta_transform(source: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    let (raw, residuals) = pointer_delta_process(source, mode, false, &[])?;
    let skeleton = got_transform(&raw, 3, true)?;
    let mut metadata = vec![mode];
    metadata.extend_from_slice(&pack(&residuals));
    let mut output = skeleton;
    output.extend_from_slice(&metadata);
    output.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    output.extend_from_slice(POINTER_DELTA_TRAILER);
    Ok(output)
}

pub fn pointer_delta_inverse(raw: &[u8]) -> Result<Vec<u8>, String> {
    let (skeleton, metadata) = split_trailer(raw, POINTER_DELTA_TRAILER, 2)?;
    let mode = metadata[0];
    if mode > 5 {
        return Err("invalid pointer-delta model".into());
    }
    let (residuals, position) = read_packed(metadata, 1)?;
    if position != metadata.len() || (mode < 4 && !residuals.is_empty()) {
        return Err("invalid pointer-delta residual stream".into());
    }
    let data = got_inverse(&skeleton)?;
    Ok(pointer_delta_process(&data, mode, true, residuals)?.0)
}

fn transform_encode(transform: Transform, source: &[u8]) -> Result<Vec<u8>, String> {
    match transform {
        // The constraints stage is independently reversible and is the parent
        // of fields/GOT/pointer/call/section/pdata transforms in the reference.
        Transform::Constraints { mode } => constraints_transform(source, mode),
        Transform::Identity => Ok(source.to_vec()),
        Transform::Fields { mode } => fields_transform(source, mode),
        Transform::Got { mode } => got_transform(source, mode, true),
        Transform::RelocatedPointers { mode, shared } => {
            relocated_pointer_transform(source, mode, shared)
        }
        Transform::PointerDeltas { mode } => pointer_delta_transform(source, mode),
        Transform::SectionPointers { mode } => section_pointer_transform(source, mode),
        Transform::PdataSymbols { mode } => pdata_symbols_transform(source, mode),
    }
}

fn transform_decode(transform: Transform, raw: &[u8]) -> Result<Vec<u8>, String> {
    match transform {
        Transform::Constraints { .. } => constraints_inverse(raw),
        Transform::Identity => Ok(raw.to_vec()),
        Transform::Fields { .. } => fields_inverse(raw),
        Transform::Got { .. } => got_inverse(raw),
        Transform::RelocatedPointers { .. } => relocated_pointer_inverse(raw),
        Transform::PointerDeltas { .. } => pointer_delta_inverse(raw),
        Transform::SectionPointers { .. } => section_pointer_inverse(raw),
        Transform::PdataSymbols { .. } => pdata_symbols_inverse(raw),
    }
}

/// Encode CIXB13 and CIXB1d frames.  The checked header always precedes the
/// payload, including mode two's identity transform, so the source length and
/// checksum remain paid and independently verifiable.
pub fn encode_checked_frame(
    frame: HistoricalFrame,
    transform: Transform,
    source: &[u8],
    mode: u8,
    request: BackendRequest,
    backend: &mut EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    let magic = match frame {
        HistoricalFrame::JointDiscount if mode < 5 => JOINT_MAGIC,
        HistoricalFrame::Filtered if mode < 3 => FILTER_MAGIC,
        _ => return Err("invalid checked executable frame mode".into()),
    };
    match frame {
        HistoricalFrame::JointDiscount
            if !matches!(
                request.variant,
                BackendVariant::PaqJointDiscount | BackendVariant::PaqV215
            ) =>
        {
            return Err("joint-discount frame requires its declared PAQ backend".into())
        }
        HistoricalFrame::Filtered
            if !matches!(
                request.variant,
                BackendVariant::Brotli | BackendVariant::PaqV216 | BackendVariant::PaqStoreState
            ) =>
        {
            return Err("filtered frame requires Brotli or its declared PAQ stage".into())
        }
        _ => {}
    }
    let expected = match (frame, mode) {
        (HistoricalFrame::JointDiscount, _) => Transform::SectionPointers { mode: 4 },
        (HistoricalFrame::Filtered, 2) => Transform::Identity,
        (HistoricalFrame::Filtered, _) => Transform::PdataSymbols { mode: 1 },
        _ => unreachable!(),
    };
    if transform != expected {
        return Err("checked executable transform does not match frame mode".into());
    }
    let raw = transform_encode(transform, source)?;
    let mut output = Vec::new();
    output.extend_from_slice(magic);
    output.push(mode);
    output.extend_from_slice(&vencode(source.len() as u64));
    output.extend_from_slice(&crc32(source).to_be_bytes());
    output.extend_from_slice(backend(&request, &raw)?.as_slice());
    Ok(output)
}

/// Decode CIXB13/CIXB1d after the full engine has selected the concrete
/// native backend stack from the frame family and mode.  This keeps nested
/// PAQ/Brotli staging explicit rather than making the generic decoder guess.
pub fn decode_checked_frame(
    frame: HistoricalFrame,
    bytes: &[u8],
    max_output: usize,
    memory_limit: usize,
    request: BackendRequest,
    backend: &mut DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if memory_limit == 0 {
        return Err("invalid executable decode memory limit".into());
    }
    let (magic, allowed) = match frame {
        HistoricalFrame::JointDiscount => (JOINT_MAGIC, 0..5),
        HistoricalFrame::Filtered => (FILTER_MAGIC, 0..3),
        _ => return Err("wrong checked executable frame family".into()),
    };
    if !bytes.starts_with(magic) {
        return Err("invalid checked executable frame".into());
    }
    let mode = *bytes
        .get(magic.len())
        .ok_or("truncated checked executable mode")?;
    if !allowed.contains(&mode) {
        return Err("unsupported checked executable mode".into());
    }
    let (declared, position) = vdecode(bytes, magic.len() + 1)?;
    let declared = usize::try_from(declared).map_err(|_| "executable output too large")?;
    if declared > max_output {
        return Err("executable output exceeds limit".into());
    }
    let checksum_end = position.checked_add(4).ok_or("checksum overflow")?;
    let checksum = u32::from_be_bytes(
        bytes
            .get(position..checksum_end)
            .ok_or("truncated executable checksum")?
            .try_into()
            .map_err(|_| "invalid checksum")?,
    );
    let transform = match (frame, mode) {
        (HistoricalFrame::Filtered, 2) => Transform::Identity,
        (HistoricalFrame::Filtered, _) => Transform::PdataSymbols { mode: 1 },
        (HistoricalFrame::JointDiscount, _) => Transform::SectionPointers { mode: 4 },
        _ => unreachable!(),
    };
    // Keep the paid restored source live while the backend result is inverted:
    // give the backend only the remaining workspace, then verify its callback
    // contract before any transform allocates its own reconstruction state.
    let backend_limit = address_inverse_workspace(declared, memory_limit)?;
    let raw = backend(&request, &bytes[checksum_end..], backend_limit)?;
    if raw.len() > backend_limit {
        return Err("backend ignored executable workspace limit".into());
    }
    if transform == Transform::Identity && raw.len() != declared {
        return Err("identity executable raw length differs from declaration".into());
    }
    let restored = transform_decode(transform, &raw)?;
    if restored.len() > memory_limit {
        return Err("legacy output exceeds memory limit".into());
    }
    verify(restored, declared, checksum)
}

/// Encode CIXB03, CIXB0a, CIXB0b and CIXB0c.  The caller picks a real native backend;
/// a mismatch is an error rather than a hidden route substitution.
pub fn encode_transformed_frame(
    frame: HistoricalFrame,
    transform: Transform,
    source: &[u8],
    request: BackendRequest,
    backend: &mut EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    let (magic, mode_prefix) = match frame {
        HistoricalFrame::Got => (
            GOT_MAGIC,
            Some(match transform {
                Transform::Got { mode } => mode.checked_add(2).ok_or("GOT mode overflow")?,
                _ => return Err("GOT frame requires GOT transform".into()),
            }),
        ),
        HistoricalFrame::Relocated => {
            if !matches!(transform, Transform::RelocatedPointers { .. }) {
                return Err("relocated frame requires relocated-pointer transform".into());
            }
            (RELOCATED_MAGIC, None)
        }
        HistoricalFrame::Pointer => (POINTER_MAGIC, None),
        HistoricalFrame::Section => (SECTION_MAGIC, None),
        _ => return Err("wrong executable frame family".into()),
    };
    let raw = transform_encode(transform, source)?;
    let mut output = Vec::new();
    output.extend_from_slice(magic);
    if let Some(mode) = mode_prefix {
        if mode > 6 {
            return Err("invalid GOT frame mode".into());
        }
        output.push(mode);
    }
    output.push(backend_selector(request.variant)?);
    output.extend_from_slice(&vencode(source.len() as u64));
    output.extend_from_slice(&crc32(source).to_be_bytes());
    output.extend_from_slice(backend(&request, &raw)?.as_slice());
    Ok(output)
}

/// Decode historic transformed frames.  The supplied callback is given the
/// declared output bound, and is responsible for bounded native decompression.
pub fn decode_transformed_frame(
    frame: HistoricalFrame,
    bytes: &[u8],
    max_output: usize,
    memory_limit: usize,
    backend: &mut DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if memory_limit == 0 {
        return Err("invalid executable decode memory limit".into());
    }
    let (magic, transform, selector_position) = match frame {
        HistoricalFrame::Got => {
            if bytes.len() < 7 || !bytes.starts_with(GOT_MAGIC) || bytes[5] > 6 {
                return Err("invalid CIXB03 GOT frame".into());
            }
            let mode = bytes[5];
            let transform = match mode {
                0 => Transform::Fields { mode: 0 },
                1 => Transform::Identity,
                value => Transform::Got { mode: value - 2 },
            };
            (GOT_MAGIC, transform, 6)
        }
        HistoricalFrame::Relocated => (
            RELOCATED_MAGIC,
            Transform::RelocatedPointers {
                mode: 5,
                shared: true,
            },
            5,
        ),
        HistoricalFrame::Pointer => (POINTER_MAGIC, Transform::PointerDeltas { mode: 5 }, 5),
        HistoricalFrame::Section => (SECTION_MAGIC, Transform::SectionPointers { mode: 4 }, 5),
        _ => return Err("wrong executable frame family".into()),
    };
    if bytes.len() < selector_position + 1 || !bytes.starts_with(magic) {
        return Err("invalid transformed executable frame".into());
    }
    let variant = selected_backend(bytes[selector_position])?;
    let (declared, position) = vdecode(bytes, selector_position + 1)?;
    let declared = usize::try_from(declared).map_err(|_| "executable output too large")?;
    if declared > max_output {
        return Err("executable output exceeds limit".into());
    }
    let checksum_end = position.checked_add(4).ok_or("checksum overflow")?;
    let checksum = u32::from_be_bytes(
        bytes
            .get(position..checksum_end)
            .ok_or("truncated executable checksum")?
            .try_into()
            .map_err(|_| "invalid checksum")?,
    );
    let payload_start = checksum_end;
    let backend_limit = address_inverse_workspace(declared, memory_limit)?;
    let raw = backend(
        &BackendRequest {
            variant,
            options: "-8".into(),
            stream: 0,
        },
        &bytes[payload_start..],
        backend_limit,
    )?;
    if raw.len() > backend_limit {
        return Err("backend ignored executable workspace limit".into());
    }
    if transform == Transform::Identity && raw.len() != declared {
        return Err("identity executable raw length differs from declaration".into());
    }
    let restored = transform_decode(transform, &raw)?;
    if restored.len() > memory_limit {
        return Err("legacy output exceeds memory limit".into());
    }
    verify(restored, declared, checksum)
}

/// Recognize only complete binary historic prefixes.  ASCII CIXB1 is handled
/// by the generic container decoder and must not be misclassified here.
pub fn recognizes_magic(prefix: &[u8]) -> bool {
    [
        JOINT_MAGIC,
        FILTER_MAGIC,
        GOT_MAGIC,
        RELOCATED_MAGIC,
        POINTER_MAGIC,
        SECTION_MAGIC,
    ]
    .iter()
    .any(|magic| prefix.starts_with(*magic))
}

/// A small, deterministic admission report suitable for selector diagnostics.
pub fn admission(source: &[u8]) -> BTreeSet<String> {
    let objects = discover_ecoff_objects(source);
    let executable_regions = discover_executable_regions(source);
    let mut result = BTreeSet::new();
    if !objects.is_empty() {
        result.insert("ecoff-object".into());
    }
    if objects.iter().any(|object| object.span.tar_member) {
        result.insert("tar-embedded-ecoff".into());
    }
    if objects
        .iter()
        .any(|object| object.sections.contains_key(b".dynsym" as &[u8]))
    {
        result.insert("ecoff-symbols".into());
    }
    if objects
        .iter()
        .any(|object| object.sections.contains_key(b".rel.dyn" as &[u8]))
    {
        result.insert("ecoff-relocations".into());
    }
    if executable_regions
        .iter()
        .any(|region| region.kind == ExecutableKind::Elf)
    {
        result.insert("elf-object".into());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        crc32, decode_checked_frame, decode_choices, decode_transformed_frame, encode_choices,
        fields_transform, got_inverse, got_transform, pdata_inverse, pdata_symbols_inverse,
        pdata_symbols_transform, pdata_transform, pointer_delta_inverse, pointer_delta_transform,
        section_pointer_inverse, section_pointer_transform, symbol_load_inverse,
        symbol_load_transform, vencode, BackendRequest, BackendVariant, HistoricalFrame,
        FILTER_MAGIC, GOT_MAGIC,
    };

    #[test]
    fn got_choice_encodings_round_trip_all_modes() {
        let digits = [0, 2, 1, 0, 3];
        let alphabets = [1, 3, 4, 2, 5];
        let contexts = [0, 1, 0, 1, 0];
        for mode in 0..4 {
            let encoded = encode_choices(&digits, &alphabets, &contexts, mode).unwrap();
            assert_eq!(
                decode_choices(&encoded, &alphabets, &contexts, mode).unwrap(),
                digits
            );
        }
    }

    #[test]
    fn got_choices_match_frozen_reference_bytes() {
        // Frozen reference ecoff_got.py golden vectors, including repeated
        // context observations that distinguish static and adaptive arithmetic.
        let digits = [0, 2, 1, 0, 3];
        let expected: [&[u8]; 4] = [&[0, 2, 1, 0, 3], &[0x4d], &[8, 0xc7], &[8, 0xc7]];
        for mode in 0..4 {
            assert_eq!(
                encode_choices(&digits, &[1, 3, 4, 2, 5], &[0, 1, 0, 1, 0], mode).unwrap(),
                expected[mode as usize]
            );
        }
        let digits = [0, 1, 1, 1, 2, 0, 2, 2, 2, 2, 1, 0];
        let expected: [&[u8]; 4] = [
            &[0, 1, 1, 1, 2, 0, 2, 2, 2, 2, 1, 0],
            &[0x42, 0xcb, 1],
            &[0x14, 0x2b, 0x8a, 0xb0],
            &[0x17, 0x39, 0xda, 0x52],
        ];
        for mode in 0..4 {
            assert_eq!(
                encode_choices(&digits, &[3; 12], &[0; 12], mode).unwrap(),
                expected[mode as usize]
            );
            assert_eq!(
                decode_choices(expected[mode as usize], &[3; 12], &[0; 12], mode).unwrap(),
                digits
            );
        }
    }

    #[test]
    fn got_empty_admission_keeps_complete_paid_transform() {
        let source = b"ordinary bytes without an ECOFF object";
        for mode in 0..4 {
            let transformed = got_transform(source, mode, true).unwrap();
            assert_eq!(got_inverse(&transformed).unwrap(), source);
        }
    }

    #[test]
    fn got_wire_mode_zero_restores_the_fields_parent() {
        let source = b"CIXB03 historical fields-parent fixture";
        let payload = fields_transform(source, 1).unwrap();
        let mut frame = GOT_MAGIC.to_vec();
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&vencode(source.len() as u64));
        frame.extend_from_slice(&crc32(source).to_be_bytes());
        frame.extend_from_slice(&payload);
        let mut backend = |_request: &_, encoded: &[u8], _limit: usize| Ok(encoded.to_vec());
        assert_eq!(
            decode_transformed_frame(
                HistoricalFrame::Got,
                &frame,
                source.len(),
                4096,
                &mut backend
            )
            .unwrap(),
            source,
        );
    }

    #[test]
    fn malformed_got_choice_lengths_are_rejected() {
        assert!(decode_choices(&[0x80], &[2], &[0], 0).is_err());
        // One declared bit with no payload is truncated. A zero-bit arithmetic
        // stream is accepted by the historical decoder's zero padding rule.
        assert!(decode_choices(&[0x01], &[2], &[0], 2).is_err());
        assert!(decode_choices(&vencode(u64::MAX), &[2], &[0], 2).is_err());
    }

    #[test]
    fn finite_word_delta_preserves_sign_boundary() {
        for (value, previous, expected) in [
            (1u64 << 63, 0, u64::MAX),
            (0, 1u64 << 63, u64::MAX),
            (u64::MAX, 0, 1),
            (0, u64::MAX, 2),
        ] {
            assert_eq!(super::zigzag_delta(value, previous, 64, false), expected);
            assert_eq!(super::zigzag_delta(expected, previous, 64, true), value);
        }
    }

    #[test]
    fn pointer_delta_empty_admission_round_trips_all_modes() {
        let source = b"non-executable pointer-delta fixture";
        for mode in 0..6 {
            let transformed = pointer_delta_transform(source, mode).unwrap();
            assert_eq!(pointer_delta_inverse(&transformed).unwrap(), source);
        }
    }

    #[test]
    fn pointer_delta_rejects_malformed_residual_metadata() {
        let source = b"pointer metadata fixture";
        let mut transformed = pointer_delta_transform(source, 4).unwrap();
        let trailer = transformed.len() - 13;
        transformed[trailer] = 4;
        transformed[trailer + 1] = 0x80;
        assert!(pointer_delta_inverse(&transformed).is_err());
    }

    #[test]
    fn section_pointer_mode_four_keeps_paid_empty_admission_reversible() {
        let source = b"non-executable section-pointer fixture";
        let transformed = section_pointer_transform(source, 4).unwrap();
        assert!(transformed.ends_with(b"ECSP1"));
        assert_eq!(section_pointer_inverse(&transformed).unwrap(), source);
        let footer = transformed.len() - 13;
        let metadata_size =
            u64::from_le_bytes(transformed[footer..footer + 8].try_into().unwrap()) as usize;
        let metadata_start = footer - metadata_size;
        let mut bad_mode = transformed.clone();
        let original_mode = bad_mode[metadata_start];
        bad_mode[metadata_start] = 255;
        assert_ne!(bad_mode[metadata_start], original_mode);
        assert!(section_pointer_inverse(&bad_mode).is_err());
        let mut bad_length = transformed;
        bad_length[footer] = bad_length[footer].checked_add(1).unwrap();
        assert!(section_pointer_inverse(&bad_length).is_err());
    }

    #[test]
    fn pdata_symbols_mode_one_keeps_nested_empty_admission_reversible() {
        let source = b"non-executable pdata-symbol fixture";
        let transformed = pdata_symbols_transform(source, 1).unwrap();
        assert!(transformed.ends_with(b"ECPD2"));
        assert_eq!(pdata_symbols_inverse(&transformed).unwrap(), source);
        let mut malformed = transformed;
        let mode = malformed.len() - 6;
        malformed[mode] = 2;
        assert!(pdata_symbols_inverse(&malformed).is_err());
    }

    #[test]
    fn frozen_ecoff_relation_vectors_match_native_raw_transforms_and_inverses() {
        const SOURCE: &[u8] = include_bytes!("../../tests/fixtures/address_relations/source.bin");
        const AMBIGUOUS: &[u8] =
            include_bytes!("../../tests/fixtures/address_relations/ambiguous_source.bin");
        assert_eq!(
            pointer_delta_transform(SOURCE, 5).unwrap(),
            include_bytes!("../../tests/fixtures/address_relations/pointer_delta_mode5.raw")
        );
        assert_eq!(
            section_pointer_transform(SOURCE, 4).unwrap(),
            include_bytes!("../../tests/fixtures/address_relations/section_mode4.raw")
        );
        assert_eq!(
            symbol_load_transform(SOURCE, 2).unwrap(),
            include_bytes!("../../tests/fixtures/address_relations/symbol_load_mode2.raw")
        );
        assert_eq!(
            symbol_load_transform(AMBIGUOUS, 2).unwrap(),
            include_bytes!(
                "../../tests/fixtures/address_relations/ambiguous_symbol_load_mode2.raw"
            )
        );
        assert_eq!(
            pdata_transform(SOURCE, 1).unwrap(),
            include_bytes!("../../tests/fixtures/address_relations/pdata_mode1.raw")
        );
        assert_eq!(
            pdata_symbols_transform(SOURCE, 1).unwrap(),
            include_bytes!("../../tests/fixtures/address_relations/pdata_symbols_mode1.raw")
        );
        assert_eq!(
            pointer_delta_inverse(include_bytes!(
                "../../tests/fixtures/address_relations/pointer_delta_mode5.raw"
            ))
            .unwrap(),
            SOURCE
        );
        assert_eq!(
            section_pointer_inverse(include_bytes!(
                "../../tests/fixtures/address_relations/section_mode4.raw"
            ))
            .unwrap(),
            SOURCE
        );
        assert_eq!(
            symbol_load_inverse(include_bytes!(
                "../../tests/fixtures/address_relations/symbol_load_mode2.raw"
            ))
            .unwrap(),
            SOURCE
        );
        assert_eq!(
            symbol_load_inverse(include_bytes!(
                "../../tests/fixtures/address_relations/ambiguous_symbol_load_mode2.raw"
            ))
            .unwrap(),
            AMBIGUOUS
        );
        assert_eq!(
            pdata_inverse(include_bytes!(
                "../../tests/fixtures/address_relations/pdata_mode1.raw"
            ))
            .unwrap(),
            SOURCE
        );
        assert_eq!(
            pdata_symbols_inverse(include_bytes!(
                "../../tests/fixtures/address_relations/pdata_symbols_mode1.raw"
            ))
            .unwrap(),
            SOURCE
        );
    }

    #[test]
    fn address_inverse_workspace_rejects_paid_lengths_before_backend_allocation() {
        assert!(super::address_inverse_workspace(1024, 6 * 1024).is_ok());
        assert!(super::address_inverse_workspace(1025, 6 * 1024).is_err());
        assert!(super::address_inverse_workspace(usize::MAX, usize::MAX).is_err());
    }

    #[test]
    fn identity_checked_frame_rejects_mismatched_raw_before_clone() {
        let source = b"x";
        let mut frame = FILTER_MAGIC.to_vec();
        frame.push(2);
        frame.extend(vencode(1));
        frame.extend(crc32(source).to_be_bytes());
        frame.push(0);
        let request = BackendRequest {
            variant: BackendVariant::Brotli,
            options: String::new(),
            stream: 0,
        };
        let mut backend = |_request: &_, _payload: &[u8], limit: usize| {
            assert_eq!(limit, 94);
            Ok(vec![0; 2])
        };
        assert!(decode_checked_frame(
            HistoricalFrame::Filtered,
            &frame,
            1,
            100,
            request,
            &mut backend
        )
        .is_err());
    }
}
