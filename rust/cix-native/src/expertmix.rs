//! Encoder and decoder for legacy CIXM6 mode 17 context-expert payloads.
//!
//! The selected expert is derived from exponentially decayed log loss, so no
//! selector stream is stored.  Keep the floating-point update order identical
//! to the frozen Python reference: changing it can change the selected model
//! and therefore the arithmetic-code intervals.

use crate::rank::{ArithmeticDecoder, ArithmeticEncoder};
use std::collections::HashMap;

const VERSION: u8 = 1;
const CONTEXT_LIMIT: u32 = 4096;
const DECAY: f64 = 0.98;

type Counts = Vec<(u8, u32)>;
type Table = HashMap<Vec<u8>, Counts>;

fn read_varint(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..=70).step_by(7) {
        let byte = *data
            .get(*pos)
            .ok_or("truncated context-expert bit length")?;
        *pos += 1;
        let part = (byte & 0x7f) as usize;
        if shift >= usize::BITS as usize || part > (usize::MAX >> shift) {
            return Err("context-expert bit length overflow".into());
        }
        value |= part
            .checked_shl(shift as u32)
            .ok_or("context-expert bit length overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized context-expert bit length".into())
}

fn count(counts: Option<&Counts>, symbol: u8) -> u32 {
    counts
        .and_then(|values| {
            values
                .binary_search_by_key(&symbol, |(candidate, _)| *candidate)
                .ok()
                .map(|index| values[index].1)
        })
        .unwrap_or(0)
}

fn total(counts: Option<&Counts>) -> usize {
    counts
        .map(|values| values.iter().map(|(_, n)| *n as usize).sum())
        .unwrap_or(0)
        + 256
}

fn update_loss(loss: f64, counts: Option<&Counts>, symbol: u8) -> f64 {
    let model_total = total(counts) as f64;
    let symbol_mass = (count(counts, symbol) + 1) as f64;
    let instantaneous = model_total.log2() - symbol_mass.log2();
    loss * DECAY + instantaneous
}

fn interval(counts: Option<&Counts>, symbol: u8) -> (usize, usize, usize) {
    let mut low = symbol as usize;
    let mut observed = 0usize;
    if let Some(values) = counts {
        for &(candidate, n) in values {
            if candidate < symbol {
                low += n as usize;
            } else if candidate == symbol {
                observed = n as usize;
                break;
            } else {
                break;
            }
        }
    }
    (low, low + observed + 1, total(counts))
}

fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn validate_orders(orders: &[u8]) -> Result<(), String> {
    if orders.is_empty()
        || orders.len() > 9
        || orders[0] != 0
        || orders.last().copied().unwrap_or(9) > 8
        || orders.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err("invalid context-expert orders".into());
    }
    Ok(())
}

fn decode_symbol(
    decoder: &mut ArithmeticDecoder<'_>,
    counts: Option<&Counts>,
) -> Result<u8, String> {
    let model_total = total(counts);
    let target = decoder.target(model_total);
    let mut cumulative = 0usize;
    for symbol in 0..=u8::MAX {
        let frequency = count(counts, symbol) as usize + 1;
        let high = cumulative + frequency;
        if target < high {
            decoder.update(cumulative, high, model_total);
            return Ok(symbol);
        }
        cumulative = high;
    }
    Err("context-expert target outside alphabet".into())
}

fn observe(counts: &mut Counts, symbol: u8) {
    match counts.binary_search_by_key(&symbol, |(candidate, _)| *candidate) {
        Ok(index) => counts[index].1 += 1,
        Err(index) => counts.insert(index, (symbol, 1)),
    }
    let sum: u32 = counts.iter().map(|(_, n)| *n).sum();
    if sum > CONTEXT_LIMIT {
        for (_, n) in counts {
            *n = n.div_ceil(2);
        }
    }
}

/// Encode a frozen CIXM6 mode-17 payload while bounding allocated contexts.
///
/// Prefix bytes seed only the causal context window. As in the Python
/// reference, every expert's count table starts empty for each payload.
pub fn encode_bounded(
    data: &[u8],
    orders: &[u8],
    prefix_history: &[u8],
    max_contexts: usize,
) -> Result<Vec<u8>, String> {
    validate_orders(orders)?;

    let max_order = orders[orders.len() - 1] as usize;
    let history_start = prefix_history.len().saturating_sub(max_order);
    let mut history = prefix_history[history_start..].to_vec();
    let mut tables: Vec<Table> = (0..orders.len()).map(|_| HashMap::new()).collect();
    let mut losses = vec![0.0f64; orders.len()];
    let mut contexts = Vec::with_capacity(orders.len());
    let mut context_count = 0usize;
    let mut encoder = ArithmeticEncoder::new();

    for &symbol in data {
        populate_contexts(&mut contexts, &history, orders);
        let selected = best_expert(&losses);
        let (low, high, model_total) = interval(tables[selected].get(&contexts[selected]), symbol);
        encoder.encode(low, high, model_total);
        update_experts(
            &mut tables,
            &mut losses,
            &contexts,
            symbol,
            &mut context_count,
            max_contexts,
        )?;
        push_history(&mut history, symbol, max_order);
    }

    let (coded, bit_length) = encoder.finish();
    let mut out = Vec::new();
    out.try_reserve_exact(2 + orders.len() + 10 + coded.len())
        .map_err(|_| "context-expert payload exceeds available memory")?;
    out.push(VERSION);
    out.push(orders.len() as u8);
    out.extend_from_slice(orders);
    put_varint(bit_length, &mut out);
    out.extend_from_slice(&coded);
    Ok(out)
}

fn populate_contexts(contexts: &mut Vec<Vec<u8>>, history: &[u8], orders: &[u8]) {
    contexts.clear();
    contexts.extend(orders.iter().map(|&order| {
        let order = order as usize;
        if order == 0 {
            Vec::new()
        } else {
            history[history.len().saturating_sub(order)..].to_vec()
        }
    }));
}

fn best_expert(losses: &[f64]) -> usize {
    losses
        .iter()
        .enumerate()
        .skip(1)
        .fold(0, |selected, (index, loss)| {
            if *loss < losses[selected] {
                index
            } else {
                selected
            }
        })
}

fn update_experts(
    tables: &mut [Table],
    losses: &mut [f64],
    contexts: &[Vec<u8>],
    symbol: u8,
    context_count: &mut usize,
    max_contexts: usize,
) -> Result<(), String> {
    for ((table, loss), context) in tables.iter_mut().zip(losses).zip(contexts) {
        *loss = update_loss(*loss, table.get(context), symbol);
        if !table.contains_key(context) {
            if *context_count >= max_contexts {
                return Err("context-expert model exceeds memory budget".into());
            }
            table.insert(context.clone(), Vec::new());
            *context_count += 1;
        }
        observe(
            table.get_mut(context).expect("inserted context is present"),
            symbol,
        );
    }
    Ok(())
}

fn push_history(history: &mut Vec<u8>, symbol: u8, max_order: usize) {
    if max_order == 0 {
        return;
    }
    history.push(symbol);
    if history.len() > max_order {
        history.remove(0);
    }
}

/// Decode while enforcing a cap on the total number of allocated contexts.
///
/// Prefix bytes seed only the causal context window.  The Python reference
/// starts every expert's count tables empty for each payload.
pub fn decode_bounded(
    blob: &[u8],
    source_length: usize,
    prefix_history: &[u8],
    max_contexts: usize,
) -> Result<Vec<u8>, String> {
    if blob.len() < 2 || blob[0] != VERSION {
        return Err("unknown context-expert version".into());
    }
    let order_count = blob[1] as usize;
    if !(1..=9).contains(&order_count) || blob.len() < 2 + order_count {
        return Err("invalid context-expert order count".into());
    }
    let orders = &blob[2..2 + order_count];
    validate_orders(orders)?;

    let mut pos = 2 + order_count;
    let bit_length = read_varint(blob, &mut pos)?;
    let byte_length = bit_length
        .checked_add(7)
        .ok_or("context-expert bit length overflow")?
        / 8;
    let end = pos
        .checked_add(byte_length)
        .ok_or("context-expert payload length overflow")?;
    if end != blob.len() {
        return Err("context-expert payload length mismatch".into());
    }

    let max_order = orders[order_count - 1] as usize;
    let history_start = prefix_history.len().saturating_sub(max_order);
    let mut history = prefix_history[history_start..].to_vec();
    let mut tables: Vec<Table> = (0..order_count).map(|_| HashMap::new()).collect();
    let mut losses = vec![0.0f64; order_count];
    let mut contexts = Vec::with_capacity(order_count);
    let mut context_count = 0usize;
    let mut decoder = ArithmeticDecoder::new(&blob[pos..end], bit_length);
    let mut out = Vec::new();
    out.try_reserve_exact(source_length)
        .map_err(|_| "context-expert output exceeds available memory")?;

    for _ in 0..source_length {
        populate_contexts(&mut contexts, &history, orders);
        let selected = best_expert(&losses);
        let symbol = decode_symbol(&mut decoder, tables[selected].get(&contexts[selected]))?;
        out.push(symbol);
        update_experts(
            &mut tables,
            &mut losses,
            &contexts,
            symbol,
            &mut context_count,
            max_contexts,
        )?;
        push_history(&mut history, symbol, max_order);
    }
    Ok(out)
}

#[cfg(test)]
mod integer_overflow_tests {
    use super::*;
    #[test]
    fn varint_accepts_max_rejects_high_bits() {
        let mut value = usize::MAX;
        let mut encoded = Vec::new();
        while value >= 128 {
            encoded.push((value as u8 & 127) | 128);
            value >>= 7;
        }
        encoded.push(value as u8);
        assert_eq!(read_varint(&encoded, &mut 0).unwrap(), usize::MAX);
        let groups = (usize::BITS as usize).div_ceil(7);
        let mut malformed = vec![128; groups - 1];
        malformed.push(1u8 << ((usize::BITS as usize - 1) % 7 + 1));
        assert!(read_varint(&malformed, &mut 0).is_err());
    }
}
