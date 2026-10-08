use crate::{adaptive, rank};
use sha2::{Digest, Sha256};
use std::fmt;

const MAGIC: &[u8; 5] = b"CIXG1";
const HEADER: usize = 13;
const FRAME: usize = 41;
const END: u8 = 255;
pub const MAX_BLOCK: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(&'static str);

impl Error {
    fn new(message: &'static str) -> Self {
        Self(message)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
impl From<String> for Error {
    fn from(_: String) -> Self {
        Self::new("portable codec rejected a substream")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EncodeConfig {
    pub block_size: usize,
    pub output_limit: usize,
    pub enable_composition: bool,
    pub enable_range: bool,
    pub enable_count_range: bool,
}
impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            block_size: MAX_BLOCK,
            output_limit: 128 << 20,
            enable_composition: true,
            enable_range: true,
            enable_count_range: true,
        }
    }
}
impl EncodeConfig {
    fn validate(self) -> Result<Self, Error> {
        if self.block_size == 0 || self.block_size > MAX_BLOCK {
            return Err(Error::new("portable block size must be 1 through 65536"));
        }
        if self.output_limit < HEADER + FRAME {
            return Err(Error::new(
                "portable output limit cannot contain a CIXG1 stream",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DecodeConfig {
    pub output_limit: usize,
    pub archive_limit: usize,
    pub max_block_size: usize,
}
impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            output_limit: 128 << 20,
            archive_limit: 128 << 20,
            max_block_size: MAX_BLOCK,
        }
    }
}
impl DecodeConfig {
    fn validate(self) -> Result<Self, Error> {
        if self.max_block_size == 0 || self.max_block_size > MAX_BLOCK {
            return Err(Error::new(
                "portable maximum block size must be 1 through 65536",
            ));
        }
        if self.archive_limit < HEADER + FRAME {
            return Err(Error::new(
                "portable archive limit cannot contain a CIXG1 stream",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum StreamState {
    NeedsInput = 0,
    NeedsOutput = 1,
    Finished = 2,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub consumed: usize,
    pub produced: usize,
    pub state: StreamState,
}

fn reserve(bytes: &mut Vec<u8>, additional: usize) -> Result<(), Error> {
    bytes
        .try_reserve_exact(additional)
        .map_err(|_| Error::new("portable allocation failed"))
}
fn add(a: usize, b: usize) -> Result<usize, Error> {
    a.checked_add(b)
        .ok_or_else(|| Error::new("portable size overflow"))
}
fn u32_len(n: usize) -> Result<u32, Error> {
    u32::try_from(n).map_err(|_| Error::new("portable frame length exceeds CIXG1"))
}
fn append(target: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
    reserve(target, bytes.len())?;
    target.extend_from_slice(bytes);
    Ok(())
}

fn rle(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < input.len() {
        let mut end = at + 1;
        while end < input.len() && input[end] == input[at] {
            end += 1;
        }
        out.push(input[at]);
        let mut n = (end - at) as u64;
        while n >= 128 {
            out.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        out.push(n as u8);
        at = end;
    }
    out
}
fn unvar(input: &[u8], at: &mut usize) -> Result<usize, Error> {
    let mut value = 0usize;
    for shift in (0..35).step_by(7) {
        let byte = *input
            .get(*at)
            .ok_or_else(|| Error::new("truncated portable RLE varint"))?;
        *at += 1;
        value |= ((byte & 127) as usize)
            .checked_shl(shift)
            .ok_or_else(|| Error::new("portable RLE varint overflow"))?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err(Error::new("oversized portable RLE varint"))
}
fn unrle(input: &[u8], expected: usize) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    reserve(&mut out, expected)?;
    let mut at = 0;
    while at < input.len() {
        let byte = input[at];
        at += 1;
        let n = unvar(input, &mut at)?;
        let end = add(out.len(), n)?;
        if n == 0 || end > expected {
            return Err(Error::new("invalid portable RLE length"));
        }
        out.resize(end, byte);
    }
    if out.len() != expected {
        return Err(Error::new("portable RLE output length mismatch"));
    }
    Ok(out)
}
fn entropy(input: &[u8]) -> f64 {
    if input.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    let mut order = Vec::new();
    for &byte in input {
        if counts[byte as usize] == 0 {
            order.push(byte as usize);
        }
        counts[byte as usize] += 1;
    }
    let n = input.len() as f64;
    n.log2()
        - order
            .into_iter()
            .map(|i| counts[i] as f64 * (counts[i] as f64).log2())
            .sum::<f64>()
            / n
}
fn substream(coder: u8, source: &[u8], payload: Vec<u8>) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    reserve(&mut out, add(10, payload.len())?)?;
    out.push(1);
    out.push(coder);
    out.extend_from_slice(&u32_len(source.len())?.to_le_bytes());
    out.extend_from_slice(&u32_len(payload.len())?.to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}
fn choose_route(input: &[u8], cfg: EncodeConfig) -> Result<(u8, Vec<u8>), Error> {
    // This is intentionally a fixed portable profile, not native portfolio
    // selection.  Every route/payload here is decoded by native CIXG1.
    let mut best = (0u8, input.to_vec());
    let runs = rle(input);
    if runs.len() < best.1.len() {
        best = (1, runs);
    }
    if cfg.enable_composition && entropy(&input[..input.len().min(8192)]) < 7.5 {
        let payload = substream(2, input, rank::encode_composition_tiles(input)?)?;
        if payload.len() < best.1.len() {
            best = (2, payload);
        }
    }
    if cfg.enable_range {
        let payload = substream(4, input, adaptive::encode(input))?;
        if payload.len() < best.1.len() {
            best = (2, payload);
        }
    }
    if cfg.enable_count_range {
        let payload = substream(5, input, rank::encode_count_range(input)?)?;
        if payload.len() < best.1.len() {
            best = (2, payload);
        }
    }
    Ok(best)
}
fn append_frame(out: &mut Vec<u8>, route: u8, source: &[u8], payload: &[u8]) -> Result<(), Error> {
    reserve(out, add(FRAME, payload.len())?)?;
    out.push(route);
    out.extend_from_slice(&u32_len(source.len())?.to_le_bytes());
    out.extend_from_slice(&u32_len(payload.len())?.to_le_bytes());
    out.extend_from_slice(&Sha256::digest(source));
    out.extend_from_slice(payload);
    Ok(())
}

pub struct StreamEncoder {
    config: EncodeConfig,
    input: Vec<u8>,
    pending: Vec<u8>,
    pending_at: usize,
    whole: Sha256,
    emitted: usize,
    started: bool,
    finishing: bool,
    finished: bool,
}
impl StreamEncoder {
    pub fn new(config: EncodeConfig) -> Result<Self, Error> {
        let config = config.validate()?;
        let mut input = Vec::new();
        reserve(&mut input, config.block_size)?;
        Ok(Self {
            config,
            input,
            pending: Vec::new(),
            pending_at: 0,
            whole: Sha256::new(),
            emitted: 0,
            started: false,
            finishing: false,
            finished: false,
        })
    }
    fn queue(&mut self, bytes: Vec<u8>) -> Result<(), Error> {
        let total = add(self.emitted, bytes.len())?;
        if total > self.config.output_limit {
            return Err(Error::new("portable archive exceeds output limit"));
        }
        self.pending = bytes;
        self.pending_at = 0;
        Ok(())
    }
    fn start(&mut self) -> Result<(), Error> {
        if !self.started {
            let mut header = Vec::new();
            reserve(&mut header, HEADER)?;
            header.extend_from_slice(MAGIC);
            header.extend_from_slice(&u32_len(self.config.block_size)?.to_le_bytes());
            header.extend_from_slice(&0u32.to_le_bytes());
            self.queue(header)?;
            self.started = true;
        }
        Ok(())
    }
    fn submit(&mut self) -> Result<(), Error> {
        if self.input.is_empty() {
            return Ok(());
        }
        let (route, payload) = choose_route(&self.input, self.config)?;
        let mut frame = Vec::new();
        append_frame(&mut frame, route, &self.input, &payload)?;
        self.whole.update(&self.input);
        self.input.clear();
        self.queue(frame)
    }
    fn finish_frame(&mut self) -> Result<(), Error> {
        let mut frame = Vec::new();
        reserve(&mut frame, FRAME)?;
        frame.push(END);
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&self.whole.clone().finalize());
        self.queue(frame)?;
        self.finished = true;
        Ok(())
    }
    fn drain(&mut self, output: &mut [u8]) -> usize {
        let n = output
            .len()
            .min(self.pending.len().saturating_sub(self.pending_at));
        output[..n].copy_from_slice(&self.pending[self.pending_at..self.pending_at + n]);
        self.pending_at += n;
        self.emitted += n;
        if self.pending_at == self.pending.len() {
            self.pending.clear();
            self.pending_at = 0;
        }
        n
    }
    fn state(&self) -> StreamState {
        if self.finished && self.pending.is_empty() {
            StreamState::Finished
        } else if !self.pending.is_empty() {
            StreamState::NeedsOutput
        } else {
            StreamState::NeedsInput
        }
    }
    pub fn process(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error> {
        self.drive(input, output, false)
    }
    pub fn finish(&mut self, output: &mut [u8]) -> Result<Progress, Error> {
        self.drive(&[], output, true)
    }
    fn drive(&mut self, input: &[u8], output: &mut [u8], finish: bool) -> Result<Progress, Error> {
        if self.finished {
            return Ok(Progress {
                consumed: 0,
                produced: self.drain(output),
                state: self.state(),
            });
        }
        let mut produced = self.drain(output);
        if !self.pending.is_empty() {
            return Ok(Progress {
                consumed: 0,
                produced,
                state: self.state(),
            });
        }
        self.start()?;
        produced += self.drain(&mut output[produced..]);
        if !self.pending.is_empty() {
            return Ok(Progress {
                consumed: 0,
                produced,
                state: self.state(),
            });
        }
        let consumed = input.len().min(self.config.block_size - self.input.len());
        self.input.extend_from_slice(&input[..consumed]);
        if self.input.len() == self.config.block_size {
            self.submit()?;
            produced += self.drain(&mut output[produced..]);
        }
        if finish && consumed == 0 && self.pending.is_empty() {
            self.finishing = true;
            if !self.input.is_empty() {
                self.submit()?;
            } else {
                self.finish_frame()?;
            }
            produced += self.drain(&mut output[produced..]);
        }
        if self.finishing && self.pending.is_empty() && self.input.is_empty() && !self.finished {
            self.finish_frame()?;
            produced += self.drain(&mut output[produced..]);
        }
        Ok(Progress {
            consumed,
            produced,
            state: self.state(),
        })
    }
}

fn decode_route(route: u8, payload: &[u8], expected: usize) -> Result<Vec<u8>, Error> {
    match route {
        0 => {
            if payload.len() != expected {
                Err(Error::new("portable raw length mismatch"))
            } else {
                Ok(payload.to_vec())
            }
        }
        1 => unrle(payload, expected),
        2 => decode_stream(payload, expected),
        _ => Err(Error::new("CIXG1 route is outside the portable subset")),
    }
}
fn decode_stream(blob: &[u8], expected: usize) -> Result<Vec<u8>, Error> {
    if blob.first() != Some(&1) || blob.len() < 10 {
        return Err(Error::new(
            "portable route-2 requires exactly one substream",
        ));
    }
    let coder = blob[1];
    let n = u32::from_le_bytes(blob[2..6].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(blob[6..10].try_into().unwrap()) as usize;
    let end = add(10, size)?;
    if n != expected || end != blob.len() {
        return Err(Error::new("invalid portable substream descriptor"));
    }
    let payload = &blob[10..];
    match coder {
        0 if payload.len() == n => Ok(payload.to_vec()),
        1 => unrle(payload, n),
        2 | 5 => rank::decode_composition_tiles(payload, n).map_err(Into::into),
        4 => adaptive::decode(payload, n).map_err(Into::into),
        _ => Err(Error::new(
            "CIX substream coder is outside the portable subset",
        )),
    }
}

pub struct StreamDecoder {
    config: DecodeConfig,
    input: Vec<u8>,
    pending: Vec<u8>,
    pending_at: usize,
    whole: Sha256,
    emitted: usize,
    received: usize,
    block: Option<usize>,
    finished: bool,
}
impl StreamDecoder {
    pub fn new(config: DecodeConfig) -> Result<Self, Error> {
        Ok(Self {
            config: config.validate()?,
            input: Vec::new(),
            pending: Vec::new(),
            pending_at: 0,
            whole: Sha256::new(),
            emitted: 0,
            received: 0,
            block: None,
            finished: false,
        })
    }
    fn drain(&mut self, output: &mut [u8]) -> usize {
        let n = output
            .len()
            .min(self.pending.len().saturating_sub(self.pending_at));
        output[..n].copy_from_slice(&self.pending[self.pending_at..self.pending_at + n]);
        self.pending_at += n;
        self.emitted += n;
        if self.pending_at == self.pending.len() {
            self.pending.clear();
            self.pending_at = 0;
        }
        n
    }
    fn state(&self) -> StreamState {
        if self.finished && self.pending.is_empty() {
            StreamState::Finished
        } else if !self.pending.is_empty() {
            StreamState::NeedsOutput
        } else {
            StreamState::NeedsInput
        }
    }
    fn parse(&mut self) -> Result<bool, Error> {
        if self.block.is_none() {
            if self.input.len() < HEADER {
                return Ok(false);
            }
            if &self.input[..5] != MAGIC
                || u32::from_le_bytes(self.input[9..13].try_into().unwrap()) != 0
            {
                return Err(Error::new(
                    "portable decoder requires CIXG1 with zero history",
                ));
            }
            let block = u32::from_le_bytes(self.input[5..9].try_into().unwrap()) as usize;
            if block == 0 || block > self.config.max_block_size {
                return Err(Error::new("CIXG1 block exceeds portable limit"));
            }
            self.input.drain(..HEADER);
            self.block = Some(block);
            return Ok(true);
        }
        if self.input.len() < FRAME {
            return Ok(false);
        }
        let route = self.input[0];
        let n = u32::from_le_bytes(self.input[1..5].try_into().unwrap()) as usize;
        let s = u32::from_le_bytes(self.input[5..9].try_into().unwrap()) as usize;
        let sum: [u8; 32] = self.input[9..41].try_into().unwrap();
        if route == END {
            if n != 0 || s != 0 || sum != <[u8; 32]>::from(self.whole.clone().finalize()) {
                return Err(Error::new("invalid portable CIXG1 terminator"));
            }
            self.input.drain(..FRAME);
            self.finished = true;
            return Ok(true);
        }
        if n == 0 || n > self.block.unwrap() {
            return Err(Error::new(
                "portable frame source length exceeds header block",
            ));
        }
        let maximum_payload = self
            .block
            .unwrap()
            .checked_mul(4)
            .and_then(|v| v.checked_add(1024))
            .ok_or_else(|| Error::new("portable frame limit overflow"))?;
        if s > maximum_payload {
            return Err(Error::new("portable frame payload exceeds subset limit"));
        }
        let frame_len = add(FRAME, s)?;
        if self.input.len() < frame_len {
            return Ok(false);
        }
        let next = add(self.emitted, n)?;
        if next > self.config.output_limit {
            return Err(Error::new("portable decoded output exceeds limit"));
        }
        let payload = self.input[FRAME..frame_len].to_vec();
        self.input.drain(..frame_len);
        let decoded = decode_route(route, &payload, n)?;
        if <[u8; 32]>::from(Sha256::digest(&decoded)) != sum {
            return Err(Error::new("portable CIXG1 frame checksum mismatch"));
        }
        self.whole.update(&decoded);
        self.pending = decoded;
        self.pending_at = 0;
        Ok(true)
    }
    pub fn process(&mut self, input: &[u8], output: &mut [u8]) -> Result<Progress, Error> {
        if self.finished && !input.is_empty() {
            return Err(Error::new("trailing bytes after portable CIXG1 terminator"));
        }
        let mut produced = self.drain(output);
        if !self.pending.is_empty() {
            return Ok(Progress {
                consumed: 0,
                produced,
                state: self.state(),
            });
        }
        let mut consumed = 0usize;
        loop {
            if self.parse()? {
                if self.finished && consumed < input.len() {
                    return Err(Error::new("trailing bytes after portable CIXG1 terminator"));
                }
                if !self.pending.is_empty() {
                    break;
                }
                continue;
            }
            let maximum_staged = match self.block {
                None => HEADER,
                Some(block) => block
                    .checked_mul(4)
                    .and_then(|bytes| bytes.checked_add(FRAME + 1024))
                    .ok_or_else(|| Error::new("portable frame staging limit overflow"))?,
            };
            let archive_left = self
                .config
                .archive_limit
                .checked_sub(self.received)
                .ok_or_else(|| Error::new("portable archive exceeds limit"))?;
            let staging_left = maximum_staged
                .checked_sub(self.input.len())
                .ok_or_else(|| Error::new("portable frame staging exceeds limit"))?;
            let take = input
                .len()
                .saturating_sub(consumed)
                .min(archive_left)
                .min(staging_left);
            if take == 0 {
                break;
            }
            reserve(&mut self.input, take)?;
            self.input
                .extend_from_slice(&input[consumed..consumed + take]);
            consumed = add(consumed, take)?;
            self.received = add(self.received, take)?;
        }
        if consumed < input.len() && self.received == self.config.archive_limit {
            return Err(Error::new("portable archive exceeds limit"));
        }
        if self.finished && !self.input.is_empty() {
            return Err(Error::new("trailing bytes after portable CIXG1 terminator"));
        }
        produced += self.drain(&mut output[produced..]);
        Ok(Progress {
            consumed,
            produced,
            state: self.state(),
        })
    }
}

pub fn encode(input: &[u8], config: EncodeConfig) -> Result<Vec<u8>, Error> {
    let mut encoder = StreamEncoder::new(config)?;
    let mut out = Vec::new();
    reserve(&mut out, HEADER + FRAME)?;
    let mut at = 0;
    let mut scratch = Vec::new();
    reserve(&mut scratch, MAX_BLOCK * 4 + 1024)?;
    scratch.resize(MAX_BLOCK * 4 + 1024, 0);
    while at < input.len() {
        let p = encoder.process(&input[at..], &mut scratch)?;
        at += p.consumed;
        append(&mut out, &scratch[..p.produced])?;
        if p.consumed == 0 && p.produced == 0 {
            return Err(Error::new("portable encoder made no progress"));
        }
    }
    loop {
        let p = encoder.finish(&mut scratch)?;
        append(&mut out, &scratch[..p.produced])?;
        if p.state == StreamState::Finished {
            break;
        }
        if p.produced == 0 {
            return Err(Error::new("portable encoder finish made no progress"));
        }
    }
    if out.len() > config.output_limit {
        return Err(Error::new("portable archive exceeds output limit"));
    }
    Ok(out)
}
pub fn decode(input: &[u8], config: DecodeConfig) -> Result<Vec<u8>, Error> {
    let mut decoder = StreamDecoder::new(config)?;
    let mut out = Vec::new();
    let mut scratch = Vec::new();
    reserve(&mut scratch, MAX_BLOCK)?;
    scratch.resize(MAX_BLOCK, 0);
    let mut at = 0;
    while at < input.len() {
        let p = decoder.process(&input[at..], &mut scratch)?;
        at += p.consumed;
        append(&mut out, &scratch[..p.produced])?;
        if p.consumed == 0 && p.produced == 0 {
            return Err(Error::new("portable decoder made no progress"));
        }
    }
    loop {
        let p = decoder.process(&[], &mut scratch)?;
        append(&mut out, &scratch[..p.produced])?;
        if p.state == StreamState::Finished {
            break;
        }
        if p.produced == 0 {
            return Err(Error::new("truncated portable CIXG1 archive"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_subset_round_trips_in_small_output_steps() {
        let input: Vec<u8> = (0..(MAX_BLOCK + 41)).map(|n| (n % 11) as u8).collect();
        let mut encoder = StreamEncoder::new(EncodeConfig {
            output_limit: input.len() * 4 + 128,
            ..EncodeConfig::default()
        })
        .unwrap();
        let mut archive = Vec::new();
        let mut at = 0;
        let mut output = [0u8; 31];
        while at < input.len() {
            let p = encoder.process(&input[at..], &mut output).unwrap();
            at += p.consumed;
            archive.extend_from_slice(&output[..p.produced]);
            assert!(p.consumed != 0 || p.produced != 0);
        }
        loop {
            let p = encoder.finish(&mut output).unwrap();
            archive.extend_from_slice(&output[..p.produced]);
            if p.state == StreamState::Finished {
                break;
            }
            assert_ne!(p.produced, 0);
        }
        assert_eq!(&archive[..5], MAGIC);
        assert!(matches!(archive[HEADER], 1 | 2));
        let mut decoder = StreamDecoder::new(DecodeConfig {
            output_limit: input.len(),
            archive_limit: archive.len(),
            ..DecodeConfig::default()
        })
        .unwrap();
        let mut restored = Vec::new();
        let mut input_at = 0;
        while input_at < archive.len() {
            let end = (input_at + 17).min(archive.len());
            let p = decoder
                .process(&archive[input_at..end], &mut output)
                .unwrap();
            input_at += p.consumed;
            restored.extend_from_slice(&output[..p.produced]);
            if p.consumed == 0 && p.produced == 0 {
                continue;
            }
        }
        loop {
            let p = decoder.process(&[], &mut output).unwrap();
            restored.extend_from_slice(&output[..p.produced]);
            if p.state == StreamState::Finished {
                break;
            }
            assert_ne!(p.produced, 0);
        }
        assert_eq!(restored, input);
    }

    #[test]
    fn decoder_rejects_native_routes_outside_subset() {
        let mut archive = encode(b"portable", EncodeConfig::default()).unwrap();
        archive[HEADER] = 8; // PPM route is valid native CIXG1 but intentionally unsupported here.
        assert!(decode(&archive, DecodeConfig::default()).is_err());
    }
}
