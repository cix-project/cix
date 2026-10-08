//! Retained CIXH1 heterogeneous carrier, with explicitly supplied codecs.
//!
//! The carrier charges region boundaries, parameters, payloads and checksums.
//! It does not discover regions or choose a codec. Those are separate catalogue
//! operations. Input/archive bytes count toward `memory_limit`, even though
//! they are borrowed. A provider must obey the additional workspace allowance
//! supplied to it; the checks here bound carrier-owned buffers, not process RSS.

use sha2::{Digest, Sha256};
use std::ops::Range;

pub const MAGIC: &[u8; 5] = b"CIXH1";
const HEADER: usize = 50;
const REGION_HEADER: usize = 43;
pub const MAX_REGIONS: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RegionBackend {
    Stored = 0,
    Zlib = 1,
    Xz = 2,
    Specialist = 128,
}

impl TryFrom<u8> for RegionBackend {
    type Error = String;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Stored),
            1 => Ok(Self::Zlib),
            2 => Ok(Self::Xz),
            128 => Ok(Self::Specialist),
            _ => Err("unknown CIXH1 region backend".into()),
        }
    }
}

pub struct RegionChoice {
    pub backend: RegionBackend,
    pub parameters: Vec<u8>,
    pub payload: Vec<u8>,
}

fn checked_sum(parts: &[usize]) -> Result<usize, String> {
    parts.iter().try_fold(0usize, |total, &value| {
        total
            .checked_add(value)
            .ok_or_else(|| "CIXH1 size overflow".into())
    })
}

fn reserve(buffer: &mut Vec<u8>, additional: usize) -> Result<(), String> {
    buffer
        .try_reserve_exact(additional)
        .map_err(|_| "CIXH1 allocation failed".into())
}

fn validate_parameters(backend: RegionBackend, parameters: &[u8]) -> Result<(), String> {
    let valid = match backend {
        RegionBackend::Stored => parameters.is_empty(),
        RegionBackend::Zlib => parameters == b"level=6",
        RegionBackend::Xz => parameters == b"preset=3",
        // Historic specialist names are descriptive; the nested frame carries
        // its actual wire identity and must select its own decoder.
        RegionBackend::Specialist => true,
    };
    if valid {
        Ok(())
    } else {
        Err("unsupported CIXH1 region parameters".into())
    }
}

/// Encode contiguous, complete input-derived ranges. `choose` receives its
/// temporary workspace allowance after retained source/output reservations.
/// Its returned payload and parameters must fit within that allowance.
pub fn encode_with(
    source: &[u8],
    ranges: &[Range<usize>],
    output_limit: usize,
    memory_limit: usize,
    mut choose: impl FnMut(&[u8], usize) -> Result<RegionChoice, String>,
) -> Result<Vec<u8>, String> {
    if ranges.len() > MAX_REGIONS || HEADER > output_limit {
        return Err("CIXH1 frame count or output limit".into());
    }
    let mut end = 0;
    for range in ranges {
        if range.start != end
            || range.end <= range.start
            || range.end > source.len()
            || range.len() > u32::MAX as usize
        {
            return Err("CIXH1 ranges must partition every input byte".into());
        }
        end = range.end;
    }
    if end != source.len() {
        return Err("incomplete CIXH1 input coverage".into());
    }
    if checked_sum(&[source.len(), HEADER])? > memory_limit {
        return Err("CIXH1 memory limit".into());
    }
    let mut output = Vec::new();
    reserve(&mut output, HEADER)?;
    output.extend_from_slice(MAGIC);
    output.push(1);
    output.extend_from_slice(&(source.len() as u64).to_le_bytes());
    output.extend_from_slice(&(ranges.len() as u32).to_le_bytes());
    output.extend_from_slice(&Sha256::digest(source));
    for range in ranges {
        let region = &source[range.clone()];
        let retained = checked_sum(&[source.len(), output.capacity()])?;
        let available = memory_limit
            .checked_sub(retained)
            .ok_or("CIXH1 memory limit")?;
        let choice = choose(region, available)?;
        validate_parameters(choice.backend, &choice.parameters)?;
        if choice.parameters.len() > u16::MAX as usize || choice.payload.len() > u32::MAX as usize {
            return Err("CIXH1 region framing limit".into());
        }
        if choice.backend == RegionBackend::Stored && choice.payload != region {
            return Err("CIXH1 stored region must equal its source".into());
        }
        let frame_size =
            checked_sum(&[REGION_HEADER, choice.parameters.len(), choice.payload.len()])?;
        let next_size = checked_sum(&[output.len(), frame_size])?;
        let temporary = checked_sum(&[choice.parameters.capacity(), choice.payload.capacity()])?;
        // A Vec growth may coexist with its old allocation. Charge both until
        // reserve finishes, then retain only the new output allocation.
        if next_size > output_limit
            || checked_sum(&[retained, next_size, temporary])? > memory_limit
        {
            return Err("CIXH1 output or simultaneous-buffer limit".into());
        }
        reserve(&mut output, frame_size)?;
        output.push(choice.backend as u8);
        output.extend_from_slice(&(region.len() as u32).to_le_bytes());
        output.extend_from_slice(&(choice.payload.len() as u32).to_le_bytes());
        output.extend_from_slice(&(choice.parameters.len() as u16).to_le_bytes());
        output.extend_from_slice(&Sha256::digest(region));
        output.extend_from_slice(&choice.parameters);
        output.extend_from_slice(&choice.payload);
    }
    Ok(output)
}

/// Restore exactly one CIXH1 member. A nested provider receives the paid source
/// length and workspace remaining after archive plus final-output allocation.
/// This API never uses an encoder-side region name to select the nested decoder.
pub fn decode_with(
    archive: &[u8],
    output_limit: usize,
    memory_limit: usize,
    mut decode: impl FnMut(RegionBackend, &[u8], &[u8], usize, usize) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    if archive.len() < HEADER || &archive[..5] != MAGIC || archive[5] != 1 {
        return Err("invalid CIXH1 header".into());
    }
    let expected = usize::try_from(u64::from_le_bytes(archive[6..14].try_into().unwrap()))
        .map_err(|_| "CIXH1 output length overflow")?;
    let count = u32::from_le_bytes(archive[14..18].try_into().unwrap()) as usize;
    if count > MAX_REGIONS
        || expected > output_limit
        || checked_sum(&[archive.len(), expected])? > memory_limit
        || count > (archive.len() - HEADER) / REGION_HEADER
    {
        return Err("CIXH1 count or resource limit".into());
    }
    let mut output = Vec::new();
    reserve(&mut output, expected)?;
    let workspace = memory_limit
        .checked_sub(checked_sum(&[archive.len(), output.capacity()])?)
        .ok_or("CIXH1 memory limit")?;
    let mut position = HEADER;
    for _ in 0..count {
        let header_end = position
            .checked_add(REGION_HEADER)
            .ok_or("CIXH1 offset overflow")?;
        let header = archive
            .get(position..header_end)
            .ok_or("truncated CIXH1 region")?;
        let backend = RegionBackend::try_from(header[0])?;
        let source_len = u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize;
        let payload_len = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
        let parameter_len = u16::from_le_bytes(header[9..11].try_into().unwrap()) as usize;
        let parameter_end = checked_sum(&[header_end, parameter_len])?;
        let end = checked_sum(&[parameter_end, payload_len])?;
        if end > archive.len() || source_len > expected - output.len() {
            return Err("invalid CIXH1 region length".into());
        }
        let parameters = &archive[header_end..parameter_end];
        let payload = &archive[parameter_end..end];
        validate_parameters(backend, parameters)?;
        if backend == RegionBackend::Stored {
            if payload.len() != source_len || Sha256::digest(payload).as_slice() != &header[11..43]
            {
                return Err("invalid CIXH1 stored region length or checksum".into());
            }
            output.extend_from_slice(payload);
        } else {
            if source_len > workspace {
                return Err("CIXH1 region workspace limit".into());
            }
            let restored = decode(backend, parameters, payload, source_len, workspace)?;
            if restored.capacity() > workspace
                || restored.len() != source_len
                || Sha256::digest(&restored).as_slice() != &header[11..43]
            {
                return Err("CIXH1 region resource, length or checksum mismatch".into());
            }
            output.extend_from_slice(&restored);
        }
        position = end;
    }
    if position != archive.len()
        || output.len() != expected
        || Sha256::digest(&output).as_slice() != &archive[18..50]
    {
        return Err("CIXH1 archive length or checksum mismatch".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(data: &[u8], _: usize) -> Result<RegionChoice, String> {
        Ok(RegionChoice {
            backend: RegionBackend::Stored,
            parameters: vec![],
            payload: data.to_vec(),
        })
    }
    fn no_provider(
        _: RegionBackend,
        _: &[u8],
        _: &[u8],
        _: usize,
        _: usize,
    ) -> Result<Vec<u8>, String> {
        panic!("stored region must not call any provider")
    }

    #[test]
    fn raw_regions_match_independent_struct_reference() {
        let source = b"prefix\0middle\xfftail";
        let archive =
            encode_with(source, &[0..7, 7..14, 14..source.len()], 1024, 4096, stored).unwrap();
        let reference = include_bytes!("../../tests/fixtures/native_ports/mixed-raw.cix");
        assert_eq!(archive, reference);
        assert_eq!(
            decode_with(reference, source.len(), 4096, no_provider).unwrap(),
            source
        );
    }

    #[test]
    fn corrupt_lengths_checksums_and_incomplete_partitions_are_rejected() {
        let source = b"independent integrity checks";
        let range = 0..source.len();
        let good = encode_with(source, std::slice::from_ref(&range), 1024, 4096, stored).unwrap();
        for at in [5, 6, 14, 18, 50, 51, 55, 59, 61, good.len() - 1] {
            let mut bad = good.clone();
            bad[at] ^= 1;
            assert!(
                decode_with(&bad, 1024, 4096, no_provider).is_err(),
                "byte {at}"
            );
        }
        assert!(decode_with(&good[..good.len() - 1], 1024, 4096, no_provider).is_err());
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(decode_with(&trailing, 1024, 4096, no_provider).is_err());
        assert!(decode_with(&good, source.len() - 1, 4096, no_provider).is_err());
        assert!(decode_with(&good, 1024, good.len() + source.len() - 1, no_provider).is_err());
        let range = 1..source.len();
        assert!(encode_with(source, std::slice::from_ref(&range), 1024, 4096, stored).is_err());
    }

    #[test]
    fn empty_carrier_and_nested_output_bounds() {
        let empty = encode_with(b"", &[], 50, 50, stored).unwrap();
        assert_eq!(decode_with(&empty, 0, 50, no_provider).unwrap(), b"");
        let range = 0..3;
        let nested = encode_with(b"abc", std::slice::from_ref(&range), 1024, 4096, |_, _| {
            Ok(RegionChoice {
                backend: RegionBackend::Specialist,
                parameters: b"description".to_vec(),
                payload: b"nested".to_vec(),
            })
        })
        .unwrap();
        let mut calls = 0;
        let restored = decode_with(&nested, 3, 4096, |backend, _, payload, size, budget| {
            calls += 1;
            assert_eq!(backend, RegionBackend::Specialist);
            assert_eq!(payload, b"nested");
            assert_eq!(size, 3);
            assert!(budget >= 3);
            Ok(b"abc".to_vec())
        })
        .unwrap();
        assert_eq!(restored, b"abc");
        assert_eq!(calls, 1);
        assert!(decode_with(&nested, 3, 4096, |_, _, _, _, _| Ok(b"wrong".to_vec())).is_err());
    }
}
