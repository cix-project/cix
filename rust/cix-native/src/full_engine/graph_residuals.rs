//! Exact historical SDFG framing plus V2000 coordinate skeleton/residuals.
//!
//! It owns the byte-preserving record operation and all native historical
//! payload representations.

use super::arithmetic::{vdecode, vencode, ArithmeticDecoder, ArithmeticEncoder};
use crate::rank::{decode_type_class, encode_type_class};
use num_bigint::BigUint;
use num_traits::{ToPrimitive, Zero};
use std::{cmp::Reverse, collections::HashMap};

const COORDINATE_STARTS: [usize; 3] = [0, 10, 20];
const MAGIC: &[u8; 5] = b"SDFG\x01";
const HYDROGEN_MAGIC: &[u8; 5] = b"SDFH\x01";
const SEP: &[u8] = b"$$$$\n";
const DEFAULT_OUTPUT_LIMIT: usize = 1 << 30;
const GROUP_COUNT: usize = 5;
const TYPE_CLASS_BLOCK: usize = 4096;
const PALETTE_SIZE: usize = 64;
type PaletteStreams = (Vec<[i64; 3]>, [Vec<u8>; GROUP_COUNT], usize);
type HydrogenChoicePayload = (Vec<Vec<u8>>, Vec<[i64; 3]>, Vec<u8>);
type HydrogenParsed = (Vec<u8>, Vec<[i64; 3]>, Vec<(usize, usize)>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedRecord {
    pub skeleton: Vec<u8>,
    pub residuals: Vec<[i64; 3]>,
    pub groups: Vec<u8>,
    pub parents: Vec<usize>,
}

fn checked_usize(value: u64, what: &str) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| format!("{what} exceeds platform size"))
}

fn signed(value: i64) -> Vec<u8> {
    let encoded = if value >= 0 {
        (value as u64) << 1
    } else {
        ((-(value + 1)) as u64) << 1 | 1
    };
    vencode(encoded)
}

fn read_signed(blob: &[u8], pos: usize) -> Result<(i64, usize), String> {
    let (value, pos) = vdecode(blob, pos)?;
    let half = (value >> 1) as i64;
    Ok((if value & 1 == 0 { half } else { -half - 1 }, pos))
}

fn append_signed_vectors(out: &mut Vec<u8>, vectors: &[[i64; 3]]) {
    for vector in vectors {
        for value in vector {
            out.extend_from_slice(&signed(*value));
        }
    }
}

fn append_signed_vector(out: &mut Vec<u8>, vector: [i64; 3]) {
    for value in vector {
        out.extend_from_slice(&signed(value));
    }
}

fn read_signed_vector(blob: &[u8], pos: usize) -> Result<([i64; 3], usize), String> {
    let (x, pos) = read_signed(blob, pos)?;
    let (y, pos) = read_signed(blob, pos)?;
    let (z, pos) = read_signed(blob, pos)?;
    Ok(([x, y, z], pos))
}

/// The historical Python helper wraps the native known-length type-class
/// stream with its own length varint.
fn encode_palette_type_class(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = vencode(data.len() as u64);
    out.extend_from_slice(&encode_type_class(data)?);
    Ok(out)
}

fn decode_palette_type_class(data: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let (declared, mut pos) = vdecode(data, 0)?;
    if checked_usize(declared, "type-class block size")? != expected {
        return Err("type class block mismatch".into());
    }
    let chunk = decode_type_class(data, &mut pos, expected)?;
    if pos != data.len() {
        return Err("unused type class bytes".into());
    }
    Ok(chunk)
}

fn append_palette(
    out: &mut Vec<u8>,
    vectors: &[[i64; 3]],
    groups: &[u8],
    palette_size: usize,
) -> Result<(), String> {
    if vectors.len() != groups.len() {
        return Err("palette coordinate/group mismatch".into());
    }
    if !(1..=254).contains(&palette_size) {
        return Err("palette outside byte alphabet".into());
    }
    let mut indices: HashMap<[i64; 3], usize> = HashMap::new();
    let mut counted = Vec::<([i64; 3], usize)>::new();
    for vector in vectors {
        if let Some(index) = indices.get(vector) {
            counted[*index].1 += 1;
        } else {
            let index = counted.len();
            indices.insert(*vector, index);
            counted.push((*vector, 1));
        }
    }
    // Python Counter.most_common() retains first-occurrence order for tied
    // counts; stable sorting preserves that exact tie break.
    counted.sort_by_key(|item| Reverse(item.1));
    let palette = counted
        .into_iter()
        .take(palette_size)
        .map(|(vector, _)| vector)
        .collect::<Vec<_>>();
    let mut identifiers = HashMap::new();
    for (index, vector) in palette.iter().enumerate() {
        identifiers.insert(*vector, index as u8);
    }
    out.extend_from_slice(&vencode(palette.len() as u64));
    append_signed_vectors(out, &palette);
    let mut streams: [Vec<u8>; GROUP_COUNT] = std::array::from_fn(|_| Vec::new());
    let mut escaped = Vec::new();
    let escape = u8::try_from(palette.len()).map_err(|_| "oversized palette")?;
    for (vector, group) in vectors.iter().zip(groups) {
        let group = usize::from(*group);
        let stream = streams.get_mut(group).ok_or("invalid atom group")?;
        match identifiers.get(vector) {
            Some(token) => stream.push(*token),
            None => {
                stream.push(escape);
                append_signed_vector(&mut escaped, *vector);
            }
        }
    }
    for stream in streams {
        out.extend_from_slice(&vencode(stream.len() as u64));
        for block in stream.chunks(TYPE_CLASS_BLOCK) {
            let frame = encode_palette_type_class(block)?;
            out.extend_from_slice(&vencode(frame.len() as u64));
            out.extend_from_slice(&frame);
        }
    }
    out.extend_from_slice(&escaped);
    Ok(())
}

fn decode_palette(
    blob: &[u8],
    mut pos: usize,
    group_counts: &[usize; GROUP_COUNT],
) -> Result<PaletteStreams, String> {
    let (count, after_count) = vdecode(blob, pos)?;
    let count = checked_usize(count, "palette count")?;
    if count > 254 {
        return Err("oversized palette".into());
    }
    pos = after_count;
    let mut palette = Vec::with_capacity(count);
    for _ in 0..count {
        let (vector, after_vector) = read_signed_vector(blob, pos)?;
        pos = after_vector;
        palette.push(vector);
    }
    let mut streams: [Vec<u8>; GROUP_COUNT] = std::array::from_fn(|_| Vec::new());
    for (group, stream) in streams.iter_mut().enumerate() {
        let (size, after_size) = vdecode(blob, pos)?;
        let size = checked_usize(size, "palette stream size")?;
        if size != group_counts[group] {
            return Err("palette stream coordinate count mismatch".into());
        }
        pos = after_size;
        stream.reserve(size);
        let mut offset = 0usize;
        while offset < size {
            let (frame_size, after_frame_size) = vdecode(blob, pos)?;
            let frame_size = checked_usize(frame_size, "type-class frame size")?;
            let frame_end = after_frame_size
                .checked_add(frame_size)
                .ok_or("frame offset overflow")?;
            let frame = blob
                .get(after_frame_size..frame_end)
                .ok_or("truncated type class frame")?;
            let expected = (size - offset).min(TYPE_CLASS_BLOCK);
            let chunk = decode_palette_type_class(frame, expected)?;
            stream.extend_from_slice(&chunk);
            offset += expected;
            pos = frame_end;
        }
    }
    Ok((palette, streams, pos))
}

fn split_records(data: &[u8]) -> Vec<&[u8]> {
    let mut records = Vec::new();
    let mut start = 0;
    while let Some(relative) = data[start..]
        .windows(SEP.len())
        .position(|bytes| bytes == SEP)
    {
        let end = start + relative;
        records.push(&data[start..end]);
        start = end + SEP.len();
    }
    records.push(&data[start..]);
    records
}

fn join_records(records: &[Vec<u8>]) -> Vec<u8> {
    let size = records
        .iter()
        .map(Vec::len)
        .sum::<usize>()
        .saturating_add(SEP.len().saturating_mul(records.len().saturating_sub(1)));
    let mut out = Vec::with_capacity(size);
    for (index, record) in records.iter().enumerate() {
        if index != 0 {
            out.extend_from_slice(SEP);
        }
        out.extend_from_slice(record);
    }
    out
}

/// Matches `bytes.splitlines()` from the historical Python decoder: bytes
/// recognize only CR, LF, and their combined form.
fn next_text_line<'a>(blob: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    if *pos == blob.len() {
        return None;
    }
    let start = *pos;
    let mut index = start;
    while index < blob.len() {
        let width = match blob[index] {
            b'\r' if blob.get(index + 1) == Some(&b'\n') => 2,
            b'\n' | b'\r' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        *pos = index + width;
        return Some(&blob[start..index]);
    }
    *pos = blob.len();
    Some(&blob[start..])
}

fn parse_text_vector(line: &[u8]) -> Result<[i64; 3], String> {
    let fields = line.split(|byte| *byte == b',').collect::<Vec<_>>();
    if fields.len() != 3 {
        return Err("invalid coordinate list".into());
    }
    let mut vector = [0; 3];
    for (index, field) in fields.into_iter().enumerate() {
        let text = std::str::from_utf8(trim_ascii(field)).map_err(|_| "invalid coordinate list")?;
        vector[index] = text.parse().map_err(|_| "invalid coordinate list")?;
    }
    Ok(vector)
}

pub fn fixed(value: i64) -> Result<[u8; 10], String> {
    let sign = if value < 0 { "-" } else { "" };
    let absolute = value.checked_abs().ok_or("coordinate overflow")?;
    let text = format!("{}{}.{:04}", sign, absolute / 10_000, absolute % 10_000);
    if text.len() > 10 {
        return Err("coordinate outside fixed field".into());
    }
    let mut out = [b' '; 10];
    out[10 - text.len()..].copy_from_slice(text.as_bytes());
    Ok(out)
}

fn lines_keepends(record: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < record.len() {
        let n = match record[i] {
            b'\n' => 1,
            b'\r' => {
                if record.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                }
            }
            _ => {
                i += 1;
                continue;
            }
        };
        lines.push(&record[start..i + n]);
        start = i + n;
        i += n;
    }
    if start < record.len() {
        lines.push(&record[start..]);
    }
    lines
}
fn trim_ascii(mut field: &[u8]) -> &[u8] {
    fn space(b: &u8) -> bool {
        matches!(*b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
    }
    while field.first().is_some_and(space) {
        field = &field[1..];
    }
    while field.last().is_some_and(space) {
        field = &field[..field.len() - 1];
    }
    field
}
fn parse_field(field: &[u8]) -> Result<i64, String> {
    let text = std::str::from_utf8(trim_ascii(field)).map_err(|_| "non-ASCII coordinate")?;
    let digits = text.replace('.', "");
    let value = digits.parse::<i64>().map_err(|_| "invalid coordinate")?;
    if fixed(value)? != field {
        return Err("noncanonical coordinate retained literally".into());
    }
    Ok(value)
}
pub fn atom_group(suffix: &[u8]) -> u8 {
    match std::str::from_utf8(trim_ascii(suffix.get(..4).unwrap_or(suffix))).unwrap_or("") {
        "C" => 0,
        "H" => 1,
        "N" => 2,
        "O" => 3,
        _ => 4,
    }
}
pub fn counts(lines: &[&[u8]]) -> Result<(usize, usize), String> {
    if lines.len() < 4 || !trim_ascii(lines[3]).ends_with(b"V2000") {
        return Err("unsupported record".into());
    }
    let atoms: usize = std::str::from_utf8(lines[3].get(..3).ok_or("short counts")?)
        .map_err(|_| "invalid counts")?
        .trim()
        .parse()
        .map_err(|_| "invalid counts")?;
    let bonds: usize = std::str::from_utf8(lines[3].get(3..6).ok_or("short counts")?)
        .map_err(|_| "invalid counts")?
        .trim()
        .parse()
        .map_err(|_| "invalid counts")?;
    if atoms == 0
        || atoms > 999
        || bonds > 999
        || lines.len() < 5 + atoms + bonds
        || !lines[4 + atoms + bonds].starts_with(b"M  ")
    {
        return Err("incomplete record".into());
    }
    Ok((atoms, bonds))
}
pub fn bond_parent(line: &[u8], atoms: usize) -> Result<(usize, usize), String> {
    let a = std::str::from_utf8(line.get(..3).ok_or("short bond")?)
        .map_err(|_| "invalid bond")?
        .trim()
        .parse::<usize>()
        .map_err(|_| "invalid bond")?
        .checked_sub(1)
        .ok_or("invalid bond")?;
    let b = std::str::from_utf8(line.get(3..6).ok_or("short bond")?)
        .map_err(|_| "invalid bond")?
        .trim()
        .parse::<usize>()
        .map_err(|_| "invalid bond")?
        .checked_sub(1)
        .ok_or("invalid bond")?;
    if a >= atoms || b >= atoms || a == b {
        return Err("unsupported bond indices".into());
    }
    Ok(if a < b { (a, b) } else { (b, a) })
}
pub fn layout(lines: &[&[u8]]) -> Result<(usize, Vec<usize>), String> {
    let (a, b) = counts(lines)?;
    let mut parents = (0..a).map(|i| i.saturating_sub(1)).collect::<Vec<_>>();
    for line in &lines[4 + a..4 + a + b] {
        let (x, y) = bond_parent(line, a)?;
        parents[y] = x;
    }
    Ok((a, parents))
}
pub fn extract(record: &[u8], mode: u8) -> Result<ExtractedRecord, String> {
    if mode > 2 {
        return Err("unknown geometry mode".into());
    }
    let lines = lines_keepends(record);
    let (atoms, parents) = layout(&lines)?;
    let mut coords = Vec::new();
    let mut groups = Vec::new();
    for line in &lines[4..4 + atoms] {
        if line.len() < 34 {
            return Err("short atom".into());
        }
        let mut v = [0; 3];
        for (i, s) in COORDINATE_STARTS.iter().enumerate() {
            v[i] = parse_field(&line[*s..*s + 10])?;
        }
        coords.push(v);
        groups.push(atom_group(&line[30..]));
    }
    let mut residuals = Vec::new();
    for (i, v) in coords.iter().enumerate() {
        let p = if mode == 0 {
            None
        } else if mode == 1 {
            i.checked_sub(1)
        } else {
            let p = parents[i];
            if p < i {
                Some(p)
            } else {
                None
            }
        };
        let base = p.map(|p| coords[p]).unwrap_or([0; 3]);
        residuals.push([v[0] - base[0], v[1] - base[1], v[2] - base[2]]);
    }
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if (4..4 + atoms).contains(&i) {
            out.extend_from_slice(&line[30..]);
        } else {
            out.extend_from_slice(line)
        }
    }
    Ok(ExtractedRecord {
        skeleton: out,
        residuals,
        groups,
        parents,
    })
}
pub fn restore(skeleton: &[u8], mode: u8, residuals: &[[i64; 3]]) -> Result<Vec<u8>, String> {
    if mode > 2 {
        return Err("unknown geometry mode".into());
    }
    let lines = lines_keepends(skeleton);
    let (atoms, parents) = layout(&lines)?;
    if atoms != residuals.len() {
        return Err("coordinate count mismatch".into());
    }
    let mut rebuilt = lines.iter().map(|x| x.to_vec()).collect::<Vec<_>>();
    let mut coords: Vec<[i64; 3]> = Vec::with_capacity(atoms);
    for i in 0..atoms {
        let p = if mode == 0 {
            None
        } else if mode == 1 {
            i.checked_sub(1)
        } else {
            let p = parents[i];
            if p < i {
                Some(p)
            } else {
                None
            }
        };
        let base = p.map(|p| coords[p]).unwrap_or([0; 3]);
        let v = [
            base[0]
                .checked_add(residuals[i][0])
                .ok_or("coordinate overflow")?,
            base[1]
                .checked_add(residuals[i][1])
                .ok_or("coordinate overflow")?,
            base[2]
                .checked_add(residuals[i][2])
                .ok_or("coordinate overflow")?,
        ];
        coords.push(v);
        let mut prefix = Vec::with_capacity(30);
        for x in v {
            prefix.extend_from_slice(&fixed(x)?)
        }
        prefix.extend_from_slice(&rebuilt[4 + i]);
        rebuilt[4 + i] = prefix;
    }
    Ok(rebuilt.concat())
}

/// Encodes the historical `SDFG\x01` geometry frame.  Unsupported records are
/// retained literally and indicated by their clear record-flag bit.
pub fn encode(data: &[u8], mode: u8, representation: u8) -> Result<Vec<u8>, String> {
    encode_with_palette(data, mode, representation, PALETTE_SIZE)
}

/// Encodes the historical `SDFG\x01` geometry frame with the supplied
/// representation-2 palette size.  Representations 0 and 1 ignore this value,
/// as the historical encoder does.
pub fn encode_with_palette(
    data: &[u8],
    mode: u8,
    representation: u8,
    palette_size: usize,
) -> Result<Vec<u8>, String> {
    if mode > 2 || representation > 2 {
        return Err("unknown geometry mode".into());
    }
    let records = split_records(data);
    let mut skeletons = Vec::with_capacity(records.len());
    let mut flags = vec![
        0u8;
        records
            .len()
            .checked_add(7)
            .ok_or("record count overflow")?
            / 8
    ];
    let mut vectors = Vec::new();
    let mut groups = Vec::new();
    for (index, record) in records.iter().enumerate() {
        match extract(record, mode) {
            Ok(extracted) => {
                flags[index / 8] |= 1 << (index % 8);
                vectors.extend(extracted.residuals);
                groups.extend(extracted.groups);
                skeletons.push(extracted.skeleton);
            }
            Err(_) => skeletons.push((*record).to_vec()),
        }
    }
    let skeleton = join_records(&skeletons);
    let mut out = Vec::with_capacity(
        MAGIC
            .len()
            .saturating_add(2)
            .saturating_add(flags.len())
            .saturating_add(skeleton.len()),
    );
    out.extend_from_slice(MAGIC);
    out.push(mode);
    out.push(representation);
    out.extend_from_slice(&vencode(records.len() as u64));
    out.extend_from_slice(&flags);
    out.extend_from_slice(&vencode(skeleton.len() as u64));
    out.extend_from_slice(&skeleton);
    out.extend_from_slice(&vencode(vectors.len() as u64));
    match representation {
        0 => append_signed_vectors(&mut out, &vectors),
        1 => {
            for vector in vectors {
                out.extend_from_slice(
                    format!("{},{},{}\n", vector[0], vector[1], vector[2]).as_bytes(),
                );
            }
        }
        2 => append_palette(&mut out, &vectors, &groups, palette_size)?,
        _ => unreachable!(),
    }
    Ok(out)
}

/// Decodes an `SDFG\x01` frame with a caller-selected cap on reconstructed
/// bytes.  The cap covers both passed-through and rebuilt records.
pub fn decode_with_limit(blob: &[u8], output_limit: usize) -> Result<Vec<u8>, String> {
    if blob.len() < 7 || blob.get(..5) != Some(MAGIC) {
        return Err("invalid geometry frame".into());
    }
    let mode = blob[5];
    let representation = blob[6];
    if mode > 2 || representation > 2 {
        return Err("unknown geometry mode".into());
    }
    let (records_count, mut pos) = vdecode(blob, 7)?;
    let records_count = checked_usize(records_count, "record count")?;
    let flag_size = records_count
        .checked_add(7)
        .ok_or("record count overflow")?
        / 8;
    let flags_end = pos.checked_add(flag_size).ok_or("frame offset overflow")?;
    let flags = blob.get(pos..flags_end).ok_or("truncated flags")?;
    pos = flags_end;
    let (skeleton_size, after_size) = vdecode(blob, pos)?;
    let skeleton_size = checked_usize(skeleton_size, "skeleton size")?;
    let skeleton_end = after_size
        .checked_add(skeleton_size)
        .ok_or("frame offset overflow")?;
    let skeleton = blob
        .get(after_size..skeleton_end)
        .ok_or("truncated skeleton")?;
    if skeleton.len() > output_limit {
        return Err("geometry output limit exceeded".into());
    }
    pos = skeleton_end;
    let (vectors_count, after_vectors) = vdecode(blob, pos)?;
    let vectors_count = checked_usize(vectors_count, "coordinate count")?;
    pos = after_vectors;
    let records = split_records(skeleton);
    if records.len() != records_count {
        return Err("record count mismatch".into());
    }
    let mut parsed_atoms = 0usize;
    let mut group_counts = [0usize; GROUP_COUNT];
    for (index, record) in records.iter().enumerate() {
        if flags[index / 8] & (1 << (index % 8)) != 0 {
            let lines = lines_keepends(record);
            let atoms = layout(&lines)?.0;
            parsed_atoms = parsed_atoms
                .checked_add(atoms)
                .ok_or("coordinate count overflow")?;
            if representation == 2 {
                for atom in 0..atoms {
                    let group = usize::from(atom_group(lines[4 + atom]));
                    group_counts[group] = group_counts[group]
                        .checked_add(1)
                        .ok_or("palette stream size overflow")?;
                }
            }
        }
    }
    if parsed_atoms != vectors_count {
        return Err("coordinate count mismatch".into());
    }
    let rebuilt_size = skeleton
        .len()
        .checked_add(
            vectors_count
                .checked_mul(30)
                .ok_or("reconstructed output overflow")?,
        )
        .ok_or("reconstructed output overflow")?;
    if rebuilt_size > output_limit {
        return Err("geometry output limit exceeded".into());
    }

    let vectors = match representation {
        0 => Vec::new(),
        1 => {
            let payload = &blob[pos..];
            let mut payload_pos = 0;
            let mut vectors = Vec::with_capacity(vectors_count);
            for _ in 0..vectors_count {
                let line =
                    next_text_line(payload, &mut payload_pos).ok_or("invalid coordinate list")?;
                vectors.push(parse_text_vector(line)?);
            }
            if next_text_line(payload, &mut payload_pos).is_some() {
                return Err("invalid coordinate list".into());
            }
            pos = blob.len();
            vectors
        }
        2 => Vec::new(),
        _ => unreachable!(),
    };
    let (palette, streams, palette_positions) = if representation == 2 {
        let (palette, streams, after_palette) = decode_palette(blob, pos, &group_counts)?;
        pos = after_palette;
        (palette, streams, [0usize; GROUP_COUNT])
    } else {
        (
            Vec::new(),
            std::array::from_fn(|_| Vec::new()),
            [0usize; GROUP_COUNT],
        )
    };
    let mut vector_at = 0usize;
    let mut palette_positions = palette_positions;
    let mut restored = Vec::with_capacity(records_count);
    for (index, record) in records.iter().enumerate() {
        if flags[index / 8] & (1 << (index % 8)) == 0 {
            restored.push((*record).to_vec());
            continue;
        }
        let residuals = if representation == 1 {
            let atoms = layout(&lines_keepends(record))?.0;
            let end = vector_at
                .checked_add(atoms)
                .ok_or("coordinate count overflow")?;
            let values = vectors
                .get(vector_at..end)
                .ok_or("missing coordinate data")?;
            vector_at = end;
            values.to_vec()
        } else if representation == 2 {
            let lines = lines_keepends(record);
            let atoms = layout(&lines)?.0;
            let mut values = Vec::with_capacity(atoms);
            for atom in 0..atoms {
                let group = usize::from(atom_group(lines[4 + atom]));
                let at = palette_positions[group];
                let token = *streams[group].get(at).ok_or("palette stream exhausted")?;
                palette_positions[group] += 1;
                if usize::from(token) < palette.len() {
                    values.push(palette[usize::from(token)]);
                } else if usize::from(token) == palette.len() {
                    let (vector, after_vector) = read_signed_vector(blob, pos)?;
                    pos = after_vector;
                    values.push(vector);
                } else {
                    return Err("palette token invalid".into());
                }
            }
            vector_at = vector_at
                .checked_add(atoms)
                .ok_or("coordinate count overflow")?;
            values
        } else {
            let atoms = layout(&lines_keepends(record))?.0;
            let mut values = Vec::with_capacity(atoms);
            for _ in 0..atoms {
                let (x, after_x) = read_signed(blob, pos)?;
                let (y, after_y) = read_signed(blob, after_x)?;
                let (z, after_z) = read_signed(blob, after_y)?;
                pos = after_z;
                values.push([x, y, z]);
            }
            vector_at = vector_at
                .checked_add(atoms)
                .ok_or("coordinate count overflow")?;
            values
        };
        restored.push(restore(record, mode, &residuals)?);
    }
    if pos != blob.len() || vector_at != vectors_count {
        return Err("unused or missing coordinate data".into());
    }
    if representation == 2
        && palette_positions
            .iter()
            .zip(streams.iter())
            .any(|(position, stream)| *position != stream.len())
    {
        return Err("unused palette tokens".into());
    }
    let output = join_records(&restored);
    if output.len() > output_limit {
        return Err("geometry output limit exceeded".into());
    }
    Ok(output)
}

/// Decodes with the native full-engine default bounded-output policy.
pub fn decode(blob: &[u8]) -> Result<Vec<u8>, String> {
    decode_with_limit(blob, DEFAULT_OUTPUT_LIMIT)
}

fn hydrogen_extract(record: &[u8]) -> Result<(Vec<u8>, Vec<[i64; 3]>), String> {
    let lines = lines_keepends(record);
    let (atoms, _) = layout(&lines)?;
    let mut coordinates = Vec::with_capacity(atoms);
    let mut skeleton = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !(4..4 + atoms).contains(&index) {
            skeleton.extend_from_slice(line);
            continue;
        }
        if line.len() < 34 {
            return Err("short coordinate line".into());
        }
        let mut coordinate = [0; 3];
        let mut signs = 0u8;
        for (axis, start) in COORDINATE_STARTS.iter().enumerate() {
            let field = &line[*start..*start + 10];
            if field == b"   -0.0000" {
                signs |= 1 << axis;
            } else {
                coordinate[axis] = parse_field(field)?;
            }
        }
        coordinates.push(coordinate);
        skeleton.push(b'0' + signs);
        skeleton.extend_from_slice(&line[30..]);
    }
    Ok((skeleton, coordinates))
}

const TURN: i64 = 1i64 << 32;
const ANGLES: [i64; 31] = [
    536870912, 316933406, 167458907, 85004756, 42667331, 21354465, 10679838, 5340245, 2670163,
    1335087, 667544, 333772, 166886, 83443, 41722, 20861, 10430, 5215, 2608, 1304, 652, 326, 163,
    81, 41, 20, 10, 5, 3, 1, 1,
];

fn rounded(value: i128, denominator: i128) -> Result<i64, String> {
    let half = denominator / 2;
    let value = if value >= 0 {
        (value + half) / denominator
    } else {
        -((-value + half) / denominator)
    };
    i64::try_from(value).map_err(|_| "hydrogen coordinate overflow".into())
}

fn hydrogen_angle(mut x: i64, mut y: i64) -> Result<i64, String> {
    if x == 0 && y == 0 {
        return Err("undefined hydrogen direction".into());
    }
    let magnitude = x.unsigned_abs().max(y.unsigned_abs());
    let shift = 40i32
        .saturating_sub(64 - magnitude.leading_zeros() as i32)
        .max(0) as u32;
    x = x.checked_shl(shift).ok_or("hydrogen direction overflow")?;
    y = y.checked_shl(shift).ok_or("hydrogen direction overflow")?;
    let mut base = 0i64;
    if x < 0 {
        x = x.checked_neg().ok_or("hydrogen direction overflow")?;
        y = y.checked_neg().ok_or("hydrogen direction overflow")?;
        base = TURN / 2;
    }
    let (mut x, mut y) = (i128::from(x), i128::from(y));
    let mut z = 0i64;
    for (index, step) in ANGLES.iter().enumerate() {
        if y > 0 {
            (x, y, z) = (x + (y >> index), y - (x >> index), z + step);
        } else if y < 0 {
            (x, y, z) = (x - (y >> index), y + (x >> index), z - step);
        } else {
            break;
        }
    }
    Ok((base + z).rem_euclid(TURN))
}

fn hydrogen_direction(phase: i64, radius: i64) -> Result<[i64; 3], String> {
    let mut z = (phase + TURN / 2).rem_euclid(TURN) - TURN / 2;
    let mut sign = 1i128;
    if z > TURN / 4 {
        z -= TURN / 2;
        sign = -1;
    } else if z < -TURN / 4 {
        z += TURN / 2;
        sign = -1;
    }
    let (mut x, mut y) = (652032874i128, 0i128);
    for (index, step) in ANGLES.iter().enumerate() {
        if z >= 0 {
            (x, y, z) = (x - (y >> index), y + (x >> index), z - step);
        } else {
            (x, y, z) = (x + (y >> index), y - (x >> index), z + step);
        }
    }
    Ok([
        rounded(sign * x * i128::from(radius), 1i128 << 30)?,
        rounded(sign * y * i128::from(radius), 1i128 << 30)?,
        0,
    ])
}

struct HydrogenTopology {
    atoms: usize,
    parents: Vec<usize>,
    adjacency: Vec<Vec<usize>>,
    types: Vec<Vec<u8>>,
}

fn hydrogen_topology(lines: &[&[u8]], stripped: bool) -> Result<HydrogenTopology, String> {
    let (atoms, parents) = layout(lines)?;
    let (_, bonds) = counts(lines)?;
    let mut adjacency = vec![Vec::new(); atoms];
    for line in &lines[4 + atoms..4 + atoms + bonds] {
        let (a, b) = bond_parent(line, atoms)?;
        adjacency[a].push(b);
        adjacency[b].push(a);
    }
    for neighbours in &mut adjacency {
        neighbours.sort_unstable();
        neighbours.dedup();
    }
    let mut types = Vec::with_capacity(atoms);
    for line in &lines[4..4 + atoms] {
        let field = if stripped {
            line.get(2..5)
        } else {
            line.get(31..34)
        }
        .ok_or("short coordinate line")?;
        types.push(trim_ascii(field).to_vec());
    }
    Ok(HydrogenTopology {
        atoms,
        parents,
        adjacency,
        types,
    })
}

fn hydrogen_candidates(
    index: usize,
    coordinates: &[[i64; 3]],
    topology: &HydrogenTopology,
    slots: &mut HashMap<usize, Vec<[i64; 3]>>,
    model: u8,
    radius: i64,
) -> Result<(Vec<[i64; 3]>, Option<usize>), String> {
    let parents = &topology.parents;
    let adjacency = &topology.adjacency;
    let types = &topology.types;
    let base = if parents[index] < coordinates.len() {
        coordinates[parents[index]]
    } else {
        [0; 3]
    };
    if model == 0 || types[index].as_slice() != b"H" || adjacency[index].len() != 1 {
        return Ok((vec![base], None));
    }
    let centre = adjacency[index][0];
    if centre >= index {
        return Ok((vec![base], None));
    }
    if let Some(options) = slots.get(&centre) {
        return Ok((options.clone(), Some(centre)));
    }
    let hydrogens = adjacency[centre]
        .iter()
        .copied()
        .filter(|atom| types[*atom].as_slice() == b"H")
        .collect::<Vec<_>>();
    let heavy = adjacency[centre]
        .iter()
        .copied()
        .filter(|atom| types[*atom].as_slice() != b"H")
        .collect::<Vec<_>>();
    if !(1..=8).contains(&hydrogens.len())
        || heavy.is_empty()
        || heavy.iter().any(|atom| *atom >= index)
        || hydrogens.first() != Some(&index)
        || hydrogens.iter().any(|atom| adjacency[*atom].len() != 1)
    {
        return Ok((vec![base], None));
    }
    let origin = *coordinates
        .get(centre)
        .ok_or("hydrogen centre unavailable")?;
    let vectors = heavy
        .iter()
        .map(|atom| {
            Ok([
                coordinates[*atom][0]
                    .checked_sub(origin[0])
                    .ok_or("hydrogen vector overflow")?,
                coordinates[*atom][1]
                    .checked_sub(origin[1])
                    .ok_or("hydrogen vector overflow")?,
                coordinates[*atom][2]
                    .checked_sub(origin[2])
                    .ok_or("hydrogen vector overflow")?,
            ])
        })
        .collect::<Result<Vec<[i64; 3]>, String>>()?;
    if vectors
        .iter()
        .any(|vector| vector[2] != 0 || (vector[0] == 0 && vector[1] == 0))
    {
        return Ok((vec![base], None));
    }
    let offsets = if model == 1 {
        if heavy.len() != 2 || hydrogens.len() != 1 {
            return Ok((vec![base], None));
        }
        let vector = [
            vectors[0][0]
                .checked_add(vectors[1][0])
                .ok_or("hydrogen vector overflow")?,
            vectors[0][1]
                .checked_add(vectors[1][1])
                .ok_or("hydrogen vector overflow")?,
            vectors[0][2]
                .checked_add(vectors[1][2])
                .ok_or("hydrogen vector overflow")?,
        ];
        let norm = hydrogen_norm(vector)?;
        let norm = integer_sqrt(norm);
        if norm == 0 {
            return Ok((vec![base], None));
        }
        vec![[
            rounded(-i128::from(vector[0]) * i128::from(radius), norm as i128)?,
            rounded(-i128::from(vector[1]) * i128::from(radius), norm as i128)?,
            rounded(-i128::from(vector[2]) * i128::from(radius), norm as i128)?,
        ]]
    } else {
        let mut phases = vectors
            .iter()
            .map(|vector| hydrogen_angle(vector[0], vector[1]))
            .collect::<Result<Vec<_>, _>>()?;
        phases.sort_unstable();
        phases.dedup();
        let mut chosen = (0i64, 0i64);
        for (n, phase) in phases.iter().enumerate() {
            let next = phases[(n + 1) % phases.len()];
            let gap = (next - phase).rem_euclid(TURN);
            if (gap, -phase) > (chosen.0, -chosen.1) {
                chosen = (gap, *phase);
            }
        }
        (0..hydrogens.len())
            .map(|n| {
                hydrogen_direction(
                    chosen.1 + chosen.0 * (n as i64 + 1) / (hydrogens.len() as i64 + 1),
                    radius,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let options = offsets
        .into_iter()
        .map(|offset| {
            Ok([
                origin[0]
                    .checked_add(offset[0])
                    .ok_or("hydrogen coordinate overflow")?,
                origin[1]
                    .checked_add(offset[1])
                    .ok_or("hydrogen coordinate overflow")?,
                origin[2]
                    .checked_add(offset[2])
                    .ok_or("hydrogen coordinate overflow")?,
            ])
        })
        .collect::<Result<Vec<_>, String>>()?;
    slots.insert(centre, options.clone());
    Ok((options, Some(centre)))
}

fn integer_sqrt(value: u128) -> u128 {
    if value == 0 {
        return 0;
    }
    let mut x = value;
    let mut y = value / 2 + 1;
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}

fn hydrogen_norm(vector: [i64; 3]) -> Result<u128, String> {
    vector.into_iter().try_fold(0u128, |total, value| {
        let magnitude = u128::from(value.unsigned_abs());
        total
            .checked_add(
                magnitude
                    .checked_mul(magnitude)
                    .ok_or("hydrogen vector overflow")?,
            )
            .ok_or_else(|| "hydrogen vector overflow".into())
    })
}

fn hydrogen_prefix(coordinate: [i64; 3], signs: u8) -> Result<Vec<u8>, String> {
    if signs >= 8 {
        return Err("invalid zero signs".into());
    }
    let mut prefix = Vec::with_capacity(30);
    for (axis, value) in coordinate.into_iter().enumerate() {
        if signs & (1 << axis) != 0 {
            if value != 0 {
                return Err("sign mask on nonzero value".into());
            }
            prefix.extend_from_slice(b"   -0.0000");
        } else {
            prefix.extend_from_slice(&fixed(value)?);
        }
    }
    Ok(prefix)
}

#[derive(Default)]
struct HydrogenEncodeState {
    skeletons: Vec<Vec<u8>>,
    vectors: Vec<[i64; 3]>,
    digits: Vec<u8>,
    rank: BigUint,
    product: BigUint,
    frequencies: HashMap<usize, Vec<u64>>,
    arithmetic: Option<ArithmeticEncoder>,
}

fn hydrogen_choice_encode(
    state: &mut HydrogenEncodeState,
    chosen: usize,
    count: usize,
    representation: u8,
) -> Result<(), String> {
    if count <= 1 {
        return Ok(());
    }
    let chosen = u8::try_from(chosen).map_err(|_| "hydrogen candidate index overflow")?;
    state.digits.push(chosen);
    state.rank += BigUint::from(usize::from(chosen)) * &state.product;
    state.product *= BigUint::from(count);
    if representation >= 2 {
        let frequencies = state
            .frequencies
            .entry(count)
            .or_insert_with(|| vec![1; count]);
        let lower = frequencies[..usize::from(chosen)].iter().sum::<u64>();
        let total = frequencies.iter().sum::<u64>();
        state
            .arithmetic
            .as_mut()
            .ok_or("missing hydrogen arithmetic state")?
            .encode(lower, lower + frequencies[usize::from(chosen)], total)?;
        if representation == 3 {
            frequencies[usize::from(chosen)] += 1;
            if frequencies.iter().sum::<u64>() > 4096 {
                for value in frequencies {
                    *value = (*value).div_ceil(2);
                }
            }
        }
    }
    Ok(())
}

fn hydrogen_choice_payload(
    mut state: HydrogenEncodeState,
    representation: u8,
) -> Result<HydrogenChoicePayload, String> {
    let mut payload = if representation == 0 {
        state.digits.into_iter().map(|digit| b'0' + digit).collect()
    } else {
        let bits = if state.product.is_zero() {
            0
        } else {
            (&state.product - BigUint::from(1u8)).bits() as usize
        };
        state.rank.to_bytes_le()[..bits.div_ceil(8)].to_vec()
    };
    if representation >= 2 {
        let (packed, bits) = state
            .arithmetic
            .take()
            .ok_or("missing hydrogen arithmetic state")?
            .finish();
        payload = vencode(u64::try_from(bits).map_err(|_| "hydrogen arithmetic length overflow")?);
        payload.extend_from_slice(&packed);
    }
    Ok((state.skeletons, state.vectors, payload))
}

/// Encodes an exact historical SDFH frame for the requested geometry model and
/// choice representation.  Existing callers retain model 0 / representation 0
/// through `encode_hydrogen`; callers selecting models 1 or 2 use this entry.
pub fn encode_hydrogen_with_options(
    data: &[u8],
    model: u8,
    representation: u8,
    radius: u64,
) -> Result<Vec<u8>, String> {
    if model > 2 || representation > 3 || !(1..=1_000_000).contains(&radius) {
        return Err("invalid hydrogen mode".into());
    }
    let records = split_records(data);
    let mut flags = vec![
        0u8;
        records
            .len()
            .checked_add(7)
            .ok_or("record count overflow")?
            / 8
    ];
    let mut state = HydrogenEncodeState {
        skeletons: Vec::with_capacity(records.len()),
        product: BigUint::from(1u8),
        arithmetic: (representation >= 2).then(ArithmeticEncoder::new),
        ..Default::default()
    };
    for (index, record) in records.iter().enumerate() {
        let parsed = (|| -> Result<HydrogenParsed, String> {
            let (skeleton, coordinates) = hydrogen_extract(record)?;
            let lines = lines_keepends(record);
            let topology = hydrogen_topology(&lines, false)?;
            if topology.atoms != coordinates.len() {
                return Err("hydrogen coordinate count mismatch".into());
            }
            let mut slots = HashMap::new();
            let mut residuals = Vec::with_capacity(topology.atoms);
            let mut choices = Vec::with_capacity(topology.atoms);
            for (atom, actual) in coordinates.iter().enumerate() {
                let (options, key) = hydrogen_candidates(
                    atom,
                    &coordinates[..atom],
                    &topology,
                    &mut slots,
                    model,
                    radius as i64,
                )?;
                let chosen = options
                    .iter()
                    .enumerate()
                    .min_by_key(|(choice, option)| {
                        let dx = i128::from(actual[0]) - i128::from(option[0]);
                        let dy = i128::from(actual[1]) - i128::from(option[1]);
                        let dz = i128::from(actual[2]) - i128::from(option[2]);
                        (dx * dx + dy * dy + dz * dz, *choice)
                    })
                    .ok_or("empty hydrogen candidates")?
                    .0;
                choices.push((chosen, options.len()));
                residuals.push([
                    actual[0]
                        .checked_sub(options[chosen][0])
                        .ok_or("hydrogen residual overflow")?,
                    actual[1]
                        .checked_sub(options[chosen][1])
                        .ok_or("hydrogen residual overflow")?,
                    actual[2]
                        .checked_sub(options[chosen][2])
                        .ok_or("hydrogen residual overflow")?,
                ]);
                if let Some(key) = key {
                    slots
                        .get_mut(&key)
                        .ok_or("missing hydrogen slots")?
                        .remove(chosen);
                }
            }
            Ok((skeleton, residuals, choices))
        })();
        match parsed {
            Ok((skeleton, residuals, choices)) => {
                for (chosen, count) in choices {
                    hydrogen_choice_encode(&mut state, chosen, count, representation)?;
                }
                state.skeletons.push(skeleton);
                state.vectors.extend(residuals);
                flags[index / 8] |= 1 << (index % 8);
            }
            Err(_) => state.skeletons.push((*record).to_vec()),
        }
    }
    let (skeletons, vectors, choices) = hydrogen_choice_payload(state, representation)?;
    let skeleton = join_records(&skeletons);
    let mut out = Vec::new();
    out.extend_from_slice(HYDROGEN_MAGIC);
    out.extend_from_slice(&[model, representation]);
    out.extend_from_slice(&vencode(radius));
    out.extend_from_slice(&vencode(records.len() as u64));
    out.extend_from_slice(&flags);
    out.extend_from_slice(&vencode(skeleton.len() as u64));
    out.extend_from_slice(&skeleton);
    out.extend_from_slice(&vencode(choices.len() as u64));
    out.extend_from_slice(&choices);
    for vector in vectors {
        out.extend_from_slice(format!("{},{},{}\n", vector[0], vector[1], vector[2]).as_bytes());
    }
    Ok(out)
}

/// The compatibility entry point retains the originally shipped model-0,
/// literal-choice frame and its historical radius.
pub fn encode_hydrogen(data: &[u8]) -> Result<Vec<u8>, String> {
    encode_hydrogen_with_options(data, 0, 0, 6200)
}

struct HydrogenDecodeState<'a> {
    rank: BigUint,
    digit_at: usize,
    frequencies: HashMap<usize, Vec<u64>>,
    arithmetic: Option<ArithmeticDecoder<'a>>,
}

fn hydrogen_choice_decode(
    state: &mut HydrogenDecodeState<'_>,
    choices: &[u8],
    representation: u8,
    count: usize,
) -> Result<usize, String> {
    if count <= 1 {
        return Ok(0);
    }
    match representation {
        0 => {
            let digit = *choices
                .get(state.digit_at)
                .ok_or("missing candidate index")?;
            state.digit_at += 1;
            if !(b'0'..b'0' + count as u8).contains(&digit) {
                return Err("candidate index outside restricted alphabet".into());
            }
            Ok(usize::from(digit - b'0'))
        }
        1 => {
            let divisor = BigUint::from(count);
            let digit = (&state.rank % &divisor)
                .to_usize()
                .ok_or("hydrogen rank overflow")?;
            state.rank /= divisor;
            Ok(digit)
        }
        2 | 3 => {
            let frequencies = state
                .frequencies
                .entry(count)
                .or_insert_with(|| vec![1; count]);
            let total = frequencies.iter().sum::<u64>();
            let target = state
                .arithmetic
                .as_ref()
                .ok_or("missing hydrogen arithmetic state")?
                .target(total)?;
            let mut lower = 0u64;
            for (chosen, frequency) in frequencies.iter().copied().enumerate() {
                if target < lower + frequency {
                    state
                        .arithmetic
                        .as_mut()
                        .ok_or("missing hydrogen arithmetic state")?
                        .update(lower, lower + frequency, total)?;
                    if representation == 3 {
                        frequencies[chosen] += 1;
                        if frequencies.iter().sum::<u64>() > 4096 {
                            for value in frequencies {
                                *value = (*value).div_ceil(2);
                            }
                        }
                    }
                    return Ok(chosen);
                }
                lower += frequency;
            }
            Err("choice arithmetic symbol invalid".into())
        }
        _ => Err("unsupported hydrogen representation".into()),
    }
}

pub fn decode_hydrogen_with_limit(blob: &[u8], output_limit: usize) -> Result<Vec<u8>, String> {
    if blob.len() < 7 || blob.get(..5) != Some(HYDROGEN_MAGIC) {
        return Err("invalid hydrogen stream".into());
    }
    let (model, representation) = (blob[5], blob[6]);
    if model > 2 || representation > 3 {
        return Err("invalid hydrogen mode".into());
    }
    let (radius, mut pos) = vdecode(blob, 7)?;
    if !(1..=1_000_000).contains(&radius) {
        return Err("invalid hydrogen mode".into());
    }
    let (record_count, after_records) = vdecode(blob, pos)?;
    let record_count = checked_usize(record_count, "record count")?;
    pos = after_records;
    let flags_size = record_count.checked_add(7).ok_or("record count overflow")? / 8;
    let flags_end = pos.checked_add(flags_size).ok_or("frame offset overflow")?;
    let flags = blob.get(pos..flags_end).ok_or("truncated record flags")?;
    let (skeleton_size, skeleton_start) = vdecode(blob, flags_end)?;
    let skeleton_size = checked_usize(skeleton_size, "skeleton size")?;
    let skeleton_end = skeleton_start
        .checked_add(skeleton_size)
        .ok_or("frame offset overflow")?;
    let skeleton = blob
        .get(skeleton_start..skeleton_end)
        .ok_or("truncated skeleton")?;
    if skeleton.len() > output_limit {
        return Err("hydrogen output limit exceeded".into());
    }
    let (choice_size, choice_start) = vdecode(blob, skeleton_end)?;
    let choice_size = checked_usize(choice_size, "choice size")?;
    let choice_end = choice_start
        .checked_add(choice_size)
        .ok_or("frame offset overflow")?;
    let choices = blob
        .get(choice_start..choice_end)
        .ok_or("truncated choices")?;
    let records = split_records(skeleton);
    if records.len() != record_count {
        return Err("record count mismatch".into());
    }
    let mut atoms_total = 0usize;
    for (index, record) in records.iter().enumerate() {
        if flags[index / 8] & (1 << (index % 8)) != 0 {
            atoms_total = atoms_total
                .checked_add(layout(&lines_keepends(record))?.0)
                .ok_or("coordinate count overflow")?;
        }
    }
    let output_size = skeleton
        .len()
        .checked_add(
            atoms_total
                .checked_mul(29)
                .ok_or("hydrogen output overflow")?,
        )
        .ok_or("hydrogen output overflow")?;
    if output_size > output_limit {
        return Err("hydrogen output limit exceeded".into());
    }
    let payload = &blob[choice_end..];
    let mut text_at = 0usize;
    let mut vectors = Vec::with_capacity(atoms_total);
    for _ in 0..atoms_total {
        let line = next_text_line(payload, &mut text_at).ok_or("missing coordinate residual")?;
        vectors.push(parse_text_vector(line).map_err(|_| "invalid residual vector")?);
    }
    if next_text_line(payload, &mut text_at).is_some() {
        return Err("unused residual state".into());
    }
    let mut state = HydrogenDecodeState {
        rank: BigUint::zero(),
        digit_at: 0,
        frequencies: HashMap::new(),
        arithmetic: None,
    };
    if representation == 1 {
        state.rank = BigUint::from_bytes_le(choices);
    }
    if representation >= 2 {
        let (bits, start) = vdecode(choices, 0)?;
        let bits = checked_usize(bits, "hydrogen arithmetic bit length")?;
        let bytes = bits
            .checked_add(7)
            .ok_or("hydrogen arithmetic length overflow")?
            / 8;
        let end = start
            .checked_add(bytes)
            .ok_or("hydrogen arithmetic length overflow")?;
        if end != choices.len() {
            return Err("invalid choice arithmetic framing".into());
        }
        state.arithmetic = Some(ArithmeticDecoder::new(&choices[start..], bits)?);
    }
    let mut vector_at = 0usize;
    let mut restored = Vec::with_capacity(record_count);
    for (index, record) in records.iter().enumerate() {
        if flags[index / 8] & (1 << (index % 8)) == 0 {
            restored.push((*record).to_vec());
            continue;
        }
        let lines = lines_keepends(record);
        let topology = hydrogen_topology(&lines, true)?;
        let mut rebuilt = lines.iter().map(|line| line.to_vec()).collect::<Vec<_>>();
        let mut coordinates = Vec::with_capacity(topology.atoms);
        let mut slots = HashMap::new();
        for atom in 0..topology.atoms {
            let signs = rebuilt[4 + atom]
                .first()
                .ok_or("short coordinate line")?
                .checked_sub(b'0')
                .ok_or("invalid zero signs")?;
            if signs >= 8 {
                return Err("invalid zero signs".into());
            }
            let (options, key) = hydrogen_candidates(
                atom,
                &coordinates,
                &topology,
                &mut slots,
                model,
                radius as i64,
            )?;
            let chosen =
                hydrogen_choice_decode(&mut state, choices, representation, options.len())?;
            let residual = *vectors
                .get(vector_at)
                .ok_or("missing coordinate residual")?;
            vector_at += 1;
            let coordinate = [
                options[chosen][0]
                    .checked_add(residual[0])
                    .ok_or("coordinate overflow")?,
                options[chosen][1]
                    .checked_add(residual[1])
                    .ok_or("coordinate overflow")?,
                options[chosen][2]
                    .checked_add(residual[2])
                    .ok_or("coordinate overflow")?,
            ];
            coordinates.push(coordinate);
            if let Some(key) = key {
                slots
                    .get_mut(&key)
                    .ok_or("missing hydrogen slots")?
                    .remove(chosen);
            }
            let mut line = hydrogen_prefix(coordinate, signs)?;
            line.extend_from_slice(&rebuilt[4 + atom][1..]);
            rebuilt[4 + atom] = line;
        }
        restored.push(rebuilt.concat());
    }
    if vector_at != vectors.len()
        || !state.rank.is_zero()
        || (representation == 0 && state.digit_at != choices.len())
    {
        return Err("unused residual state".into());
    }
    Ok(join_records(&restored))
}

pub fn decode_hydrogen(blob: &[u8]) -> Result<Vec<u8>, String> {
    decode_hydrogen_with_limit(blob, DEFAULT_OUTPUT_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Generated fields: the edge 1 -> 3 differs from sequential prediction.
    // No evaluation-corpus bytes or assumed record identity are used here.
    fn fixture(endings: &[&[u8]]) -> Vec<u8> {
        let lines: [&[u8]; 11] = [
            b"coordinate fixture",
            b"native test",
            b"fixed-point coordinates",
            b"  3  2  0  0  0  0            999 V2000 \t",
            b"    1.0000   -2.0000    0.0000 C   0  0  0",
            b"    3.0000    2.0000    0.0000 H   0  0  0",
            b"    4.0000   -1.0000    0.5000 O   0  0  0",
            b"  1  2  1  0",
            b"  1  3  1  0",
            b"M  END",
            b"> extra\x0bdata",
        ];
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            out.extend_from_slice(line);
            if i + 1 != lines.len() {
                out.extend_from_slice(endings[i % endings.len()]);
            }
        }
        out
    }

    #[test]
    fn known_graph_and_sequential_residuals() {
        let input = fixture(&[b"\n"]);
        let absolute = extract(&input, 0).unwrap();
        assert_eq!(
            absolute.residuals,
            [[10000, -20000, 0], [30000, 20000, 0], [40000, -10000, 5000]]
        );
        assert_eq!(absolute.parents, [0, 0, 0]); // First vertex uses zero base, not itself.
        assert_eq!(absolute.groups, [0, 1, 3]);
        assert_eq!(
            extract(&input, 1).unwrap().residuals,
            [[10000, -20000, 0], [20000, 40000, 0], [10000, -30000, 5000]]
        );
        assert_eq!(
            extract(&input, 2).unwrap().residuals,
            [[10000, -20000, 0], [20000, 40000, 0], [30000, 10000, 5000]]
        );
    }

    #[test]
    fn all_modes_preserve_line_endings_and_literal_bytes() {
        let styles: &[&[&[u8]]] = &[&[b"\n"], &[b"\r"], &[b"\r\n"], &[b"\r\n", b"\r", b"\n"]];
        for endings in styles {
            let input = fixture(endings);
            for mode in 0..=2 {
                let record = extract(&input, mode).unwrap();
                assert_eq!(record.skeleton.len() + 90, input.len());
                assert_eq!(
                    restore(&record.skeleton, mode, &record.residuals).unwrap(),
                    input
                );
            }
        }
    }

    #[test]
    fn rejects_noncanonical_fields_modes_and_counts() {
        assert!(parse_field(b"   -0.0000").is_err());
        assert!(parse_field(b"   +1.0000").is_err());
        assert!(parse_field(b"    1.00.0").is_err());
        let mut input = fixture(&[b"\n"]);
        assert!(extract(&input, 3).is_err());
        assert!(restore(&input, 3, &[]).is_err());
        let at = input.windows(6).position(|s| s == b"  3  2").unwrap();
        input[at..at + 3].copy_from_slice(b" -1");
        assert!(extract(&input, 2).is_err());
        assert!(extract(b"short", 0).is_err());
        assert!(bond_parent(b"  1  1", 3).is_err());
        assert!(bond_parent(b"  1  4", 3).is_err());
    }

    #[test]
    fn rejects_reconstruction_overflow_and_missing_vectors() {
        let record = extract(&fixture(&[b"\n"]), 2).unwrap();
        assert!(restore(&record.skeleton, 2, &record.residuals[..2]).is_err());
        let mut residuals = record.residuals;
        residuals[1][0] = i64::MAX;
        assert!(restore(&record.skeleton, 2, &residuals).is_err());
        assert!(fixed(i64::MIN).is_err());
        assert!(fixed(10_000_000_000).is_err());
        assert_eq!(fixed(-1).unwrap(), *b"   -0.0001");
    }

    #[test]
    fn historical_frame_zero_is_exact_and_retains_unparsed_records() {
        let mut input = fixture(&[b"\n"]);
        input.extend_from_slice(SEP);
        input.extend_from_slice(b"literal tail\r\n");
        let extracted = extract(&fixture(&[b"\n"]), 0).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(MAGIC);
        expected.extend_from_slice(&[0, 0, 2, 1]);
        let mut skeleton = extracted.skeleton;
        skeleton.extend_from_slice(SEP);
        skeleton.extend_from_slice(b"literal tail\r\n");
        expected.extend_from_slice(&vencode(skeleton.len() as u64));
        expected.extend_from_slice(&skeleton);
        expected.extend_from_slice(&[3]);
        expected.extend_from_slice(&[
            0xa0, 0x9c, 0x01, 0xbf, 0xb8, 0x02, 0, 0xe0, 0xd4, 0x03, 0xc0, 0xb8, 0x02, 0, 0x80,
            0xf1, 0x04, 0x9f, 0x9c, 0x01, 0x90, 0x4e,
        ]);
        let encoded = encode(&input, 0, 0).unwrap();
        assert_eq!(encoded, expected);
        assert_eq!(decode(&encoded).unwrap(), input);
    }

    #[test]
    fn text_frame_is_exact_for_all_prediction_modes() {
        let input = fixture(&[b"\r\n"]);
        for mode in 0..=2 {
            let record = extract(&input, mode).unwrap();
            let mut expected = Vec::new();
            expected.extend_from_slice(MAGIC);
            expected.extend_from_slice(&[mode, 1, 1, 1]);
            expected.extend_from_slice(&vencode(record.skeleton.len() as u64));
            expected.extend_from_slice(&record.skeleton);
            expected.extend_from_slice(&[3]);
            for vector in record.residuals {
                expected.extend_from_slice(
                    format!("{},{},{}\n", vector[0], vector[1], vector[2]).as_bytes(),
                );
            }
            assert_eq!(encode(&input, mode, 1).unwrap(), expected);
            assert_eq!(decode(&expected).unwrap(), input);
        }
    }

    #[test]
    fn frame_rejects_truncation_trailing_and_bounded_expansion() {
        let input = fixture(&[b"\n"]);
        let binary = encode(&input, 2, 0).unwrap();
        let text = encode(&input, 2, 1).unwrap();
        assert!(decode(&binary[..binary.len() - 1]).is_err());
        let mut trailing = binary.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        assert!(decode_with_limit(&binary, input.len() - 1).is_err());
        assert!(decode_with_limit(&binary, 0).is_err());
        assert!(decode_with_limit(&binary, 1).is_err());
        let mut malformed_text = text.clone();
        malformed_text.extend_from_slice(b"not,a,vector");
        assert!(decode(&malformed_text).is_err());
        let mut palette = binary;
        palette[6] = 2;
        assert!(decode(&palette).is_err());
    }

    #[test]
    fn frame_validates_declared_coordinate_count_before_rebuild() {
        let input = fixture(&[b"\n"]);
        let frame = encode(&input, 2, 0).unwrap();
        let (_, after_records) = vdecode(&frame, 7).unwrap();
        let (skeleton_size, skeleton_start) = vdecode(&frame, after_records + 1).unwrap();
        let count_at = skeleton_start + skeleton_size as usize;
        let mut too_few = frame.clone();
        too_few[count_at] = 2;
        assert!(decode_with_limit(&too_few, usize::MAX).is_err());
        let mut too_many = frame;
        too_many[count_at] = 4;
        assert!(decode_with_limit(&too_many, usize::MAX).is_err());
    }

    #[test]
    fn text_payload_uses_bytes_splitlines_rules() {
        let input = fixture(&[b"\n"]);
        let mut frame = encode(&input, 0, 1).unwrap();
        let (_, after_records) = vdecode(&frame, 7).unwrap();
        let (skeleton_size, skeleton_start) = vdecode(&frame, after_records + 1).unwrap();
        let payload_at = skeleton_start + skeleton_size as usize + 1;
        frame[payload_at..]
            .copy_from_slice(b"10000,-20000,0\x0b30000,20000,0\n40000,-10000,5000\n");
        assert!(decode(&frame).is_err());
    }

    #[test]
    fn hydrogen_model_zero_preserves_masks_records_and_bounds() {
        let mut input = fixture(&[b"\r\n", b"\n"]);
        let zero = input
            .windows(10)
            .position(|field| field == b"    0.0000")
            .unwrap();
        input[zero..zero + 10].copy_from_slice(b"   -0.0000");
        input.extend_from_slice(SEP);
        input.extend_from_slice(b"literal tail");
        let frame = encode_hydrogen(&input).unwrap();
        assert_eq!(&frame[..5], HYDROGEN_MAGIC);
        assert_eq!(decode_hydrogen(&frame).unwrap(), input);
        assert!(decode_hydrogen_with_limit(&frame, input.len() - 1).is_err());
        let mut trailing = frame;
        trailing.push(0);
        assert!(decode_hydrogen(&trailing).is_err());
    }

    fn hydrogen_candidate_fixture() -> Vec<u8> {
        b"hydrogen fixture\nnative parity\ngenerated source\n  5  4  0  0  0  0            999 V2000\n    1.0000    0.0000    0.0000 O   0  0  0\n    0.0000    1.0000    0.0000 N   0  0  0\n    0.0000    0.0000    0.0000 C   0  0  0\n   -0.4384   -0.4384    0.0000 H   0  0  0\n   -0.4384   -0.4384    0.0000 H   0  0  0\n  3  1  1  0\n  3  2  1  0\n  3  4  1  0\n  3  5  1  0\nM  END\n".to_vec()
    }

    #[test]
    fn hydrogen_extract_keeps_absolute_coordinates_for_candidate_selection() {
        let (_, coordinates) = hydrogen_extract(&hydrogen_candidate_fixture()).unwrap();
        assert_eq!(coordinates[0], [10_000, 0, 0]);
        assert_eq!(coordinates[1], [0, 10_000, 0]);
        assert_eq!(coordinates[2], [0, 0, 0]);
    }

    #[test]
    fn hydrogen_all_models_and_choice_representations_match_reference_vectors() {
        // Generated directly from runtime/cix_runtime/legacy/sdf_hydrogen.py;
        // the two hydrogen slots make model 2 exercise all choice coders.
        let input = hydrogen_candidate_fixture();
        for model in 0..=2 {
            for representation in 0..=3 {
                let frame =
                    encode_hydrogen_with_options(&input, model, representation, 6200).unwrap();
                assert_eq!(
                    decode_hydrogen(&frame).unwrap(),
                    input,
                    "model {model}, representation {representation}"
                );
                assert_eq!(
                    &frame[..7],
                    &[b'S', b'D', b'F', b'H', 1, model, representation]
                );
            }
        }
        // Model 2's reference vector selects the first of two CORDIC slots.
        // These are the exact Python choice payloads for representations 0–3.
        for (representation, expected) in [
            (0, b"0".as_slice()),
            (1, b"\0".as_slice()),
            (2, b"\x03\x20".as_slice()),
            (3, b"\x03\x20".as_slice()),
        ] {
            let frame = encode_hydrogen_with_options(&input, 2, representation, 6200).unwrap();
            let (_, after_radius) = vdecode(&frame, 7).unwrap();
            let (_, after_records) = vdecode(&frame, after_radius).unwrap();
            let flags = 1usize.div_ceil(8);
            let (skeleton_size, skeleton_start) = vdecode(&frame, after_records + flags).unwrap();
            let (choice_size, choice_start) =
                vdecode(&frame, skeleton_start + skeleton_size as usize).unwrap();
            assert_eq!(choice_size as usize, expected.len());
            assert_eq!(
                &frame[choice_start..choice_start + choice_size as usize],
                expected
            );
        }
    }

    #[test]
    fn palette_frames_restore_all_prediction_modes_and_reject_trailing_bytes() {
        let input = fixture(&[b"\r\n", b"\n"]);
        for mode in 0..=2 {
            let frame = encode(&input, mode, 2).unwrap();
            assert_eq!(decode(&frame).unwrap(), input);
            let mut trailing = frame;
            trailing.push(0);
            assert!(decode(&trailing).is_err());
        }
        assert!(encode_with_palette(&input, 0, 2, 0).is_err());
        assert!(encode_with_palette(&input, 0, 2, 255).is_err());
    }

    #[test]
    fn palette_uses_first_seen_order_and_escapes_the_sixty_fifth_vector() {
        let vectors = (0..65)
            .map(|value| [value, -value, value * 2])
            .collect::<Vec<_>>();
        let groups = vec![0; vectors.len()];
        let mut payload = Vec::new();
        append_palette(&mut payload, &vectors, &groups, 64).unwrap();
        let (palette, streams, escaped_at) =
            decode_palette(&payload, 0, &[65, 0, 0, 0, 0]).unwrap();
        assert_eq!(palette, vectors[..64]);
        assert_eq!(streams[0].len(), 65);
        assert_eq!(streams[0][64], 64);
        for stream in &streams[1..] {
            assert!(stream.is_empty());
        }
        let (escaped, end) = read_signed_vector(&payload, escaped_at).unwrap();
        assert_eq!(escaped, vectors[64]);
        assert_eq!(end, payload.len());
    }
}
