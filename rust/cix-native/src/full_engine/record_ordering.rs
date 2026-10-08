//! Historic MyISAM-specialist admission and CIXD envelopes.
//!
//! The transform itself is intentionally a separate callback seam while the
//! shared exact composition/range primitives are finalised.  That prevents a
//! near-equivalent rank implementation from creating archives this decoder
//! cannot read.

use super::arithmetic::{
    fixed_be, multinomial_cardinality, read_fixed, vdecode, vencode, verify_frame,
    ArithmeticDecoder, ArithmeticEncoder, Fenwick,
};
use num_bigint::BigUint;
use num_traits::{ToPrimitive, Zero};
use std::collections::BTreeMap;

pub const RECORD_MAGIC: &[u8] = b"CIXD\x01";
pub const FIXED_MAGIC: &[u8] = b"CIXD\x02";
pub const RECORD_RAW_MAGIC: &[u8] = b"MYCS1";
pub const FIXED_RAW_MAGIC: &[u8] = b"MYCC1";
// A CIXD decoder keeps the backend result while reconstructing the final
// source.  Reserve six source-lengths for inverse columns, intermediate raw
// material, and the final output before giving the remainder to the backend.
// This is deliberately conservative: it makes `memory` a combined live-buffer
// cap rather than merely a decompressor-output cap.
const INVERSE_WORKING_MULTIPLIER: usize = 6;
// The canonical constrained row has a 60-byte body and a four-byte wrapper.
// This converts the paid source-length cap into a safe bound before allocating
// the nine `Vec<Vec<u8>>` column representations.
const MIN_CONSTRAINED_RECORD_BYTES: usize = 64;
const MAX_RAW_INVERSE_BYTES: usize = 256 * 1024 * 1024;
pub type EncodeBackend<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a;
pub type DecodeBackend<'a> = dyn Fn(&[u8], usize) -> Result<Vec<u8>, String> + 'a;

pub fn admit_mysql_records(data: &[u8]) -> bool {
    data.len() >= 3 && matches!(data[0], 1 | 3) && u16::from_be_bytes([data[1], data[2]]) != 0
}
pub fn admit_mysql_record_constraints(data: &[u8]) -> bool {
    admit_mysql_records(data)
}
pub fn admit_mysql_remaining_counts(data: &[u8]) -> bool {
    admit_mysql_records(data)
}
pub fn admit_mysql_fixed_columns(data: &[u8]) -> bool {
    admit_mysql_records(data)
}
pub fn recognizes_frame(frame: &[u8]) -> bool {
    frame.starts_with(RECORD_MAGIC) || frame.starts_with(FIXED_MAGIC)
}

fn packed(blob: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = vencode(blob.len() as u64);
    out.extend_from_slice(blob);
    Ok(out)
}

fn read_packed(blob: &[u8], at: usize) -> Result<(&[u8], usize), String> {
    let (length, start) = vdecode(blob, at)?;
    let length = usize::try_from(length).map_err(|_| "column length overflow")?;
    let end = start.checked_add(length).ok_or("column length overflow")?;
    Ok((blob.get(start..end).ok_or("truncated column")?, end))
}

fn signed_encode(value: i64) -> Result<Vec<u8>, String> {
    let encoded = if value >= 0 {
        u64::try_from(value)
            .map_err(|_| "signed value overflow")?
            .checked_mul(2)
            .ok_or("signed value overflow")?
    } else {
        value
            .unsigned_abs()
            .checked_mul(2)
            .and_then(|v| v.checked_sub(1))
            .ok_or("signed value overflow")?
    };
    Ok(vencode(encoded))
}

fn signed_decode(blob: &[u8], at: usize) -> Result<(i64, usize), String> {
    let (value, next) = vdecode(blob, at)?;
    let decoded = if value & 1 == 0 {
        i64::try_from(value / 2).map_err(|_| "signed value overflow")?
    } else {
        let half = i64::try_from(value / 2).map_err(|_| "signed value overflow")?;
        half.checked_neg()
            .and_then(|v| v.checked_sub(1))
            .ok_or("signed value overflow")?
    };
    Ok((decoded, next))
}

fn leap_year(year: i32) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

fn month_days(month: u32, year: i32) -> Option<u32> {
    let normal = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut days = *normal.get(usize::try_from(month.checked_sub(1)?).ok()?)?;
    if month == 2 && leap_year(year) {
        days = 29;
    }
    Some(days)
}

/// Python's ``date.toordinal`` / ``fromordinal`` convention without a time
/// dependency.  Dates in a MyISAM row are always positive Gregorian years.
fn date_to_ordinal(month: u32, day: u32, year: i32) -> Result<u32, String> {
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > month_days(month, year).ok_or("invalid date")?
    {
        return Err("invalid date".into());
    }
    let y = i64::from(year - 1);
    let prior_years = 365i64
        .checked_mul(y)
        .and_then(|v| v.checked_add(y / 4))
        .and_then(|v| v.checked_sub(y / 100))
        .and_then(|v| v.checked_add(y / 400))
        .ok_or("date overflow")?;
    let cumulative = [0u32, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut ordinal = prior_years
        .checked_add(i64::from(
            *cumulative.get((month - 1) as usize).ok_or("invalid date")?,
        ))
        .and_then(|v| v.checked_add(i64::from(day)))
        .ok_or("date overflow")?;
    if month > 2 && leap_year(year) {
        ordinal += 1;
    }
    u32::try_from(ordinal).map_err(|_| "date overflow".into())
}

fn ordinal_to_date(ordinal: u32) -> Result<(u32, u32, i32), String> {
    if ordinal == 0 {
        return Err("invalid ordinal date".into());
    }
    let mut days = i64::from(ordinal) - 1;
    let cycles = days / 146_097;
    let mut year = cycles
        .checked_mul(400)
        .and_then(|v| v.checked_add(1))
        .ok_or("date overflow")?;
    days %= 146_097;
    while days
        >= if leap_year(i32::try_from(year).map_err(|_| "date overflow")?) {
            366
        } else {
            365
        }
    {
        days -= if leap_year(i32::try_from(year).map_err(|_| "date overflow")?) {
            366
        } else {
            365
        };
        year += 1;
    }
    let year_i32 = i32::try_from(year).map_err(|_| "date overflow")?;
    if year_i32 > 9999 {
        return Err("invalid ordinal date".into());
    }
    let mut month = 1u32;
    while days >= i64::from(month_days(month, year_i32).ok_or("invalid date")?) {
        days -= i64::from(month_days(month, year_i32).ok_or("invalid date")?);
        month += 1;
    }
    Ok((
        month,
        u32::try_from(days + 1).map_err(|_| "date overflow")?,
        year_i32,
    ))
}

fn parse_date(value: &[u8]) -> Result<[u8; 4], String> {
    let text = std::str::from_utf8(value).map_err(|_| "invalid date")?;
    let mut parts = text.split('/');
    let month = parts
        .next()
        .ok_or("invalid date")?
        .parse::<u32>()
        .map_err(|_| "invalid date")?;
    let day = parts
        .next()
        .ok_or("invalid date")?
        .parse::<u32>()
        .map_err(|_| "invalid date")?;
    let year = parts
        .next()
        .ok_or("invalid date")?
        .parse::<i32>()
        .map_err(|_| "invalid date")?;
    if parts.next().is_some() || format!("{month}/{day}/{year}").as_bytes() != value {
        return Err("noncanonical date".into());
    }
    Ok(date_to_ordinal(month, day, year)?.to_le_bytes())
}

fn format_date(value: &[u8]) -> Result<Vec<u8>, String> {
    let bytes: [u8; 4] = value.try_into().map_err(|_| "invalid date width")?;
    let (month, day, year) = ordinal_to_date(u32::from_le_bytes(bytes))?;
    Ok(format!("{month}/{day}/{year}").into_bytes())
}

fn record_wrap(body: &[u8]) -> Result<Vec<u8>, String> {
    let length = u16::try_from(body.len()).map_err(|_| "oversized record")?;
    if length == 0 {
        return Err("empty record".into());
    }
    if body.len() % 4 == 1 {
        let mut out = vec![1];
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(body);
        return Ok(out);
    }
    let padding = (4 - body.len() % 4) % 4;
    let mut out = vec![3];
    out.extend_from_slice(&length.to_be_bytes());
    out.push(u8::try_from(padding).map_err(|_| "padding overflow")?);
    out.extend_from_slice(body);
    out.resize(out.len() + padding, 0);
    Ok(out)
}

fn record_fields(body: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if body.len() < 24 || !matches!(body.get(0..2), Some(b"\x60\x00") | Some(b"\x60\x02")) {
        return Err("unsupported field flags".into());
    }
    let mut at = 22usize;
    let money_len = usize::from(*body.get(at).ok_or("truncated money")?);
    at += 1;
    let money = body
        .get(at..at.checked_add(money_len).ok_or("money length overflow")?)
        .ok_or("truncated money")?;
    at += money_len;
    let date_len = usize::from(*body.get(at).ok_or("truncated date")?);
    at += 1;
    let date = body
        .get(at..at.checked_add(date_len).ok_or("date length overflow")?)
        .ok_or("truncated date")?;
    at += date_len;
    let fixed = body
        .get(at..at.checked_add(30).ok_or("fixed field overflow")?)
        .ok_or("truncated fixed fields")?;
    at += 30;
    let mut money_field = body.get(18..22).ok_or("truncated fixed fields")?.to_vec();
    money_field.push(u8::try_from(money.len()).map_err(|_| "money too long")?);
    money_field.extend_from_slice(money);
    let mut address = body.get(at..).ok_or("truncated address")?.to_vec();
    if body[1] == 2 {
        let declared = usize::from(*address.first().ok_or("invalid short address")?);
        if declared != address.len().saturating_sub(1) || declared >= 80 {
            return Err("invalid short address".into());
        }
        address.remove(0);
    } else if address.len() != 80 {
        return Err("invalid full address".into());
    }
    Ok(vec![
        body[2..6].to_vec(),
        body[6..10].to_vec(),
        body[10..14].to_vec(),
        body[14..18].to_vec(),
        money_field,
        parse_date(date)?.to_vec(),
        fixed[0..10].to_vec(),
        fixed[10..30].to_vec(),
        address,
    ])
}

fn join_record_fields(row: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if row.len() != 9
        || row[0].len() != 4
        || row[1].len() != 4
        || row[2].len() != 4
        || row[3].len() != 4
        || row[5].len() != 4
        || row[6].len() != 10
        || row[7].len() != 20
        || row[4].len() < 5
        || usize::from(row[4][4]) != row[4].len() - 5
        || row[8].len() > 80
    {
        return Err("invalid fixed column width".into());
    }
    let date = format_date(&row[5])?;
    let mut body = Vec::new();
    body.extend_from_slice(b"\x60");
    body.push(if row[8].len() < 80 { 2 } else { 0 });
    for field in row.iter().take(4) {
        body.extend_from_slice(field);
    }
    body.extend_from_slice(&row[4]);
    body.push(u8::try_from(date.len()).map_err(|_| "date too long")?);
    body.extend_from_slice(&date);
    body.extend_from_slice(&row[6]);
    body.extend_from_slice(&row[7]);
    if row[8].len() < 80 {
        body.push(u8::try_from(row[8].len()).map_err(|_| "address too long")?);
    }
    body.extend_from_slice(&row[8]);
    Ok(body)
}

type RecordColumns = Vec<Vec<Vec<u8>>>;
fn split_records(data: &[u8]) -> (Vec<Option<Vec<u8>>>, RecordColumns) {
    let mut at = 0usize;
    let mut records = Vec::new();
    let mut columns = vec![Vec::new(); 9];
    while at < data.len() {
        if data.len().saturating_sub(at) < 4 || !matches!(data[at], 1 | 3) {
            records.push(Some(data[at..].to_vec()));
            break;
        }
        let kind = data[at];
        let size = usize::from(u16::from_be_bytes([data[at + 1], data[at + 2]]));
        let header = if kind == 3 { 4 } else { 3 };
        let padding = if kind == 3 {
            usize::from(data[at + 3])
        } else {
            0
        };
        let Some(end) = at
            .checked_add(header)
            .and_then(|v| v.checked_add(size))
            .and_then(|v| v.checked_add(padding))
        else {
            records.push(Some(data[at..].to_vec()));
            break;
        };
        if end > data.len() || size == 0 {
            records.push(Some(data[at..].to_vec()));
            break;
        }
        let raw = &data[at..end];
        let body = &data[at + header..at + header + size];
        at = end;
        match record_fields(body).and_then(|row| {
            if record_wrap(&join_record_fields(&row)?)? == raw {
                Ok(row)
            } else {
                Err("noncanonical record storage".into())
            }
        }) {
            Ok(row) => {
                records.push(None);
                for (column, value) in columns.iter_mut().zip(row) {
                    column.push(value);
                }
            }
            Err(_) => records.push(Some(raw.to_vec())),
        }
    }
    (records, columns)
}

fn column_encode(values: &[Vec<u8>], index: usize, mode: u8) -> Result<Vec<u8>, String> {
    if mode == 0 || index == 6 {
        let mut out = vec![0];
        for value in values {
            out.extend(packed(value)?);
        }
        return Ok(out);
    }
    let numeric = matches!(index, 0 | 1 | 2 | 5);
    let mut palette = values.to_vec();
    palette.sort();
    palette.dedup();
    if numeric {
        palette
            .sort_by_key(|value| u32::from_le_bytes(value.as_slice().try_into().unwrap_or([0; 4])));
    }
    let mut lookup = BTreeMap::new();
    for (id, value) in palette.iter().enumerate() {
        lookup.insert(value.clone(), id);
    }
    let ids: Vec<usize> = values
        .iter()
        .map(|value| lookup.get(value).copied().ok_or("dictionary lookup"))
        .collect::<Result<_, _>>()?;
    let mut counts = vec![0usize; palette.len()];
    for &id in &ids {
        counts[id] = counts[id].checked_add(1).ok_or("column count overflow")?;
    }
    let mut out = vec![if numeric { 2 } else { 1 }];
    out.extend(vencode(palette.len() as u64));
    let mut previous = 0u32;
    for (value, count) in palette.iter().zip(&counts) {
        if numeric {
            let current = u32::from_le_bytes(
                value
                    .as_slice()
                    .try_into()
                    .map_err(|_| "numeric column width")?,
            );
            out.extend(vencode(u64::from(
                current
                    .checked_sub(previous)
                    .ok_or("numeric palette order")?,
            )));
            previous = current;
        } else {
            out.extend(packed(value)?);
        }
        out.extend(vencode(*count as u64));
    }
    out.extend(packed(&remaining_count_order_encode(&ids, &counts, mode)?)?);
    Ok(out)
}

fn column_decode(blob: &[u8], n: usize) -> Result<Vec<Vec<u8>>, String> {
    let kind = *blob.first().ok_or("missing column mode")?;
    if kind == 0 {
        let mut at = 1;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let (value, next) = read_packed(blob, at)?;
            out.push(value.to_vec());
            at = next;
        }
        if at != blob.len() {
            return Err("trailing column data".into());
        }
        return Ok(out);
    }
    if !matches!(kind, 1 | 2) {
        return Err("invalid column mode".into());
    }
    let (k, mut at) = vdecode(blob, 1)?;
    let k = usize::try_from(k).map_err(|_| "dictionary size overflow")?;
    if k > n {
        return Err("invalid dictionary size".into());
    }
    let mut palette = Vec::with_capacity(k);
    let mut counts = Vec::with_capacity(k);
    let mut previous = 0u32;
    for _ in 0..k {
        let value = if kind == 2 {
            let (delta, next) = vdecode(blob, at)?;
            at = next;
            previous = previous
                .checked_add(u32::try_from(delta).map_err(|_| "integer dictionary overflow")?)
                .ok_or("integer dictionary overflow")?;
            previous.to_le_bytes().to_vec()
        } else {
            let (value, next) = read_packed(blob, at)?;
            at = next;
            value.to_vec()
        };
        let (count, next) = vdecode(blob, at)?;
        at = next;
        let count = usize::try_from(count).map_err(|_| "count overflow")?;
        if count == 0 {
            return Err("empty dictionary colour".into());
        }
        palette.push(value);
        counts.push(count);
    }
    if counts.iter().try_fold(0usize, |sum, value| {
        sum.checked_add(*value).ok_or("composition overflow")
    })? != n
    {
        return Err("composition length mismatch".into());
    }
    let (payload, end) = read_packed(blob, at)?;
    if end != blob.len() {
        return Err("trailing column data".into());
    }
    remaining_count_order_decode(payload, &counts)?
        .into_iter()
        .map(|id| {
            palette
                .get(id)
                .cloned()
                .ok_or("dictionary symbol outside range".into())
        })
        .collect()
}

/// Exact `MYCS1` raw transform used by the historical record-constraint frame.
pub fn transform_mysql_record_constraints(data: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    if mode > 4 {
        return Err("invalid column policy".into());
    }
    let (records, columns) = split_records(data);
    let mut flags = vec![0u8; records.len().div_ceil(8)];
    let mut exceptions = Vec::new();
    for (index, record) in records.iter().enumerate() {
        match record {
            None => flags[index / 8] |= 1 << (index % 8),
            Some(raw) => exceptions.extend(packed(raw)?),
        }
    }
    let mut out = RECORD_RAW_MAGIC.to_vec();
    out.extend(vencode(records.len() as u64));
    out.extend(flags);
    out.extend(packed(&exceptions)?);
    for (index, column) in columns.iter().enumerate() {
        out.extend(packed(&column_encode(column, index, mode)?)?);
    }
    Ok(out)
}

/// Exact inverse of `transform_mysql_record_constraints`, including literal
/// record placement and canonical MyISAM record wrapping.
fn inverse_mysql_record_constraints_bounded(
    blob: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, String> {
    if !blob.starts_with(RECORD_RAW_MAGIC) {
        return Err("invalid MyISAM transform".into());
    }
    let (records, mut at) = vdecode(blob, RECORD_RAW_MAGIC.len())?;
    let records = usize::try_from(records).map_err(|_| "record count overflow")?;
    let flag_len = records.div_ceil(8);
    let flags = blob
        .get(at..at.checked_add(flag_len).ok_or("flag length overflow")?)
        .ok_or("truncated record flags")?;
    at += flag_len;
    let parsed = flags
        .iter()
        .enumerate()
        .map(|(byte_index, byte)| {
            (0..8)
                .filter(move |bit| byte_index * 8 + *bit < records && (*byte >> *bit) & 1 != 0)
                .count()
        })
        .sum();
    if parsed > max_output / MIN_CONSTRAINED_RECORD_BYTES {
        return Err("record count exceeds output admission".into());
    }
    let (exceptions, next) = read_packed(blob, at)?;
    at = next;
    let mut columns = Vec::new();
    for _ in 0..9 {
        let (column, next) = read_packed(blob, at)?;
        at = next;
        columns.push(column_decode(column, parsed)?);
    }
    if at != blob.len() {
        return Err("trailing transform".into());
    }
    let mut out = Vec::new();
    out.try_reserve(max_output.min(blob.len()))
        .map_err(|_| "record output allocation")?;
    let mut value_at = 0usize;
    let mut exception_at = 0usize;
    for index in 0..records {
        if (flags[index / 8] >> (index % 8)) & 1 != 0 {
            let row = columns
                .iter()
                .map(|column| {
                    column
                        .get(value_at)
                        .cloned()
                        .ok_or("missing column value".into())
                })
                .collect::<Result<Vec<_>, String>>()?;
            let wrapped = record_wrap(&join_record_fields(&row)?)?;
            let end = out
                .len()
                .checked_add(wrapped.len())
                .ok_or("record output overflow")?;
            if end > max_output {
                return Err("record output exceeds admission".into());
            }
            out.try_reserve(wrapped.len())
                .map_err(|_| "record output allocation")?;
            out.extend(wrapped);
            value_at += 1;
        } else {
            let (raw, next) = read_packed(exceptions, exception_at)?;
            let end = out
                .len()
                .checked_add(raw.len())
                .ok_or("record output overflow")?;
            if end > max_output {
                return Err("record output exceeds admission".into());
            }
            out.try_reserve(raw.len())
                .map_err(|_| "record output allocation")?;
            out.extend_from_slice(raw);
            exception_at = next;
        }
    }
    if value_at != parsed || exception_at != exceptions.len() {
        return Err("trailing exceptions".into());
    }
    Ok(out)
}

/// Exact inverse of `transform_mysql_record_constraints`.  Archive decoding
/// uses the bounded internal form below; this raw helper retains the historic
/// direct-test API.
pub fn inverse_mysql_record_constraints(blob: &[u8]) -> Result<Vec<u8>, String> {
    inverse_mysql_record_constraints_bounded(blob, MAX_RAW_INVERSE_BYTES)
}

/// Bounded raw inverse for CIXD frame decoders.  The cap is the frame's
/// already-validated declared source length and is checked during
/// reconstruction, before a malicious raw record layout can grow an output.
pub fn inverse_mysql_record_constraints_with_limit(
    blob: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, String> {
    inverse_mysql_record_constraints_bounded(blob, max_output)
}

/// Exact remaining-count MyISAM route: the historical database candidate is
/// the `MYCS1` transform with dynamic remaining-count ordering (mode 3), then
/// the ordinary `CIXD\x01` backend envelope.  This named entry prevents a
/// caller from silently substituting another record-constraint mode.
pub fn transform_mysql_remaining_counts(data: &[u8]) -> Result<Vec<u8>, String> {
    transform_mysql_record_constraints(data, 3)
}

/// Bounded inverse for the named remaining-count route.  The paid frame source
/// length is supplied by the CIXD decoder and checked before row allocation.
pub fn inverse_mysql_remaining_counts_with_limit(
    blob: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, String> {
    inverse_mysql_record_constraints_bounded(blob, max_output)
}

fn pack_ids(ids: &[usize], width: usize) -> Result<Vec<u8>, String> {
    if width == 0 {
        if ids.iter().all(|value| *value == 0) {
            return Ok(Vec::new());
        }
        return Err("fixed alphabet width".into());
    }
    let mut out = Vec::new();
    let mut acc = 0u64;
    let mut bits = 0usize;
    for &value in ids {
        if value
            >= (1usize
                .checked_shl(u32::try_from(width).map_err(|_| "fixed alphabet width")?)
                .ok_or("fixed alphabet width")?)
        {
            return Err("fixed alphabet symbol".into());
        }
        acc = (acc << width) | u64::try_from(value).map_err(|_| "fixed alphabet symbol")?;
        bits += width;
        while bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 255) as u8);
        }
        if bits == 0 {
            acc = 0;
        } else {
            acc &= (1u64 << bits) - 1;
        }
    }
    if bits != 0 {
        out.push((acc << (8 - bits)) as u8);
    }
    Ok(out)
}

fn unpack_ids(blob: &[u8], count: usize, width: usize) -> Result<Vec<usize>, String> {
    let bits = count
        .checked_mul(width)
        .ok_or("fixed alphabet length overflow")?;
    if blob.len() != bits.div_ceil(8) {
        return Err("fixed alphabet payload length".into());
    }
    if width == 0 {
        return Ok(vec![0; count]);
    }
    let mut out = Vec::with_capacity(count);
    let mut acc = 0u64;
    let mut available = 0usize;
    let mut at = 0usize;
    let mask = (1u64 << width) - 1;
    for _ in 0..count {
        while available < width {
            acc = (acc << 8) | u64::from(*blob.get(at).ok_or("fixed alphabet payload")?);
            at += 1;
            available += 8;
        }
        available -= width;
        out.push(usize::try_from((acc >> available) & mask).map_err(|_| "fixed alphabet symbol")?);
        if available == 0 {
            acc = 0;
        } else {
            acc &= (1u64 << available) - 1;
        }
    }
    if acc != 0 {
        return Err("nonzero alphabet padding".into());
    }
    Ok(out)
}

fn fixed_id_width(alphabet: usize) -> usize {
    if alphabet <= 1 {
        0
    } else {
        (alphabet - 1).ilog2() as usize + 1
    }
}

fn fixed_content(content: &[u8], width: usize, mode: u8) -> Result<Vec<u8>, String> {
    if width == 0 || !content.len().is_multiple_of(width) {
        return Err("invalid fixed column width".into());
    }
    let mut out = vec![mode];
    out.extend(vencode(width as u64));
    if mode == 1 {
        out.extend_from_slice(content);
        return Ok(out);
    }
    let mut palette = content.to_vec();
    palette.sort_unstable();
    palette.dedup();
    if palette.len() > 256 {
        return Err("fixed alphabet too large".into());
    }
    let mut lookup = BTreeMap::new();
    for (index, value) in palette.iter().enumerate() {
        lookup.insert(*value, index);
    }
    let ids = content
        .iter()
        .map(|value| lookup.get(value).copied().ok_or("fixed alphabet lookup"))
        .collect::<Result<Vec<_>, _>>()?;
    out.extend(vencode(palette.len() as u64));
    out.extend_from_slice(&palette);
    if mode == 2 {
        out.extend(pack_ids(&ids, fixed_id_width(palette.len()))?);
        return Ok(out);
    }
    let mut counts = vec![0usize; palette.len()];
    for &id in &ids {
        counts[id] = counts[id]
            .checked_add(1)
            .ok_or("fixed composition overflow")?;
    }
    let average = ids.len() / palette.len().max(1);
    for &count in &counts {
        out.extend(signed_encode(
            i64::try_from(count).map_err(|_| "fixed composition overflow")?
                - i64::try_from(average).map_err(|_| "fixed composition overflow")?,
        )?);
    }
    out.extend(remaining_count_order_encode(
        &ids,
        &counts,
        if mode == 3 { 3 } else { 2 },
    )?);
    Ok(out)
}

fn fixed_decode(column: &[u8], records: usize, max_output: usize) -> Result<Vec<u8>, String> {
    let mode = *column.first().ok_or("missing fixed-column mode")?;
    if mode == 0 {
        return Ok(column.get(1..).ok_or("missing fixed-column")?.to_vec());
    }
    if !matches!(mode, 1..=4) {
        return Err("invalid fixed-column coding".into());
    }
    let (width, mut at) = vdecode(column, 1)?;
    let width = usize::try_from(width).map_err(|_| "fixed width overflow")?;
    if width == 0 {
        return Err("invalid fixed column width".into());
    }
    let length = width
        .checked_mul(records)
        .ok_or("fixed column length overflow")?;
    if length > max_output {
        return Err("fixed column exceeds output admission".into());
    }
    let content = if mode == 1 {
        column.get(at..).ok_or("truncated fixed column")?.to_vec()
    } else {
        let (k, next) = vdecode(column, at)?;
        at = next;
        let k = usize::try_from(k).map_err(|_| "fixed alphabet overflow")?;
        if k > 256 || (k == 0 && length != 0) {
            return Err("invalid fixed-column alphabet".into());
        }
        let palette_end = at.checked_add(k).ok_or("fixed alphabet overflow")?;
        let palette = column
            .get(at..palette_end)
            .ok_or("truncated fixed alphabet")?;
        at = palette_end;
        let ids = if mode == 2 {
            unpack_ids(
                column.get(at..).ok_or("truncated fixed ids")?,
                length,
                fixed_id_width(k),
            )?
        } else {
            let average = length / k.max(1);
            let mut counts = Vec::with_capacity(k);
            for _ in 0..k {
                let (delta, next) = signed_decode(column, at)?;
                at = next;
                let count = i64::try_from(average)
                    .map_err(|_| "fixed composition overflow")?
                    .checked_add(delta)
                    .ok_or("fixed composition overflow")?;
                if count <= 0 {
                    return Err("invalid fixed-column composition".into());
                }
                counts.push(usize::try_from(count).map_err(|_| "fixed composition overflow")?);
            }
            if counts.iter().sum::<usize>() != length {
                return Err("invalid fixed-column composition".into());
            }
            remaining_count_order_decode(
                column.get(at..).ok_or("truncated fixed ordering")?,
                &counts,
            )?
        };
        if ids.iter().any(|id| *id >= k) {
            return Err("fixed-column symbol outside alphabet".into());
        }
        ids.into_iter().map(|id| palette[id]).collect()
    };
    if content.len() != length {
        return Err("fixed column length mismatch".into());
    }
    let mut out = vec![0];
    for value in content.chunks(width) {
        out.extend(packed(value)?);
    }
    Ok(out)
}

/// Exact `MYCC1` refinement of the historical constraints transform.  Only
/// literal base columns are replaced; dictionary columns retain their already
/// paid palette/composition representation.
pub fn transform_mysql_fixed_columns(data: &[u8], mode: u8) -> Result<Vec<u8>, String> {
    if !matches!(mode, 1..=4) {
        return Err("invalid fixed-column policy".into());
    }
    let raw = transform_mysql_record_constraints(data, 3)?;
    let (records, mut at) = vdecode(&raw, RECORD_RAW_MAGIC.len())?;
    let records = usize::try_from(records).map_err(|_| "record count overflow")?;
    let flags_len = records.div_ceil(8);
    let flags = raw.get(at..at + flags_len).ok_or("truncated flags")?;
    at += flags_len;
    let parsed = flags
        .iter()
        .enumerate()
        .map(|(byte_index, byte)| {
            (0..8)
                .filter(move |bit| byte_index * 8 + *bit < records && (*byte >> *bit) & 1 != 0)
                .count()
        })
        .sum::<usize>();
    let (_, next) = read_packed(&raw, at)?;
    let prefix_end = next;
    at = next;
    let mut out = FIXED_RAW_MAGIC.to_vec();
    out.extend_from_slice(
        raw.get(RECORD_RAW_MAGIC.len()..prefix_end)
            .ok_or("truncated record transform")?,
    );
    for index in 0..9 {
        let (column, next) = read_packed(&raw, at)?;
        at = next;
        let replacement = if column.first() == Some(&0) && parsed != 0 {
            let values = column_decode(column, parsed)?;
            let width = values.first().map(Vec::len).ok_or("empty fixed column")?;
            if values.iter().all(|value| value.len() == width) {
                fixed_content(&values.concat(), width, mode)?
            } else {
                let mut fallback = vec![0];
                fallback.extend_from_slice(column);
                fallback
            }
        } else {
            let mut fallback = vec![0];
            fallback.extend_from_slice(column);
            fallback
        };
        let _ = index;
        out.extend(packed(&replacement)?);
    }
    if at != raw.len() {
        return Err("trailing database transform".into());
    }
    Ok(out)
}

fn inverse_mysql_fixed_columns_bounded(blob: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    if !blob.starts_with(FIXED_RAW_MAGIC) {
        return Err("invalid fixed-column transform".into());
    }
    let (records, mut at) = vdecode(blob, FIXED_RAW_MAGIC.len())?;
    let records = usize::try_from(records).map_err(|_| "record count overflow")?;
    let flags_len = records.div_ceil(8);
    let flags = blob.get(at..at + flags_len).ok_or("truncated flags")?;
    at += flags_len;
    let parsed = flags
        .iter()
        .enumerate()
        .map(|(byte_index, byte)| {
            (0..8)
                .filter(move |bit| byte_index * 8 + *bit < records && (*byte >> *bit) & 1 != 0)
                .count()
        })
        .sum::<usize>();
    if parsed > max_output / MIN_CONSTRAINED_RECORD_BYTES {
        return Err("record count exceeds output admission".into());
    }
    let (_, next) = read_packed(blob, at)?;
    let prefix_end = next;
    at = next;
    let base_prefix = blob
        .get(FIXED_RAW_MAGIC.len()..prefix_end)
        .ok_or("truncated fixed transform")?;
    let mut base = Vec::new();
    base.try_reserve(
        RECORD_RAW_MAGIC
            .len()
            .checked_add(base_prefix.len())
            .ok_or("fixed transform overflow")?,
    )
    .map_err(|_| "fixed transform allocation")?;
    base.extend_from_slice(RECORD_RAW_MAGIC);
    base.extend_from_slice(base_prefix);
    for _ in 0..9 {
        let (column, next) = read_packed(blob, at)?;
        at = next;
        let replacement = packed(&fixed_decode(column, parsed, max_output)?)?;
        let end = base
            .len()
            .checked_add(replacement.len())
            .ok_or("fixed transform overflow")?;
        if end > max_output.saturating_mul(2) {
            return Err("fixed transform exceeds working admission".into());
        }
        base.try_reserve(replacement.len())
            .map_err(|_| "fixed transform allocation")?;
        base.extend(replacement);
    }
    if at != blob.len() {
        return Err("trailing fixed columns".into());
    }
    inverse_mysql_record_constraints_bounded(&base, max_output)
}

pub fn inverse_mysql_fixed_columns(blob: &[u8]) -> Result<Vec<u8>, String> {
    inverse_mysql_fixed_columns_bounded(blob, MAX_RAW_INVERSE_BYTES)
}

pub fn inverse_mysql_fixed_columns_with_limit(
    blob: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, String> {
    inverse_mysql_fixed_columns_bounded(blob, max_output)
}

/// Exact `mysql_remaining_counts.order_encode` port.  Modes match the legacy
/// transform: 1 literal; 2 static arithmetic; 3 dynamic arithmetic; 4 exact
/// type-class/permutation rank where bounded.
pub fn remaining_count_order_encode(
    ids: &[usize],
    counts: &[usize],
    mode: u8,
) -> Result<Vec<u8>, String> {
    let count = counts.iter().try_fold(0usize, |sum, &n| {
        sum.checked_add(n).ok_or("ordering length overflow")
    })?;
    if count != ids.len() {
        return Err("ordering composition mismatch".into());
    }
    let mut observed = vec![0usize; counts.len()];
    for &id in ids {
        let tally = observed
            .get_mut(id)
            .ok_or("ordering symbol outside composition")?;
        *tally = tally.checked_add(1).ok_or("ordering count overflow")?;
    }
    if observed != counts {
        return Err("ordering composition mismatch".into());
    }
    if ids.iter().any(|&id| id >= counts.len()) {
        return Err("ordering symbol outside composition".into());
    }
    if mode == 1 {
        let mut out = vec![0];
        for &id in ids {
            out.extend(vencode(id as u64));
        }
        return Ok(out);
    }
    if mode == 4 && counts.iter().all(|&n| n == 1) {
        return permutation_blocks_encode(ids, counts);
    }
    if mode == 4 && ids.len() <= 4096 {
        return type_class_rank_encode(ids, counts);
    }
    let dynamic = mode != 2;
    let mut tree = Fenwick::new(counts)?;
    let mut mutable = counts.to_vec();
    let mut remaining = ids.len();
    let mut coder = ArithmeticEncoder::new();
    for &id in ids {
        if mutable[id] == 0 {
            return Err("ordering composition mismatch".into());
        };
        let low = u64::try_from(tree.prefix(id)?).map_err(|_| "negative arithmetic prefix")?;
        let hi = low
            .checked_add(mutable[id] as u64)
            .ok_or("arithmetic interval overflow")?;
        coder.encode(low, hi, remaining as u64)?;
        if dynamic {
            mutable[id] -= 1;
            tree.add(id, -1)?;
            remaining -= 1;
        }
    }
    let (payload, bits) = coder.finish();
    let mut out = vec![if dynamic { 2 } else { 1 }];
    out.extend(vencode(bits as u64));
    out.extend(payload);
    Ok(out)
}
pub fn remaining_count_order_decode(blob: &[u8], counts: &[usize]) -> Result<Vec<usize>, String> {
    let kind = *blob.first().ok_or("missing ordering mode")?;
    let n: usize = counts.iter().try_fold(0usize, |a, &v| {
        a.checked_add(v).ok_or("ordering length overflow")
    })?;
    let out = match kind {
        0 => {
            let mut p = 1;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                let (x, q) = vdecode(blob, p)?;
                p = q;
                let x = usize::try_from(x).map_err(|_| "ordering symbol overflow")?;
                v.push(x)
            }
            if p != blob.len() {
                return Err("trailing IDs".into());
            }
            v
        }
        1 | 2 => {
            let (bits, p) = vdecode(blob, 1)?;
            let bits = usize::try_from(bits).map_err(|_| "arithmetic bit length overflow")?;
            if blob.len().saturating_sub(p) != bits.div_ceil(8) {
                return Err("order arithmetic length".into());
            }
            let mut d = ArithmeticDecoder::new(&blob[p..], bits)?;
            let mut tree = Fenwick::new(counts)?;
            let mut m = counts.to_vec();
            let mut rem = n;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                let t = i128::from(d.target(rem as u64)?);
                let s = tree.select(t)?;
                let lo =
                    u64::try_from(tree.prefix(s)?).map_err(|_| "negative arithmetic prefix")?;
                d.update(lo, lo + m[s] as u64, rem as u64)?;
                v.push(s);
                if kind == 2 {
                    if m[s] == 0 {
                        return Err("ordering composition mismatch".into());
                    };
                    m[s] -= 1;
                    tree.add(s, -1)?;
                    rem -= 1
                }
            }
            v
        }
        3 => type_class_rank_decode(blob, counts)?,
        4 => permutation_blocks_decode(blob, counts)?,
        _ => return Err("invalid ordering mode".into()),
    };
    let mut observed = vec![0usize; counts.len()];
    for &id in &out {
        if id >= counts.len() {
            return Err("ordering symbol outside composition".into());
        }
        observed[id] = observed[id]
            .checked_add(1)
            .ok_or("ordering count overflow")?
    }
    if observed != counts {
        return Err("ordering composition mismatch".into());
    }
    Ok(out)
}
fn type_class_rank_encode(ids: &[usize], counts: &[usize]) -> Result<Vec<u8>, String> {
    let card = multinomial_cardinality(counts)?;
    let mut total = card.clone();
    let mut tree = Fenwick::new(counts)?;
    let mut m = counts.to_vec();
    let mut rem = ids.len();
    let mut rank = BigUint::zero();
    for &s in ids {
        if m[s] == 0 {
            return Err("ordering composition mismatch".into());
        }
        let prefix = u64::try_from(tree.prefix(s)?).map_err(|_| "negative rank prefix")?;
        rank += (&total * BigUint::from(prefix)) / rem;
        total = (&total * BigUint::from(m[s])) / rem;
        m[s] -= 1;
        tree.add(s, -1)?;
        rem -= 1
    }
    let mut out = vec![3];
    out.extend(fixed_be(
        &rank,
        (&card - BigUint::from(1u8)).bits() as usize,
    )?);
    Ok(out)
}
fn type_class_rank_decode(blob: &[u8], counts: &[usize]) -> Result<Vec<usize>, String> {
    let n: usize = counts.iter().try_fold(0usize, |a, &v| {
        a.checked_add(v).ok_or("ordering length overflow")
    })?;
    if n > 4096 {
        return Err("oversized exact rank".into());
    }
    let card = multinomial_cardinality(counts)?;
    let (bits, p) = read_fixed(blob, 1, (&card - BigUint::from(1u8)).bits() as usize)?;
    if p != blob.len() || bits >= card {
        return Err("invalid type-class rank".into());
    }
    let mut rank = bits;
    let mut total = card;
    let mut tree = Fenwick::new(counts)?;
    let mut m = counts.to_vec();
    let mut rem = n;
    let mut out = Vec::with_capacity(n);
    while rem > 0 {
        let target = ((&rank * rem) / &total)
            .to_i128()
            .ok_or("rank select overflow")?;
        let s = tree.select(target)?;
        let prefix = u64::try_from(tree.prefix(s)?).map_err(|_| "negative rank prefix")?;
        rank -= (&total * BigUint::from(prefix)) / rem;
        total = (&total * BigUint::from(m[s])) / rem;
        out.push(s);
        m[s] -= 1;
        tree.add(s, -1)?;
        rem -= 1
    }
    Ok(out)
}
fn permutation_blocks_encode(ids: &[usize], counts: &[usize]) -> Result<Vec<u8>, String> {
    if counts.iter().any(|&n| n != 1) {
        return Err("not a permutation".into());
    }
    let mut out = vec![4];
    let mut tree = Fenwick::new(counts)?;
    let mut remaining = ids.len();
    for chunk in ids.chunks(1024) {
        let mut rank = BigUint::zero();
        let mut product = BigUint::from(1u8);
        for &symbol in chunk {
            let digit =
                usize::try_from(tree.prefix(symbol)?).map_err(|_| "permutation digit overflow")?;
            rank = rank * remaining + digit;
            product *= remaining;
            tree.add(symbol, -1)?;
            remaining -= 1;
        }
        out.extend(fixed_be(
            &rank,
            (&product - BigUint::from(1u8)).bits() as usize,
        )?);
    }
    Ok(out)
}

fn permutation_blocks_decode(blob: &[u8], counts: &[usize]) -> Result<Vec<usize>, String> {
    if counts.iter().any(|&n| n != 1) {
        return Err("not a permutation".into());
    }
    let mut position = 1;
    let mut tree = Fenwick::new(counts)?;
    let mut remaining = counts.len();
    let mut out = Vec::with_capacity(remaining);
    while remaining > 0 {
        let length = remaining.min(1024);
        let mut product = BigUint::from(1u8);
        for base in (remaining - length + 1)..=remaining {
            product *= base;
        }
        let (mut rank, end) = read_fixed(
            blob,
            position,
            (&product - BigUint::from(1u8)).bits() as usize,
        )?;
        position = end;
        if rank >= product {
            return Err("invalid conditional permutation rank".into());
        }
        let mut digits = vec![0usize; length];
        for index in (0..length).rev() {
            let base = BigUint::from(remaining - index);
            digits[index] = (&rank % &base)
                .to_usize()
                .ok_or("permutation digit overflow")?;
            rank /= base;
        }
        for digit in digits {
            let symbol = tree.select(digit as i128)?;
            out.push(symbol);
            tree.add(symbol, -1)?;
            remaining -= 1;
        }
    }
    if position != blob.len() {
        return Err("trailing permutation ranks".into());
    }
    Ok(out)
}

fn pack(
    magic: &[u8],
    backend_id: Option<u8>,
    source: &[u8],
    transformed: &[u8],
    encode: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    let payload = encode(transformed)?;
    let mut out = Vec::with_capacity(magic.len() + 17 + payload.len());
    out.extend_from_slice(magic);
    if let Some(id) = backend_id {
        out.push(id);
    }
    out.extend(vencode(source.len() as u64));
    out.extend_from_slice(&crc32fast::hash(source).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

fn transformed_budget(output: usize, memory: usize) -> Result<usize, String> {
    let inverse = output
        .checked_mul(INVERSE_WORKING_MULTIPLIER)
        .ok_or("MyISAM working-memory overflow")?;
    let reserved = output
        .checked_add(inverse)
        .ok_or("MyISAM working-memory overflow")?;
    memory
        .checked_sub(reserved)
        .ok_or_else(|| "MyISAM combined live-buffer limit".to_string())
}

fn unpack(
    magic: &[u8],
    backend_id: Option<u8>,
    frame: &[u8],
    limit: usize,
    memory: usize,
    decode: &DecodeBackend<'_>,
) -> Result<(usize, u32, Vec<u8>), String> {
    if !frame.starts_with(magic) || memory == 0 {
        return Err("invalid MyISAM frame".into());
    }
    let mut at = magic.len();
    if let Some(expected) = backend_id {
        let actual = *frame.get(at).ok_or("truncated MyISAM backend")?;
        if actual != expected {
            return Err("unexpected MyISAM backend".into());
        }
        at += 1;
    }
    let (n, p) = vdecode(frame, at)?;
    at = p;
    let n = usize::try_from(n).map_err(|_| "MyISAM output size overflow")?;
    if n > limit || n > memory {
        return Err("MyISAM output exceeds admission limit".into());
    }
    let backend_budget = transformed_budget(n, memory)?;
    let checksum = u32::from_be_bytes(
        frame
            .get(at..at.checked_add(4).ok_or("MyISAM frame overflow")?)
            .ok_or("truncated MyISAM frame")?
            .try_into()
            .unwrap(),
    );
    at = at.checked_add(4).ok_or("MyISAM frame overflow")?;
    let transformed = decode(
        frame.get(at..).ok_or("truncated MyISAM payload")?,
        backend_budget,
    )?;
    if transformed.len() > backend_budget {
        return Err("backend ignored transformed-output limit".into());
    }
    Ok((n, checksum, transformed))
}

/// Historic CIXD01: backend byte 0=Brotli, 1=the separately-qualified PAQ wrapper.
pub fn encode_mysql_record_constraints(
    source: &[u8],
    transformed: &[u8],
    backend_id: u8,
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if backend_id > 1 {
        return Err("invalid record backend".into());
    }
    pack(RECORD_MAGIC, Some(backend_id), source, transformed, backend)
}
pub fn decode_mysql_record_constraints(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
    inverse: impl FnOnce(&[u8], usize) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    if !frame.starts_with(RECORD_MAGIC) {
        return Err("invalid MyISAM frame".into());
    };
    let id = *frame
        .get(RECORD_MAGIC.len())
        .ok_or("truncated MyISAM backend")?;
    if id > 1 {
        return Err("invalid record backend".into());
    };
    let (n, crc, t) = unpack(RECORD_MAGIC, Some(id), frame, limit, memory, backend)?;
    let out = inverse(&t, n)?;
    verify_frame(&out, n, crc)?;
    Ok(out)
}
/// Historic CIXD02 fixed-column frame; the codec always uses its declared Brotli provider.
pub fn encode_mysql_fixed_columns(
    source: &[u8],
    transformed: &[u8],
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    pack(FIXED_MAGIC, None, source, transformed, backend)
}
pub fn decode_mysql_fixed_columns(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
    inverse: impl FnOnce(&[u8], usize) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    let (n, crc, t) = unpack(FIXED_MAGIC, None, frame, limit, memory, backend)?;
    let out = inverse(&t, n)?;
    verify_frame(&out, n, crc)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn each_order_code_restores_nontrivial_composition() {
        let ids = [2, 0, 2, 1, 0, 2];
        let counts = [2, 1, 3];
        for mode in 1..=4 {
            let encoded = remaining_count_order_encode(&ids, &counts, mode).unwrap();
            assert_eq!(
                remaining_count_order_decode(&encoded, &counts).unwrap(),
                ids
            );
        }
        assert!(remaining_count_order_encode(&[0, 0], &[1, 1], 2).is_err());
    }
    #[test]
    fn permutation_crosses_rank_block_boundary() {
        let ids: Vec<usize> = (0..1025).rev().collect();
        let counts = vec![1; ids.len()];
        let encoded = remaining_count_order_encode(&ids, &counts, 4).unwrap();
        assert_eq!(
            remaining_count_order_decode(&encoded, &counts).unwrap(),
            ids
        );
        assert!(remaining_count_order_decode(&encoded[..encoded.len() - 1], &counts).is_err());
    }

    fn sample_record(key: u32, address: &[u8]) -> Vec<u8> {
        let row = vec![
            key.to_le_bytes().to_vec(),
            (9u32).to_le_bytes().to_vec(),
            (3u32).to_le_bytes().to_vec(),
            (7u32).to_le_bytes().to_vec(),
            vec![0, 0, 0, 0, 3, b'1', b'.', b'0'],
            date_to_ordinal(2, 3, 2024).unwrap().to_le_bytes().to_vec(),
            b"CODE      ".to_vec(),
            b"NAME                ".to_vec(),
            address.to_vec(),
        ];
        record_wrap(&join_record_fields(&row).unwrap()).unwrap()
    }

    #[test]
    fn record_constraint_and_fixed_raw_transforms_restore_paid_literals() {
        let mut source = sample_record(5, b"short address");
        source.extend_from_slice(b"literal tail");
        for mode in 0..=4 {
            let raw = transform_mysql_record_constraints(&source, mode).unwrap();
            assert_eq!(inverse_mysql_record_constraints(&raw).unwrap(), source);
        }
        for mode in 1..=4 {
            let raw = transform_mysql_fixed_columns(&source, mode).unwrap();
            assert_eq!(inverse_mysql_fixed_columns(&raw).unwrap(), source);
        }
    }

    #[test]
    fn matches_tiny_legacy_mycs_and_mycc_vectors() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/record_stride/record.input"
        ));
        let mycs = [
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/record_stride/record-mycs-mode0.raw"
            ))
            .as_slice(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/record_stride/record-mycs-mode1.raw"
            ))
            .as_slice(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/record_stride/record-mycs-mode2.raw"
            ))
            .as_slice(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/record_stride/record-mycs-mode3.raw"
            ))
            .as_slice(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/record_stride/record-mycs-mode4.raw"
            ))
            .as_slice(),
        ];
        for (mode, expected) in mycs.into_iter().enumerate() {
            assert_eq!(
                transform_mysql_record_constraints(source, mode as u8).unwrap(),
                expected
            );
            assert_eq!(inverse_mysql_record_constraints(expected).unwrap(), source);
        }
        for mode in 1..=4 {
            let expected = match mode {
                1 => include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/record_stride/record-mycc-mode1.raw"
                ))
                .as_slice(),
                2 => include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/record_stride/record-mycc-mode2.raw"
                ))
                .as_slice(),
                3 => include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/record_stride/record-mycc-mode3.raw"
                ))
                .as_slice(),
                _ => include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/record_stride/record-mycc-mode4.raw"
                ))
                .as_slice(),
            };
            assert_eq!(
                transform_mysql_fixed_columns(source, mode).unwrap(),
                expected
            );
            assert_eq!(inverse_mysql_fixed_columns(expected).unwrap(), source);
        }
    }

    #[test]
    fn remaining_count_route_is_exact_mycs_mode_three_with_bounded_inverse() {
        let source = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/record_stride/record.input"
        ));
        let expected = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/record_stride/record-mycs-mode3.raw"
        ));
        assert_eq!(transform_mysql_remaining_counts(source).unwrap(), expected);
        assert_eq!(
            inverse_mysql_remaining_counts_with_limit(expected, source.len()).unwrap(),
            source
        );
        assert!(inverse_mysql_remaining_counts_with_limit(expected, source.len() - 1).is_err());
    }
}
