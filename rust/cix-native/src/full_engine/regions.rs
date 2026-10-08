//! Bounded, content-derived structural spans used by the historical CIXH1
//! planner.  Discovery only partitions bytes: it neither selects a codec nor
//! recursively interprets nested members.

pub const MAX_GENERIC_REGION: usize = 64 * 1024;
pub const MAX_STRUCTURED_REGIONS: usize = 256;
pub const MAX_DICOM_HEADER: usize = 1 << 20;
pub const MAX_SDF_RECORD: usize = 16 << 20;
pub const MAX_ELF_TABLE: usize = 16 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RegionKind {
    Generic,
    SdfV2000,
    WcsToolsSao28,
    DicomImplicitVr,
    Elf,
}

impl RegionKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::SdfV2000 => "sdf-v2000",
            Self::WcsToolsSao28 => "wcstools-sao-28",
            Self::DicomImplicitVr => "dicom-implicit-vr",
            Self::Elf => "elf",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
    pub kind: RegionKind,
}

impl Region {
    const fn new(start: usize, end: usize, kind: RegionKind) -> Self {
        Self { start, end, kind }
    }
}

fn find(data: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > data.len() {
        return None;
    }
    data.get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

fn rfind_newline(data: &[u8], end: usize) -> Option<usize> {
    data.get(..end)?.iter().rposition(|byte| *byte == b'\n')
}

fn be_fixed3(value: &[u8]) -> Option<usize> {
    std::str::from_utf8(value).ok()?.trim().parse().ok()
}

fn rstrip_ascii_whitespace(value: &[u8]) -> &[u8] {
    let end = value
        .iter()
        .rposition(|byte| !(*byte).is_ascii_whitespace())
        .map_or(0, |at| at + 1);
    &value[..end]
}

fn sdf_candidate(data: &[u8], marker: usize) -> Option<Region> {
    let line_end = find(data, b"\n", marker)?;
    if line_end.checked_sub(marker)? > 256 {
        return None;
    }
    let mut starts = Vec::with_capacity(4);
    let mut probe = marker;
    for _ in 0..4 {
        let previous = rfind_newline(data, probe)?;
        starts.push(previous.checked_add(1)?);
        probe = previous;
    }
    let start = *starts.last()?;
    let scan_end = data.len().min(start.checked_add(MAX_SDF_RECORD)?);
    let separator = find(&data[..scan_end], b"$$$$\n", line_end.checked_add(1)?)?;
    let lines = data
        .get(start..separator)?
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    let counts = *lines.get(3)?;
    let atoms = be_fixed3(counts.get(..3)?)?;
    let bonds = be_fixed3(counts.get(3..6)?)?;
    if !(rstrip_ascii_whitespace(counts).ends_with(b"V2000")
        && atoms > 0
        && atoms <= 999
        && bonds <= 999)
    {
        return None;
    }
    let marker_line = lines.get(4usize.checked_add(atoms)?.checked_add(bonds)?)?;
    marker_line.starts_with(b"M  ").then_some(Region::new(
        start,
        separator + 5,
        RegionKind::SdfV2000,
    ))
}

fn group_adjacent(records: Vec<Region>) -> Vec<Region> {
    let mut grouped: Vec<Region> = Vec::new();
    for item in records {
        if let Some(last) = grouped.last_mut() {
            if last.end == item.start {
                last.end = item.end;
                continue;
            }
        }
        grouped.push(item);
    }
    grouped
}

fn sdf_spans(data: &[u8]) -> Vec<Region> {
    let mut records = Vec::new();
    let mut anchor = 0;
    while records.len() < MAX_STRUCTURED_REGIONS {
        let Some(marker) = find(data, b"V2000", anchor) else {
            break;
        };
        if let Some(item) = sdf_candidate(data, marker) {
            anchor = item.end;
            records.push(item);
        } else {
            anchor = marker + 5;
        }
    }
    group_adjacent(records)
}

fn le_i32(data: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn wcstools_spans(data: &[u8]) -> Vec<Region> {
    let mut spans = Vec::new();
    let stop = data.len().saturating_sub(27);
    for offset in (0..stop).step_by(4) {
        let count = match le_i32(data, offset + 8) {
            Some(value) => value,
            None => break,
        };
        let stnum = match le_i32(data, offset + 12) {
            Some(value) => value,
            None => break,
        };
        let mprop = match le_i32(data, offset + 16) {
            Some(value) => value,
            None => break,
        };
        let nmag = match le_i32(data, offset + 20) {
            Some(value) => value,
            None => break,
        };
        let width = match le_i32(data, offset + 24) {
            Some(value) => value,
            None => break,
        };
        let records = count.unsigned_abs() as usize;
        let end = offset
            .checked_add(28)
            .and_then(|value| value.checked_add(28usize.checked_mul(records)?));
        if records != 0
            && records <= 8_000_000
            && stnum == 0
            && mprop == 1
            && nmag.unsigned_abs() == 1
            && width == 28
            && end.is_some_and(|value| value <= data.len())
        {
            spans.push(Region::new(offset, end.unwrap(), RegionKind::WcsToolsSao28));
            if spans.len() >= MAX_STRUCTURED_REGIONS {
                break;
            }
        }
    }
    spans
}

fn le_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn le_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn dicom_candidate(data: &[u8], start: usize) -> Option<Region> {
    let mut pos = start;
    let limit = data.len().min(start.checked_add(MAX_DICOM_HEADER)?);
    let (mut rows, mut cols, mut bits) = (0usize, 0usize, 0usize);
    while pos.checked_add(8)? <= limit {
        let group = le_u16(data, pos)?;
        let element = le_u16(data, pos + 2)?;
        let size = le_u32(data, pos + 4)? as usize;
        let value_start = pos.checked_add(8)?;
        let value_end = value_start.checked_add(size)?;
        if size == u32::MAX as usize || value_end > data.len() || value_end > limit {
            return None;
        }
        if group == 0x28 && size == 2 {
            let value = le_u16(data, value_start)? as usize;
            match element {
                0x10 => rows = value,
                0x11 => cols = value,
                0x100 => bits = value,
                _ => {}
            }
        }
        if (group, element) == (0x7fe0, 0x10) {
            let pixels = rows.checked_mul(cols)?.checked_mul(2)?;
            return (bits == 16 && rows != 0 && cols != 0 && size.is_multiple_of(pixels))
                .then_some(Region::new(start, value_end, RegionKind::DicomImplicitVr));
        }
        pos = value_end;
    }
    None
}

fn dicom_spans(data: &[u8]) -> Vec<Region> {
    let mut spans = Vec::new();
    let mut search = 0;
    while spans.len() < MAX_STRUCTURED_REGIONS {
        let Some(start) = find(data, b"\x28\x00\x10\x00", search) else {
            break;
        };
        search = start + 1;
        if let Some(item) = dicom_candidate(data, start) {
            spans.push(item);
        }
    }
    spans
}

#[derive(Clone, Copy)]
enum ByteOrder {
    Little,
    Big,
}
fn u16_at(data: &[u8], at: usize, order: ByteOrder) -> Option<u16> {
    let bytes: [u8; 2] = data.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(match order {
        ByteOrder::Little => u16::from_le_bytes(bytes),
        ByteOrder::Big => u16::from_be_bytes(bytes),
    })
}
fn u32_at(data: &[u8], at: usize, order: ByteOrder) -> Option<u32> {
    let bytes: [u8; 4] = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(match order {
        ByteOrder::Little => u32::from_le_bytes(bytes),
        ByteOrder::Big => u32::from_be_bytes(bytes),
    })
}
fn u64_at(data: &[u8], at: usize, order: ByteOrder) -> Option<u64> {
    let bytes: [u8; 8] = data.get(at..at.checked_add(8)?)?.try_into().ok()?;
    Some(match order {
        ByteOrder::Little => u64::from_le_bytes(bytes),
        ByteOrder::Big => u64::from_be_bytes(bytes),
    })
}

#[derive(Clone, Copy)]
struct ElfHeader {
    kind: u8,
    order: ByteOrder,
    phoff: usize,
    shoff: usize,
    phentsize: usize,
    phnum: usize,
    shentsize: usize,
    shnum: usize,
    ehsize: usize,
}

fn elf_header(data: &[u8], offset: usize) -> Option<ElfHeader> {
    if offset.checked_add(64)? > data.len() || data.get(offset..offset + 4)? != b"\x7fELF" {
        return None;
    }
    let kind = *data.get(offset + 4)?;
    let order = match *data.get(offset + 5)? {
        1 => ByteOrder::Little,
        2 => ByteOrder::Big,
        _ => return None,
    };
    if !matches!(kind, 1 | 2) || data.get(offset + 6) != Some(&1) {
        return None;
    }
    let (phoff, shoff, fields_at) = if kind == 1 {
        (
            usize::try_from(u32_at(data, offset + 28, order)?).ok()?,
            usize::try_from(u32_at(data, offset + 32, order)?).ok()?,
            offset + 40,
        )
    } else {
        (
            usize::try_from(u64_at(data, offset + 32, order)?).ok()?,
            usize::try_from(u64_at(data, offset + 40, order)?).ok()?,
            offset + 52,
        )
    };
    let ehsize = usize::from(u16_at(data, fields_at, order)?);
    let phentsize = usize::from(u16_at(data, fields_at + 2, order)?);
    let phnum = usize::from(u16_at(data, fields_at + 4, order)?);
    let shentsize = usize::from(u16_at(data, fields_at + 6, order)?);
    let shnum = usize::from(u16_at(data, fields_at + 8, order)?);
    if ehsize < if kind == 1 { 52 } else { 64 } || phentsize == 0 || shentsize == 0 {
        return None;
    }
    Some(ElfHeader {
        kind,
        order,
        phoff,
        shoff,
        phentsize,
        phnum,
        shentsize,
        shnum,
        ehsize,
    })
}

fn elf_load_extent(
    data: &[u8],
    offset: usize,
    header: ElfHeader,
    mut extent: usize,
) -> Option<usize> {
    for index in 0..header.phnum {
        let entry = offset
            .checked_add(header.phoff)?
            .checked_add(index.checked_mul(header.phentsize)?)?;
        let kind = u32_at(data, entry, header.order)?;
        if kind == 1 {
            let (file_offset, file_size) = if header.kind == 1 {
                (
                    usize::try_from(u32_at(data, entry + 4, header.order)?).ok()?,
                    usize::try_from(u32_at(data, entry + 16, header.order)?).ok()?,
                )
            } else {
                (
                    usize::try_from(u64_at(data, entry + 8, header.order)?).ok()?,
                    usize::try_from(u64_at(data, entry + 32, header.order)?).ok()?,
                )
            };
            extent = extent.max(file_offset.checked_add(file_size)?);
        }
    }
    Some(extent)
}

fn elf_span(data: &[u8], offset: usize) -> Option<Region> {
    let header = elf_header(data, offset)?;
    let tables_end = header
        .ehsize
        .max(
            header
                .phoff
                .checked_add(header.phentsize.checked_mul(header.phnum)?)?,
        )
        .max(
            header
                .shoff
                .checked_add(header.shentsize.checked_mul(header.shnum)?)?,
        );
    if tables_end > MAX_ELF_TABLE || offset.checked_add(tables_end)? > data.len() {
        return None;
    }
    let extent = elf_load_extent(data, offset, header, tables_end)?;
    if extent == 0 || offset.checked_add(extent)? > data.len() {
        return None;
    }
    Some(Region::new(offset, offset + extent, RegionKind::Elf))
}

fn elf_spans(data: &[u8]) -> Vec<Region> {
    let mut spans = Vec::new();
    let mut offset = 0;
    while spans.len() < MAX_STRUCTURED_REGIONS {
        let Some(found) = find(data, b"\x7fELF", offset) else {
            break;
        };
        if let Some(item) = elf_span(data, found) {
            offset = item.end;
            spans.push(item);
        } else {
            offset = found + 4;
        }
    }
    spans
}

/// Non-overlapping recognised spans.  When formats overlap, earliest start
/// wins; equal starts select the longest span, then the stable kind name.
pub fn structured_regions(data: &[u8]) -> Vec<Region> {
    let mut proposed = sdf_spans(data);
    proposed.extend(wcstools_spans(data));
    proposed.extend(dicom_spans(data));
    proposed.extend(elf_spans(data));
    proposed.sort_by_key(|item| {
        (
            item.start,
            std::cmp::Reverse(item.end - item.start),
            item.kind.name(),
        )
    });
    let mut accepted = Vec::new();
    for item in proposed {
        if accepted
            .last()
            .is_none_or(|last: &Region| item.start >= last.end)
        {
            accepted.push(item);
        }
        if accepted.len() >= MAX_STRUCTURED_REGIONS {
            break;
        }
    }
    accepted
}

/// Complete input partition. Unknown gaps are the only spans cut at the fixed
/// generic limit. Known spans are never recursively scanned or split here.
pub fn regions(data: &[u8]) -> Vec<Region> {
    let mut result = Vec::new();
    let mut position = 0usize;
    for known in structured_regions(data) {
        while known.start - position > MAX_GENERIC_REGION {
            result.push(Region::new(
                position,
                position + MAX_GENERIC_REGION,
                RegionKind::Generic,
            ));
            position += MAX_GENERIC_REGION;
        }
        if known.start > position {
            result.push(Region::new(position, known.start, RegionKind::Generic));
        }
        result.push(known);
        position = known.end;
    }
    while data.len() - position > MAX_GENERIC_REGION {
        result.push(Region::new(
            position,
            position + MAX_GENERIC_REGION,
            RegionKind::Generic,
        ));
        position += MAX_GENERIC_REGION;
    }
    if position < data.len() {
        result.push(Region::new(position, data.len(), RegionKind::Generic));
    }
    debug_assert!(
        result.is_empty()
            || (result[0].start == 0 && result.last().is_some_and(|last| last.end == data.len()))
    );
    debug_assert!(result.windows(2).all(|pair| pair[0].end == pair[1].start));
    result
}

pub fn boundaries(data: &[u8]) -> Vec<(usize, usize)> {
    regions(data)
        .into_iter()
        .map(|item| (item.start, item.end))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> &'static [u8] {
        match name {
            "sdf" => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/regions/sdf.bin"
            )),
            "grid" => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/regions/grid.bin"
            )),
            "dicom" => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/regions/dicom.bin"
            )),
            "elf" => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/regions/elf.bin"
            )),
            "mixed" => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/regions/mixed.bin"
            )),
            _ => unreachable!(),
        }
    }

    fn items(data: &[u8]) -> Vec<(usize, usize, RegionKind)> {
        regions(data)
            .into_iter()
            .map(|item| (item.start, item.end, item.kind))
            .collect()
    }

    #[test]
    fn matches_tiny_python_region_fixtures() {
        assert_eq!(
            items(fixture("sdf")),
            vec![(0, 8, RegionKind::Generic), (8, 114, RegionKind::SdfV2000)]
        );
        assert_eq!(
            items(fixture("grid")),
            vec![(0, 56, RegionKind::WcsToolsSao28)]
        );
        assert_eq!(
            items(fixture("dicom")),
            vec![(0, 42, RegionKind::DicomImplicitVr)]
        );
        assert_eq!(items(fixture("elf")), vec![(0, 120, RegionKind::Elf)]);
        assert_eq!(
            items(fixture("mixed")),
            vec![
                (0, 15, RegionKind::Generic),
                (15, 121, RegionKind::SdfV2000),
                (121, 124, RegionKind::Generic),
                (124, 180, RegionKind::WcsToolsSao28),
                (180, 184, RegionKind::Generic),
                (184, 226, RegionKind::DicomImplicitVr),
                (226, 230, RegionKind::Generic),
                (230, 350, RegionKind::Elf),
                (350, 351, RegionKind::Generic),
            ]
        );
    }

    #[test]
    fn malformed_counts_and_elf_entries_are_rejected_without_losing_coverage() {
        let mut malformed_grid = fixture("grid").to_vec();
        malformed_grid[8..12].copy_from_slice(&(8_000_001i32).to_le_bytes());
        assert_eq!(
            regions(&malformed_grid),
            vec![Region::new(0, malformed_grid.len(), RegionKind::Generic)]
        );
        let mut malformed_elf = fixture("elf").to_vec();
        malformed_elf[64..68].copy_from_slice(&1u32.to_le_bytes());
        malformed_elf[96..104].copy_from_slice(&(usize::MAX as u64).to_le_bytes());
        assert_eq!(
            regions(&malformed_elf),
            vec![Region::new(0, malformed_elf.len(), RegionKind::Generic)]
        );
    }

    #[test]
    fn unknown_gaps_are_only_cut_above_the_fixed_limit() {
        let data = vec![0u8; MAX_GENERIC_REGION * 2 + 1];
        assert_eq!(
            boundaries(&data),
            vec![
                (0, MAX_GENERIC_REGION),
                (MAX_GENERIC_REGION, MAX_GENERIC_REGION * 2),
                (MAX_GENERIC_REGION * 2, MAX_GENERIC_REGION * 2 + 1)
            ]
        );
    }
}
