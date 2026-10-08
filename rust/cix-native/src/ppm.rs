//! Bounded-order PPM compatible with deterministic payload versions 2 through 5.
use crate::rank::{ArithmeticDecoder, ArithmeticEncoder};
use std::collections::HashMap;

type Tables = HashMap<Vec<u8>, Vec<(u8, u32)>>;
type SeeKey = (usize, u8, u8);
type SeeStates = HashMap<SeeKey, (u32, u32)>;

const SEE_PRIOR_ESCAPE: u32 = 1;
const SEE_PRIOR_HIT: u32 = 2;
const SEE_RESCALE_LIMIT: u32 = 256;
const HISTORY_MAX: usize = 65_536;
const HISTORY_MIN: usize = 4_096;

fn context(history: &[u8], order: usize) -> &[u8] {
    if order == 0 {
        &[]
    } else {
        &history[history.len() - order..]
    }
}

fn eligible(counts: &[(u8, u32)], excluded: &[bool; 256]) -> Vec<(u8, u32)> {
    counts
        .iter()
        .copied()
        .filter(|(symbol, _)| !excluded[*symbol as usize])
        .collect()
}

/// Fixed-encoder summary of one ordered eligible-symbol scan.
///
/// This deliberately mirrors `eligible`: counts are already sorted by symbol,
/// and every unexcluded count contributes in that order.  The fixed encoder
/// needs only these aggregates and the source symbol interval, so it does not
/// materialize a temporary vector on each context probe.
struct FixedEligible {
    mass: usize,
    symbols: usize,
    singletons: usize,
    hit: Option<(usize, usize)>,
}

fn fixed_eligible(
    counts: &[(u8, u32)],
    excluded: &[bool; 256],
    source: u8,
) -> Option<FixedEligible> {
    let mut mass = 0usize;
    let mut symbols = 0usize;
    let mut singletons = 0usize;
    let mut hit = None;
    for &(value, count) in counts {
        if excluded[value as usize] {
            continue;
        }
        let count = count as usize;
        if value == source {
            hit = Some((mass, count));
        }
        mass += count;
        symbols += 1;
        if count == 1 {
            singletons += 1;
        }
    }
    (symbols != 0).then_some(FixedEligible {
        mass,
        symbols,
        singletons,
        hit,
    })
}

fn fixed_escape_mass(symbols: &FixedEligible, method: u8) -> Result<usize, String> {
    match method {
        0 => Ok(1),
        1 => Ok(symbols.symbols.max(1)),
        2 => Ok(symbols.symbols.div_ceil(2).max(1)),
        3 => Ok(symbols.singletons.max(1)),
        _ => Err("unsupported PPM escape method".into()),
    }
}

fn exclude_eligible(counts: &[(u8, u32)], excluded: &mut [bool; 256]) {
    for &(value, _) in counts {
        if !excluded[value as usize] {
            excluded[value as usize] = true;
        }
    }
}

fn exclude_symbols(symbols: &[(u8, u32)], excluded: &mut [bool; 256]) {
    for (value, _) in symbols {
        excluded[*value as usize] = true;
    }
}

fn symbol_mass(symbols: &[(u8, u32)]) -> usize {
    symbols.iter().map(|(_, count)| *count as usize).sum()
}

fn checked_order(max_order: usize) -> Result<(), String> {
    if max_order > 8 {
        Err("PPM order exceeds supported maximum".into())
    } else {
        Ok(())
    }
}

fn retain_shorter(best: &mut Option<Vec<u8>>, candidate: Vec<u8>) {
    if best
        .as_ref()
        .is_none_or(|incumbent| candidate.len() < incumbent.len())
    {
        *best = Some(candidate);
    }
}

fn fallback_rank(symbol: u8, excluded: &[bool; 256]) -> Result<(usize, usize), String> {
    let mut remaining = 0;
    let mut rank = None;
    for (symbol_id, is_excluded) in excluded.iter().enumerate() {
        if !is_excluded {
            if symbol_id == usize::from(symbol) {
                rank = Some(remaining);
            }
            remaining += 1;
        }
    }
    rank.map(|rank| (rank, remaining))
        .ok_or("PPM escape excluded source symbol".into())
}

fn decode_fallback(
    decoder: &mut ArithmeticDecoder<'_>,
    excluded: &[bool; 256],
    empty_message: &'static str,
    outside_message: &'static str,
) -> Result<u8, String> {
    let remaining = (0..=255)
        .filter(|&value| !excluded[value as usize])
        .collect::<Vec<_>>();
    if remaining.is_empty() {
        return Err(empty_message.into());
    }
    let target = decoder.target(remaining.len());
    let value = *remaining.get(target).ok_or(outside_message)?;
    decoder.update(target, target + 1, remaining.len());
    Ok(value)
}

fn observe_from(
    tables: &mut Tables,
    history: &mut Vec<u8>,
    symbol: u8,
    max_order: usize,
    min_order: usize,
) {
    let highest = max_order.min(history.len());
    let min_order = min_order.min(highest);
    for order in (min_order..=highest).rev() {
        let key = context(history, order);
        let counts = if let Some(counts) = tables.get_mut(key) {
            counts
        } else {
            tables.entry(key.to_vec()).or_default()
        };
        match counts.binary_search_by_key(&symbol, |(value, _)| *value) {
            Ok(index) => counts[index].1 += 1,
            Err(index) => counts.insert(index, (symbol, 1)),
        }
        if counts
            .iter()
            .map(|(_, count)| *count as usize)
            .sum::<usize>()
            > 16384
        {
            for (_, count) in counts.iter_mut() {
                *count = (*count).div_ceil(2).max(1);
            }
        }
    }
    if max_order > 0 {
        history.push(symbol);
        if history.len() > max_order {
            history.drain(..history.len() - max_order);
        }
    }
}

fn observe(tables: &mut Tables, history: &mut Vec<u8>, symbol: u8, max_order: usize) {
    observe_from(tables, history, symbol, max_order, 0);
}

fn update_floor(tables: &Tables, history: &[u8], symbol: u8, max_order: usize) -> usize {
    let highest = max_order.min(history.len());
    for order in (0..=highest).rev() {
        if tables.get(context(history, order)).is_some_and(|counts| {
            counts
                .binary_search_by_key(&symbol, |(value, _)| *value)
                .is_ok()
        }) {
            return order;
        }
    }
    0
}

fn training_start(prefix_length: usize, source_length: usize) -> usize {
    let training = prefix_length
        .min(HISTORY_MAX)
        .min(HISTORY_MIN.max(source_length.saturating_mul(2)));
    prefix_length - training
}

fn initial_history(
    prefix_history: &[u8],
    source_length: usize,
    max_order: usize,
) -> (usize, Vec<u8>) {
    let start = training_start(prefix_history.len(), source_length);
    let mut history = Vec::with_capacity(max_order);
    if max_order > 0 && start > 0 {
        history.extend_from_slice(&prefix_history[start.saturating_sub(max_order)..start]);
    }
    (start, history)
}

fn prime_tables(
    prefix_history: &[u8],
    source_length: usize,
    max_order: usize,
    update_exclusion: bool,
) -> Result<(Tables, Vec<u8>), String> {
    let mut tables = Tables::new();
    if prefix_history.is_empty() {
        return Ok((tables, Vec::with_capacity(max_order)));
    }
    let (start, mut history) = initial_history(prefix_history, source_length, max_order);
    for (index, &symbol) in prefix_history[start..].iter().enumerate() {
        if index & 31 == 0 {
            crate::limits::check()?;
        }
        let floor = if update_exclusion {
            update_floor(&tables, &history, symbol, max_order)
        } else {
            0
        };
        observe_from(&mut tables, &mut history, symbol, max_order, floor);
    }
    Ok((tables, history))
}

fn prime(
    prefix_history: &[u8],
    source_length: usize,
    max_order: usize,
    update_exclusion: bool,
) -> Result<(Tables, Vec<u8>), String> {
    prime_tables(prefix_history, source_length, max_order, update_exclusion)
}

fn escape_mass(symbols: &[(u8, u32)], method: u8) -> Result<usize, String> {
    match method {
        0 => Ok(1),
        1 => Ok(symbols.len().max(1)),
        2 => Ok(symbols.len().div_ceil(2).max(1)),
        3 => Ok(symbols
            .iter()
            .filter(|(_, count)| *count == 1)
            .count()
            .max(1)),
        _ => Err("unsupported PPM escape method".into()),
    }
}

fn bit_bucket(value: usize, maximum: u8) -> u8 {
    let value = value.max(1);
    ((usize::BITS - value.leading_zeros() - 1) as u8).min(maximum)
}

fn see_key(order: usize, symbols: &[(u8, u32)]) -> SeeKey {
    let total = symbols
        .iter()
        .map(|(_, count)| *count as usize)
        .sum::<usize>();
    (order, bit_bucket(symbols.len(), 7), bit_bucket(total, 15))
}

fn see_state(states: &SeeStates, key: SeeKey) -> (u32, u32) {
    states
        .get(&key)
        .copied()
        .unwrap_or((SEE_PRIOR_ESCAPE, SEE_PRIOR_HIT))
}

fn see_update(states: &mut SeeStates, key: SeeKey, escaped: bool) {
    let (mut escape_count, mut hit_count) = see_state(states, key);
    if escaped {
        escape_count += 1;
    } else {
        hit_count += 1;
    }
    if escape_count + hit_count > SEE_RESCALE_LIMIT {
        escape_count = escape_count.div_ceil(2).max(1);
        hit_count = hit_count.div_ceil(2).max(1);
    }
    states.insert(key, (escape_count, hit_count));
}

fn prime_see(
    prefix_history: &[u8],
    source_length: usize,
    max_order: usize,
    update_exclusion: bool,
) -> Result<(Tables, Vec<u8>, SeeStates), String> {
    let mut states = SeeStates::new();
    let (mut tables, mut history) = if prefix_history.is_empty() {
        return Ok((Tables::new(), Vec::with_capacity(max_order), states));
    } else {
        let (_, history) = initial_history(prefix_history, source_length, max_order);
        (Tables::new(), history)
    };
    let start = training_start(prefix_history.len(), source_length);
    for (index, &symbol) in prefix_history[start..].iter().enumerate() {
        if index & 31 == 0 {
            crate::limits::check()?;
        }
        prime_see_symbol(
            &mut tables,
            &mut history,
            &mut states,
            symbol,
            max_order,
            update_exclusion,
        );
    }
    Ok((tables, history, states))
}

fn prime_see_symbol(
    tables: &mut Tables,
    history: &mut Vec<u8>,
    states: &mut SeeStates,
    symbol: u8,
    max_order: usize,
    update_exclusion: bool,
) {
    let mut excluded = [false; 256];
    let mut matched_order = 0;
    let highest = max_order.min(history.len());
    for order in (0..=highest).rev() {
        let Some(counts) = tables.get(context(history, order)) else {
            continue;
        };
        let symbols = eligible(counts, &excluded);
        if symbols.is_empty() {
            continue;
        }
        let key = see_key(order, &symbols);
        let hit = !excluded[symbol as usize]
            && counts
                .binary_search_by_key(&symbol, |(value, _)| *value)
                .is_ok();
        see_update(states, key, !hit);
        if hit {
            matched_order = order;
            break;
        }
        exclude_symbols(&symbols, &mut excluded);
    }
    observe_from(
        tables,
        history,
        symbol,
        max_order,
        if update_exclusion { matched_order } else { 0 },
    );
}

fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..70).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated PPM bit length")?;
        *pos += 1;
        let low = (byte & 0x7f) as usize;
        if shift >= usize::BITS as usize || low > usize::MAX >> shift {
            return Err("oversized PPM varint".into());
        }
        value |= low << shift;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized PPM varint".into())
}

/// Encode one deterministic fixed-escape PPM candidate.
///
/// `update_exclusion = false` emits the frozen v3 format; `true` emits v4.
/// The output is canonical for a fixed input, history, order, escape method,
/// and update policy.
pub fn encode_fixed(
    data: &[u8],
    max_order: usize,
    prefix_history: &[u8],
    escape_method: u8,
    update_exclusion: bool,
) -> Result<Vec<u8>, String> {
    checked_order(max_order)?;
    if escape_method > 3 {
        return Err("unsupported PPM escape method".into());
    }
    let (tables, history) = prime(prefix_history, data.len(), max_order, update_exclusion)?;
    encode_fixed_from_state(
        data,
        max_order,
        escape_method,
        update_exclusion,
        tables,
        history,
    )
}

fn encode_fixed_from_state(
    data: &[u8],
    max_order: usize,
    escape_method: u8,
    update_exclusion: bool,
    mut tables: Tables,
    mut history: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let mut encoder = ArithmeticEncoder::new();
    for (index, &symbol) in data.iter().enumerate() {
        if index & 31 == 0 {
            crate::limits::check()?;
        }
        let mut excluded = [false; 256];
        let mut coded = false;
        let mut matched_order = 0;
        let highest = max_order.min(history.len());
        for order in (0..=highest).rev() {
            let Some(counts) = tables.get(context(&history, order)) else {
                continue;
            };
            let Some(symbols) = fixed_eligible(counts, &excluded, symbol) else {
                continue;
            };
            let escape = fixed_escape_mass(&symbols, escape_method)?;
            let total = symbols.mass + escape;
            if let Some((low, count)) = symbols.hit {
                let high = low + count;
                encoder.encode(low, high, total);
                coded = true;
                matched_order = order;
                break;
            }
            encoder.encode(symbols.mass, total, total);
            exclude_eligible(counts, &mut excluded);
        }
        if !coded {
            let (rank, remaining) = fallback_rank(symbol, &excluded)?;
            encoder.encode(rank, rank + 1, remaining);
        }
        observe_from(
            &mut tables,
            &mut history,
            symbol,
            max_order,
            if update_exclusion { matched_order } else { 0 },
        );
    }
    let (payload, bit_length) = encoder.finish();
    let version = if update_exclusion { 4 } else { 3 };
    let mut out = vec![version, max_order as u8, escape_method];
    put_varint(bit_length, &mut out);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Encode one deterministic v5 secondary escape estimation candidate.
pub fn encode_secondary(
    data: &[u8],
    max_order: usize,
    prefix_history: &[u8],
    update_exclusion: bool,
) -> Result<Vec<u8>, String> {
    checked_order(max_order)?;
    let (mut tables, mut history, mut states) =
        prime_see(prefix_history, data.len(), max_order, update_exclusion)?;
    let mut encoder = ArithmeticEncoder::new();
    for (index, &symbol) in data.iter().enumerate() {
        if index & 31 == 0 {
            crate::limits::check()?;
        }
        encode_see_symbol(
            &mut encoder,
            &mut tables,
            &mut history,
            &mut states,
            symbol,
            max_order,
            update_exclusion,
        )?;
    }
    let (payload, bit_length) = encoder.finish();
    let mut out = vec![5, max_order as u8, u8::from(update_exclusion)];
    put_varint(bit_length, &mut out);
    out.extend_from_slice(&payload);
    Ok(out)
}

fn encode_see_symbol(
    encoder: &mut ArithmeticEncoder,
    tables: &mut Tables,
    history: &mut Vec<u8>,
    states: &mut SeeStates,
    symbol: u8,
    max_order: usize,
    update_exclusion: bool,
) -> Result<(), String> {
    let mut excluded = [false; 256];
    let mut matched_order = 0;
    let mut coded = false;
    let highest = max_order.min(history.len());
    for order in (0..=highest).rev() {
        let Some(counts) = tables.get(context(history, order)) else {
            continue;
        };
        let symbols = eligible(counts, &excluded);
        if symbols.is_empty() {
            continue;
        }
        let key = see_key(order, &symbols);
        let (escape_count, hit_count) = see_state(states, key);
        let event_total = (escape_count + hit_count) as usize;
        if let Some(index) = symbols.iter().position(|(value, _)| *value == symbol) {
            encoder.encode(0, hit_count as usize, event_total);
            encode_see_hit(encoder, &symbols, index);
            coded = true;
            matched_order = order;
            see_update(states, key, false);
            break;
        }
        encoder.encode(hit_count as usize, event_total, event_total);
        see_update(states, key, true);
        exclude_symbols(&symbols, &mut excluded);
    }
    if !coded {
        let (rank, remaining) = fallback_rank(symbol, &excluded)
            .map_err(|_| "PPM SEE escape excluded source symbol".to_owned())?;
        encoder.encode(rank, rank + 1, remaining);
    }
    observe_from(
        tables,
        history,
        symbol,
        max_order,
        if update_exclusion { matched_order } else { 0 },
    );
    Ok(())
}

fn encode_see_hit(encoder: &mut ArithmeticEncoder, symbols: &[(u8, u32)], index: usize) {
    let mass = symbol_mass(symbols);
    let low = symbols[..index]
        .iter()
        .map(|(_, count)| *count as usize)
        .sum::<usize>();
    let high = low + symbols[index].1 as usize;
    encoder.encode(low, high, mass);
}

/// Select the shortest payload from the frozen Python PPM candidate portfolio.
///
/// `None` matches the Python default for each option. Equal-length candidates
/// retain the first candidate in Python's iteration order.
pub fn encode_best(
    data: &[u8],
    max_order: usize,
    prefix_history: &[u8],
    escape_method: Option<u8>,
    update_exclusion: Option<bool>,
    secondary_escape: Option<bool>,
) -> Result<Vec<u8>, String> {
    checked_order(max_order)?;
    if escape_method.is_some_and(|method| method > 3) {
        return Err("unsupported PPM escape method".into());
    }
    let variants: &[bool] = match update_exclusion {
        Some(false) => &[false],
        Some(true) => &[true],
        None => &[false, true],
    };
    let methods: &[u8] = match escape_method {
        Some(0) => &[0],
        Some(1) => &[1],
        Some(2) => &[2],
        Some(3) => &[3],
        Some(_) => unreachable!(),
        None => &[0, 1, 2, 3],
    };
    let mut best: Option<Vec<u8>> = None;
    if secondary_escape != Some(true) {
        for &variant in variants {
            let (tables, history) = prime(prefix_history, data.len(), max_order, variant)?;
            for &method in methods {
                let candidate = encode_fixed_from_state(
                    data,
                    max_order,
                    method,
                    variant,
                    tables.clone(),
                    history.clone(),
                )?;
                retain_shorter(&mut best, candidate);
            }
        }
    }
    if secondary_escape != Some(false) {
        for &variant in variants {
            let candidate = encode_secondary(data, max_order, prefix_history, variant)?;
            retain_shorter(&mut best, candidate);
        }
    }
    best.ok_or_else(|| "no PPM variants requested".into())
}

/// Frozen BWT-v6 order estimate: method-1 escape mass, no update exclusion,
/// measured on at most 4096 leading bytes and scaled to the complete stream.
pub fn estimate_bits(data: &[u8], max_order: usize) -> Result<f64, String> {
    checked_order(max_order)?;
    let sample_length = data.len().min(4096);
    if sample_length == 0 {
        return Ok(0.0);
    }
    let mut tables = Tables::new();
    let mut history = Vec::with_capacity(max_order);
    let mut bits = 0.0f64;
    for (index, &symbol) in data[..sample_length].iter().enumerate() {
        if index & 31 == 0 {
            crate::limits::check()?;
        }
        let mut excluded = [false; 256];
        let mut coded = false;
        let highest = max_order.min(history.len());
        for order in (0..=highest).rev() {
            let Some(counts) = tables.get(context(&history, order)) else {
                continue;
            };
            let symbols = eligible(counts, &excluded);
            if symbols.is_empty() {
                continue;
            }
            let mass = symbol_mass(&symbols);
            let escape = symbols.len().max(1);
            let total = mass + escape;
            if let Some((_, count)) = symbols.iter().find(|(value, _)| *value == symbol) {
                bits += (total as f64).log2() - (*count as f64).log2();
                coded = true;
                break;
            }
            bits += (total as f64).log2() - (escape as f64).log2();
            exclude_symbols(&symbols, &mut excluded);
        }
        if !coded {
            let remaining = 256 - excluded.iter().filter(|&&value| value).count();
            bits += (remaining.max(1) as f64).log2();
        }
        observe(&mut tables, &mut history, symbol, max_order);
    }
    Ok(bits * data.len() as f64 / sample_length as f64)
}

/// Preserve the original native CIXG1 route API and canonical v3/method-0 bytes.
pub fn encode(data: &[u8], max_order: usize) -> Result<Vec<u8>, String> {
    encode_fixed(data, max_order, &[], 0, false)
}

pub fn decode(blob: &[u8], source_length: usize) -> Result<Vec<u8>, String> {
    decode_with_history(blob, source_length, &[])
}
pub fn decode_with_history(
    blob: &[u8],
    source_length: usize,
    prefix_history: &[u8],
) -> Result<Vec<u8>, String> {
    let (version, max_order, control, payload, bit_length) = decode_header(blob)?;
    if version == 5 {
        return decode_see(
            payload,
            bit_length,
            source_length,
            prefix_history,
            max_order,
            control & 1 != 0,
        );
    }
    let escape_method = control;
    let update_exclusion = version == 4;
    let (mut tables, mut history) = if version == 2 {
        (
            Tables::new(),
            prefix_history[prefix_history.len().saturating_sub(max_order)..].to_vec(),
        )
    } else {
        prime(prefix_history, source_length, max_order, update_exclusion)?
    };
    let mut decoder = ArithmeticDecoder::new(payload, bit_length);
    let mut out = Vec::with_capacity(source_length);
    for index in 0..source_length {
        if index & 31 == 0 {
            crate::limits::check()?;
        }
        let value = decode_fixed_symbol(
            &mut decoder,
            &mut tables,
            &mut history,
            max_order,
            escape_method,
            update_exclusion,
        )?;
        out.push(value);
    }
    Ok(out)
}

fn decode_header(blob: &[u8]) -> Result<(u8, usize, u8, &[u8], usize), String> {
    if blob.len() < 3 || !matches!(blob[0], 2..=5) {
        return Err("unsupported PPM payload version".into());
    }
    let (version, max_order, control) = (blob[0], blob[1] as usize, blob[2]);
    if max_order > 8 {
        return Err("unsupported PPM order".into());
    }
    if version == 5 && control & !1 != 0 {
        return Err("unsupported PPM SEE flags".into());
    }
    if version != 5 && control > 3 {
        return Err("unsupported PPM escape method".into());
    }
    let mut pos = 3;
    let bit_length = read_varint(blob, &mut pos)?;
    let end = pos
        .checked_add(bit_length.div_ceil(8))
        .ok_or("PPM payload length overflow")?;
    if end != blob.len() {
        return Err("PPM payload length mismatch".into());
    }
    Ok((version, max_order, control, &blob[pos..end], bit_length))
}

fn decode_fixed_symbol(
    decoder: &mut ArithmeticDecoder,
    tables: &mut Tables,
    history: &mut Vec<u8>,
    max_order: usize,
    escape_method: u8,
    update_exclusion: bool,
) -> Result<u8, String> {
    let mut excluded = [false; 256];
    let mut symbol = None;
    let mut matched_order = 0;
    for order in (0..=max_order.min(history.len())).rev() {
        let Some(counts) = tables.get(context(history, order)) else {
            continue;
        };
        let symbols = eligible(counts, &excluded);
        if symbols.is_empty() {
            continue;
        }
        let mass = symbol_mass(&symbols);
        let total = mass + escape_mass(&symbols, escape_method)?;
        let target = decoder.target(total);
        if target < mass {
            symbol = decode_symbol_interval(decoder, &symbols, target, total)?;
            if symbol.is_none() {
                return Err("PPM symbol interval not found".into());
            }
            matched_order = order;
            break;
        }
        decoder.update(mass, total, total);
        exclude_symbols(&symbols, &mut excluded);
    }
    let value = if let Some(value) = symbol {
        value
    } else {
        decode_fallback(
            decoder,
            &excluded,
            "PPM fallback alphabet is empty",
            "PPM fallback target outside alphabet",
        )?
    };
    observe_from(
        tables,
        history,
        value,
        max_order,
        if update_exclusion { matched_order } else { 0 },
    );
    Ok(value)
}

fn decode_symbol_interval(
    decoder: &mut ArithmeticDecoder,
    symbols: &[(u8, u32)],
    target: usize,
    total: usize,
) -> Result<Option<u8>, String> {
    let mut low = 0usize;
    for &(candidate, count) in symbols {
        let high = low + count as usize;
        if target < high {
            decoder.update(low, high, total);
            return Ok(Some(candidate));
        }
        low = high;
    }
    Ok(None)
}

fn decode_see(
    payload: &[u8],
    bit_length: usize,
    source_length: usize,
    prefix_history: &[u8],
    max_order: usize,
    update_exclusion: bool,
) -> Result<Vec<u8>, String> {
    let (mut tables, mut history, mut states) =
        prime_see(prefix_history, source_length, max_order, update_exclusion)?;
    let mut decoder = ArithmeticDecoder::new(payload, bit_length);
    let mut out = Vec::with_capacity(source_length);
    for index in 0..source_length {
        if index & 31 == 0 {
            crate::limits::check()?;
        }
        let value = decode_see_symbol(
            &mut decoder,
            &mut tables,
            &mut history,
            &mut states,
            max_order,
            update_exclusion,
        )?;
        out.push(value);
    }
    Ok(out)
}

fn decode_see_symbol(
    decoder: &mut ArithmeticDecoder,
    tables: &mut Tables,
    history: &mut Vec<u8>,
    states: &mut SeeStates,
    max_order: usize,
    update_exclusion: bool,
) -> Result<u8, String> {
    let mut excluded = [false; 256];
    let mut symbol = None;
    let mut matched_order = 0;
    for order in (0..=max_order.min(history.len())).rev() {
        let Some(counts) = tables.get(context(history, order)) else {
            continue;
        };
        let symbols = eligible(counts, &excluded);
        if symbols.is_empty() {
            continue;
        }
        let key = see_key(order, &symbols);
        let (escape_count, hit_count) = see_state(states, key);
        let event_total = (escape_count + hit_count) as usize;
        if decoder.target(event_total) < hit_count as usize {
            decoder.update(0, hit_count as usize, event_total);
            let mass = symbol_mass(&symbols);
            symbol = decode_symbol_interval(decoder, &symbols, decoder.target(mass), mass)?;
            if symbol.is_none() {
                return Err("PPM SEE symbol interval not found".into());
            }
            matched_order = order;
            see_update(states, key, false);
            break;
        }
        decoder.update(hit_count as usize, event_total, event_total);
        see_update(states, key, true);
        exclude_symbols(&symbols, &mut excluded);
    }
    let value = if let Some(value) = symbol {
        value
    } else {
        decode_fallback(
            decoder,
            &excluded,
            "PPM SEE fallback alphabet is empty",
            "PPM SEE fallback target outside alphabet",
        )?
    };
    observe_from(
        tables,
        history,
        value,
        max_order,
        if update_exclusion { matched_order } else { 0 },
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn rescale_fixture_input() -> Vec<u8> {
        (0..20_000)
            .map(|index| ((index * 17 + index / 7) % 11) as u8)
            .collect()
    }

    #[test]
    fn rescale_ceil_matches_old_formula_for_normal_model_counts() {
        for count in 0u32..=32_768 {
            assert_eq!(
                ((count >> 1) + (count & 1)).max(1),
                count.div_ceil(2).max(1)
            );
        }
    }

    #[test]
    fn rescale_ceil_near_u32_max_matches_wide_oracle() {
        for count in [u32::MAX - 3, u32::MAX - 2, u32::MAX - 1, u32::MAX] {
            let oracle = (u64::from(count) + 1) >> 1;
            assert_eq!(u64::from(count.div_ceil(2).max(1)), oracle);
        }
    }

    #[test]
    fn observe_rescale_follows_the_prechange_count_transition() {
        let mut tables = Tables::new();
        tables.insert(Vec::new(), vec![(b'x', 16_384)]);
        let mut history = Vec::new();
        observe_from(&mut tables, &mut history, b'x', 0, 0);
        assert_eq!(tables.get(&Vec::new()), Some(&vec![(b'x', 8_193)]));
    }

    #[test]
    fn see_rescale_follows_the_prechange_count_transition() {
        let key = (0, 0, 0);
        let mut states = SeeStates::new();
        for _ in 0..254 {
            see_update(&mut states, key, false);
        }
        assert_eq!(states.get(&key), Some(&(1, 128)));
    }

    #[test]
    fn fixed_and_see_rescale_payloads_match_prechange_fixtures_and_restore() {
        let input = rescale_fixture_input();
        let fixed_v3 = encode_fixed(&input, 3, &[], 2, false).expect("v3 fixed fixture");
        let fixed_v4 = encode_fixed(&input, 3, &[], 2, true).expect("v4 fixed fixture");
        let see = encode_secondary(&input, 3, &[], true).expect("SEE fixture");
        assert_eq!(fixed_v3.len(), 1_356);
        assert_eq!(
            hex_digest(&fixed_v3),
            "97f10d47bc98cc9ac14dda5196baae609136f0a7e778029f1ef80af82971762d"
        );
        assert_eq!(fixed_v4.len(), 1_357);
        assert_eq!(
            hex_digest(&fixed_v4),
            "3555ac79d01304720df52ec9ff486d6c627848345b2938c8efa40ab6b7285ffd"
        );
        assert_eq!(see.len(), 1_356);
        assert_eq!(
            hex_digest(&see),
            "efb2cbbe5d8f6b43016123c4046e30a2c51953499ebaf748b0903fff31927772"
        );
        assert_eq!(
            decode(&fixed_v3, input.len()).expect("v3 fixed decode"),
            input
        );
        assert_eq!(
            decode(&fixed_v4, input.len()).expect("v4 fixed decode"),
            input
        );
        assert_eq!(decode(&see, input.len()).expect("SEE decode"), input);
    }

    #[test]
    fn malformed_payloads_remain_rejected() {
        for payload in [
            vec![6, 0, 0],
            vec![3, 9, 0],
            vec![3],
            vec![3, 0, 0, 8],
            vec![3, 0, 0, 0, 0],
            vec![5, 0, 2, 0],
        ] {
            assert!(decode(&payload, 1).is_err(), "payload {payload:?}");
        }
    }

    #[test]
    fn varint_rejects_bits_outside_usize() {
        let groups = usize::BITS.div_ceil(7) as usize;
        let mut maximum = vec![0xff; groups];
        let last_bits = usize::BITS as usize - (groups - 1) * 7;
        maximum[groups - 1] = ((1u16 << last_bits) - 1) as u8;
        let mut pos = 0;
        assert_eq!(read_varint(&maximum, &mut pos), Ok(usize::MAX));

        let mut overflow = maximum;
        overflow[groups - 1] += 1;
        let mut pos = 0;
        assert_eq!(
            read_varint(&overflow, &mut pos),
            Err("oversized PPM varint".into())
        );
    }

    fn hex_digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
}
