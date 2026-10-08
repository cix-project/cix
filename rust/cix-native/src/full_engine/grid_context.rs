//! Historical `CIXY\x01` grid-feedback compatibility.
//!
//! `CIXY\x01` is deliberately separate from the later `CIXB\x1e` bounded
//! envelope.  Its raw parent is `SGFB1`, whose policy byte is the correction
//! symbol coding (one: literal IDs, two: multinomial rank, three: arithmetic
//! range coding) and whose model byte selects causal coordinate feedback.
//! The backend is injected by the caller; this module never substitutes an
//! alternative compressor for a historical backend tag.

use super::arithmetic::{vdecode, vencode};
use super::conditional_values::{base_prefix, feedback, read_signed, signed, write_base_field};
use super::strided_values::{inverse_wcs_grid_numeric_with_limit, transform_wcs_grid_numeric};

pub const MAGIC: &[u8] = b"CIXY\x01";
pub const RAW_MAGIC: &[u8] = b"SGFB1";
const WIDTHS: [usize; 4] = [8, 8, 4, 4];
const MAX_RECORDS: usize = 8_000_000;
type GridField = (Vec<i64>, Vec<i64>, Vec<u8>, usize);

pub type EncodeBackend<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a;
pub type DecodeBackend<'a> = dyn Fn(&[u8], usize) -> Result<Vec<u8>, String> + 'a;

fn read_q(data: &[u8], n: usize) -> Result<Vec<i64>, String> {
    let mut values = Vec::new();
    values
        .try_reserve(n)
        .map_err(|_| "feedback coordinate allocation")?;
    let mut at = 0;
    let mut previous = 0i64;
    for _ in 0..n {
        let (delta, next) = read_signed(data, at)?;
        at = next;
        previous = previous
            .checked_add(delta)
            .ok_or("quantized coordinate overflow")?;
        if previous <= i64::from(i32::MIN) || previous >= i64::from(i32::MAX) {
            return Err("quantized coordinate outside range".into());
        }
        values.push(previous);
    }
    if at != data.len() {
        return Err("coordinate stream length mismatch".into());
    }
    Ok(values)
}

fn read_le_values(data: &[u8], width: usize, n: usize) -> Result<Vec<i64>, String> {
    if data.len() != n.checked_mul(width).ok_or("raw correction size")? {
        return Err("raw correction size".into());
    }
    match width {
        4 => Ok(data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|item| i64::from(i32::from_le_bytes(*item)))
            .collect()),
        8 => Ok(data
            .as_chunks::<8>()
            .0
            .iter()
            .map(|item| i64::from_le_bytes(*item))
            .collect()),
        _ => Err("unsupported correction width".into()),
    }
}

fn read_ids(payload: &[u8], n: usize, mode: u8) -> Result<Vec<u8>, String> {
    if mode == 1 {
        return (payload.len() == n)
            .then(|| payload.to_vec())
            .ok_or_else(|| "correction ID length".into());
    }
    let mut ids = Vec::new();
    ids.try_reserve(n).map_err(|_| "correction ID allocation")?;
    let mut at = 0;
    for offset in (0..n).step_by(1024) {
        let (length, start) = vdecode(payload, at)?;
        let length = usize::try_from(length).map_err(|_| "correction class length")?;
        let end = start.checked_add(length).ok_or("correction class length")?;
        let frame = payload
            .get(start..end)
            .ok_or("truncated correction classes")?;
        let (part, used) =
            super::strided_values::type_class_decode(frame, 0, (n - offset).min(1024))?;
        if used != frame.len() {
            return Err("correction class mismatch".into());
        }
        ids.extend(part);
        at = end;
    }
    if at != payload.len() || ids.len() != n {
        return Err("trailing correction classes".into());
    }
    Ok(ids)
}

fn read_field(
    data: &[u8],
    at: usize,
    n: usize,
    width: usize,
    mode: u8,
) -> Result<GridField, String> {
    let start = at;
    let scale_end = at.checked_add(8).ok_or("grid scale overflow")?;
    data.get(at..scale_end).ok_or("truncated grid scale")?;
    let (size, q_start) = vdecode(data, scale_end)?;
    let size = usize::try_from(size).map_err(|_| "coordinate size overflow")?;
    let q_end = q_start
        .checked_add(size)
        .ok_or("coordinate size overflow")?;
    let q = read_q(
        data.get(q_start..q_end)
            .ok_or("truncated coordinate stream")?,
        n,
    )?;
    let prefix = data
        .get(start..q_end)
        .ok_or("truncated grid field")?
        .to_vec();
    let coding = *data.get(q_end).ok_or("missing correction coding")?;
    let mut pos = q_end.checked_add(1).ok_or("correction offset")?;
    let values = match coding {
        0 => {
            let (size, next) = vdecode(data, pos)?;
            pos = next;
            let size = usize::try_from(size).map_err(|_| "raw correction length")?;
            let end = pos.checked_add(size).ok_or("raw correction length")?;
            let raw = data.get(pos..end).ok_or("truncated raw corrections")?;
            pos = end;
            read_le_values(raw, width, n)?
        }
        1 => {
            let (count, next) = vdecode(data, pos)?;
            pos = next;
            let count = usize::try_from(count).map_err(|_| "correction palette overflow")?;
            if count > 256 || (count == 0 && n != 0) {
                return Err("invalid correction palette".into());
            }
            let mut palette = Vec::new();
            palette
                .try_reserve(count)
                .map_err(|_| "correction palette allocation")?;
            for _ in 0..count {
                let (value, next) = read_signed(data, pos)?;
                pos = next;
                let valid = match width {
                    4 => i64::from(i32::MIN)..=i64::from(i32::MAX),
                    8 => i64::MIN..=i64::MAX,
                    _ => return Err("unsupported correction width".into()),
                };
                if !valid.contains(&value) {
                    return Err("correction outside range".into());
                }
                palette.push(value);
            }
            let (size, next) = vdecode(data, pos)?;
            pos = next;
            let size = usize::try_from(size).map_err(|_| "correction payload length")?;
            let end = pos.checked_add(size).ok_or("correction payload length")?;
            let payload = data.get(pos..end).ok_or("truncated corrections")?;
            pos = end;
            let ids = read_ids(payload, n, mode)?;
            ids.into_iter()
                .map(|id| {
                    palette
                        .get(usize::from(id))
                        .copied()
                        .ok_or("invalid correction IDs".into())
                })
                .collect::<Result<Vec<_>, String>>()?
        }
        _ => return Err("invalid correction coding".into()),
    };
    Ok((q, values, prefix, pos))
}

fn write_field(values: &[i64], width: usize, mode: u8) -> Result<Vec<u8>, String> {
    if mode == 1 {
        return write_base_field(values, width);
    }
    let mut counts = std::collections::BTreeMap::<i64, usize>::new();
    for value in values {
        *counts.entry(*value).or_default() += 1;
    }
    let mut palette = counts.into_iter().collect::<Vec<_>>();
    palette.sort_by_key(|(value, count)| (std::cmp::Reverse(*count), *value));
    if palette.len() > 256 {
        let mut out = vec![0];
        let mut raw = Vec::new();
        for value in values {
            match width {
                4 => raw.extend(
                    i32::try_from(*value)
                        .map_err(|_| "correction outside range")?
                        .to_le_bytes(),
                ),
                8 => raw.extend(value.to_le_bytes()),
                _ => return Err("unsupported correction width".into()),
            }
        }
        out.extend(vencode(raw.len() as u64));
        out.extend(raw);
        return Ok(out);
    }
    let mut out = vec![1];
    out.extend(vencode(palette.len() as u64));
    let mut ids = std::collections::BTreeMap::new();
    for (index, (value, _)) in palette.iter().enumerate() {
        out.extend(signed(*value)?);
        ids.insert(
            *value,
            u8::try_from(index).map_err(|_| "correction palette")?,
        );
    }
    let stream = values.iter().map(|value| ids[value]).collect::<Vec<_>>();
    let mut payload = Vec::new();
    for chunk in stream.chunks(1024) {
        let frame = super::strided_values::type_class_encode(chunk, mode == 3)?;
        payload.extend(vencode(frame.len() as u64));
        payload.extend(frame);
    }
    out.extend(vencode(payload.len() as u64));
    out.extend(payload);
    Ok(out)
}

pub fn transform_raw(source: &[u8], mode: u8, model: u8) -> Result<Vec<u8>, String> {
    if !matches!(mode, 1..=3) || model > 1 {
        return Err("invalid feedback policy".into());
    }
    let base = transform_wcs_grid_numeric(source, 1)?;
    let (n, mut at) = base_prefix(&base)?;
    let mut out = RAW_MAGIC.to_vec();
    out.extend([mode, model]);
    out.extend_from_slice(&base[..at]);
    for width in WIDTHS {
        let (q, values, prefix, next) = read_field(&base, at, n, width, 1)?;
        at = next;
        out.extend(prefix);
        let field_values = if model == 1 {
            feedback(&q, &values, width, false)?
        } else {
            values
        };
        out.extend(write_field(&field_values, width, mode)?);
    }
    if at != base.len() {
        return Err("trailing base grid stream".into());
    }
    Ok(out)
}

pub fn inverse_raw(raw: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    if !raw.starts_with(RAW_MAGIC) || raw.len() < 7 {
        return Err("invalid feedback transform".into());
    }
    let mode = raw[5];
    let model = raw[6];
    if !matches!(mode, 1..=3) || model > 1 {
        return Err("invalid feedback transform".into());
    }
    let inner = &raw[7..];
    let (n, mut at) = base_prefix(inner)?;
    if n > MAX_RECORDS {
        return Err("feedback record count exceeds limit".into());
    }
    let output = 28usize
        .checked_add(28usize.checked_mul(n).ok_or("feedback output overflow")?)
        .ok_or("feedback output overflow")?;
    if output > max_output {
        return Err("feedback output exceeds admission".into());
    }
    let mut base = inner[..at].to_vec();
    for width in WIDTHS {
        let (q, values, prefix, next) = read_field(inner, at, n, width, mode)?;
        at = next;
        let restored = if model == 1 {
            feedback(&q, &values, width, true)?
        } else {
            values
        };
        base.extend(prefix);
        base.extend(write_base_field(&restored, width)?);
    }
    if at != inner.len() {
        return Err("trailing feedback transform".into());
    }
    inverse_wcs_grid_numeric_with_limit(&base, max_output)
}

pub fn encode_frame(
    source: &[u8],
    mode: u8,
    model: u8,
    backend_id: u8,
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if backend_id > 3 {
        return Err("invalid feedback backend".into());
    }
    let raw = transform_raw(source, mode, model)?;
    let payload = backend(&raw)?;
    let mut out = MAGIC.to_vec();
    out.push(backend_id);
    out.extend(vencode(source.len() as u64));
    out.extend(crc32fast::hash(source).to_be_bytes());
    out.extend(payload);
    Ok(out)
}

pub fn decode_frame(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if !frame.starts_with(MAGIC) || frame.len() < 6 || memory == 0 {
        return Err("invalid feedback frame".into());
    }
    if frame[5] > 3 {
        return Err("invalid feedback backend".into());
    }
    let (length, at) = vdecode(frame, 6)?;
    let length = usize::try_from(length).map_err(|_| "feedback output overflow")?;
    if length > limit || length > memory {
        return Err("feedback output exceeds limit".into());
    }
    let checksum: [u8; 4] = frame
        .get(at..at.checked_add(4).ok_or("feedback frame overflow")?)
        .ok_or("truncated feedback frame")?
        .try_into()
        .map_err(|_| "truncated feedback checksum")?;
    let work = length
        .checked_mul(8)
        .ok_or("feedback working-memory overflow")?;
    let budget = memory
        .checked_sub(work)
        .ok_or("feedback combined live-buffer limit")?;
    let payload_at = at.checked_add(4).ok_or("feedback frame overflow")?;
    let payload = frame
        .get(payload_at..)
        .ok_or("truncated feedback payload")?;
    let raw = backend(payload, budget)?;
    if raw.len() > budget {
        return Err("backend ignored feedback memory limit".into());
    }
    let restored = inverse_raw(&raw, length)?;
    if restored.len() != length || crc32fast::hash(&restored) != u32::from_be_bytes(checksum) {
        return Err("feedback checksum mismatch".into());
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/grid_feedback/input.bin"
    ));

    fn fixtures() -> [(u8, u8, &'static [u8]); 6] {
        [
            (
                1,
                0,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_feedback/mode1-model0.raw"
                )),
            ),
            (
                1,
                1,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_feedback/mode1-model1.raw"
                )),
            ),
            (
                2,
                0,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_feedback/mode2-model0.raw"
                )),
            ),
            (
                2,
                1,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_feedback/mode2-model1.raw"
                )),
            ),
            (
                3,
                0,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_feedback/mode3-model0.raw"
                )),
            ),
            (
                3,
                1,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_feedback/mode3-model1.raw"
                )),
            ),
        ]
    }

    #[test]
    fn grid_context_raw_fixture_identity_and_reference_inverse() {
        for (mode, model, raw) in fixtures() {
            assert_eq!(transform_raw(INPUT, mode, model).unwrap(), raw);
            assert_eq!(inverse_raw(raw, INPUT.len()).unwrap(), INPUT);
        }
    }

    #[test]
    fn grid_context_frame_identity_crc_trailing_and_caps() {
        for (mode, model, raw) in fixtures() {
            let frame = encode_frame(INPUT, mode, model, 0, &|value| Ok(value.to_vec())).unwrap();
            let mut expected = MAGIC.to_vec();
            expected.push(0);
            expected.extend(vencode(INPUT.len() as u64));
            expected.extend(crc32fast::hash(INPUT).to_be_bytes());
            expected.extend_from_slice(raw);
            assert_eq!(frame, expected);

            let memory = INPUT.len() * 8 + raw.len();
            assert_eq!(
                decode_frame(&frame, INPUT.len(), memory, &|value, _| Ok(value.to_vec())).unwrap(),
                INPUT
            );
            assert!(inverse_raw(raw, INPUT.len() - 1).is_err());
            assert!(
                decode_frame(&frame, INPUT.len() - 1, memory, &|value, _| Ok(
                    value.to_vec()
                ))
                .is_err()
            );
            assert!(
                decode_frame(&frame, INPUT.len(), memory - 1, &|value, _| Ok(
                    value.to_vec()
                ))
                .is_err()
            );

            let mut trailing = raw.to_vec();
            trailing.push(0);
            assert!(inverse_raw(&trailing, INPUT.len()).is_err());

            let (_, crc_at) = vdecode(&frame, MAGIC.len() + 1).unwrap();
            let mut bad_crc = frame.clone();
            bad_crc[crc_at] ^= 1;
            assert!(decode_frame(
                &bad_crc,
                INPUT.len(),
                memory,
                &|value, _| Ok(value.to_vec())
            )
            .is_err());
        }
    }
}
