use flate2::{write::DeflateEncoder, Compression, Decompress, FlushDecompress, Status};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    io::{self, Read, Write},
    thread,
    time::{Duration, Instant},
};

#[path = "adaptive.rs"]
pub mod adaptive;
#[path = "bitplane.rs"]
mod bitplane;
#[path = "bounded_reader.rs"]
mod bounded_reader;
#[path = "bsc_ffi.rs"]
pub mod bsc_ffi;
#[path = "bwt_legacy.rs"]
mod bwt_legacy;
#[path = "chain.rs"]
mod chain;
#[path = "combinatorics.rs"]
pub mod combinatorics;
#[path = "expertmix.rs"]
mod expertmix;
#[path = "external.rs"]
pub mod external;
#[path = "fast_lz.rs"]
mod fast_lz;
#[path = "fixedmix.rs"]
pub mod fixedmix;
#[path = "gstream.rs"]
mod gstream;
#[path = "histogram.rs"]
mod histogram;
#[path = "huffman.rs"]
pub mod huffman;
#[path = "legacy_lz.rs"]
pub mod legacy_lz;
#[path = "limits.rs"]
pub mod limits;
#[path = "numeric.rs"]
mod numeric;
#[path = "parallel.rs"]
pub mod parallel;
#[path = "ppm.rs"]
pub mod ppm;
#[path = "rank.rs"]
pub mod rank;
#[path = "record_periodic.rs"]
mod record_periodic;
#[path = "resources.rs"]
pub mod resources;
#[path = "selection.rs"]
pub(crate) mod selection;
#[path = "sparse_runs.rs"]
mod sparse_runs;
#[path = "stateful.rs"]
mod stateful;
#[path = "stdout_writer.rs"]
pub mod stdout_writer;
#[path = "streaming_backend.rs"]
pub(crate) mod streaming_backend;
#[path = "transform.rs"]
mod transform;
#[path = "wavelet.rs"]
mod wavelet;
#[path = "zpaq_context.rs"]
pub mod zpaq_context;

pub(crate) fn put_frame<W: Write>(
    w: &mut W,
    r: u8,
    n: u32,
    s: u32,
    h: &[u8; 32],
) -> io::Result<()> {
    w.write_all(&[r])?;
    w.write_all(&n.to_le_bytes())?;
    w.write_all(&s.to_le_bytes())?;
    w.write_all(h)
}

const MAGIC: &[u8; 5] = b"CIXG1";
pub(crate) const MAGIC_V2: &[u8; 5] = b"CIXG2";
const HEADER: usize = 13;
const END: u8 = 255;
pub(crate) const MAX_BLOCK: u32 = 65536;
const EXPERT_CONTEXT_MEMORY_ESTIMATE: usize = 8192;
pub(crate) fn check_interrupted() -> Result<(), String> {
    crate::core::engine::limits::check()
}
fn read_poll<R: Read>(
    src: &mut R,
    buffer: &mut [u8],
    fd: i32,
    timeout: Option<std::time::Duration>,
) -> Result<Option<usize>, String> {
    loop {
        check_interrupted()?;
        if !input_ready(fd, timeout)? {
            return Ok(None);
        }
        if let Some(read) = read_once(src, buffer)? {
            return Ok(Some(read));
        }
    }
}

fn read_once<R: Read>(src: &mut R, buffer: &mut [u8]) -> Result<Option<usize>, String> {
    match src.read(buffer) {
        Ok(read) => Ok(Some(read)),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(None),
        Err(_error) if limits::cli_interrupted() => Err("interrupted".into()),
        Err(error) => Err(ioerr(error)),
    }
}

#[cfg(unix)]
fn input_ready(fd: i32, timeout: Option<Duration>) -> Result<bool, String> {
    if fd < 0 {
        return Ok(true);
    }
    let slice = Duration::from_millis(250);
    let wait = timeout.map(|value| value.min(slice)).unwrap_or(slice);
    wait_input(fd, wait).map_err(|error| {
        if limits::cli_interrupted() {
            "interrupted".into()
        } else {
            ioerr(error)
        }
    })
}

#[cfg(not(unix))]
fn input_ready(_fd: i32, timeout: Option<Duration>) -> Result<bool, String> {
    if timeout.is_some() {
        // A synchronous Read can provide bounded block buffering on this
        // platform, but it cannot report an idle deadline while a redirected
        // console or pipe is blocked. Do not pretend that --flush-interval
        // remains a timed guarantee without a platform readiness primitive.
        return Err("timed pipe reads require Unix poll; use blocking stdin without --flush-interval on this platform".into());
    }
    Ok(true)
}
fn read_exact_poll<R: Read>(src: &mut R, mut buffer: &mut [u8], fd: i32) -> Result<(), String> {
    while !buffer.is_empty() {
        match read_poll(src, buffer, fd, None)? {
            Some(0) => return Err("truncated archive".into()),
            Some(n) => {
                let (_, rest) = buffer.split_at_mut(n);
                buffer = rest;
            }
            None => continue,
        }
    }
    Ok(())
}
fn read_u32<R: Read>(r: &mut R, fd: i32) -> Result<u32, String> {
    let mut b = [0; 4];
    read_exact_poll(r, &mut b, fd)?;
    Ok(u32::from_le_bytes(b))
}
#[cfg(unix)]
fn wait_input(fd: std::os::fd::RawFd, timeout: std::time::Duration) -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let milliseconds = timeout.as_millis().min(i32::MAX as u128) as i32;
    loop {
        let ready = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
        if ready >= 0 {
            return Ok(ready != 0);
        }
        let error = io::Error::last_os_error();
        if limits::cli_interrupted() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
        }
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
fn rle(b: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let mut j = i + 1;
        while j < b.len() && b[j] == b[i] {
            j += 1
        }
        o.push(b[i]);
        let mut n = (j - i) as u64;
        while n >= 128 {
            o.push((n as u8 & 127) | 128);
            n >>= 7
        }
        o.push(n as u8);
        i = j
    }
    o
}
fn unvar(b: &[u8], p: &mut usize) -> Result<usize, String> {
    let mut v = 0usize;
    for sh in (0..35).step_by(7) {
        if *p >= b.len() {
            return Err("truncated run varint".into());
        }
        let x = b[*p];
        *p += 1;
        v |= ((x & 127) as usize) << sh;
        if x < 128 {
            return Ok(v);
        }
    }
    Err("oversized run varint".into())
}
fn unrle(b: &[u8], n: usize) -> Result<Vec<u8>, String> {
    let (mut p, mut o) = (0, Vec::with_capacity(n));
    while p < b.len() {
        let c = b[p];
        p += 1;
        let k = unvar(b, &mut p)?;
        if k == 0 || o.len().checked_add(k).filter(|&x| x <= n).is_none() {
            return Err("invalid run length".into());
        }
        o.resize(o.len() + k, c)
    }
    if o.len() != n {
        return Err("run output length mismatch".into());
    }
    Ok(o)
}
fn deflate(b: &[u8], level: u8) -> io::Result<Vec<u8>> {
    let mut e = DeflateEncoder::new(Vec::new(), Compression::new(level as u32));
    e.write_all(b)?;
    e.finish()
}
fn entropy(b: &[u8]) -> f64 {
    if b.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    let mut order = Vec::new();
    for &x in b {
        let c = &mut counts[x as usize];
        if *c == 0 {
            order.push(x as usize);
        }
        *c += 1;
    }
    let n = b.len() as f64;
    n.log2()
        - order
            .into_iter()
            .map(|i| counts[i] as f64 * (counts[i] as f64).log2())
            .sum::<f64>()
            / n
}

fn stream_payload(data: &[u8], level: u8, backend: &str) -> Result<(u8, Vec<u8>), String> {
    if backend == "huffman" {
        // Huffman has its own complete, versioned substream framing. Raw is
        // still the exact fallback, and an explicit override never quietly
        // evaluates or reports an unrelated coder.
        let encoded = huffman::encode(data).map_err(|error| error.to_string())?;
        return [(0, data.to_vec()), (7, encoded)]
            .into_iter()
            .min_by_key(|(_, payload)| payload.len())
            .ok_or_else(|| "no CIX substream candidates".into());
    }
    if let Some((order, bits)) = match backend {
        "context-range-o1-b4" => Some((1, 4)),
        "context-range-o1-b8" => Some((1, 8)),
        "context-range-o2-b8" => Some((2, 8)),
        _ => None,
    } {
        // An explicit backend override constrains the coder dimension. Keep
        // raw as the lossless fallback, but do not silently compare unrelated
        // backends and report their result as this backend.
        let context = adaptive::encode_general(data, order, bits, &[], 1usize << bits)?;
        let candidates = [(0, data.to_vec()), (6, context)];
        return candidates
            .into_iter()
            .min_by_key(|(_, payload)| payload.len())
            .ok_or_else(|| "no CIX substream candidates".into());
    }
    let mut candidates: Vec<(u8, Vec<u8>)> = vec![(0, data.to_vec()), (1, rle(data))];
    if matches!(backend, "cix" | "hybrid") && entropy(&data[..data.len().min(8192)]) < 7.5 {
        candidates.push((2, rank::encode_composition_tiles(data)?));
    }
    if matches!(backend, "deflate" | "hybrid") {
        let compressed = deflate(data, level).map_err(ioerr)?;
        candidates.push((3, compressed));
    }
    if backend == "range" {
        candidates.push((4, adaptive::encode(data)));
    }
    if backend == "count-range" {
        candidates.push((5, rank::encode_count_range(data)?));
    }
    // Python min() keeps the first equal-sized candidate; preserve its order.
    candidates
        .into_iter()
        .min_by_key(|(_, payload)| payload.len())
        .ok_or_else(|| "no CIX substream candidates".into())
}

fn streams_encode(parts: &[&[u8]], level: u8, backend: &str) -> Result<Vec<u8>, String> {
    if parts.is_empty() || parts.len() > 8 {
        return Err("CIX stream count must be from 1 to 8".into());
    }
    let mut out = Vec::new();
    out.push(parts.len() as u8);
    for part in parts {
        limits::check()?;
        let (coder, payload) = stream_payload(part, level, backend)?;
        out.push(coder);
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&payload);
    }
    Ok(out)
}

fn streams_decode(
    blob: &[u8],
    limit: usize,
    allow_extended_coders: bool,
) -> Result<Vec<Vec<u8>>, String> {
    if blob.is_empty() || blob[0] == 0 || blob[0] > 8 {
        return Err("invalid CIX substream count".into());
    }
    let mut pos = 1usize;
    let mut total = 0usize;
    let mut parts = Vec::with_capacity(blob[0] as usize);
    for _ in 0..blob[0] {
        if pos + 9 > blob.len() {
            return Err("truncated CIX substream descriptor".into());
        }
        let coder = blob[pos];
        let n = u32::from_le_bytes(blob[pos + 1..pos + 5].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(blob[pos + 5..pos + 9].try_into().unwrap()) as usize;
        pos += 9;
        total = total
            .checked_add(n)
            .ok_or("CIX substream length overflow")?;
        let end = pos
            .checked_add(size)
            .ok_or("CIX substream payload overflow")?;
        if total > limit || end > blob.len() {
            return Err("CIX substream resource/truncation limit".into());
        }
        let decoded = decode_substream(coder, &blob[pos..end], n, size, allow_extended_coders)?;
        parts.push(decoded);
        pos = end;
    }
    if pos != blob.len() {
        return Err("trailing CIX substream bytes".into());
    }
    Ok(parts)
}

fn decode_substream(
    coder: u8,
    payload: &[u8],
    decoded_len: usize,
    encoded_len: usize,
    allow_extended_coders: bool,
) -> Result<Vec<u8>, String> {
    match coder {
        0 => decode_raw_substream(payload, decoded_len, encoded_len),
        1 => unrle(payload, decoded_len),
        2 | 5 => rank::decode_composition_tiles(payload, decoded_len),
        3 => inflate(payload, decoded_len),
        4 => adaptive::decode(payload, decoded_len),
        6 | 7 => decode_versioned_substream(coder, payload, decoded_len, allow_extended_coders),
        _ => Err(format!("unsupported CIX substream coder {coder}")),
    }
}

fn decode_raw_substream(
    payload: &[u8],
    decoded_len: usize,
    encoded_len: usize,
) -> Result<Vec<u8>, String> {
    if encoded_len != decoded_len {
        return Err("raw substream size mismatch".into());
    }
    Ok(payload.to_vec())
}

fn decode_versioned_substream(
    coder: u8,
    payload: &[u8],
    decoded_len: usize,
    allow_extended_coders: bool,
) -> Result<Vec<u8>, String> {
    if !allow_extended_coders {
        return Err(match coder {
            6 => "CIXG1 cannot contain versioned context-range substreams",
            7 => "CIXG1 cannot contain versioned Huffman substreams",
            _ => "CIXG1 cannot contain versioned substreams",
        }
        .into());
    }
    match coder {
        6 => decode_context_range_substream(payload, decoded_len),
        7 => decode_huffman_substream(payload, decoded_len),
        _ => Err("invalid versioned CIX substream coder".into()),
    }
}

fn decode_context_range_substream(payload: &[u8], decoded_len: usize) -> Result<Vec<u8>, String> {
    if payload.len() < 4 || payload[0] != 1 {
        return Err("invalid context-range substream header".into());
    }
    let order = payload[1] as usize;
    let bits = payload[2] as usize;
    if !(1..=16).contains(&order) || !(1..=8).contains(&bits) {
        return Err("context-range parameters exceed version-1 bounds".into());
    }
    adaptive::decode_general(payload, decoded_len, &[], 1usize << bits)
}

fn decode_huffman_substream(payload: &[u8], decoded_len: usize) -> Result<Vec<u8>, String> {
    if payload.len() < 11
        || u64::from_le_bytes(payload[3..11].try_into().unwrap()) != decoded_len as u64
    {
        return Err("Huffman substream decoded length mismatch".into());
    }
    let decoded = huffman::decode(payload).map_err(|error| error.to_string())?;
    if decoded.len() != decoded_len {
        return Err("Huffman substream decoded length mismatch".into());
    }
    Ok(decoded)
}

fn stream_encode(data: &[u8], level: u8, backend: &str) -> Result<Vec<u8>, String> {
    streams_encode(&[data], level, backend)
}

fn stream_decode(
    blob: &[u8],
    expected: usize,
    allow_extended_coders: bool,
) -> Result<Vec<u8>, String> {
    let mut parts = streams_decode(blob, expected, allow_extended_coders)?;
    if parts.len() != 1 || parts[0].len() != expected {
        return Err("expected exactly one CIX byte substream".into());
    }
    Ok(parts.remove(0))
}

fn predictor_residual(data: &[u8], inverse: bool) -> Vec<u8> {
    let mut values = vec![0u8; 65536];
    let mut valid = vec![false; 65536];
    let mut scores = [0u32; 3];
    let mut out = Vec::with_capacity(data.len());
    for (i, &value) in data.iter().enumerate() {
        let past = if inverse { out.as_slice() } else { data };
        let context = predictor_context(past, i);
        let predictions = predictor_predictions(past, i, context, &values, &valid);
        let winner = predictor_winner(&scores);
        let actual = if inverse {
            value ^ predictions[winner]
        } else {
            value
        };
        out.push(if inverse {
            actual
        } else {
            value ^ predictions[winner]
        });
        predictor_update(
            &mut values,
            &mut valid,
            &mut scores,
            &predictions,
            context,
            i,
            actual,
        );
    }
    out
}

fn predictor_context(past: &[u8], index: usize) -> usize {
    if index >= 2 {
        past[index - 2] as usize * 256 + past[index - 1] as usize
    } else {
        0
    }
}

fn predictor_predictions(
    past: &[u8],
    index: usize,
    context: usize,
    values: &[u8],
    valid: &[bool],
) -> [u8; 3] {
    [
        if index > 0 { past[index - 1] } else { 0 },
        if index >= 4 { past[index - 4] } else { 0 },
        if index >= 2 && valid[context] {
            values[context]
        } else {
            0
        },
    ]
}

fn predictor_winner(scores: &[u32; 3]) -> usize {
    let mut winner = 0;
    for index in 1..scores.len() {
        if scores[index] > scores[winner] {
            winner = index;
        }
    }
    winner
}

fn predictor_update(
    values: &mut [u8],
    valid: &mut [bool],
    scores: &mut [u32; 3],
    predictions: &[u8; 3],
    context: usize,
    index: usize,
    actual: u8,
) {
    for score_index in 0..scores.len() {
        scores[score_index] += u32::from(predictions[score_index] == actual);
    }
    if index >= 2 {
        values[context] = actual;
        valid[context] = true;
    }
    if (index + 1).is_multiple_of(256) {
        for score in scores {
            *score /= 2;
        }
    }
}

fn read_word(bytes: &[u8], endian_le: bool) -> u64 {
    let mut word = [0u8; 8];
    if endian_le {
        word[..bytes.len()].copy_from_slice(bytes);
        u64::from_le_bytes(word)
    } else {
        word[8 - bytes.len()..].copy_from_slice(bytes);
        u64::from_be_bytes(word)
    }
}

fn write_word(word: u64, width: usize, endian_le: bool, output: &mut Vec<u8>) {
    let bytes = if endian_le {
        word.to_le_bytes()
    } else {
        word.to_be_bytes()
    };
    if endian_le {
        output.extend_from_slice(&bytes[..width]);
    } else {
        output.extend_from_slice(&bytes[8 - width..]);
    }
}

fn stride_spec(id: u8) -> Result<(usize, bool, bool), String> {
    match id {
        4 => Ok((1, true, false)),
        6 => Ok((4, true, true)),
        7 => Ok((8, true, true)),
        9 => Ok((4, false, true)),
        17 => Ok((4, true, false)),
        18 => Ok((8, true, false)),
        _ => Err(format!("unsupported stride transform {id}")),
    }
}

fn apply_stride(data: &[u8], id: u8) -> Result<Vec<u8>, String> {
    let (width, little, delta) = stride_spec(id)?;
    if width == 1 {
        let mut out = data.to_vec();
        for i in (1..out.len()).rev() {
            out[i] = out[i].wrapping_sub(out[i - 1]);
        }
        return Ok(out);
    }
    let usable = data.len() / width * width;
    let words: Vec<u64> = data[..usable]
        .chunks_exact(width)
        .map(|chunk| read_word(chunk, little))
        .collect();
    let mask = if width == 8 {
        u64::MAX
    } else {
        (1u64 << (width * 8)) - 1
    };
    let mut transformed = Vec::with_capacity(data.len());
    let mut previous = 0u64;
    for (i, &word) in words.iter().enumerate() {
        let value = if i == 0 {
            word
        } else if delta {
            word.wrapping_sub(previous) & mask
        } else {
            word ^ previous
        };
        write_word(value, width, little, &mut transformed);
        previous = word;
    }
    transformed.extend_from_slice(&data[usable..]);
    let usable = transformed.len() / width * width;
    let mut shuffled = Vec::with_capacity(transformed.len());
    for lane in 0..width {
        for index in (lane..usable).step_by(width) {
            shuffled.push(transformed[index]);
        }
    }
    shuffled.extend_from_slice(&transformed[usable..]);
    Ok(shuffled)
}

fn invert_stride(data: &[u8], id: u8) -> Result<Vec<u8>, String> {
    let (width, little, delta) = stride_spec(id)?;
    if width == 1 {
        let mut out = data.to_vec();
        for i in 1..out.len() {
            out[i] = out[i].wrapping_add(out[i - 1]);
        }
        return Ok(out);
    }
    let usable = data.len() / width * width;
    let words = usable / width;
    let mut unshuffled = vec![0u8; usable];
    for lane in 0..width {
        for word in 0..words {
            unshuffled[lane + word * width] = data[lane * words + word];
        }
    }
    unshuffled.extend_from_slice(&data[usable..]);
    let raw_usable = unshuffled.len() / width * width;
    let encoded_words: Vec<u64> = unshuffled[..raw_usable]
        .chunks_exact(width)
        .map(|chunk| read_word(chunk, little))
        .collect();
    let mask = if width == 8 {
        u64::MAX
    } else {
        (1u64 << (width * 8)) - 1
    };
    let mut restored = Vec::with_capacity(data.len());
    let mut previous = 0u64;
    for (i, &encoded) in encoded_words.iter().enumerate() {
        let value = if i == 0 {
            encoded
        } else if delta {
            previous.wrapping_add(encoded) & mask
        } else {
            previous ^ encoded
        };
        write_word(value, width, little, &mut restored);
        previous = value;
    }
    restored.extend_from_slice(&unshuffled[raw_usable..]);
    Ok(restored)
}

fn select_stride(sample: &[u8]) -> Result<u8, String> {
    let mut best = None;
    for id in [4u8, 6, 7, 9, 17, 18] {
        let transformed = apply_stride(sample, id)?;
        let score = entropy(&transformed);
        if best.is_none_or(|(_, old)| score < old) {
            best = Some((id, score));
        }
    }
    best.map(|(id, _)| id)
        .ok_or("empty stride candidate set".into())
}

fn phrase_encode(data: &[u8], level: u8, backend: &str) -> Result<Vec<u8>, String> {
    use std::collections::HashMap;
    let mut counts: HashMap<[u8; 2], (usize, usize)> = HashMap::new();
    for i in 0..data.len().saturating_sub(1) {
        if i & 255 == 0 {
            limits::check()?;
        }
        let key = [data[i], data[i + 1]];
        let entry = counts.entry(key).or_insert((0, i));
        entry.0 += 1;
    }
    let mut dictionary: Vec<([u8; 2], usize, usize)> = counts
        .into_iter()
        .map(|(key, (count, first))| (key, count, first))
        .collect();
    dictionary.sort_by_key(|(_, count, first)| (std::cmp::Reverse(*count), *first));
    let dictionary: Vec<[u8; 2]> = dictionary
        .into_iter()
        .take(32)
        .filter_map(|(key, count, _)| (count >= 8).then_some(key))
        .collect();
    let mut lookup = HashMap::new();
    for (index, &key) in dictionary.iter().enumerate() {
        lookup.insert(key, index as u8);
    }
    let mut tags = Vec::new();
    let mut literals = Vec::new();
    let mut indices = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if i & 255 == 0 {
            limits::check()?;
        }
        if i + 1 < data.len() {
            if let Some(&index) = lookup.get(&[data[i], data[i + 1]]) {
                tags.push(1);
                indices.push(index);
                i += 2;
                continue;
            }
        }
        tags.push(0);
        literals.push(data[i]);
        i += 1;
    }
    let owned = [tags, literals, indices];
    let refs: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
    let streams = streams_encode(&refs, level, backend)?;
    let mut out = Vec::with_capacity(1 + 2 * dictionary.len() + streams.len());
    out.push(dictionary.len() as u8);
    for key in dictionary {
        out.extend_from_slice(&key);
    }
    out.extend_from_slice(&streams);
    Ok(out)
}

fn phrase_decode(blob: &[u8], n: usize, allow_extended_coders: bool) -> Result<Vec<u8>, String> {
    if blob.is_empty() || blob[0] > 32 {
        return Err("invalid phrase dictionary size".into());
    }
    let count = blob[0] as usize;
    let start = 1 + count * 2;
    if start > blob.len() {
        return Err("truncated phrase dictionary".into());
    }
    let (dictionary_bytes, []) = blob[1..start].as_chunks::<2>() else {
        return Err("invalid phrase dictionary size".into());
    };
    let dictionary: Vec<[u8; 2]> = dictionary_bytes.to_vec();
    let parts = streams_decode(&blob[start..], n.saturating_mul(2), allow_extended_coders)?;
    if parts.len() != 3 {
        return Err("phrase stream count mismatch".into());
    }
    let (tags, literals, indices) = (&parts[0], &parts[1], &parts[2]);
    let (mut a, mut b) = (0usize, 0usize);
    let mut out = Vec::with_capacity(n);
    for &tag in tags {
        match tag {
            0 if a < literals.len() => {
                out.push(literals[a]);
                a += 1;
            }
            1 if b < indices.len() && (indices[b] as usize) < dictionary.len() => {
                out.extend_from_slice(&dictionary[indices[b] as usize]);
                b += 1;
            }
            _ => return Err("invalid phrase tag/index".into()),
        }
        if out.len() > n {
            return Err("phrase output overflow".into());
        }
    }
    if out.len() != n || a != literals.len() || b != indices.len() {
        return Err("phrase stream lengths mismatch".into());
    }
    Ok(out)
}

fn stride_encode(data: &[u8], level: u8, backend: &str) -> Result<Vec<u8>, String> {
    let id = select_stride(&data[..data.len().min(4096)])?;
    let transformed = apply_stride(data, id)?;
    let (width, _, _) = stride_spec(id)?;
    let plane = transformed.len() / width;
    let mut refs: Vec<&[u8]> = Vec::with_capacity(width);
    for lane in 0..width - 1 {
        refs.push(&transformed[lane * plane..(lane + 1) * plane]);
    }
    refs.push(&transformed[(width - 1) * plane..]);
    let streams = streams_encode(&refs, level, backend)?;
    let mut out = Vec::with_capacity(1 + streams.len());
    out.push(id);
    out.extend_from_slice(&streams);
    Ok(out)
}

fn join_streams(blob: &[u8], limit: usize, allow_extended_coders: bool) -> Result<Vec<u8>, String> {
    let parts = streams_decode(blob, limit, allow_extended_coders)?;
    let total = parts.iter().map(Vec::len).sum();
    if total > limit {
        return Err("decoded CIX stream length exceeds limit".into());
    }
    let mut out = Vec::with_capacity(total);
    for part in parts {
        out.extend_from_slice(&part);
    }
    Ok(out)
}

fn encode_route(
    route: u8,
    data: &[u8],
    level: u8,
    backend: &str,
    parameter: Option<u8>,
) -> Result<Vec<u8>, String> {
    match route {
        0 => Ok(data.to_vec()),
        1 => Ok(rle(data)),
        2 => stream_encode(data, level, backend),
        3 => lz_encode(data, level, backend),
        4 => stream_encode(&predictor_residual(data, false), level, backend),
        5 => bwt_encode(data, level, backend),
        6 => stride_encode(data, level, backend),
        7 => phrase_encode(data, level, backend),
        8 => ppm::encode(data, usize::from(parameter.unwrap_or(3))),
        9 => fixedmix::encode_with_history_eta(data, &[], 6),
        10 => stream_encode(data, level, "deflate"),
        _ => Err(format!("unsupported native route ID {route}")),
    }
}

pub(crate) fn selector_effort_name(level: u8) -> &'static str {
    match level {
        0..=2 => "fast",
        3..=8 => "balanced",
        _ => "maximum",
    }
}

/// Names shown to users describe the compression objective. The selector's
/// historical profile names remain internal implementation identifiers.
fn effort_display_name(level: u8) -> &'static str {
    match level {
        0..=2 => "fast",
        3..=8 => "default",
        _ => "best",
    }
}

#[derive(Default)]
struct AutoSelectionStats {
    blocks: u64,
    profile_seconds: f64,
    search_seconds: f64,
    candidate_seconds: f64,
    candidate_attempts: u64,
    memory_skips: u64,
    budget_skips: u64,
    selected_source_bytes: [u64; 11],
    explanations: Vec<String>,
}

fn route_minimum_memory(route: u8, length: usize) -> usize {
    // Conservative maximum allocation admission from the bounded block's
    // table insertion opportunities. These include allocator/table slack;
    // executable and shared-library mappings are measured separately by RSS.
    match route {
        0 => 0,
        1 => length.saturating_mul(3).saturating_add(4096),
        9 => length.saturating_mul(4096).saturating_add(8 << 20),
        8 => length.saturating_mul(2048).saturating_add(8 << 20),
        3 => length.saturating_mul(128).saturating_add(16 << 20),
        _ => length.saturating_mul(128).saturating_add(1 << 20),
    }
}

fn candidate_memory(route: u8, length: usize, level: u8, backend: &str) -> usize {
    if route == 3 && level <= 2 && backend == "deflate" {
        // Fixed 256 KiB index, bounded raw/RLE/deflate substreams and zlib
        // workspace. Keep expert coders on their conservative old bound.
        length.saturating_mul(24).saturating_add(1 << 20)
    } else {
        route_minimum_memory(route, length)
    }
}

pub(crate) fn default_block(level: u8) -> u32 {
    selection::effort(selector_effort_name(level))
        .expect("all numeric efforts have a selector policy")
        .block_bytes as u32
}

/// Evaluate candidates in bounded batches. Only the current winner and at
/// most `workers` transient payloads are resident, and ordered joins retain
/// the historical first-candidate-wins rule for equal sizes.
#[derive(Clone, Copy)]
struct CandidateTrialBudget {
    search_deadline: Option<Instant>,
    trial_budget: Duration,
}

struct CandidateTrialPolicy {
    level: u8,
    worker_memory: usize,
    search_budget_seconds: f64,
    limits: CandidateTrialBudget,
}

struct CandidateTrialContext<'a> {
    candidates: &'a [selection::Candidate],
    data: &'a [u8],
    budget: &'a resources::MemoryBudget,
    search_started: Instant,
    policy: CandidateTrialPolicy,
}

struct CandidateWinner {
    index: Option<usize>,
    route: u8,
    backend: String,
    payload: Vec<u8>,
}

#[derive(Default)]
struct CandidateTrialTotals {
    seconds: f64,
    attempts: u64,
    memory_skips: u64,
    budget_skips: u64,
}

struct CandidateTrialShared {
    next: std::sync::atomic::AtomicUsize,
    failed: AtomicBool,
    first_error: std::sync::Mutex<Option<String>>,
    winner: std::sync::Mutex<CandidateWinner>,
    records: std::sync::Mutex<Vec<Option<String>>>,
    totals: std::sync::Mutex<CandidateTrialTotals>,
}

struct CandidateSearchRequest<'a> {
    candidates: &'a [selection::Candidate],
    data: &'a [u8],
    level: u8,
    budget: &'a resources::MemoryBudget,
    workers: usize,
    search_budget_seconds: f64,
    trial_budget_seconds: f64,
    worker_memory_mib: usize,
}

struct CandidateSelectionState<'a> {
    stats: &'a mut AutoSelectionStats,
    best_route: &'a mut u8,
    best_backend: &'a mut String,
    best_payload: &'a mut Vec<u8>,
    evaluated: &'a mut Vec<String>,
}

struct CandidateRunResult<'a> {
    index: usize,
    route: u8,
    backend: &'a str,
    label: String,
    seconds: f64,
    effective_trial_budget: Duration,
    result: Result<Vec<u8>, String>,
}

fn candidate_wins(
    within_budget: bool,
    size: usize,
    incumbent_size: usize,
    index: usize,
    incumbent_index: Option<usize>,
) -> bool {
    within_budget
        && (size < incumbent_size
            || (size == incumbent_size && incumbent_index.is_some_and(|old| index < old)))
}

fn record_candidate_results(
    records: Vec<Option<String>>,
    candidates: &[selection::Candidate],
    stats: &mut AutoSelectionStats,
    evaluated: &mut Vec<String>,
) {
    stats.budget_skips += records.iter().filter(|record| record.is_none()).count() as u64;
    evaluated.extend(records.into_iter().enumerate().map(|(index, record)| {
        record.unwrap_or_else(|| {
            format!(
                "{}:{}=skipped(search-budget)",
                selection::ROUTES[candidates[index].route as usize],
                candidates[index].backend
            )
        })
    }));
}

fn candidate_label(candidate: &selection::Candidate) -> String {
    let route = candidate.route;
    match candidate.parameter {
        Some(parameter) if route == 8 => format!(
            "{}:{}:order-{parameter}",
            selection::ROUTES[route as usize],
            candidate.backend
        ),
        _ => format!(
            "{}:{}",
            selection::ROUTES[route as usize],
            candidate.backend
        ),
    }
}

fn record_candidate_skip(
    shared: &CandidateTrialShared,
    index: usize,
    record: String,
    memory: bool,
) {
    shared.records.lock().unwrap()[index] = Some(record);
    let mut totals = shared.totals.lock().unwrap();
    if memory {
        totals.memory_skips += 1;
    } else {
        totals.budget_skips += 1;
    }
}

fn fail_candidate_trial(shared: &CandidateTrialShared, error: String) {
    let mut first = shared.first_error.lock().unwrap();
    if first.is_none() {
        *first = Some(error);
    }
    shared.failed.store(true, Ordering::Relaxed);
}

fn evaluate_cixg_candidate(
    context: &CandidateTrialContext<'_>,
    shared: &CandidateTrialShared,
    index: usize,
) -> bool {
    let candidate = &context.candidates[index];
    let route = candidate.route;
    let backend = candidate.backend.as_str();
    let label = candidate_label(candidate);
    let needed = candidate_memory(route, context.data.len(), context.policy.level, backend);
    if context.policy.worker_memory < needed {
        record_candidate_skip(
            shared,
            index,
            format!("{label}=skipped(memory-per-worker)"),
            true,
        );
        return true;
    }
    let permit = match budget_acquire_for_candidate(context, shared, needed) {
        Ok(permit) => permit,
        Err(None) => {
            record_candidate_skip(
                shared,
                index,
                format!("{label}=skipped(search-budget)"),
                false,
            );
            return true;
        }
        Err(Some(error)) => {
            fail_candidate_trial(shared, error);
            return false;
        }
    };
    let remaining_search = match candidate_remaining_search(context) {
        Some(remaining) => remaining,
        None => {
            record_candidate_skip(
                shared,
                index,
                format!("{label}=skipped(search-budget)"),
                false,
            );
            return false;
        }
    };
    let started = Instant::now();
    let effective_trial_budget = context.policy.limits.trial_budget.min(remaining_search);
    let deadline_guard = crate::core::engine::limits::DeadlineGuard::new(effective_trial_budget);
    let result = encode_route(
        route,
        context.data,
        context.policy.level,
        backend,
        candidate.parameter,
    );
    drop(deadline_guard);
    let keep_working = record_candidate_result(
        context,
        shared,
        CandidateRunResult {
            index,
            route,
            backend,
            label,
            seconds: started.elapsed().as_secs_f64(),
            effective_trial_budget,
            result,
        },
    );
    drop(permit);
    keep_working
}

fn budget_acquire_for_candidate(
    context: &CandidateTrialContext<'_>,
    shared: &CandidateTrialShared,
    needed: usize,
) -> Result<resources::MemoryPermit, Option<String>> {
    context
        .budget
        .acquire(needed, context.policy.limits.search_deadline, || {
            crate::core::engine::limits::check().is_err() || shared.failed.load(Ordering::Relaxed)
        })
        .map_err(|error| match error {
            resources::MemoryError::DeadlineExceeded => None,
            error => Some(error.to_string()),
        })
}

fn candidate_remaining_search(context: &CandidateTrialContext<'_>) -> Option<Duration> {
    match context.policy.limits.search_deadline {
        Some(deadline) => deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero()),
        None => Some(context.policy.limits.trial_budget),
    }
}

fn record_candidate_result(
    context: &CandidateTrialContext<'_>,
    shared: &CandidateTrialShared,
    run: CandidateRunResult<'_>,
) -> bool {
    let CandidateRunResult {
        index,
        route,
        backend,
        label,
        seconds,
        effective_trial_budget,
        result,
    } = run;
    let exhausted_search = context
        .policy
        .limits
        .search_deadline
        .is_some_and(|deadline| Instant::now() >= deadline);
    match result {
        Err(error) if error == crate::core::engine::limits::DEADLINE_ERROR => {
            let reason = if exhausted_search {
                "search-budget"
            } else {
                "trial-budget"
            };
            shared.records.lock().unwrap()[index] =
                Some(format!("{label}=skipped({reason},{seconds:.3}s)"));
            let mut totals = shared.totals.lock().unwrap();
            totals.seconds += seconds;
            totals.attempts += 1;
            totals.budget_skips += 1;
            true
        }
        Err(error) => {
            fail_candidate_trial(shared, error);
            false
        }
        Ok(payload) => {
            let size = payload.len();
            let within_trial_budget =
                seconds <= effective_trial_budget.as_secs_f64() && !exhausted_search;
            {
                let mut winner = shared.winner.lock().unwrap();
                if candidate_wins(
                    within_trial_budget,
                    size,
                    winner.payload.len(),
                    index,
                    winner.index,
                ) {
                    *winner = CandidateWinner {
                        index: Some(index),
                        route,
                        backend: backend.to_owned(),
                        payload: payload.into_boxed_slice().into_vec(),
                    };
                }
            }
            shared.records.lock().unwrap()[index] = Some(if within_trial_budget {
                format!("{label}={size}B/{seconds:.3}s")
            } else if exhausted_search {
                format!("{label}=skipped(search-budget,{seconds:.3}s)")
            } else {
                format!("{label}=skipped(trial-budget,{seconds:.3}s)")
            });
            let mut totals = shared.totals.lock().unwrap();
            totals.seconds += seconds;
            totals.attempts += 1;
            if !within_trial_budget {
                totals.budget_skips += 1;
            }
            true
        }
    }
}

fn evaluate_cixg_candidate_worker(
    context: &CandidateTrialContext<'_>,
    shared: &CandidateTrialShared,
) {
    loop {
        if shared.failed.load(Ordering::Relaxed) {
            break;
        }
        if let Err(error) = check_interrupted() {
            fail_candidate_trial(shared, error);
            break;
        }
        if context.search_started.elapsed().as_secs_f64() >= context.policy.search_budget_seconds {
            break;
        }
        let index = shared.next.fetch_add(1, Ordering::Relaxed);
        if index >= context.candidates.len() || !evaluate_cixg_candidate(context, shared, index) {
            break;
        }
    }
}

fn consider_cixg_candidates_parallel(
    request: CandidateSearchRequest<'_>,
    selection: CandidateSelectionState<'_>,
) -> Result<(), String> {
    let CandidateSearchRequest {
        candidates,
        data,
        level,
        budget,
        workers,
        search_budget_seconds,
        trial_budget_seconds,
        worker_memory_mib,
    } = request;
    let CandidateSelectionState {
        stats,
        best_route,
        best_backend,
        best_payload,
        evaluated,
    } = selection;
    let workers = workers.max(1).min(candidates.len().max(1));
    let worker_memory = budget
        .capacity_bytes()
        .min(worker_memory_mib.saturating_mul(1024 * 1024));
    let search_started = Instant::now();
    let context = CandidateTrialContext {
        candidates,
        data,
        budget,
        search_started,
        policy: CandidateTrialPolicy {
            level,
            worker_memory,
            search_budget_seconds,
            limits: CandidateTrialBudget {
                search_deadline: search_started
                    .checked_add(Duration::from_secs_f64(search_budget_seconds)),
                trial_budget: Duration::from_secs_f64(trial_budget_seconds),
            },
        },
    };
    let shared = CandidateTrialShared {
        next: std::sync::atomic::AtomicUsize::new(0),
        failed: AtomicBool::new(false),
        first_error: std::sync::Mutex::new(None),
        winner: std::sync::Mutex::new(CandidateWinner {
            index: None,
            route: *best_route,
            backend: best_backend.clone(),
            payload: std::mem::take(best_payload),
        }),
        records: std::sync::Mutex::new(vec![None; candidates.len()]),
        totals: std::sync::Mutex::new(CandidateTrialTotals::default()),
    };
    if workers == 1 {
        evaluate_cixg_candidate_worker(&context, &shared);
    } else {
        let cancellation_context = limits::capture_context();
        thread::scope(|scope| {
            let candidate_context = &context;
            let candidate_shared = &shared;
            for _ in 0..workers {
                let worker_context = cancellation_context.clone();
                scope.spawn(move || {
                    let _scope = limits::install_context(&worker_context);
                    evaluate_cixg_candidate_worker(candidate_context, candidate_shared)
                });
            }
        });
    }
    if let Some(error) = shared.first_error.lock().unwrap().clone() {
        return Err(error);
    }
    let totals = shared.totals.into_inner().unwrap();
    stats.candidate_seconds += totals.seconds;
    stats.candidate_attempts += totals.attempts;
    stats.memory_skips += totals.memory_skips;
    stats.budget_skips += totals.budget_skips;
    record_candidate_results(
        shared.records.into_inner().unwrap(),
        candidates,
        stats,
        evaluated,
    );
    let winner = shared.winner.into_inner().unwrap();
    *best_route = winner.route;
    *best_backend = winner.backend;
    *best_payload = winner.payload;
    Ok(())
}

/// Keep an explicit route fixed while the effort policy still searches its
/// compatible nested coders. An explicit backend narrows only that dimension.
struct CixgFixedRouteRequest<'a> {
    route: u8,
    data: &'a [u8],
    backend_override: Option<&'a str>,
    format: &'a str,
    effort_name: &'a str,
    budget: &'a resources::MemoryBudget,
    workers: usize,
    stats: &'a mut AutoSelectionStats,
    explain: bool,
}

fn select_cixg_fixed_route(request: CixgFixedRouteRequest<'_>) -> Result<(u8, Vec<u8>), String> {
    let CixgFixedRouteRequest {
        route,
        data,
        backend_override,
        format,
        effort_name,
        budget,
        workers,
        stats,
        explain,
    } = request;
    let search_started = Instant::now();
    let profile_started = Instant::now();
    let effort = selection::effort(effort_name).ok_or("invalid selector effort")?;
    // A constrained representation does not use feature routing. Avoid
    // duplicate analysis and its tables for explicit diagnostic controls.
    let features = selection::profile(&[], 1);
    stats.profile_seconds += profile_started.elapsed().as_secs_f64();
    let mut best_backend = "raw".to_owned();
    let mut best_payload = data.to_vec();
    let mut evaluated: Vec<String> = Vec::new();
    let mut candidates =
        selection::generate_candidates(&features, effort_name, Some(route), backend_override)?;
    if format == "cixg1" {
        if candidates.iter().any(|candidate| {
            (candidate.backend.starts_with("context-range-") || candidate.backend == "huffman")
                && backend_override.is_some_and(|backend| {
                    backend.starts_with("context-range-") || backend == "huffman"
                })
        }) {
            return Err(
                "versioned substream coder requires --format cixg2 or --format auto".into(),
            );
        }
        candidates.retain(|candidate| {
            !candidate.backend.starts_with("context-range-")
                && candidate.backend != "huffman"
                && !(candidate.route == 8 && candidate.parameter.is_some_and(|order| order > 3))
        });
    }
    // A route override constrains the non-raw candidates.  Raw remains an
    // unconditional valid fallback and must carry route ID 0 if it wins (or
    // ties), otherwise raw bytes would be decoded as the forced route.
    let mut selected_route = 0u8;
    let candidates: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| candidate.route == route)
        .filter(|candidate| candidate.backend != "raw")
        .collect();
    consider_cixg_candidates_parallel(
        CandidateSearchRequest {
            candidates: &candidates,
            data,
            level: match effort_name {
                "fast" => 1,
                "maximum" => 9,
                _ => 6,
            },
            budget,
            workers,
            search_budget_seconds: effort.search_seconds,
            trial_budget_seconds: effort.trial_seconds,
            worker_memory_mib: effort.worker_memory_mib,
        },
        CandidateSelectionState {
            stats,
            best_route: &mut selected_route,
            best_backend: &mut best_backend,
            best_payload: &mut best_payload,
            evaluated: &mut evaluated,
        },
    )?;
    if explain {
        stats.explanations.push(format!(
            "source_bytes={} candidates=[{}] selected_route={} backend={} payload_bytes={} (route explicitly constrained)",
            data.len(), evaluated.join(","),
            selection::ROUTES[selected_route as usize], best_backend, best_payload.len()
        ));
    }
    stats.blocks += 1;
    stats.selected_source_bytes[selected_route as usize] += data.len() as u64;
    stats.search_seconds += search_started.elapsed().as_secs_f64();
    Ok((selected_route, best_payload))
}

/// Select a CIXG1 route from the bounded profile's ordered route set.
///
/// Raw is always the incumbent. Every candidate is self-identifying within
/// its framed payload, so its complete physical block cost differs only by
/// payload length; the 41-byte frame is equal for all candidates.
struct CixgAutoRequest<'a> {
    data: &'a [u8],
    level: u8,
    backend_override: Option<&'a str>,
    format: &'a str,
    budget: &'a resources::MemoryBudget,
    workers: usize,
    stats: &'a mut AutoSelectionStats,
    explain: bool,
}

fn select_cixg_auto(request: CixgAutoRequest<'_>) -> Result<(u8, Vec<u8>), String> {
    let CixgAutoRequest {
        data,
        level,
        backend_override,
        format,
        budget,
        workers,
        stats,
        explain,
    } = request;
    let effort_name = selector_effort_name(level);
    let effort = selection::effort(effort_name).ok_or("invalid selector effort")?;
    let search_started = Instant::now();
    let profile_started = Instant::now();
    let features = selection::profile(data, effort.sample_bytes);
    stats.profile_seconds += profile_started.elapsed().as_secs_f64();
    let mut candidates =
        selection::generate_candidates(&features, effort_name, None, backend_override)?;
    if format == "cixg1" {
        if backend_override
            .is_some_and(|backend| backend.starts_with("context-range-") || backend == "huffman")
        {
            return Err(
                "versioned substream coder requires --format cixg2 or --format auto".into(),
            );
        }
        candidates.retain(|candidate| {
            !candidate.backend.starts_with("context-range-")
                && candidate.backend != "huffman"
                && !(candidate.route == 8 && candidate.parameter.is_some_and(|order| order > 3))
        });
    }
    let mut best_route = 0u8;
    let mut best_backend = "raw".to_string();
    let mut best_payload = data.to_vec();
    let mut evaluated = Vec::new();
    // The raw candidate is already materialized as the initial incumbent.
    evaluated.push(format!("raw:raw={}B", data.len()));
    let candidates: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| candidate.route != 0)
        .collect();
    consider_cixg_candidates_parallel(
        CandidateSearchRequest {
            candidates: &candidates,
            data,
            level: match effort_name {
                "fast" => 1,
                "maximum" => 9,
                _ => 6,
            },
            budget,
            workers,
            search_budget_seconds: effort.search_seconds,
            trial_budget_seconds: effort.trial_seconds,
            worker_memory_mib: effort.worker_memory_mib,
        },
        CandidateSelectionState {
            stats,
            best_route: &mut best_route,
            best_backend: &mut best_backend,
            best_payload: &mut best_payload,
            evaluated: &mut evaluated,
        },
    )?;
    stats.search_seconds += search_started.elapsed().as_secs_f64();
    stats.blocks += 1;
    stats.selected_source_bytes[best_route as usize] += data.len() as u64;
    if explain {
        stats.explanations.push(format!(
            "source_bytes={} candidates=[{}] selected_route={} backend={} payload_bytes={} block_bytes={}",
            data.len(),
            evaluated.join(","),
            selection::ROUTES[best_route as usize],
            best_backend,
            best_payload.len(),
            data.len(),
        ));
    }
    Ok((best_route, best_payload))
}

fn bwt_transform(data: &[u8]) -> Result<(usize, Vec<u8>), String> {
    let n = data.len();
    if n == 0 {
        return Ok((0, Vec::new()));
    }
    let mut suffixes: Vec<usize> = (0..n).collect();
    let mut ranks: Vec<i64> = data.iter().map(|&b| b as i64).collect();
    let mut width = 1usize;
    loop {
        check_interrupted()?;
        suffixes.sort_unstable_by_key(|&i| bwt_rank_pair(&ranks, i, width));
        let mut next = vec![0i64; n];
        let mut class = 0i64;
        let mut previous: Option<(i64, i64)> = None;
        for &i in &suffixes {
            let pair = bwt_rank_pair(&ranks, i, width);
            if previous.is_some() && previous != Some(pair) {
                class += 1;
            }
            next[i] = class;
            previous = Some(pair);
        }
        ranks = next;
        if class as usize == n - 1 {
            break;
        }
        width = width.checked_mul(2).ok_or("BWT suffix width overflow")?;
    }
    let primary = suffixes
        .iter()
        .position(|&i| i == 0)
        .ok_or("BWT suffix array omitted source start")?
        + 1;
    let mut transformed = Vec::with_capacity(n);
    transformed.push(data[n - 1]);
    for &i in &suffixes {
        if i > 0 {
            transformed.push(data[i - 1]);
        }
    }
    if transformed.len() != n {
        return Err("invalid BWT transform size".into());
    }
    Ok((primary, transformed))
}

fn bwt_rank_pair(ranks: &[i64], index: usize, width: usize) -> (i64, i64) {
    (
        ranks[index],
        ranks.get(index + width).copied().unwrap_or(-1),
    )
}

fn mtf_encode(data: &[u8]) -> Vec<u8> {
    let mut symbols: Vec<u8> = (0..=255).collect();
    let mut positions: Vec<usize> = (0..256).collect();
    let mut out = Vec::with_capacity(data.len());
    for &byte in data {
        let index = positions[byte as usize];
        out.push(index as u8);
        if index > 0 {
            let moved = symbols[index];
            for pos in (1..=index).rev() {
                symbols[pos] = symbols[pos - 1];
                positions[symbols[pos] as usize] = pos;
            }
            symbols[0] = moved;
            positions[moved as usize] = 0;
        }
    }
    out
}

fn mtf_decode(data: &[u8]) -> Vec<u8> {
    let mut symbols: Vec<u8> = (0..=255).collect();
    let mut out = Vec::with_capacity(data.len());
    for &index in data {
        let index = index as usize;
        let symbol = symbols[index];
        out.push(symbol);
        if index > 0 {
            symbols.copy_within(0..index, 1);
            symbols[0] = symbol;
        }
    }
    out
}

fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn mtf_runs(data: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mtf = mtf_encode(data);
    let mut tags = Vec::new();
    let mut literals = Vec::new();
    let mut lengths = Vec::new();
    let mut i = 0;
    while i < mtf.len() {
        if mtf[i] == 0 {
            let start = i;
            i += 1;
            while i < mtf.len() && mtf[i] == 0 {
                i += 1;
            }
            tags.push(1);
            put_varint(i - start, &mut lengths);
        } else {
            tags.push(0);
            literals.push(mtf[i]);
            i += 1;
        }
    }
    (tags, literals, lengths)
}

fn unmtf_runs(parts: &[Vec<u8>], n: usize) -> Result<Vec<u8>, String> {
    if parts.len() != 3 {
        return Err("MTF substream count mismatch".into());
    }
    let (tags, literals, lengths) = (&parts[0], &parts[1], &parts[2]);
    let (mut literal_pos, mut length_pos) = (0usize, 0usize);
    let mut mtf = Vec::with_capacity(n);
    for &tag in tags {
        match tag {
            0 if literal_pos < literals.len() => {
                mtf.push(literals[literal_pos]);
                literal_pos += 1;
            }
            1 => {
                let count = unvar(lengths, &mut length_pos)?;
                if count == 0 || mtf.len().checked_add(count).filter(|&v| v <= n).is_none() {
                    return Err("invalid MTF zero run".into());
                }
                mtf.resize(mtf.len() + count, 0);
            }
            _ => return Err("invalid MTF tag/literal".into()),
        }
    }
    if mtf.len() != n || literal_pos != literals.len() || length_pos != lengths.len() {
        return Err("MTF stream lengths mismatch".into());
    }
    Ok(mtf_decode(&mtf))
}

fn bwt_inverse(primary: usize, transformed: &[u8]) -> Result<Vec<u8>, String> {
    let n = transformed.len();
    if n == 0 {
        return if primary <= 1 {
            Ok(Vec::new())
        } else {
            Err("invalid empty BWT index".into())
        };
    }
    if primary == 0 || primary > n {
        return Err("invalid BWT primary index".into());
    }
    let last: Vec<(u8, usize)> = transformed
        .iter()
        .copied()
        .enumerate()
        .map(|(i, b)| (b, i))
        .collect();
    let mut first = last.clone();
    first.sort_unstable();
    let mut inverse_last = vec![0usize; n];
    for (index, &item) in last.iter().enumerate() {
        inverse_last[item.1] = index + usize::from(index >= primary);
    }
    let mut current = first[primary - 1];
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(current.0);
        let mapped = inverse_last[current.1];
        let index = if mapped == 0 { n - 1 } else { mapped - 1 };
        current = first[index];
    }
    Ok(out)
}

fn bwt_encode(data: &[u8], level: u8, backend: &str) -> Result<Vec<u8>, String> {
    let (primary, transformed) = bwt_transform(data)?;
    let (tags, literals, lengths) = mtf_runs(&transformed);
    let owned = [tags, literals, lengths];
    let refs: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
    let streams = streams_encode(&refs, level, backend)?;
    let mut out = Vec::with_capacity(4 + streams.len());
    out.extend_from_slice(&(primary as u32).to_le_bytes());
    out.extend_from_slice(&streams);
    Ok(out)
}

fn lz_insert(table: &mut std::collections::HashMap<[u8; 4], Vec<usize>>, data: &[u8], pos: usize) {
    let Some(key) = lz_key(data, pos) else {
        return;
    };
    let candidates = table.entry(key).or_default();
    candidates.push(pos);
    if candidates.len() > 8 {
        candidates.remove(0);
    }
}

fn lz_key(data: &[u8], position: usize) -> Option<[u8; 4]> {
    data.get(position..position.checked_add(4)?)
        .and_then(|bytes| bytes.try_into().ok())
}

fn lz_encode(data: &[u8], level: u8, backend: &str) -> Result<Vec<u8>, String> {
    if level <= 2 {
        return lz_encode_fast(data, level, backend);
    }
    let parts = lz_parse(data)?;
    let owned = [parts.tags, parts.literals, parts.lengths, parts.distances];
    let refs: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
    streams_encode(&refs, level, backend)
}

fn lz_encode_fast(data: &[u8], level: u8, backend: &str) -> Result<Vec<u8>, String> {
    let parts = fast_lz::parse(data)?;
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    streams_encode(&refs, level, backend)
}

struct LzParts {
    tags: Vec<u8>,
    literals: Vec<u8>,
    lengths: Vec<u8>,
    distances: Vec<u8>,
}

fn lz_parse(data: &[u8]) -> Result<LzParts, String> {
    use std::collections::HashMap;
    let mut table: HashMap<[u8; 4], Vec<usize>> = HashMap::new();
    let mut parts = LzParts {
        tags: Vec::new(),
        literals: Vec::new(),
        lengths: Vec::new(),
        distances: Vec::new(),
    };
    let mut position = 0usize;
    while position < data.len() {
        limits::check()?;
        let (best_length, best_distance) = lz_best_match(data, position, &table);
        if best_length >= 4 {
            lz_emit_match(
                &mut parts,
                &mut table,
                data,
                position,
                best_length,
                best_distance,
            );
            position += best_length;
        } else {
            lz_emit_literal(&mut parts, &mut table, data, position);
            position += 1;
        }
    }
    Ok(parts)
}

fn lz_best_match(
    data: &[u8],
    position: usize,
    table: &std::collections::HashMap<[u8; 4], Vec<usize>>,
) -> (usize, usize) {
    let Some(key) = lz_key(data, position) else {
        return (0, 0);
    };
    let Some(candidates) = table.get(&key) else {
        return (0, 0);
    };
    let limit = 65535.min(data.len() - position);
    let mut best = (i64::MIN, 0usize, 0usize);
    for &candidate in candidates.iter().rev().take(8) {
        lz_consider_match(data, position, candidate, limit, &mut best);
    }
    (best.1, best.2)
}

fn lz_consider_match(
    data: &[u8],
    position: usize,
    candidate: usize,
    limit: usize,
    best: &mut (i64, usize, usize),
) {
    let distance = position - candidate;
    if distance == 0 || distance > 65536 {
        return;
    }
    let mut length = 4usize;
    while length < limit && data[position - distance + length] == data[position + length] {
        length += 1;
    }
    if length < 4 {
        return;
    }
    let score = (length * 8) as i64
        - (usize::BITS - distance.leading_zeros()) as i64
        - (usize::BITS - length.leading_zeros()) as i64
        - 3;
    if score > best.0 || (score == best.0 && length > best.1) {
        *best = (score, length, distance);
    }
}

fn lz_emit_match(
    parts: &mut LzParts,
    table: &mut std::collections::HashMap<[u8; 4], Vec<usize>>,
    data: &[u8],
    position: usize,
    length: usize,
    distance: usize,
) {
    parts.tags.push(1);
    put_varint(length - 4, &mut parts.lengths);
    put_varint(distance, &mut parts.distances);
    let stop = data.len().min(position + length);
    for indexed_position in position..stop {
        lz_insert(table, data, indexed_position);
    }
}

fn lz_emit_literal(
    parts: &mut LzParts,
    table: &mut std::collections::HashMap<[u8; 4], Vec<usize>>,
    data: &[u8],
    position: usize,
) {
    parts.tags.push(0);
    parts.literals.push(data[position]);
    lz_insert(table, data, position);
}

fn lz_decode(blob: &[u8], n: usize, allow_extended_coders: bool) -> Result<Vec<u8>, String> {
    let parts = streams_decode(blob, n.saturating_mul(4), allow_extended_coders)?;
    if parts.len() != 4 {
        return Err("LZ stream count mismatch".into());
    }
    let (tags, literals, lengths, distances) = (&parts[0], &parts[1], &parts[2], &parts[3]);
    let (mut literal_pos, mut length_pos, mut distance_pos) = (0usize, 0usize, 0usize);
    let mut out = Vec::with_capacity(n);
    for &tag in tags {
        match tag {
            0 if literal_pos < literals.len() => {
                out.push(literals[literal_pos]);
                literal_pos += 1;
            }
            1 => {
                let length = unvar(lengths, &mut length_pos)?
                    .checked_add(4)
                    .ok_or("LZ match length overflow")?;
                let distance = unvar(distances, &mut distance_pos)?;
                if distance == 0
                    || distance > out.len().min(65536)
                    || out.len().checked_add(length).filter(|&v| v <= n).is_none()
                {
                    return Err("invalid LZ match descriptor".into());
                }
                for _ in 0..length {
                    let byte = out[out.len() - distance];
                    out.push(byte);
                }
            }
            _ => return Err("invalid LZ tag/literal".into()),
        }
        if out.len() > n {
            return Err("LZ output overflow".into());
        }
    }
    if out.len() != n
        || literal_pos != literals.len()
        || length_pos != lengths.len()
        || distance_pos != distances.len()
    {
        return Err("LZ stream lengths mismatch".into());
    }
    Ok(out)
}
fn inflate(b: &[u8], n: usize) -> Result<Vec<u8>, String> {
    let mut d = Decompress::new(false);
    let mut out = Vec::with_capacity(n.saturating_add(1));
    let status = d
        .decompress_vec(b, &mut out, FlushDecompress::Finish)
        .map_err(|e| format!("DEFLATE: {e}"))?;
    if status != Status::StreamEnd || d.total_in() != b.len() as u64 || out.len() != n {
        return Err("DEFLATE framing/size mismatch".into());
    }
    Ok(out)
}
fn route_id(name: &str) -> Result<u8, String> {
    selection::ROUTES
        .iter()
        .position(|route| *route == name)
        .map(|id| id as u8)
        .ok_or_else(|| format!("unsupported route: {name}"))
}

fn native_magic(format: &str, level: u8, backend: &str, backend_set: bool) -> &'static [u8; 5] {
    if format == "cixg2"
        || format == "auto"
            && (level >= 9
                || backend_set && (backend.starts_with("context-range-") || backend == "huffman"))
    {
        MAGIC_V2
    } else {
        MAGIC
    }
}

fn parallel_retained(block: u32, workers: usize) -> usize {
    (block as usize)
        .saturating_mul(2 * workers + 1)
        .saturating_add((4 << 20usize) * workers)
        .saturating_add(256 << 10)
}

pub(crate) fn automatic_workers(
    requested: usize,
    explicit: bool,
    block: u32,
    memory: usize,
) -> usize {
    if explicit {
        return requested;
    }
    let mut workers = requested.max(1);
    while workers > 1 && parallel_retained(block, workers).saturating_add(64 << 10) >= memory {
        workers -= 1;
    }
    workers
}

pub(crate) struct EncodeOptions<'a> {
    pub(crate) block: u32,
    pub(crate) level: u8,
    pub(crate) forced: Option<&'a str>,
    pub(crate) backend: &'a str,
    pub(crate) backend_set: bool,
    pub(crate) format: &'a str,
    pub(crate) input_fd: i32,
    pub(crate) flush_interval: Option<Duration>,
    pub(crate) memory: usize,
    pub(crate) workers: usize,
    pub(crate) explain: bool,
    pub(crate) verbose: bool,
    pub(crate) strategy: &'a str,
}

pub(crate) fn encode<R: Read, W: Write>(
    src: R,
    dst: W,
    options: EncodeOptions<'_>,
) -> Result<(), String> {
    encode_with_strategy(src, dst, options)
}

pub(crate) fn encode_with_strategy<R: Read, W: Write>(
    src: R,
    dst: W,
    options: EncodeOptions<'_>,
) -> Result<(), String> {
    let EncodeOptions {
        block,
        level,
        forced,
        backend,
        backend_set,
        format,
        input_fd,
        flush_interval,
        memory,
        workers,
        explain,
        verbose,
        strategy,
    } = options;
    if block == 0 || block > MAX_BLOCK {
        return Err("invalid CIXG block size".into());
    }
    if format == "cixg1"
        && backend_set
        && (backend.starts_with("context-range-") || backend == "huffman")
    {
        return Err("versioned substream coder requires --format cixg2 or --format auto".into());
    }
    selection::generate_candidates(
        &selection::profile(&[], 1),
        selector_effort_name(level),
        forced.map(route_id).transpose()?,
        backend_set.then_some(backend),
    )?;
    if workers > 1 && strategy == "blocks" {
        gstream::encode(
            src,
            dst,
            gstream::EncodeOptions {
                block,
                level,
                forced,
                backend,
                backend_set,
                format,
                input_fd,
                flush_interval,
                memory,
                workers,
                explain,
                verbose,
            },
        )
    } else {
        encode_candidates(
            src,
            dst,
            EncodeOptions {
                block,
                level,
                forced,
                backend,
                backend_set,
                format,
                input_fd,
                flush_interval,
                memory,
                workers,
                explain,
                verbose,
                strategy,
            },
        )
    }
}

fn fill_timed_block<R: Read>(
    src: &mut R,
    buf: &mut [u8],
    input_fd: i32,
    flush_interval: Option<Duration>,
) -> Result<(usize, bool), String> {
    let mut got = 0;
    let mut eof = false;
    let mut buffered_since: Option<Instant> = None;
    while got < buf.len() {
        let timeout = match (flush_interval, buffered_since) {
            (Some(interval), Some(since)) => {
                let elapsed = since.elapsed();
                if elapsed >= interval {
                    break;
                }
                Some(interval - elapsed)
            }
            _ => None,
        };
        let n = match read_poll(src, &mut buf[got..], input_fd, timeout)? {
            Some(n) => n,
            None => continue,
        };
        if n == 0 {
            eof = true;
            break;
        }
        got += n;
        if buffered_since.is_none() {
            buffered_since = Some(Instant::now());
        }
    }
    Ok((got, eof))
}

fn fill_candidate_block<R: Read>(
    src: &mut R,
    buf: &mut [u8],
    input_fd: i32,
    flush_interval: Option<Duration>,
) -> Result<(usize, bool, [u32; 256]), String> {
    let (got, eof) = fill_timed_block(src, buf, input_fd, flush_interval)?;
    let mut symbol_counts = [0u32; 256];
    for &byte in &buf[..got] {
        symbol_counts[byte as usize] += 1;
    }
    Ok((got, eof, symbol_counts))
}

struct CandidateRouteSelection<'a> {
    forced: Option<&'a str>,
    level: u8,
    backend: &'a str,
    backend_set: bool,
    format: &'a str,
    budget: &'a resources::MemoryBudget,
    workers: usize,
    stats: &'a mut AutoSelectionStats,
    explain: bool,
}

fn select_candidate_route(
    data: &[u8],
    selection: CandidateRouteSelection<'_>,
) -> Result<(u8, Vec<u8>), String> {
    let CandidateRouteSelection {
        forced,
        level,
        backend,
        backend_set,
        format,
        budget,
        workers,
        stats,
        explain,
    } = selection;
    if let Some(forced) = forced {
        let route = match forced {
            "raw" => 0,
            "runs" => 1,
            "composition" => 2,
            "lz" => 3,
            "bwt" => 5,
            "predictor" => 4,
            "stride" => 6,
            "phrases" => 7,
            "ppm" => 8,
            "mixture" => 9,
            "deflate" => 10,
            _ => return Err(format!("unsupported route: {forced}")),
        };
        select_cixg_fixed_route(CixgFixedRouteRequest {
            route,
            data,
            backend_override: backend_set.then_some(backend),
            format,
            effort_name: selector_effort_name(level),
            budget,
            workers,
            stats,
            explain,
        })
    } else {
        select_cixg_auto(CixgAutoRequest {
            data,
            level,
            backend_override: backend_set.then_some(backend),
            format,
            budget,
            workers,
            stats,
            explain,
        })
    }
}

struct CandidateFrame<'a> {
    route: u8,
    source_len: usize,
    payload: &'a [u8],
    digest: &'a [u8; 32],
}

fn write_candidate_frame<W: Write>(
    dst: &mut W,
    frame: CandidateFrame<'_>,
    archive_bytes: &mut u64,
) -> Result<(), String> {
    *archive_bytes = archive_bytes
        .checked_add(41 + frame.payload.len() as u64)
        .ok_or("archive length overflow")?;
    put_frame(
        dst,
        frame.route,
        frame.source_len as u32,
        frame.payload.len() as u32,
        frame.digest,
    )
    .map_err(ioerr)?;
    dst.write_all(frame.payload).map_err(ioerr)?;
    dst.flush().map_err(ioerr)
}

fn candidate_archive_magic(
    format: &str,
    level: u8,
    backend: &str,
    backend_set: bool,
) -> &'static [u8; 5] {
    if format == "cixg2"
        || format == "auto"
            && (level >= 9
                || backend_set && (backend.starts_with("context-range-") || backend == "huffman"))
    {
        MAGIC_V2
    } else {
        MAGIC
    }
}

fn report_candidate_selection(
    stats: &AutoSelectionStats,
    level: u8,
    archive_magic: &[u8; 5],
    forced: Option<&str>,
    archive_bytes: u64,
) {
    let route_bytes = stats
        .selected_source_bytes
        .iter()
        .enumerate()
        .filter(|(_, bytes)| **bytes != 0)
        .map(|(route, bytes)| format!("{}:{bytes}", selection::ROUTES[route]))
        .collect::<Vec<_>>()
        .join(",");
    eprintln!(
        "cix: effort={} format={} route={} blocks={} candidates={} memory_skips={} budget_skips={} analyse_seconds={:.3} candidate_seconds={:.3} search_seconds={:.3} archive_bytes={} selected_source_bytes=[{}]",
        effort_display_name(level), std::str::from_utf8(archive_magic).unwrap_or("CIXG?"), forced.unwrap_or("auto"),
        stats.blocks, stats.candidate_attempts, stats.memory_skips, stats.budget_skips, stats.profile_seconds,
        stats.candidate_seconds, stats.search_seconds, archive_bytes, route_bytes,
    );
}

struct CandidateEncoderState {
    archive_magic: &'static [u8; 5],
    budget: resources::MemoryBudget,
    buf: Vec<u8>,
    selection_stats: AutoSelectionStats,
    whole: Sha256,
    total_bytes: u64,
    archive_bytes: u64,
}

fn prepare_candidate_encoder<W: Write>(
    dst: &mut W,
    options: &EncodeOptions<'_>,
) -> Result<CandidateEncoderState, String> {
    let EncodeOptions {
        block,
        level,
        forced,
        backend,
        backend_set,
        format,
        memory,
        workers,
        ..
    } = *options;
    if format == "cixg1"
        && backend_set
        && (backend.starts_with("context-range-") || backend == "huffman")
    {
        return Err("versioned substream coder requires --format cixg2 or --format auto".into());
    }
    let archive_magic = candidate_archive_magic(format, level, backend, backend_set);
    dst.write_all(archive_magic).map_err(ioerr)?;
    dst.write_all(&block.to_le_bytes()).map_err(ioerr)?;
    dst.write_all(&0u32.to_le_bytes()).map_err(ioerr)?;
    let whole = Sha256::new();
    let total_bytes = 0;
    let archive_bytes = HEADER as u64;
    let buf = vec![0; block as usize];
    let selection_stats = AutoSelectionStats::default();
    let retained = (block as usize)
        .saturating_mul(3)
        .saturating_add(if forced.is_some() { 64 << 10 } else { 2 << 20 })
        .saturating_add(if workers > 1 { (2 << 20) * workers } else { 0 });
    if retained >= memory {
        return Err("input/profiler buffers exceed --memory; reduce --block-size".into());
    }
    Ok(CandidateEncoderState {
        archive_magic,
        budget: resources::MemoryBudget::new(memory.saturating_sub(retained)),
        buf,
        selection_stats,
        whole,
        total_bytes,
        archive_bytes,
    })
}

fn encode_candidates<R: Read, W: Write>(
    mut src: R,
    mut dst: W,
    options: EncodeOptions<'_>,
) -> Result<(), String> {
    let mut state = prepare_candidate_encoder(&mut dst, &options)?;
    let EncodeOptions {
        level,
        forced,
        backend,
        backend_set,
        format,
        input_fd,
        flush_interval,
        workers,
        explain,
        verbose,
        ..
    } = options;
    // CIXG2 is required for versioned substream coder 6 and extended PPM
    // orders. Existing efforts retain the byte-compatible CIXG1 header.
    loop {
        let (got, eof, symbol_counts) =
            fill_candidate_block(&mut src, &mut state.buf, input_fd, flush_interval)?;
        if got == 0 {
            if eof {
                break;
            }
            continue;
        }
        if symbol_counts.iter().sum::<u32>() != got as u32 {
            return Err("internal block symbol count mismatch".into());
        }
        state.total_bytes = state
            .total_bytes
            .checked_add(got as u64)
            .ok_or("input length overflow")?;
        let data = &state.buf[..got];
        let rh: [u8; 32] = Sha256::digest(data).into();
        state.whole.update(data);
        let (route, payload) = select_candidate_route(
            data,
            CandidateRouteSelection {
                forced,
                level,
                backend,
                backend_set,
                format,
                budget: &state.budget,
                workers,
                stats: &mut state.selection_stats,
                explain,
            },
        )?;
        for explanation in state.selection_stats.explanations.drain(..) {
            eprintln!(
                "cix: region={} {}",
                state.selection_stats.blocks, explanation
            );
        }
        write_candidate_frame(
            &mut dst,
            CandidateFrame {
                route,
                source_len: got,
                payload: &payload,
                digest: &rh,
            },
            &mut state.archive_bytes,
        )?;
        if eof {
            break;
        }
    }
    let h: [u8; 32] = state.whole.finalize().into();
    put_frame(&mut dst, END, 0, 0, &h).map_err(ioerr)?;
    state.archive_bytes = state
        .archive_bytes
        .checked_add(41)
        .ok_or("archive length overflow")?;
    dst.flush().map_err(ioerr)?;
    if verbose || explain {
        report_candidate_selection(
            &state.selection_stats,
            level,
            state.archive_magic,
            forced,
            state.archive_bytes,
        );
    }
    Ok(())
}

// Keep the contiguous causal history without extending beyond its capacity.
fn append_bounded_history(history: &mut Vec<u8>, data: &[u8], limit: usize) {
    if data.len() >= limit {
        history.clear();
        history.extend_from_slice(&data[data.len() - limit..]);
        return;
    }
    let discard = history
        .len()
        .saturating_add(data.len())
        .saturating_sub(limit);
    if discard != 0 {
        history.drain(..discard);
    }
    history.extend_from_slice(data);
}

fn m6_frame_size(source_len: usize, payload_len: usize) -> usize {
    1usize
        .saturating_add(varint_len(source_len))
        .saturating_add(varint_len(payload_len))
        .saturating_add(payload_len)
}

struct M6Selection {
    mode: u8,
    payload: Vec<u8>,
    size: usize,
}

impl M6Selection {
    fn raw(data: &[u8]) -> Self {
        let payload = data.to_vec();
        Self {
            mode: 0,
            size: m6_frame_size(data.len(), payload.len()),
            payload,
        }
    }

    fn consider(&mut self, mode: u8, payload: Vec<u8>, source_len: usize) {
        let size = m6_frame_size(source_len, payload.len());
        if size < self.size {
            self.mode = mode;
            self.payload = payload;
            self.size = size;
        }
    }
}

struct M6TrialContext<'a> {
    selected: &'a mut M6Selection,
    data: &'a [u8],
    prefix: &'a [u8],
    effort: u8,
    available: usize,
    budget_skips: &'a mut std::collections::BTreeSet<u8>,
}

fn m6_memory_plan(block_len: usize, memory: usize) -> Result<(usize, usize, bool), String> {
    const HISTORY_LIMIT: usize = 1 << 20;
    let fixed = HISTORY_LIMIT
        .saturating_add(block_len.saturating_mul(3))
        .saturating_add(256 * 1024);
    if fixed > memory {
        return Err("CIXM6 history, source and candidate buffers exceed --memory".into());
    }
    let reference = memory
        .saturating_div(8)
        .min(16 * 1024 * 1024)
        .min(memory.saturating_sub(fixed));
    let persistent = stateful::RETAINED_MEMORY_BOUND.saturating_add(stateful::WORKING_MEMORY_BOUND);
    Ok((
        fixed,
        reference,
        fixed.saturating_add(reference).saturating_add(persistent) <= memory,
    ))
}

fn m6_try_recent_reference(
    selected: &mut M6Selection,
    recent: &std::collections::VecDeque<Vec<u8>>,
    data: &[u8],
) {
    if let Some((distance, _)) = recent
        .iter()
        .rev()
        .enumerate()
        .find(|(_, prior)| prior.as_slice() == data)
    {
        let mut payload = Vec::new();
        put_varint(distance + 1, &mut payload);
        selected.consider(9, payload, data.len());
    }
}

fn m6_try_sparse_candidates(selected: &mut M6Selection, data: &[u8]) -> Result<(), String> {
    if data.len() < 2 {
        return Ok(());
    }
    let mut counts = [0usize; 256];
    for &byte in data {
        counts[byte as usize] += 1;
    }
    if counts.iter().copied().max().unwrap_or(0).saturating_mul(4) >= data.len() {
        selected.consider(11, sparse_runs::encode_sparse(data)?, data.len());
    }
    let runs = 1 + data.windows(2).filter(|pair| pair[0] != pair[1]).count();
    if runs.saturating_mul(3) <= data.len().saturating_mul(2) {
        selected.consider(12, sparse_runs::encode_runs(data)?, data.len());
    }
    Ok(())
}

fn m6_try_legacy_candidates(
    selected: &mut M6Selection,
    data: &[u8],
    prefix: &[u8],
    effort: u8,
    available: usize,
    budget_skips: &mut std::collections::BTreeSet<u8>,
) -> Result<(), String> {
    if data.len() >= 16 && effort >= 2 {
        if available >= data.len().saturating_mul(64) {
            selected.consider(
                3,
                legacy_lz::encode(data, prefix, effort)?.payload,
                data.len(),
            );
        } else {
            budget_skips.insert(3);
        }
    }
    if data.len() >= 32 && effort >= 4 {
        if available >= data.len().saturating_mul(128) {
            selected.consider(4, bwt_legacy::encode(data)?.0, data.len());
        } else {
            budget_skips.insert(4);
        }
    }
    if data.len() >= 4 && effort >= 2 {
        if available >= data.len().saturating_mul(64) {
            selected.consider(
                5,
                transform::encode(data, usize::from(effort.min(10)))?.0,
                data.len(),
            );
        } else {
            budget_skips.insert(5);
        }
    }
    Ok(())
}

fn m6_try_context_candidates(ctx: &mut M6TrialContext<'_>) -> Result<Option<Vec<u8>>, String> {
    let (data, prefix, effort, available) = (ctx.data, ctx.prefix, ctx.effort, ctx.available);
    let selected = &mut *ctx.selected;
    let budget_skips = &mut *ctx.budget_skips;
    let multinomial = if available >= data.len().saturating_mul(32) {
        let payload = rank::encode_type_class(data)?;
        selected.consider(1, payload.clone(), data.len());
        Some(payload)
    } else {
        budget_skips.insert(1);
        None
    };
    let context_models = [
        (1usize, 6usize),
        (1, 8),
        (2, 8),
        (2, 10),
        (3, 10),
        (4, 10),
        (4, 12),
        (6, 12),
    ];
    let context_beam = usize::from(effort.min(4));
    if available >= data.len().saturating_mul(32) {
        for &(order, bucket_bits) in context_models.iter().take(context_beam) {
            selected.consider(
                2,
                rank::encode_context_type_classes(data, order, bucket_bits, prefix)?,
                data.len(),
            );
        }
    } else if context_beam > 0 {
        budget_skips.insert(2);
    }
    Ok(multinomial)
}

fn m6_try_adaptive_candidates(ctx: &mut M6TrialContext<'_>) -> Result<(), String> {
    let (data, prefix, effort, available) = (ctx.data, ctx.prefix, ctx.effort, ctx.available);
    let selected = &mut *ctx.selected;
    let budget_skips = &mut *ctx.budget_skips;
    if data.len() >= 2 {
        selected.consider(18, wavelet::encode(data), data.len());
        if available >= data.len().saturating_mul(32) {
            let adaptive_models = [
                (0usize, 8usize),
                (1, 6),
                (1, 8),
                (2, 8),
                (2, 10),
                (3, 10),
                (4, 10),
                (4, 12),
                (6, 12),
            ];
            for &(order, bucket_bits) in adaptive_models.iter().take(usize::from(effort)) {
                match adaptive::encode_general(
                    data,
                    order,
                    bucket_bits,
                    prefix,
                    (available / 4096).max(1),
                ) {
                    Ok(payload) => selected.consider(10, payload, data.len()),
                    Err(error) if error.contains("memory budget") => {
                        budget_skips.insert(10);
                    }
                    Err(error) => return Err(error),
                }
            }
        } else {
            budget_skips.insert(10);
        }
    }
    Ok(())
}

fn m6_try_chain_and_numeric_candidates(ctx: &mut M6TrialContext<'_>) -> Result<(), String> {
    let (data, effort, available) = (ctx.data, ctx.effort, ctx.available);
    let selected = &mut *ctx.selected;
    let budget_skips = &mut *ctx.budget_skips;
    if data.len() >= 2 {
        selected.consider(6, bitplane::encode(data), data.len());
    }
    if data.len() >= 32 && effort >= 8 {
        if available >= data.len().saturating_mul(128) {
            let cache_allowance = available
                .saturating_sub(data.len().saturating_mul(128))
                .min(4 * 1024 * 1024);
            let encoded = chain::encode_with_cache(
                data,
                chain::EncodeOptions {
                    beam: usize::from(effort.min(9)),
                    effort,
                },
                cache_allowance,
            )?;
            selected.consider(7, encoded.payload, data.len());
        } else {
            budget_skips.insert(7);
        }
    }
    if data.len() >= 6 {
        if available >= data.len().saturating_mul(128) {
            selected.consider(8, numeric::encode(data)?.payload, data.len());
        } else {
            budget_skips.insert(8);
        }
    }
    Ok(())
}

fn m6_try_histogram_candidate(
    ctx: &mut M6TrialContext<'_>,
    multinomial: Option<&[u8]>,
) -> Result<(), String> {
    let (data, available) = (ctx.data, ctx.available);
    let selected = &mut *ctx.selected;
    let budget_skips = &mut *ctx.budget_skips;
    if data.len() >= 2 {
        if available >= data.len().saturating_mul(32) {
            if let Some(multinomial) = multinomial {
                if let Some(payload) = histogram::type_class_candidate(data, multinomial)? {
                    selected.consider(20, payload, data.len());
                }
            } else {
                budget_skips.insert(20);
            }
        } else {
            budget_skips.insert(20);
        }
    }
    Ok(())
}

fn m6_try_record_periodic_candidates(ctx: &mut M6TrialContext<'_>) -> Result<(), String> {
    let (data, effort, available) = (ctx.data, ctx.effort, ctx.available);
    let selected = &mut *ctx.selected;
    let budget_skips = &mut *ctx.budget_skips;
    if data.len() >= 256 && effort >= 4 {
        if available >= data.len().saturating_mul(128) {
            selected.consider(
                14,
                record_periodic::encode_record(data, usize::from(effort.min(6)))?.0,
                data.len(),
            );
            selected.consider(
                15,
                record_periodic::encode_periodic(data, usize::from(effort.min(8)))?.0,
                data.len(),
            );
        } else {
            budget_skips.extend([14, 15]);
        }
    }
    Ok(())
}

fn m6_try_ppm_candidate(ctx: &mut M6TrialContext<'_>) -> Result<(), String> {
    let (data, prefix, effort, available) = (ctx.data, ctx.prefix, ctx.effort, ctx.available);
    let selected = &mut *ctx.selected;
    let budget_skips = &mut *ctx.budget_skips;
    if data.len() >= 512 && effort >= 4 {
        let trained = prefix
            .len()
            .min(65536)
            .min(4096usize.max(data.len().saturating_mul(2)));
        if available >= route_minimum_memory(8, data.len().saturating_add(trained)) {
            let order = match effort {
                0..=3 => 2,
                4..=6 => 4,
                7..=8 => 5,
                _ => 6,
            };
            selected.consider(
                16,
                ppm::encode_best(data, order, prefix, None, None, None)?,
                data.len(),
            );
        } else {
            budget_skips.insert(16);
        }
    }
    Ok(())
}

fn m6_try_expert_and_fixedmix_candidates(ctx: &mut M6TrialContext<'_>) -> Result<(), String> {
    let (data, prefix, effort, available) = (ctx.data, ctx.prefix, ctx.effort, ctx.available);
    let selected = &mut *ctx.selected;
    let budget_skips = &mut *ctx.budget_skips;
    if data.len() >= 512 && effort >= 6 {
        let max_contexts = available / EXPERT_CONTEXT_MEMORY_ESTIMATE;
        if max_contexts > 0 {
            match expertmix::encode_bounded(data, &[0, 1, 2, 4], prefix, max_contexts) {
                Ok(payload) => selected.consider(17, payload, data.len()),
                Err(error) if error.contains("memory budget") => {
                    budget_skips.insert(17);
                }
                Err(error) => return Err(error),
            }
        } else {
            budget_skips.insert(17);
        }
    }
    if data.len() >= 512 && data.len() <= 32768 && effort >= 7 {
        let trained = prefix
            .len()
            .min(65536)
            .min(4096usize.max(data.len().saturating_mul(2)));
        if available >= route_minimum_memory(9, data.len().saturating_add(trained)) {
            selected.consider(
                19,
                fixedmix::encode_with_history_eta(data, prefix, 6)?,
                data.len(),
            );
        } else {
            budget_skips.insert(19);
        }
    }
    Ok(())
}

fn m6_try_persistent_candidate(
    ctx: &mut M6TrialContext<'_>,
    states: &mut Option<stateful::PersistentAdaptiveStates>,
) -> Result<(), String> {
    let (data, prefix) = (ctx.data, ctx.prefix);
    let selected = &mut *ctx.selected;
    let budget_skips = &mut *ctx.budget_skips;
    if let Some(states) = states.as_mut() {
        let (_, payload) = states.encode_best(data, prefix, stateful::WORKING_MEMORY_BOUND)?;
        selected.consider(13, payload, data.len());
        states.observe(data, prefix)?;
    } else {
        budget_skips.insert(13);
    }
    Ok(())
}

fn m6_write_selected_frame<W: Write>(
    dst: &mut W,
    selected: &M6Selection,
    source_len: usize,
) -> Result<(), String> {
    let mut frame = vec![selected.mode];
    put_varint(source_len, &mut frame);
    put_varint(selected.payload.len(), &mut frame);
    dst.write_all(&frame).map_err(ioerr)?;
    dst.write_all(&selected.payload).map_err(ioerr)?;
    dst.flush().map_err(ioerr)
}

fn m6_fill_block<R: Read>(
    src: &mut R,
    block: &mut [u8],
    input_fd: i32,
    flush_interval: Option<Duration>,
) -> Result<(usize, bool), String> {
    fill_timed_block(src, block, input_fd, flush_interval)
}

fn finish_m6_encode<W: Write>(
    dst: &mut W,
    budget_skips: &std::collections::BTreeSet<u8>,
    whole: usize,
    crc: crc32fast::Hasher,
    file_name: Option<&str>,
) -> Result<(), String> {
    if !budget_skips.is_empty() && !limits::is_library_scope() {
        let modes = budget_skips
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        eprintln!("cix: omitted CIXM6 candidates due to --memory budget: modes {modes}");
    }
    let mut footer = vec![255];
    put_varint(whole, &mut footer);
    footer.extend_from_slice(&crc.finalize().to_le_bytes());
    dst.write_all(&footer).map_err(ioerr)?;
    if let Some(name) = file_name {
        let bytes = name.as_bytes();
        let name_len = u32::try_from(bytes.len()).map_err(|_| "CIXF1 filename is too long")?;
        dst.write_all(bytes).map_err(ioerr)?;
        dst.write_all(&name_len.to_le_bytes()).map_err(ioerr)?;
        dst.write_all(b"CIXF1").map_err(ioerr)?;
    }
    dst.flush().map_err(ioerr)
}

pub(crate) fn encode_m6<R: Read, W: Write>(
    src: R,
    dst: W,
    block_size: usize,
    file_name: Option<&str>,
    effort: u8,
    memory: usize,
) -> Result<(), String> {
    encode_m6_timed(
        src,
        dst,
        block_size,
        file_name,
        effort,
        memory,
        M6ReadOptions {
            input_fd: -1,
            flush_interval: None,
        },
    )
}

#[derive(Clone, Copy)]
pub(crate) struct M6ReadOptions {
    pub(crate) input_fd: i32,
    pub(crate) flush_interval: Option<Duration>,
}

struct M6BlockEncoder<'a, W> {
    dst: &'a mut W,
    whole: usize,
    crc: crc32fast::Hasher,
    effort: u8,
    memory: usize,
    fixed_reserve: usize,
    reference_budget: usize,
    persistent_enabled: bool,
    persistent_reserve: usize,
    persistent_states: Option<stateful::PersistentAdaptiveStates>,
    history: Vec<u8>,
    recent: std::collections::VecDeque<Vec<u8>>,
    recent_bytes: usize,
    budget_skips: std::collections::BTreeSet<u8>,
}

impl<W: Write> M6BlockEncoder<'_, W> {
    fn encode_block(&mut self, data: &[u8]) -> Result<(), String> {
        const HISTORY_LIMIT: usize = 1 << 20;
        let prefix = self.history.as_slice();
        let mut selected = M6Selection::raw(data);
        let available = self
            .memory
            .saturating_sub(self.fixed_reserve)
            .saturating_sub(self.reference_budget)
            .saturating_sub(if self.persistent_enabled {
                self.persistent_reserve
            } else {
                0
            });
        let mut trials = M6TrialContext {
            selected: &mut selected,
            data,
            prefix,
            effort: self.effort,
            available,
            budget_skips: &mut self.budget_skips,
        };
        let multinomial = m6_try_context_candidates(&mut trials)?;
        m6_try_adaptive_candidates(&mut trials)?;
        m6_try_sparse_candidates(trials.selected, data)?;
        m6_try_recent_reference(trials.selected, &self.recent, data);
        m6_try_legacy_candidates(
            trials.selected,
            data,
            prefix,
            self.effort,
            available,
            trials.budget_skips,
        )?;
        m6_try_chain_and_numeric_candidates(&mut trials)?;
        m6_try_histogram_candidate(&mut trials, multinomial.as_deref())?;
        m6_try_record_periodic_candidates(&mut trials)?;
        m6_try_ppm_candidate(&mut trials)?;
        m6_try_expert_and_fixedmix_candidates(&mut trials)?;
        m6_try_persistent_candidate(&mut trials, &mut self.persistent_states)?;
        m6_write_selected_frame(self.dst, &selected, data.len())?;
        self.crc.update(data);
        self.whole = self
            .whole
            .checked_add(data.len())
            .ok_or("CIXM6 stream length overflow")?;
        append_bounded_history(&mut self.history, data, HISTORY_LIMIT);
        self.recent_bytes = self.recent_bytes.saturating_add(data.len());
        self.recent.push_back(data.to_vec());
        while self.recent.len() > 1024 || self.recent_bytes > self.reference_budget {
            if let Some(oldest) = self.recent.pop_front() {
                self.recent_bytes = self.recent_bytes.saturating_sub(oldest.len());
            } else {
                break;
            }
        }
        Ok(())
    }
}

pub(crate) fn encode_m6_timed<R: Read, W: Write>(
    mut src: R,
    mut dst: W,
    block_size: usize,
    file_name: Option<&str>,
    effort: u8,
    memory: usize,
    read_options: M6ReadOptions,
) -> Result<(), String> {
    let M6ReadOptions {
        input_fd,
        flush_interval,
    } = read_options;
    const HISTORY_LIMIT: usize = 1 << 20;
    let mut block = vec![0u8; block_size.min(MAX_BLOCK as usize).max(1)];
    let (fixed_reserve, reference_budget, persistent_enabled) =
        m6_memory_plan(block.len(), memory)?;
    let persistent_reserve =
        stateful::RETAINED_MEMORY_BOUND.saturating_add(stateful::WORKING_MEMORY_BOUND);
    dst.write_all(b"CIXM6").map_err(ioerr)?;
    let mut header = Vec::new();
    put_varint(HISTORY_LIMIT, &mut header);
    dst.write_all(&header).map_err(ioerr)?;

    let mut encoder = M6BlockEncoder {
        dst: &mut dst,
        whole: 0,
        crc: crc32fast::Hasher::new(),
        effort,
        memory,
        fixed_reserve,
        reference_budget,
        persistent_enabled,
        persistent_reserve,
        persistent_states: persistent_enabled.then(stateful::PersistentAdaptiveStates::new),
        history: Vec::with_capacity(HISTORY_LIMIT),
        recent: std::collections::VecDeque::new(),
        recent_bytes: 0,
        budget_skips: std::collections::BTreeSet::new(),
    };
    loop {
        let (got, eof) = m6_fill_block(&mut src, &mut block, input_fd, flush_interval)?;
        if got == 0 {
            if eof {
                break;
            }
            continue;
        }
        encoder.encode_block(&block[..got])?;
        if eof {
            break;
        }
    }
    let M6BlockEncoder {
        dst,
        budget_skips,
        whole,
        crc,
        ..
    } = encoder;
    finish_m6_encode(dst, &budget_skips, whole, crc, file_name)
}

fn varint_len(mut value: usize) -> usize {
    let mut n = 1;
    while value >= 128 {
        value >>= 7;
        n += 1;
    }
    n
}

fn cixg_candidate(data: &[u8], route: Option<u8>, level: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&MAX_BLOCK.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    let mut whole = Sha256::new();
    for block in data.chunks(MAX_BLOCK as usize) {
        let (selected, payload) = if let Some(id) = route {
            (id, encode_route(id, block, level, "hybrid", None)?)
        } else {
            let candidates = [
                (0, block.to_vec()),
                (1, rle(block)),
                (2, encode_route(2, block, level, "hybrid", None)?),
                (10, encode_route(10, block, level, "hybrid", None)?),
            ];
            candidates.into_iter().min_by_key(|(_, p)| p.len()).unwrap()
        };
        let digest: [u8; 32] = Sha256::digest(block).into();
        put_frame(
            &mut out,
            selected,
            block.len() as u32,
            payload.len() as u32,
            &digest,
        )
        .map_err(ioerr)?;
        out.extend_from_slice(&payload);
        whole.update(block);
    }
    let digest: [u8; 32] = whole.finalize().into();
    put_frame(&mut out, END, 0, 0, &digest).map_err(ioerr)?;
    Ok(out)
}

/// The raw CIXG1 fallback is always available before portfolio search.  Keep
/// a conservative complete-archive bound so source plus fallback can be
/// admitted before allocating either.  Every raw frame carries its route,
/// lengths and digest, followed by the terminal frame.
fn portfolio_raw_fallback_bytes(input_len: usize) -> Result<usize, String> {
    let blocks = input_len
        .checked_add(MAX_BLOCK as usize - 1)
        .ok_or("portfolio raw fallback block count overflow")?
        / MAX_BLOCK as usize;
    let frame_overhead = blocks
        .checked_mul(48)
        .and_then(|value| value.checked_add(64))
        .ok_or("portfolio raw fallback size overflow")?;
    input_len
        .checked_add(frame_overhead)
        .ok_or("portfolio raw fallback size overflow".into())
}

/// Explicit portfolio profiles retain their established candidate families,
/// while each entry is a fully configured CIXB1 descriptor.  BEST-like size
/// candidates therefore retain their exact native dictionary/model settings
/// instead of rebuilding an uncharged backend from a backend name.
fn portfolio_descriptors(profile: &str) -> Vec<&'static external::CandidateDescriptor> {
    external::best_candidates()
        .iter()
        .filter(|descriptor| match profile {
            "fast" => matches!(descriptor.id, "gzip-fast" | "zstd-fast" | "brotli-fast-0"),
            "default" => matches!(
                descriptor.id,
                "gzip-size" | "bzip2-size" | "xz-default-6" | "zstd-default-3"
            ),
            "size" => descriptor.profile == "size",
            _ => false,
        })
        .collect()
}

/// The direct `--external` spelling has no configuration syntax.  Keep its
/// historical backend/profile interface, but attach the same resource model
/// as the automatic descriptor before a native call is made.
fn direct_external_descriptor(
    backend: &str,
    profile: &str,
) -> Result<external::CandidateDescriptor, String> {
    let profile: &'static str = match profile {
        "fast" => "fast",
        "default" | "current" => "default",
        "size" => "size",
        _ => return Err(format!("unsupported CIXB1 profile: {profile}")),
    };
    let backend: &'static str = match backend {
        "gzip" => "gzip",
        "bzip2" => "bzip2",
        "xz" => "xz",
        "zstd" => "zstd",
        "brotli" => "brotli",
        "zpaq" => "zpaq",
        "bsc" => "bsc",
        _ => return Err(format!("unsupported CIXB1 backend: {backend}")),
    };
    let config = match (backend, profile) {
        ("zpaq", _) => external::CandidateConfig::Zpaq { level: 5 },
        ("bsc", _) => external::CandidateConfig::Bsc {
            lzp_hash_size: 15,
            lzp_min_len: 72,
            adaptive_coder: false,
            fast_mode: true,
        },
        ("brotli", "size") => external::CandidateConfig::BrotliQuality {
            quality: 11,
            lgwin: 22,
            mode: external::BrotliMode::Generic,
        },
        _ => external::CandidateConfig::Canonical,
    };
    // Validate the actual direct encoder profile now.  This retains the
    // established diagnostics for unsupported backend/profile combinations.
    match (backend, profile) {
        ("gzip" | "bzip2" | "xz" | "zstd", "fast" | "default" | "size")
        | ("brotli", "fast" | "size")
        | ("zpaq" | "bsc", "default" | "size") => Ok(external::CandidateDescriptor {
            id: "explicit-external",
            backend,
            profile,
            config,
        }),
        _ => Err(format!(
            "CIXB1 backend {backend} is not eligible for profile {profile}"
        )),
    }
}

struct PortfolioWinner {
    archive: Vec<u8>,
    name: String,
    omitted: Vec<String>,
}

struct PortfolioCandidate<'a> {
    data: &'a [u8],
    data_capacity: usize,
    memory: usize,
    descriptor: Option<&'a external::CandidateDescriptor>,
    omitted_name: &'a str,
}

impl PortfolioWinner {
    fn retained_bytes(&self, data: &Vec<u8>) -> usize {
        data.capacity().saturating_add(self.archive.capacity())
    }

    fn consider(
        &mut self,
        candidate: Vec<u8>,
        context: PortfolioCandidate<'_>,
        name: impl FnOnce() -> String,
    ) -> Result<(), String> {
        if candidate.len() >= self.archive.len() {
            return Ok(());
        }
        match portfolio_candidate_matches(
            &candidate,
            candidate.capacity(),
            context.data,
            context
                .data_capacity
                .saturating_add(self.archive.capacity()),
            context.memory,
            context.descriptor,
        )? {
            PortfolioVerification::Verified => {
                self.archive = candidate;
                self.name = name();
            }
            PortfolioVerification::MemoryOmission(reason) => {
                self.omitted
                    .push(format!("{} ({reason})", context.omitted_name));
            }
        }
        Ok(())
    }
}

fn portfolio_search_external(
    data: &Vec<u8>,
    profile: &str,
    memory: usize,
    winner: &mut PortfolioWinner,
) -> Result<(), String> {
    for descriptor in portfolio_descriptors(profile) {
        check_interrupted()?;
        let available = memory.saturating_sub(winner.retained_bytes(data));
        if let Err(omission) = descriptor.admit(data.len(), available) {
            winner
                .omitted
                .push(format!("{} ({})", descriptor.id, omission.reason));
            continue;
        }
        let candidate = match descriptor.full_archive_encode(data) {
            Ok(candidate) => candidate,
            // BSC can decline uncompressible input. It is a measured candidate
            // rejection, while real backend errors remain fatal.
            Err(error)
                if error.starts_with("BSC candidate rejected:")
                    || error.starts_with("ZPAQ candidate rejected:") =>
            {
                winner.omitted.push(format!("{} ({error})", descriptor.id));
                continue;
            }
            Err(error) => return Err(error),
        };
        if winner
            .retained_bytes(data)
            .saturating_add(candidate.capacity())
            > memory
        {
            return Err(format!(
                "CIXB1 {} exceeded its admitted --memory accounting",
                descriptor.id
            ));
        }
        winner.consider(
            candidate,
            PortfolioCandidate {
                data,
                data_capacity: data.capacity(),
                memory,
                descriptor: Some(descriptor),
                omitted_name: descriptor.id,
            },
            || descriptor.id.to_string(),
        )?;
    }
    Ok(())
}

struct PortfolioNativeAttempt<'a> {
    route: Option<&'a str>,
    timeout_seconds: u64,
    name: String,
}

fn portfolio_search_native(
    data: &Vec<u8>,
    level: u8,
    memory: usize,
    winner: &mut PortfolioWinner,
    attempt: PortfolioNativeAttempt<'_>,
    runner: &PortfolioNativeRunner,
) -> Result<(), String> {
    let PortfolioNativeAttempt {
        route,
        timeout_seconds,
        name,
    } = attempt;
    let Some(candidate) = runner(
        data,
        level,
        memory,
        winner.retained_bytes(data),
        route,
        timeout_seconds,
    )?
    else {
        winner.omitted.push(name);
        return Ok(());
    };
    let omitted_name = name.clone();
    winner.consider(
        candidate,
        PortfolioCandidate {
            data,
            data_capacity: data.capacity(),
            memory,
            descriptor: None,
            omitted_name: &omitted_name,
        },
        || name,
    )
}

fn portfolio_search_size_routes(
    data: &Vec<u8>,
    level: u8,
    memory: usize,
    deadline: Instant,
    candidate_seconds: u64,
    winner: &mut PortfolioWinner,
    runner: &PortfolioNativeRunner,
) -> Result<(), String> {
    let routes = [
        "raw",
        "runs",
        "composition",
        "lz",
        "predictor",
        "bwt",
        "stride",
        "phrases",
        "ppm",
        "mixture",
        "deflate",
    ];
    for route in routes {
        check_interrupted()?;
        let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
        let name = format!("cix-route:{route}");
        if remaining == 0 {
            winner.omitted.push(format!("{name}:overall-budget"));
            continue;
        }
        portfolio_search_native(
            data,
            level,
            memory,
            winner,
            PortfolioNativeAttempt {
                route: Some(route),
                timeout_seconds: candidate_seconds.min(remaining),
                name,
            },
            runner,
        )?;
    }
    Ok(())
}

pub(crate) type PortfolioNativeRunner =
    dyn Fn(&[u8], u8, usize, usize, Option<&str>, u64) -> Result<Option<Vec<u8>>, String>;

pub(crate) fn encode_portfolio<R: Read, W: Write>(
    src: R,
    mut dst: W,
    profile: &str,
    memory: usize,
    runner: &PortfolioNativeRunner,
) -> Result<(), String> {
    let profile = if profile == "current" {
        "default"
    } else {
        profile
    };
    if !matches!(profile, "fast" | "default" | "size") {
        return Err("--portfolio accepts fast, default or size".into());
    }
    let profile_minimum = if profile == "size" {
        384 * 1024 * 1024
    } else {
        128 * 1024 * 1024
    };
    let fallback_required = external::MAX_INPUT
        .checked_add(portfolio_raw_fallback_bytes(external::MAX_INPUT)?)
        .ok_or("portfolio fallback memory estimate overflow")?;
    let required = profile_minimum.max(fallback_required);
    if memory < required {
        return Err(format!(
            "{profile} portfolio requires --memory of at least {} MiB",
            required / (1024 * 1024)
        ));
    }
    let mut data = Vec::with_capacity(external::MAX_INPUT);
    src.take((external::MAX_INPUT + 1) as u64)
        .read_to_end(&mut data)
        .map_err(ioerr)?;
    if data.len() > external::MAX_INPUT {
        return Err(format!(
            "native portfolio input exceeds {} MiB",
            external::MAX_INPUT / (1024 * 1024)
        ));
    }
    let total_search_seconds = match profile {
        "fast" => 60,
        "size" => 240,
        _ => 120,
    };
    let search_deadline = Instant::now() + Duration::from_secs(total_search_seconds);
    let level = match profile {
        "fast" => 1,
        "default" => 5,
        _ => 9,
    };
    // Establish a complete, valid archive before any search. The Python
    // selector does this first too; otherwise one expensive model can prevent
    // the caller from receiving even a usable fallback.
    let archive = cixg_candidate(&data, Some(0), level)?;
    if data.capacity().saturating_add(archive.capacity()) > memory {
        return Err("portfolio source plus raw fallback exceeds --memory".into());
    }
    let mut winner = PortfolioWinner {
        archive,
        name: "cix-route:raw".to_string(),
        omitted: Vec::new(),
    };
    portfolio_search_external(&data, profile, memory, &mut winner)?;
    // Give size routes more time than the default profile while retaining an
    // explicit whole-search ceiling. Earlier 4s/12s limits cut off CIX routes
    // before they could complete on full Silesia files.
    let candidate_seconds = match profile {
        "fast" => 12,
        "size" => 180,
        _ => 90,
    };
    // Candidate encoders run in child processes so each route has a real
    // deadline. Joining a Rust thread after a timeout would leave the route
    // consuming CPU, defeating the bounded portfolio contract.
    let remaining = search_deadline
        .saturating_duration_since(Instant::now())
        .as_secs();
    if remaining == 0 {
        winner
            .omitted
            .push("cix-adaptive:overall-budget".to_string());
    } else {
        portfolio_search_native(
            &data,
            level,
            memory,
            &mut winner,
            PortfolioNativeAttempt {
                route: None,
                timeout_seconds: candidate_seconds.min(remaining),
                name: "cix-adaptive".to_string(),
            },
            runner,
        )?;
    }
    if profile == "size" {
        portfolio_search_size_routes(
            &data,
            level,
            memory,
            search_deadline,
            candidate_seconds,
            &mut winner,
            runner,
        )?;
    }
    dst.write_all(&winner.archive).map_err(ioerr)?;
    dst.flush().map_err(ioerr)?;
    eprintln!(
        "cix: portfolio profile={profile}, selected={}, archive_bytes={}, search_budget_seconds={total_search_seconds}, time_budget_omitted={}",
        winner.name,
        winner.archive.len(),
        if winner.omitted.is_empty() { "none".to_string() } else { winner.omitted.join(",") }
    );
    Ok(())
}

enum PortfolioVerification {
    Verified,
    MemoryOmission(String),
}

/// Verify without pretending that the decode is free. During portfolio search
/// the source, incumbent and candidate are all live. The reconstructed output
/// and decoder working set must fit in the remaining declared budget before a
/// candidate is eligible to replace the incumbent.
fn portfolio_candidate_matches(
    archive: &[u8],
    archive_capacity: usize,
    source: &[u8],
    retained_bytes: usize,
    memory: usize,
    descriptor: Option<&external::CandidateDescriptor>,
) -> Result<PortfolioVerification, String> {
    let restored_bytes = source.len();
    let remaining = memory
        .checked_sub(retained_bytes)
        .and_then(|value| value.checked_sub(archive_capacity))
        .and_then(|value| value.checked_sub(restored_bytes));
    let Some(decoder_memory) = remaining else {
        return Ok(PortfolioVerification::MemoryOmission(
            "source, incumbent, candidate and restored output exceed --memory".into(),
        ));
    };
    if let Some(descriptor) = descriptor {
        let required = descriptor.decoder_peak_bytes(source.len())?;
        if required > decoder_memory {
            return Ok(PortfolioVerification::MemoryOmission(format!(
                "verification decoder needs {required} bytes, available {decoder_memory}"
            )));
        }
    }
    let mut restored = Vec::with_capacity(source.len());
    match decode(
        io::Cursor::new(archive),
        &mut restored,
        false,
        false,
        -1,
        decoder_memory,
    ) {
        Ok(()) => {}
        Err(error) if error.contains("memory") || error.contains("resource") => {
            return Ok(PortfolioVerification::MemoryOmission(format!(
                "verification decoder cannot fit: {error}"
            )));
        }
        Err(error) => return Err(format!("portfolio candidate decode failed: {error}")),
    }
    if restored != source {
        return Err("portfolio candidate did not restore the original bytes".into());
    }
    Ok(PortfolioVerification::Verified)
}

pub(crate) fn encode_external<R: Read, W: Write>(
    src: R,
    mut dst: W,
    backend: &str,
    profile: &str,
    memory: usize,
) -> Result<(), String> {
    let descriptor = direct_external_descriptor(backend, profile)?;
    // Source reading precedes all codec/model/output allocation. Reserve for
    // the 64 KiB stack chunk and overlapping old/new Vec allocations during
    // growth; post-read codec admission still charges the actual capacity.
    if memory < 64 * 1024 {
        return Err("CIXB1 bounded source reading requires at least 64 KiB of --memory".into());
    }
    let allowance = bounded_reader::conservative_allowance(memory).min(external::MAX_INPUT);
    let data = bounded_reader::read_bounded(src, external::MAX_INPUT, allowance)
        .map_err(|error| format!("CIXB1 bounded source: {error}"))?;
    let available = memory.saturating_sub(data.capacity());
    descriptor
        .admit(data.len(), available)
        .map_err(|omission| {
            format!(
                "CIXB1 external encoding cannot admit {}: {}",
                descriptor.backend, omission.reason
            )
        })?;
    let archive = descriptor.full_archive_encode(&data)?;
    if data.capacity().saturating_add(archive.capacity()) > memory {
        return Err("CIXB1 external archive exceeded --memory accounting".into());
    }
    dst.write_all(&archive).map_err(ioerr)?;
    dst.flush().map_err(ioerr)
}
fn read_stream_varint<R: Read>(src: &mut R, fd: i32) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..usize::BITS as usize).step_by(7) {
        let mut b = [0u8; 1];
        read_exact_poll(src, &mut b, fd)?;
        let low = (b[0] & 0x7f) as usize;
        if shift + 7 > usize::BITS as usize && low > (usize::MAX >> shift) {
            return Err("CIXM6 varint overflow".into());
        }
        let part = low
            .checked_shl(shift as u32)
            .ok_or("CIXM6 varint overflow")?;
        value = value.checked_add(part).ok_or("CIXM6 varint overflow")?;
        if b[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("oversized CIXM6 varint".into())
}

fn m6_read_bits(data: &[u8], bitpos: &mut usize, count: usize) -> Result<usize, String> {
    if count >= usize::BITS as usize || bitpos.saturating_add(count) > data.len() * 8 {
        return Err("truncated CIXM6 bitstream".into());
    }
    let mut value = 0usize;
    for _ in 0..count {
        value = (value << 1) | (((data[*bitpos / 8] >> (7 - (*bitpos % 8))) & 1) as usize);
        *bitpos += 1;
    }
    Ok(value)
}
fn m6_decode_bytes(blob: &[u8], count: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    if blob.is_empty() {
        return Err("empty CIXM6 byte substream".into());
    }
    let mut pos = 1;
    let mut out = Vec::with_capacity(count);
    match blob[0] {
        0 => {
            let values = rank::decode_type_class(blob, &mut pos, count)?;
            if pos != blob.len() {
                return Err("trailing CIXM6 static bytes".into());
            }
            Ok(values)
        }
        2 => adaptive::decode_general(&blob[1..], count, &[], model_limit),
        3 => {
            if blob.len() < 2 || !matches!(blob[1], 1 | 2) {
                return Err("invalid CIXM6 byte context order".into());
            }
            adaptive::decode_general(&blob[2..], count, &[], model_limit)
        }
        4 => wavelet::decode(&blob[1..], count),
        5 => histogram::decode_type_class(&blob[1..], count),
        1 => {
            let chunk = unvar(blob, &mut pos)?;
            if chunk == 0 {
                return Err("invalid CIXM6 byte chunk size".into());
            }
            while out.len() < count {
                let n = chunk.min(count - out.len());
                out.extend(rank::decode_type_class(blob, &mut pos, n)?);
            }
            if pos != blob.len() {
                return Err("trailing CIXM6 chunk bytes".into());
            }
            Ok(out)
        }
        mode => Err(format!("unsupported CIXM6 byte substream mode {mode}")),
    }
}
fn m6_decode_uints(blob: &[u8], count: usize, model_limit: usize) -> Result<Vec<usize>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mode = *blob.first().ok_or("empty CIXM6 integer stream")?;
    let mut pos = 1;
    match mode {
        0 => m6_decode_varints(blob, &mut pos, count),
        1 => m6_decode_rice(blob, pos, count),
        2 | 3 => {
            let class_len = unvar(blob, &mut pos)?;
            let end = pos.checked_add(class_len).ok_or("class length overflow")?;
            if end > blob.len() {
                return Err("truncated CIXM6 integer classes".into());
            }
            let classes = if mode == 2 {
                let mut cp = 0;
                let v = rank::decode_type_class(&blob[pos..end], &mut cp, count)?;
                if cp != class_len {
                    return Err("trailing CIXM6 integer classes".into());
                }
                v
            } else {
                m6_decode_bytes(&blob[pos..end], count, model_limit)?
            };
            let bits = &blob[end..];
            let mut bp = 0;
            let mut out = Vec::with_capacity(count);
            for class in classes {
                let width = class as usize;
                if width >= usize::BITS as usize {
                    return Err("CIXM6 integer class too wide".into());
                }
                let low = m6_read_bits(bits, &mut bp, width)?;
                out.push(
                    (1usize << width)
                        .checked_add(low)
                        .and_then(|v| v.checked_sub(1))
                        .ok_or("CIXM6 integer overflow")?,
                );
            }
            Ok(out)
        }
        other => Err(format!("unsupported CIXM6 integer stream mode {other}")),
    }
}

fn m6_decode_varints(blob: &[u8], pos: &mut usize, count: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(unvar(blob, pos)?);
    }
    if *pos != blob.len() {
        return Err("trailing CIXM6 varints".into());
    }
    Ok(out)
}

fn m6_decode_rice(blob: &[u8], pos: usize, count: usize) -> Result<Vec<usize>, String> {
    let k = *blob.get(pos).ok_or("missing CIXM6 Rice parameter")? as usize;
    if k >= usize::BITS as usize {
        return Err("invalid CIXM6 Rice parameter".into());
    }
    let bits = &blob[pos + 1..];
    let mut bp = 0;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut q = 0usize;
        while m6_read_bits(bits, &mut bp, 1)? != 0 {
            q = q.checked_add(1).ok_or("Rice quotient overflow")?;
        }
        let low = m6_read_bits(bits, &mut bp, k)?;
        if q > usize::MAX >> k {
            return Err("Rice value overflow".into());
        }
        out.push((q << k) | low);
    }
    Ok(out)
}

#[cfg(test)]
mod m6_rice_tests {
    use super::*;

    #[test]
    fn rice_accepts_usize_max_and_rejects_high_quotient() {
        let width = (usize::BITS - 1) as u8;
        let mut maximum = vec![1, width, 0b1011_1111];
        maximum.extend([0xff; 8]);
        assert_eq!(m6_decode_rice(&maximum, 1, 1).unwrap(), vec![usize::MAX]);
        let mut overflow = vec![1, width, 0b1100_0000];
        overflow.extend([0; 8]);
        assert!(m6_decode_rice(&overflow, 1, 1).is_err());
    }
}
fn m6_decode_periodic(
    blob: &[u8],
    source_len: usize,
    model_limit: usize,
) -> Result<Vec<u8>, String> {
    if blob.len() < 4 || !matches!(blob[0], 1 | 2) {
        return Err("unknown periodic block version".into());
    }
    let op = blob[1];
    let inner = blob[2];
    let mut pos = 3;
    let lag = unvar(blob, &mut pos)?;
    if lag == 0 || lag >= source_len {
        return Err("invalid periodic lag".into());
    }
    let payload = &blob[pos..];
    let residual = match inner {
        0 => {
            let mut p = 0;
            let out = rank::decode_type_class(payload, &mut p, source_len)?;
            if p != payload.len() {
                return Err("trailing periodic multinomial bytes".into());
            }
            out
        }
        1 => rank::decode_sparse_blob(payload, source_len)?,
        2 => m6_decode_runs(payload, source_len, model_limit)?,
        3 => bitplane::decode(payload, source_len)?,
        4 if blob[0] >= 2 => m6_decode_bytes(payload, source_len, model_limit)?,
        _ => return Err(format!("unsupported periodic inner mode {inner}")),
    };
    let mut out = residual;
    match op {
        0 => {
            for i in lag..source_len {
                out[i] ^= out[i - lag];
            }
        }
        1 => {
            for i in lag..source_len {
                out[i] = out[i].wrapping_add(out[i - lag]);
            }
        }
        _ => return Err("unknown periodic operation".into()),
    }
    Ok(out)
}

fn m6_decode_record(blob: &[u8], source_len: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    if blob.is_empty() || !matches!(blob[0], 1 | 2) {
        return Err("unknown CIXM6 record block version".into());
    }
    let version = blob[0];
    let mut pos = 1;
    let width = unvar(blob, &mut pos)?;
    if width == 0 || width > source_len || width > 2048 {
        return Err("invalid CIXM6 record width".into());
    }
    let mut lanes = Vec::with_capacity(width);
    for lane in 0..width {
        let count = if lane < source_len {
            (source_len - 1 - lane) / width + 1
        } else {
            0
        };
        let values = if version == 1 {
            rank::decode_type_class(blob, &mut pos, count)?
        } else {
            let len = unvar(blob, &mut pos)?;
            let end = pos.checked_add(len).ok_or("record lane length overflow")?;
            if end > blob.len() {
                return Err("truncated CIXM6 record lane".into());
            }
            let out = m6_decode_bytes(&blob[pos..end], count, model_limit)?;
            pos = end;
            out
        };
        lanes.push(values);
    }
    if pos != blob.len() {
        return Err("trailing CIXM6 record bytes".into());
    }
    m6_reconstruct_record(lanes, width, source_len)
}

fn m6_reconstruct_record(
    lanes: Vec<Vec<u8>>,
    width: usize,
    source_len: usize,
) -> Result<Vec<u8>, String> {
    let mut cursors = vec![0usize; width];
    let mut out = vec![0u8; source_len];
    for (index, byte) in out.iter_mut().enumerate() {
        let lane = index % width;
        *byte = *lanes[lane]
            .get(cursors[lane])
            .ok_or("truncated CIXM6 record lane")?;
        cursors[lane] += 1;
    }
    Ok(out)
}

fn m6_finish_stream<R: Read>(
    src: &mut R,
    fd: i32,
    total: usize,
    crc: &crc32fast::Hasher,
    footer_allowance: usize,
) -> Result<(), String> {
    let expected = read_stream_varint(src, fd)?;
    let mut checksum = [0u8; 4];
    read_exact_poll(src, &mut checksum, fd)?;
    if expected != total || u32::from_le_bytes(checksum) != crc.clone().finalize() {
        return Err("CIXM6 length or CRC32 checksum mismatch".into());
    }
    m6_validate_footer_tail(&m6_read_footer_tail(src, fd, footer_allowance)?)
}

fn m6_read_footer_tail<R: Read>(
    src: &mut R,
    fd: i32,
    footer_allowance: usize,
) -> Result<Vec<u8>, String> {
    const MAX_TAIL: usize = 1024 * 1024 + 9;
    let mut tail = Vec::new();
    let mut scratch = [0u8; 4096];
    loop {
        check_interrupted()?;
        let read_limit = m6_footer_read_limit(tail.len(), tail.capacity(), scratch.len());
        let read = match read_poll(src, &mut scratch[..read_limit], fd, None)? {
            Some(read) => read,
            None => continue,
        };
        if read == 0 {
            break;
        }
        m6_append_footer_tail(&mut tail, &scratch[..read], footer_allowance, MAX_TAIL)?;
    }
    Ok(tail)
}

fn m6_footer_read_limit(tail_len: usize, tail_capacity: usize, scratch_len: usize) -> usize {
    if tail_len == 0 || tail_len == tail_capacity {
        // Probe EOF with a stack byte. Any input here would require a retained
        // allocation beyond the admitted footer capacity.
        1
    } else {
        scratch_len.min(tail_capacity - tail_len)
    }
}

fn m6_append_footer_tail(
    tail: &mut Vec<u8>,
    bytes: &[u8],
    footer_allowance: usize,
    max_tail: usize,
) -> Result<(), String> {
    if tail.is_empty() {
        let capacity = footer_allowance.min(max_tail);
        if capacity < 9 {
            return Err("CIXF1 filename footer exceeds remaining decode memory".into());
        }
        tail.try_reserve_exact(capacity)
            .map_err(|_| "CIXF1 filename footer allocation failed")?;
        if tail.capacity() > footer_allowance || tail.capacity() > max_tail {
            return Err("CIXF1 filename footer exceeds remaining decode memory".into());
        }
    }
    let new_len = tail.len().saturating_add(bytes.len());
    if new_len > max_tail {
        return Err("CIXF1 filename footer exceeds limit".into());
    }
    if new_len > tail.capacity() {
        return Err("CIXF1 filename footer exceeds remaining decode memory".into());
    }
    tail.extend_from_slice(bytes);
    Ok(())
}

fn m6_validate_footer_tail(tail: &[u8]) -> Result<(), String> {
    if tail.is_empty() {
        return Ok(());
    }
    if tail.len() < 9 || &tail[tail.len() - 5..] != b"CIXF1" {
        return Err("trailing bytes after CIXM6 stream".into());
    }
    let name_len =
        u32::from_le_bytes(tail[tail.len() - 9..tail.len() - 5].try_into().unwrap()) as usize;
    if name_len != tail.len() - 9 || std::str::from_utf8(&tail[..name_len]).is_err() {
        return Err("invalid CIXF1 filename footer".into());
    }
    Ok(())
}

fn m6_model_budget(mode: u8, source_len: usize, history: &[u8], available: usize) -> usize {
    let trained = source_len.saturating_add(
        history
            .len()
            .min(65536)
            .min(4096usize.max(source_len.saturating_mul(2))),
    );
    match mode {
        10 => source_len.saturating_mul(64),
        2 => source_len.saturating_mul(32),
        6 => source_len.saturating_mul(8),
        8 => source_len.saturating_mul(16),
        16 => route_minimum_memory(8, trained),
        17 => source_len
            .saturating_mul(9)
            .saturating_mul(EXPERT_CONTEXT_MEMORY_ESTIMATE)
            .min(available),
        7 | 3 | 4 => source_len.saturating_mul(128),
        18 => source_len.saturating_mul(4),
        19 => route_minimum_memory(9, trained),
        5 => source_len.saturating_mul(64),
        13 => stateful::WORKING_MEMORY_BOUND,
        _ => 0,
    }
}

fn m6_decode_runs(blob: &[u8], source_len: usize, model_limit: usize) -> Result<Vec<u8>, String> {
    if blob.first() != Some(&1) {
        return Err("unsupported CIXM6 run block version".into());
    }
    let mut pos = 1;
    let run_count = unvar(blob, &mut pos)?;
    if run_count == 0 || run_count > source_len {
        return Err("invalid CIXM6 run count".into());
    }
    let symbol_len = unvar(blob, &mut pos)?;
    let end = pos
        .checked_add(symbol_len)
        .ok_or("run symbol length overflow")?;
    if end > blob.len() {
        return Err("truncated CIXM6 run symbols".into());
    }
    let mut symbol_pos = 0;
    let symbols = rank::decode_type_class(&blob[pos..end], &mut symbol_pos, run_count)?;
    if symbol_pos != symbol_len {
        return Err("unused CIXM6 run-symbol bytes".into());
    }
    let lengths = m6_decode_uints(&blob[end..], run_count, model_limit)?;
    let mut out = Vec::with_capacity(source_len);
    for (symbol, minus_one) in symbols.into_iter().zip(lengths) {
        let n = minus_one.checked_add(1).ok_or("run length overflow")?;
        if out
            .len()
            .checked_add(n)
            .filter(|&v| v <= source_len)
            .is_none()
        {
            return Err("CIXM6 runs exceed source length".into());
        }
        out.resize(out.len() + n, symbol);
    }
    if out.len() != source_len {
        return Err("CIXM6 run source length mismatch".into());
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct M6DecodeAdmission {
    memory: usize,
    history_limit: usize,
    reference_budget: usize,
}

fn m6_admit_decode_block(
    mode: u8,
    source_len: usize,
    payload_len: usize,
    history: &[u8],
    admission: M6DecodeAdmission,
) -> Result<usize, String> {
    let available_model_budget = admission
        .memory
        .saturating_sub(source_len)
        .saturating_sub(payload_len)
        .saturating_sub(admission.history_limit)
        .saturating_sub(admission.reference_budget)
        .saturating_sub(stateful::RETAINED_MEMORY_BOUND);
    let model_budget = m6_model_budget(mode, source_len, history, available_model_budget);
    if source_len == 0
        || source_len > 1024 * 1024
        || payload_len > 8 * source_len + 4096
        || source_len
            .saturating_add(payload_len)
            .saturating_add(admission.history_limit)
            .saturating_add(admission.reference_budget)
            .saturating_add(stateful::RETAINED_MEMORY_BOUND)
            .saturating_add(model_budget)
            > admission.memory
    {
        return Err("CIXM6 block exceeds memory or format limit".into());
    }
    Ok(model_budget)
}

struct M6DecodeContext<'a> {
    history: &'a [u8],
    recent: &'a std::collections::VecDeque<Vec<u8>>,
    memory: usize,
    model_budget: usize,
    persistent_states: &'a mut stateful::PersistentAdaptiveStates,
}

fn decode_m6_block(
    mode: u8,
    source_len: usize,
    payload: Vec<u8>,
    context: M6DecodeContext<'_>,
) -> Result<Vec<u8>, String> {
    Ok(match mode {
        0 => {
            if payload.len() != source_len {
                return Err("CIXM6 raw block length mismatch".into());
            }
            payload
        }
        1 => {
            let mut pos = 0;
            let out = rank::decode_type_class(&payload, &mut pos, source_len)?;
            if pos != payload.len() {
                return Err("unused CIXM6 multinomial bytes".into());
            }
            out
        }
        2 => rank::decode_context_type_classes(&payload, source_len, context.history)?,
        3 => legacy_lz::decode(&payload, source_len, context.history, context.memory / 4096)?,
        4 => bwt_legacy::decode(&payload, source_len, context.memory / 4096)?,
        6 => bitplane::decode(&payload, source_len)?,
        7 => chain::decode(&payload, source_len, context.memory / 4096)?,
        10 => {
            adaptive::decode_general(&payload, source_len, context.history, context.memory / 4096)?
        }
        13 => context.persistent_states.decode(
            &payload,
            source_len,
            context.history,
            stateful::WORKING_MEMORY_BOUND,
        )?,
        11 => rank::decode_sparse_blob(&payload, source_len)?,
        12 => m6_decode_runs(&payload, source_len, context.memory / 4096)?,
        14 => m6_decode_record(&payload, source_len, context.memory / 4096)?,
        5 => transform::decode(&payload, source_len)?,
        15 => m6_decode_periodic(&payload, source_len, context.memory / 4096)?,
        8 => numeric::decode(&payload, source_len)?,
        16 => ppm::decode_with_history(&payload, source_len, context.history)?,
        17 => expertmix::decode_bounded(
            &payload,
            source_len,
            context.history,
            context.model_budget / EXPERT_CONTEXT_MEMORY_ESTIMATE,
        )?,
        18 => wavelet::decode(&payload, source_len)?,
        20 => histogram::decode_type_class(&payload, source_len)?,
        19 => fixedmix::decode_with_history(&payload, source_len, context.history)?,
        9 => {
            let mut pos = 0;
            let distance = unvar(&payload, &mut pos)?;
            if pos != payload.len() || distance == 0 || distance > context.recent.len() {
                return Err("invalid CIXM6 block reference".into());
            }
            let out = context.recent[context.recent.len() - distance].clone();
            if out.len() != source_len {
                return Err("CIXM6 reference length mismatch".into());
            }
            out
        }
        other => return Err(format!("unsupported CIXM6 block mode {other}")),
    })
}

struct M6DecodeOptions {
    fd: i32,
    memory: usize,
    verify_only: bool,
}

fn decode_m6<R: Read, W: Write>(
    mut src: R,
    mut dst: W,
    options: M6DecodeOptions,
) -> Result<(), String> {
    let M6DecodeOptions {
        fd,
        memory,
        verify_only,
    } = options;
    let history_limit = read_stream_varint(&mut src, fd)?;
    if history_limit == 0 || history_limit > 16 * 1024 * 1024 {
        return Err("invalid CIXM6 history window".into());
    }
    let mut total = 0usize;
    let mut crc = crc32fast::Hasher::new();
    if history_limit > memory {
        return Err("CIXM6 history exceeds --memory".into());
    }
    let mut history: Vec<u8> = Vec::with_capacity(history_limit);
    let mut persistent_states = stateful::PersistentAdaptiveStates::new();
    let mut recent: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::new();
    let mut recent_bytes = 0usize;
    let reference_budget = memory.saturating_div(8).clamp(1, 16 * 1024 * 1024);
    let retained_decode_state = history_limit
        .saturating_add(reference_budget)
        .saturating_add(stateful::RETAINED_MEMORY_BOUND);
    if retained_decode_state > memory {
        return Err("CIXM6 history, references, and persistent models exceed --memory".into());
    }
    let admission = M6DecodeAdmission {
        memory,
        history_limit,
        reference_budget,
    };
    loop {
        let mut mode = [0u8; 1];
        read_exact_poll(&mut src, &mut mode, fd)?;
        if mode[0] == 255 {
            m6_finish_stream(
                &mut src,
                fd,
                total,
                &crc,
                memory.saturating_sub(retained_decode_state),
            )?;
            break;
        }
        let source_len = read_stream_varint(&mut src, fd)?;
        let payload_len = read_stream_varint(&mut src, fd)?;
        let model_budget =
            m6_admit_decode_block(mode[0], source_len, payload_len, &history, admission)?;
        let mut payload = vec![0u8; payload_len];
        read_exact_poll(&mut src, &mut payload, fd)?;
        let block = decode_m6_block(
            mode[0],
            source_len,
            payload,
            M6DecodeContext {
                history: &history,
                recent: &recent,
                memory,
                model_budget,
                persistent_states: &mut persistent_states,
            },
        )?;
        if block.len() != source_len {
            return Err("CIXM6 decoded block length mismatch".into());
        }
        persistent_states.observe(&block, &history)?;
        crc.update(&block);
        append_bounded_history(&mut history, &block, history_limit);
        total = total
            .checked_add(block.len())
            .ok_or("CIXM6 output length overflow")?;
        if !verify_only {
            dst.write_all(&block).map_err(ioerr)?;
            dst.flush().map_err(ioerr)?;
        }
        recent_bytes = recent_bytes.saturating_add(block.len());
        recent.push_back(block);
        while recent.len() > 4096 || recent_bytes > reference_budget {
            recent_bytes = recent_bytes.saturating_sub(recent.pop_front().unwrap().len());
        }
    }
    Ok(())
}

struct CixgHeader {
    magic: [u8; 5],
    block: u32,
}

fn read_cixg_header<R: Read>(src: &mut R, magic: [u8; 5], fd: i32) -> Result<CixgHeader, String> {
    let mut header = [0; HEADER];
    header[..5].copy_from_slice(&magic);
    read_exact_poll(src, &mut header[5..], fd)?;
    if &header[..5] != MAGIC && &header[..5] != MAGIC_V2 {
        return Err("unsupported archive (expected CIXG1 or CIXG2)".into());
    }
    let block = u32::from_le_bytes(header[5..9].try_into().unwrap());
    let history = u32::from_le_bytes(header[9..13].try_into().unwrap());
    if block == 0 || block > MAX_BLOCK || history != 0 && history != 65536 {
        return Err("invalid CIXG header".into());
    }
    Ok(CixgHeader { magic, block })
}

fn decode_cixb1<R: Read, W: Write>(
    mut src: R,
    mut dst: W,
    magic: [u8; 5],
    verify_only: bool,
    list: bool,
    fd: i32,
    memory: usize,
) -> Result<(), String> {
    let mut fields = [0u8; 42];
    read_exact_poll(&mut src, &mut fields, fd)?;
    let payload_size = u32::from_le_bytes(fields[6..10].try_into().unwrap()) as usize;
    let expected = u32::from_le_bytes(fields[2..6].try_into().unwrap()) as usize;
    if payload_size > external::MAX_PAYLOAD
        || expected > external::MAX_INPUT
        || 47usize
            .saturating_add(expected)
            .saturating_add(payload_size)
            > memory
    {
        return Err("CIXB1 archive exceeds --memory or format limit".into());
    }
    let mut archive = Vec::with_capacity(47 + payload_size);
    archive.extend_from_slice(&magic);
    archive.extend_from_slice(&fields);
    archive.resize(47 + payload_size, 0);
    read_exact_poll(&mut src, &mut archive[47..], fd)?;
    let data = external::decode(
        &archive,
        memory
            .checked_sub(archive.capacity())
            .ok_or("CIXB1 archive exceeds --memory or format limit")?,
    )?;
    loop {
        let mut extra = [0u8; 1];
        match read_poll(&mut src, &mut extra, fd, None)? {
            Some(0) => break,
            Some(_) => return Err("trailing bytes after CIXB1 archive".into()),
            None => continue,
        }
    }
    if list {
        eprintln!(
            "CIXB1 stream: backend={}, profile={}, decoded {} bytes",
            archive[5],
            archive[6],
            data.len()
        );
    }
    if !verify_only {
        dst.write_all(&data).map_err(ioerr)?;
        dst.flush().map_err(ioerr)?;
    }
    Ok(())
}

struct CixgFinish<'a> {
    fd: i32,
    verify_only: bool,
    list: bool,
    magic: &'a [u8; 5],
    total: u64,
    n: u32,
    s: u32,
    sum: [u8; 32],
    whole: &'a Sha256,
}

fn finish_cixg<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    finish: CixgFinish<'_>,
) -> Result<(), String> {
    let CixgFinish {
        fd,
        verify_only,
        list,
        magic,
        total,
        n,
        s,
        sum,
        whole,
    } = finish;
    if n != 0 || s != 0 || sum != <[u8; 32]>::from(whole.clone().finalize()) {
        return Err("archive checksum/terminator mismatch".into());
    }
    loop {
        let mut extra = [0];
        match read_poll(src, &mut extra, fd, None)? {
            Some(0) => break,
            Some(_) => return Err("trailing archive bytes".into()),
            None => continue,
        }
    }
    if !verify_only {
        dst.flush().map_err(ioerr)?;
    }
    if list {
        eprintln!(
            "{} stream: {} decoded bytes",
            std::str::from_utf8(magic).unwrap_or("CIXG?"),
            total
        );
    }
    Ok(())
}

fn read_cixg_frame<R: Read>(src: &mut R, fd: i32) -> Result<(u8, u32, u32, [u8; 32]), String> {
    let mut route = [0; 1];
    read_exact_poll(src, &mut route, fd)?;
    let n = read_u32(src, fd)?;
    let s = read_u32(src, fd)?;
    let mut sum = [0; 32];
    read_exact_poll(src, &mut sum, fd)?;
    Ok((route[0], n, s, sum))
}

fn read_cixg_payload<R: Read>(
    src: &mut R,
    fd: i32,
    block: u32,
    n: u32,
    s: u32,
    memory: usize,
) -> Result<Vec<u8>, String> {
    if n == 0 || n > block || s as u64 > 8 * n as u64 + 4096 {
        return Err("invalid block descriptor".into());
    }
    let fixed = (n as usize)
        .checked_add(s as usize)
        .ok_or("CIXG frame buffer size overflow")?;
    if fixed > memory {
        return Err(format!(
            "CIXG frame needs {fixed} bytes for payload and output; raise --memory"
        ));
    }
    let mut payload = vec![0; s as usize];
    read_exact_poll(src, &mut payload, fd)?;
    Ok(payload)
}

fn decode_cixg_route(
    route: u8,
    payload: Vec<u8>,
    length: usize,
    extended: bool,
) -> Result<Vec<u8>, String> {
    match route {
        0 => {
            if payload.len() != length {
                return Err("raw size mismatch".into());
            }
            Ok(payload)
        }
        1 => unrle(&payload, length),
        2 | 10 => stream_decode(&payload, length, extended),
        3 => lz_decode(&payload, length, extended),
        4 => Ok(predictor_residual(
            &stream_decode(&payload, length, extended)?,
            true,
        )),
        5 => {
            if payload.len() < 4 {
                return Err("truncated BWT primary index".into());
            }
            let primary = u32::from_le_bytes(payload[..4].try_into().unwrap()) as usize;
            let parts = streams_decode(&payload[4..], length.saturating_mul(4), extended)?;
            bwt_inverse(primary, &unmtf_runs(&parts, length)?)
        }
        6 => {
            if payload.is_empty() {
                return Err("truncated stride descriptor".into());
            }
            let restored =
                invert_stride(&join_streams(&payload[1..], length, extended)?, payload[0])?;
            if restored.len() != length {
                return Err("stride output size mismatch".into());
            }
            Ok(restored)
        }
        7 => phrase_decode(&payload, length, extended),
        8 => ppm::decode(&payload, length),
        9 => fixedmix::decode(&payload, length),
        _ => Err(format!("unsupported CIXG1 route ID {route}")),
    }
}

fn check_cixg_live_memory(
    fixed: usize,
    route: usize,
    nested: usize,
    memory: usize,
    route_id: u8,
) -> Result<(), String> {
    let total = fixed
        .checked_add(route)
        .and_then(|bytes| bytes.checked_add(nested))
        .ok_or("CIXG decode memory estimate overflow")?;
    if total > memory {
        return Err(format!("CIXG {} block needs about {total} bytes including frame buffers and model state; raise --memory", selection::ROUTES[route_id as usize]));
    }
    Ok(())
}

fn cixg_substream_workspace(
    coder: u8,
    decoded: usize,
    payload: &[u8],
    allow_extended_coders: bool,
) -> Result<usize, String> {
    Ok(match coder {
        0 | 1 => 0,
        // Rank, adaptive order-0, DEFLATE and canonical Huffman
        // all have bounded temporary tables under this reserve.
        2 | 3 | 4 | 5 | 7 => 64 * 1024,
        6 => {
            if !allow_extended_coders {
                return Err("CIXG1 cannot contain versioned context-range substreams".into());
            }
            if payload.len() < 4 || payload[0] != 1 {
                return Err("invalid context-range substream header".into());
            }
            let order = payload[1] as usize;
            let bits = payload[2] as usize;
            if !(1..=16).contains(&order) || !(1..=8).contains(&bits) {
                return Err("context-range parameters exceed version-1 bounds".into());
            }
            let models = decoded.min(1usize << bits);
            models
                .checked_mul(4096)
                .and_then(|bytes| bytes.checked_add(64 * 1024))
                .ok_or("context-range model memory estimate overflow")?
        }
        _ => return Err(format!("unsupported CIX substream coder {coder}")),
    })
}

fn cixg_nested_stream_scratch(
    payload: &[u8],
    offset: usize,
    limit: usize,
    allow_extended_coders: bool,
) -> Result<usize, String> {
    let stream = payload.get(offset..).ok_or("truncated CIX stream prefix")?;
    if stream.is_empty() || stream[0] == 0 || stream[0] > 8 {
        return Err("invalid CIX substream count".into());
    }
    let mut pos = 1usize;
    let mut total = 0usize;
    let mut maximum = 0usize;
    for _ in 0..stream[0] {
        let descriptor_end = pos
            .checked_add(9)
            .ok_or("CIX substream descriptor overflow")?;
        if descriptor_end > stream.len() {
            return Err("truncated CIX substream descriptor".into());
        }
        let coder = stream[pos];
        let decoded = u32::from_le_bytes(stream[pos + 1..pos + 5].try_into().unwrap()) as usize;
        let encoded = u32::from_le_bytes(stream[pos + 5..pos + 9].try_into().unwrap()) as usize;
        pos = descriptor_end;
        total = total
            .checked_add(decoded)
            .ok_or("CIX substream length overflow")?;
        let end = pos
            .checked_add(encoded)
            .ok_or("CIX substream payload overflow")?;
        if total > limit || end > stream.len() {
            return Err("CIX substream resource/truncation limit".into());
        }
        let payload = &stream[pos..end];
        let workspace = cixg_substream_workspace(coder, decoded, payload, allow_extended_coders)?;
        maximum = maximum.max(workspace);
        pos = end;
    }
    if pos != stream.len() {
        return Err("trailing CIX substream bytes".into());
    }
    Ok(maximum)
}

fn cixg_route_scratch(
    route: u8,
    source_bytes: usize,
    payload: &[u8],
    allow_extended_coders: bool,
) -> Result<usize, String> {
    let source_scratch = |multiple: usize, reserve: usize, route: &str| {
        source_bytes
            .checked_mul(multiple)
            .and_then(|bytes| bytes.checked_add(reserve))
            .ok_or_else(|| format!("{route} decode memory estimate overflow"))
    };
    Ok(match route {
        // Stream decoding may hold decoded substreams while it validates
        // descriptors and returns the final part.  Keep one source-sized
        // temporary beyond the fixed payload/output frame buffers.
        2 | 10 => source_scratch(1, 0, "CIX substream")?,
        // LZ holds up to four decoded side streams and reconstructed
        // output while parsing match descriptors.
        3 => source_scratch(5, 0, "LZ")?,
        // Predictor reconstruction retains its residual until the output
        // is complete, plus two fixed 64 KiB context tables.
        4 => source_scratch(1, 128 * 1024, "predictor")?,
        // BWT materializes decoded streams and transformed bytes, then
        // allocates `last` and `first` (u8, usize) tables and an inverse
        // usize table.  48n + 64 KiB is deliberately conservative on
        // supported 64-bit hosts, and is an admission estimate rather
        // than an RSS claim.
        5 => source_scratch(48, 64 * 1024, "BWT")?,
        // Lane and phrase routes retain side streams and a reconstructed
        // intermediate while producing the final bytes.
        6 | 7 => source_scratch(3, 0, "structured stream")?,
        8 => {
            // Validate this fixed header before `ppm::decode` creates its
            // context table.  The model estimate reserves 2 KiB/source
            // byte plus 8 MiB for variable-size map/table allocation;
            // order 8 needs 9 context-key/counter lanes and is charged
            // at 9 * 256 bytes/source instead.  The reserve also covers
            // the v5 SEE side table.
            let (&version, &order) = match payload {
                [version, order, ..] => (version, order),
                _ => return Err("truncated PPM payload header".into()),
            };
            if !matches!(version, 2..=5) {
                return Err("unsupported PPM payload version".into());
            }
            if order > 8 {
                return Err("unsupported PPM order".into());
            }
            if !allow_extended_coders && order > 3 {
                return Err("CIXG1 cannot contain extended PPM orders".into());
            }
            let bytes_per_source = 2048usize.max(
                usize::from(order)
                    .checked_add(1)
                    .and_then(|contexts| contexts.checked_mul(256))
                    .ok_or("PPM order memory estimate overflow")?,
            );
            source_bytes
                .checked_mul(bytes_per_source)
                .and_then(|bytes| bytes.checked_add(8 * 1024 * 1024))
                .ok_or("PPM model memory estimate overflow")?
        }
        9 => {
            if !allow_extended_coders && payload.first() != Some(&1) {
                return Err("CIXG1 cannot contain extended mixture payloads".into());
            }
            // Fixed-mix has several adaptive maps; use the same dynamic
            // reservation as the encoder, in addition to frame buffers.
            source_bytes
                .checked_mul(4096)
                .and_then(|bytes| bytes.checked_add(8 * 1024 * 1024))
                .ok_or("mixture model memory estimate overflow")?
        }
        // Raw and RLE route output is already included in
        // `fixed_frame_bytes`; they need no second route buffer.
        0 | 1 => 0,
        _ => return Err(format!("unsupported CIXG1 route ID {}", route)),
    })
}

fn cixg_route_nested_scratch(
    route: u8,
    payload: &[u8],
    source_bytes: usize,
    allow_extended_coders: bool,
) -> Result<usize, String> {
    Ok(match route {
        2 | 4 | 10 => cixg_nested_stream_scratch(payload, 0, source_bytes, allow_extended_coders)?,
        3 => cixg_nested_stream_scratch(
            payload,
            0,
            source_bytes.saturating_mul(4),
            allow_extended_coders,
        )?,
        5 => cixg_nested_stream_scratch(
            payload,
            4,
            source_bytes.saturating_mul(4),
            allow_extended_coders,
        )?,
        6 => cixg_nested_stream_scratch(payload, 1, source_bytes, allow_extended_coders)?,
        7 => {
            let dictionary_entries =
                *payload.first().ok_or("truncated phrase dictionary")? as usize;
            if dictionary_entries > 32 {
                return Err("invalid phrase dictionary size".into());
            }
            let offset = 1usize
                .checked_add(
                    dictionary_entries
                        .checked_mul(2)
                        .ok_or("phrase dictionary size overflow")?,
                )
                .ok_or("phrase dictionary size overflow")?;
            cixg_nested_stream_scratch(
                payload,
                offset,
                source_bytes.saturating_mul(2),
                allow_extended_coders,
            )?
        }
        _ => 0,
    })
}

pub(crate) fn decode<R: Read, W: Write>(
    mut src: R,
    mut dst: W,
    verify_only: bool,
    list: bool,
    input_fd: i32,
    memory: usize,
) -> Result<(), String> {
    let mut archive_magic = [0u8; 5];
    read_exact_poll(&mut src, &mut archive_magic, input_fd)?;
    if &archive_magic == b"CIXM6" || &archive_magic == b"CIXM5" {
        return decode_m6(
            src,
            dst,
            M6DecodeOptions {
                fd: input_fd,
                memory,
                verify_only,
            },
        );
    }
    if &archive_magic == b"CIXB1" {
        return decode_cixb1(src, dst, archive_magic, verify_only, list, input_fd, memory);
    }
    if &archive_magic == streaming_backend::MAGIC {
        return streaming_backend::decode(src, dst, verify_only, list, input_fd, memory);
    }
    let header = read_cixg_header(&mut src, archive_magic, input_fd)?;
    let block = header.block;
    let allow_extended_coders = &header.magic == MAGIC_V2;
    let mut whole = Sha256::new();
    let mut total_bytes = 0u64;
    loop {
        let (route, n, s, sum) = read_cixg_frame(&mut src, input_fd)?;
        if route == END {
            finish_cixg(
                &mut src,
                &mut dst,
                CixgFinish {
                    fd: input_fd,
                    verify_only,
                    list,
                    magic: &archive_magic,
                    total: total_bytes,
                    n,
                    s,
                    sum,
                    whole: &whole,
                },
            )?;
            return Ok(());
        }
        let b = read_cixg_payload(&mut src, input_fd, block, n, s, memory)?;
        let fixed_frame_bytes = n as usize + s as usize;
        // The payload remains live while route decoders build their output.
        // Charge both buffers before allocating the attacker-controlled
        // payload.  Per-route state is added below before a route can create
        // an unbounded context table.
        let source_bytes = n as usize;
        // Inspect only self-describing CIX stream descriptors.  This happens
        // before `streams_decode` allocates any substream output or coder
        // model.  A stream processes coders serially, so reserve the largest
        // single coder workspace; route scratch above separately covers the
        // retained decoded parts.
        let route_scratch = cixg_route_scratch(route, source_bytes, &b, allow_extended_coders)?;
        let nested_coder_scratch =
            cixg_route_nested_scratch(route, &b, source_bytes, allow_extended_coders)?;
        check_cixg_live_memory(
            fixed_frame_bytes,
            route_scratch,
            nested_coder_scratch,
            memory,
            route,
        )?;
        let data = decode_cixg_route(route, b, n as usize, allow_extended_coders)?;
        if <[u8; 32]>::from(Sha256::digest(&data)) != sum {
            return Err("block checksum mismatch".into());
        }
        whole.update(&data);
        total_bytes = total_bytes
            .checked_add(data.len() as u64)
            .ok_or("decoded length overflow")?;
        if list {
            eprintln!(
                "{} block: route={}, source={} bytes, payload={} bytes",
                std::str::from_utf8(&archive_magic).unwrap_or("CIXG?"),
                route,
                n,
                s
            )
        }
        if !verify_only {
            dst.write_all(&data).map_err(ioerr)?;
            dst.flush().map_err(ioerr)?;
        }
    }
}

type SquashRead = unsafe extern "C" fn(*mut usize, *mut u8, *mut std::ffi::c_void) -> i32;
type SquashWrite = unsafe extern "C" fn(*mut usize, *const u8, *mut std::ffi::c_void) -> i32;
struct SquashReader {
    callback: SquashRead,
    user: *mut std::ffi::c_void,
}
struct SquashWriter {
    callback: SquashWrite,
    user: *mut std::ffi::c_void,
}
impl Read for SquashReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut size = buffer.len();
        let status = unsafe { (self.callback)(&mut size, buffer.as_mut_ptr(), self.user) };
        if status == 3 {
            return Ok(0);
        }
        if status != 1 || size > buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Squash input callback failed",
            ));
        }
        Ok(size)
    }
}
impl Write for SquashWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut size = buffer.len();
        let status = unsafe { (self.callback)(&mut size, buffer.as_ptr(), self.user) };
        if status != 1 || size > buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Squash output callback failed",
            ));
        }
        if size == 0 && !buffer.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "Squash output callback wrote no bytes",
            ));
        }
        Ok(size)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[no_mangle]
/// Adapts CIX's bounded stream encoder and decoder to the Squash callback ABI.
///
/// # Safety
/// `read_cb` and `write_cb`, when supplied, must be valid callbacks for the
/// duration of this call. `user` must be a pointer that both callbacks accept;
/// CIX never dereferences it itself. Callbacks must uphold their documented
/// buffer and length contracts and must not unwind through this FFI boundary.
pub unsafe extern "C" fn cix_squash_splice(
    direction: i32,
    level: i32,
    read_cb: Option<SquashRead>,
    write_cb: Option<SquashWrite>,
    user: *mut std::ffi::c_void,
) -> i32 {
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
            let read = read_cb.ok_or("missing Squash read callback")?;
            let write = write_cb.ok_or("missing Squash write callback")?;
            let src = SquashReader {
                callback: read,
                user,
            };
            let dst = SquashWriter {
                callback: write,
                user,
            };
            match direction {
                1 if (1..=9).contains(&level) => encode(
                    src,
                    dst,
                    EncodeOptions {
                        block: default_block(level as u8),
                        level: level as u8,
                        forced: None,
                        backend: "hybrid",
                        backend_set: false,
                        format: "auto",
                        input_fd: -1,
                        flush_interval: None,
                        memory: 512 * 1024 * 1024,
                        workers: 1,
                        explain: false,
                        verbose: false,
                        strategy: "blocks",
                    },
                ),
                1 => Err("invalid Squash CIX compression level".into()),
                2 => decode(src, dst, false, false, -1, 512 * 1024 * 1024),
                _ => Err("invalid Squash stream direction".into()),
            }
        }));
    match result {
        Ok(Ok(())) => 1,
        _ => -1,
    }
}

pub(crate) fn ioerr(e: io::Error) -> String {
    e.to_string()
}

/// Low-level CIXZ1 request.  The CLI decides when this path is eligible; this
/// codec helper only consumes the supplied stream and explicit parameters.
pub(crate) struct PersistentStreamRequest {
    pub(crate) block: u32,
    pub(crate) input_fd: i32,
    pub(crate) flush_interval: Option<Duration>,
    pub(crate) memory: usize,
    pub(crate) workers: usize,
    pub(crate) explain: bool,
    pub(crate) verbose: bool,
}

pub(crate) fn encode_persistent_stream<R: Read, W: Write>(
    mut src: R,
    dst: W,
    request: PersistentStreamRequest,
) -> Result<(), String> {
    let PersistentStreamRequest {
        block,
        input_fd,
        flush_interval,
        memory,
        workers,
        explain,
        verbose,
    } = request;
    let (first, first_eof) =
        streaming_backend::read_probe(&mut src, block, input_fd, flush_interval)?;
    // Observability belongs to the CLI adapter.  The codec receives the
    // flags because streaming candidate selection uses them internally, but
    // it never writes a diagnostic on behalf of an embedding caller.
    streaming_backend::encode(
        src,
        dst,
        streaming_backend::EncodeRequest {
            first,
            first_eof,
            block,
            level: 9,
            input_fd,
            flush_interval,
            memory,
            workers,
            explain,
            verbose,
        },
    )
}

pub(crate) fn persistent_stream_required_memory(block: u32) -> usize {
    streaming_backend::required_memory(block)
}

#[cfg(test)]
mod fast_path_tests {
    use super::*;

    #[test]
    fn fast_lz_uses_production_substreams_and_decoder() {
        let mut random = Vec::new();
        let mut state = 0x1367_ac48u32;
        for _ in 0..MAX_BLOCK {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            random.push((state >> 24) as u8);
        }
        let mut mixed = random[..16384].to_vec();
        mixed.extend_from_slice(&b"generic repeated dictionary records ".repeat(1200));
        let cases = [
            Vec::new(),
            b"a".to_vec(),
            b"abcde".to_vec(),
            vec![0x42; MAX_BLOCK as usize],
            random,
            mixed,
        ];
        for input in cases {
            for backend in ["deflate", "cix", "range", "huffman"] {
                let payload = lz_encode(&input, 1, backend).unwrap();
                assert_eq!(lz_decode(&payload, input.len(), true).unwrap(), input);
            }
            // Existing parser/wire representation remains readable.
            let old = lz_encode(&input, 6, "deflate").unwrap();
            assert_eq!(lz_decode(&old, input.len(), false).unwrap(), input);
        }
    }

    #[test]
    fn cli_and_squash_share_default_block_and_fast_admission() {
        assert_eq!(default_block(1), 65536);
        assert_eq!(default_block(6), 32768);
        assert_eq!(default_block(9), 65536);
        assert!(candidate_memory(3, 65536, 1, "deflate") < 3 << 20);
        assert_eq!(
            candidate_memory(3, 65536, 6, "deflate"),
            route_minimum_memory(3, 65536)
        );
    }
}

#[cfg(test)]
mod cixg_huffman_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn forced_route_raw_fallback_is_tagged_raw() {
        let input: Vec<u8> = (0u8..64).collect();
        let mut archive = Vec::new();
        encode(
            Cursor::new(input.clone()),
            &mut archive,
            EncodeOptions {
                block: 1024,
                level: 9,
                forced: Some("runs"),
                backend: "hybrid",
                backend_set: false,
                format: "cixg1",
                input_fd: -1,
                flush_interval: None,
                memory: 128 * 1024 * 1024,
                workers: 1,
                explain: false,
                verbose: false,
                strategy: "blocks",
            },
        )
        .unwrap();
        // Header is 13 bytes; the next byte is the actual block route ID.
        assert_eq!(
            archive[HEADER], 0,
            "raw fallback must not be mislabeled runs"
        );
        let mut restored = Vec::new();
        decode(
            Cursor::new(archive),
            &mut restored,
            false,
            false,
            -1,
            128 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(restored, input);
    }

    #[test]
    fn cixg2_huffman_substream_round_trips_and_cixg1_rejects_it() {
        let input = vec![b'Q'; 4096];
        assert!(encode(
            Cursor::new(input.clone()),
            Vec::new(),
            EncodeOptions {
                block: 8192,
                level: 9,
                forced: Some("composition"),
                backend: "huffman",
                backend_set: true,
                format: "cixg1",
                input_fd: -1,
                flush_interval: None,
                memory: 128 * 1024 * 1024,
                workers: 1,
                explain: false,
                verbose: false,
                strategy: "blocks"
            }
        )
        .unwrap_err()
        .contains("requires --format cixg2"));
        let mut archive = Vec::new();
        encode(
            Cursor::new(input.clone()),
            &mut archive,
            EncodeOptions {
                block: 8192,
                level: 9,
                forced: Some("composition"),
                backend: "huffman",
                backend_set: true,
                format: "cixg2",
                input_fd: -1,
                flush_interval: None,
                memory: 128 * 1024 * 1024,
                workers: 1,
                explain: false,
                verbose: false,
                strategy: "blocks",
            },
        )
        .unwrap();
        assert_eq!(&archive[..5], MAGIC_V2);
        // Header + CIXG frame + substream count, then its coder ID.
        assert_eq!(archive[HEADER + 41 + 1], 7);
        let mut restored = Vec::new();
        decode(
            Cursor::new(archive.clone()),
            &mut restored,
            false,
            false,
            -1,
            128 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(restored, input);

        let mut invalid_v1 = archive;
        invalid_v1[..5].copy_from_slice(MAGIC);
        assert!(decode(
            Cursor::new(invalid_v1),
            Vec::new(),
            false,
            false,
            -1,
            128 * 1024 * 1024,
        )
        .is_err());
    }
}

#[cfg(test)]
mod bounded_history_tests {
    use super::*;
    #[test]
    fn history_never_grows_past_reserved_capacity() {
        let mut history = Vec::with_capacity(8);
        let cap = history.capacity();
        for input in [b"abcdef".as_slice(), b"ghijk", b"lmnopqrstuv"] {
            append_bounded_history(&mut history, input, 8);
            assert!(history.len() <= 8);
            assert_eq!(history.capacity(), cap);
        }
        assert_eq!(history, b"opqrstuv");
    }
    #[test]
    fn default_workers_shrink_to_fit_but_explicit_workers_do_not() {
        let minimum = parallel_retained(4096, 2) + 65537;
        assert_eq!(automatic_workers(4, false, 4096, minimum), 2);
        assert_eq!(automatic_workers(4, true, 4096, minimum), 4);
        assert_eq!(automatic_workers(4, false, 4096, 131072), 1);
    }

    #[test]
    fn explicit_twenty_workers_remains_requested_when_memory_is_admitted() {
        let memory = parallel_retained(4096, 20) + 65537;
        assert_eq!(automatic_workers(20, true, 4096, memory), 20);
    }
}

#[cfg(test)]
mod timed_block_tests {
    use super::*;
    use std::io::{Cursor, Read};

    struct ChunkedReader {
        inner: Cursor<Vec<u8>>,
        chunk_size: usize,
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let limit = buffer.len().min(self.chunk_size);
            self.inner.read(&mut buffer[..limit])
        }
    }

    #[test]
    fn timed_fill_reports_eof_after_the_available_bytes() {
        let mut source = Cursor::new(b"timed input".to_vec());
        let mut block = [0u8; 32];
        let (got, eof) = fill_timed_block(&mut source, &mut block, -1, None).unwrap();
        assert_eq!(got, 11);
        assert!(eof);
        assert_eq!(&block[..got], b"timed input");
    }

    #[test]
    fn candidate_histogram_only_counts_the_filled_prefix() {
        let mut source = Cursor::new(vec![1, 1, 4]);
        let mut block = [255u8; 8];
        let (got, eof, histogram) =
            fill_candidate_block(&mut source, &mut block, -1, None).unwrap();
        assert_eq!((got, eof), (3, true));
        assert_eq!(histogram[1], 2);
        assert_eq!(histogram[4], 1);
        assert_eq!(histogram[255], 0);
    }

    #[test]
    fn timed_fill_stops_after_the_first_partial_read_when_idle_flush_expires() {
        let mut source = ChunkedReader {
            inner: Cursor::new(b"abcd".to_vec()),
            chunk_size: 2,
        };
        let mut block = [0u8; 8];
        let (got, eof) = m6_fill_block(&mut source, &mut block, -1, Some(Duration::ZERO)).unwrap();
        assert_eq!((got, eof), (2, false));
        assert_eq!(&block[..got], b"ab");
    }
}
