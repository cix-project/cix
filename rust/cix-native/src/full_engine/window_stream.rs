//! CIXW1: bounded, independent whole-window transport.
//!
//! Windows are deliberately independent.  This carrier never represents a
//! retained codec context across records.

use super::dispatch::OperationLimits;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

pub const MAGIC: &[u8; 5] = b"CIXW1";
pub const VERSION: u8 = 1;
const HEADER: usize = 10;
const RECORD: u8 = 1;
const END: u8 = 2;
const RECORD_HEADER: usize = 1 + 8 + 4 + 4 + 32;
const END_BYTES: usize = 1 + 8 + 8 + 32;

#[derive(Clone, Debug)]
pub struct EncodeLimits {
    pub window_bytes: usize,
    pub memory_bytes: usize,
    pub output_bytes: usize,
    pub deadline: Option<Instant>,
    pub cancellation: Option<Arc<AtomicBool>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamSummary {
    pub windows: u64,
    pub input_bytes: u64,
    pub archive_bytes: u64,
    /// Every record starts with an independent codec context. This describes
    /// the transport model; it is not a candidate-omission count.
    pub independent_windows: u64,
}

fn check(l: &EncodeLimits) -> Result<(), String> {
    if l.window_bytes == 0
        || l.window_bytes > u32::MAX as usize
        || l.memory_bytes < l.window_bytes
        || l.output_bytes < HEADER + END_BYTES
    {
        return Err("invalid CIXW1 limits".into());
    }
    if l.cancellation
        .as_ref()
        .is_some_and(|x| x.load(Ordering::Acquire))
    {
        return Err("CIXW1 cancelled".into());
    }
    if l.deadline.is_some_and(|d| Instant::now() >= d) {
        return Err("CIXW1 deadline exceeded".into());
    }
    Ok(())
}
fn put<W: Write>(out: &mut W, bytes: &[u8]) -> Result<(), String> {
    out.write_all(bytes).map_err(|e| e.to_string())
}
pub fn encode<R: Read, W: Write>(
    mut source: R,
    destination: &mut W,
    limits: &EncodeLimits,
    mut choose: impl FnMut(&[u8], usize) -> Result<Vec<u8>, String>,
) -> Result<StreamSummary, String> {
    check(limits)?;
    put(destination, MAGIC)?;
    put(destination, &[VERSION])?;
    put(destination, &(limits.window_bytes as u32).to_le_bytes())?;
    let mut window = Vec::new();
    window
        .try_reserve_exact(limits.window_bytes)
        .map_err(|_| "CIXW1 window allocation")?;
    if window.capacity() > limits.memory_bytes {
        return Err("CIXW1 window allocation exceeds memory limit".into());
    }
    window.resize(limits.window_bytes, 0);
    let mut whole = Sha256::new();
    let mut total = 0u64;
    let mut sequence = 0u64;
    let mut used = HEADER;
    loop {
        check(limits)?;
        let mut filled = 0;
        while filled < window.len() {
            check(limits)?;
            let n = source
                .read(&mut window[filled..])
                .map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        let payload = choose(
            &window[..filled],
            limits
                .memory_bytes
                .checked_sub(window.capacity())
                .ok_or("CIXW1 memory limit")?,
        )?;
        check(limits)?;
        if used
            .checked_add(RECORD_HEADER)
            .and_then(|n| n.checked_add(payload.len()))
            .and_then(|n| n.checked_add(END_BYTES))
            .is_none()
            || payload.len() > u32::MAX as usize
            || payload.len()
                > limits
                    .output_bytes
                    .saturating_sub(used.saturating_add(RECORD_HEADER).saturating_add(END_BYTES))
            || window
                .capacity()
                .checked_add(payload.capacity())
                .is_none_or(|n| n > limits.memory_bytes)
        {
            return Err("CIXW1 payload or memory limit".into());
        }
        put(destination, &[RECORD])?;
        put(destination, &sequence.to_le_bytes())?;
        put(destination, &(filled as u32).to_le_bytes())?;
        put(destination, &(payload.len() as u32).to_le_bytes())?;
        put(destination, &Sha256::digest(&window[..filled]))?;
        put(destination, &payload)?;
        destination.flush().map_err(|e| e.to_string())?;
        whole.update(&window[..filled]);
        total = total
            .checked_add(filled as u64)
            .ok_or("CIXW1 length overflow")?;
        used = used
            .checked_add(RECORD_HEADER + payload.len())
            .ok_or("CIXW1 size overflow")?;
        sequence = sequence.checked_add(1).ok_or("CIXW1 sequence overflow")?;
        if filled < window.len() {
            break;
        }
    }
    check(limits)?;
    used = used.checked_add(END_BYTES).ok_or("CIXW1 size overflow")?;
    if used > limits.output_bytes {
        return Err("CIXW1 output limit".into());
    }
    put(destination, &[END])?;
    put(destination, &sequence.to_le_bytes())?;
    put(destination, &total.to_le_bytes())?;
    put(destination, &whole.finalize())?;
    destination.flush().map_err(|e| e.to_string())?;
    Ok(StreamSummary {
        windows: sequence,
        input_bytes: total,
        archive_bytes: used as u64,
        independent_windows: sequence,
    })
}

fn exact<R: Read>(source: &mut R, bytes: &mut [u8]) -> Result<(), String> {
    source
        .read_exact(bytes)
        .map_err(|_| "truncated CIXW1 record".into())
}
fn check_decode(l: &OperationLimits) -> Result<(), String> {
    if l.memory_bytes == 0 {
        return Err("invalid CIXW1 decode limits".into());
    }
    if l.cancellation
        .as_ref()
        .is_some_and(|x| x.load(Ordering::Acquire))
    {
        return Err("CIXW1 cancelled".into());
    }
    if l.deadline.is_some_and(|d| Instant::now() >= d) {
        return Err("CIXW1 deadline exceeded".into());
    }
    Ok(())
}
struct RecordWriter<'destination, 'whole, W> {
    destination: &'destination mut W,
    expected: usize,
    written: usize,
    hash: Sha256,
    whole: &'whole mut Sha256,
}

impl<W: Write> Write for RecordWriter<'_, '_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.expected.saturating_sub(self.written) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "CIXW1 record decoded beyond declared length",
            ));
        }
        self.destination.write_all(bytes)?;
        self.written += bytes.len();
        self.hash.update(bytes);
        self.whole.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.destination.flush()
    }
}

/// Incrementally restore independent CIXW1 records. The caller supplies the
/// archive decoder so CIXW1 can carry every archive class accepted by the
/// generic I/O bridge without recursively accepting a CIXW1 carrier.
pub fn decode<R: Read, W: Write, F>(
    mut source: R,
    destination: &mut W,
    limits: &OperationLimits,
    mut decode_payload: F,
) -> Result<StreamSummary, String>
where
    F: for<'a> FnMut(&[u8], &'a mut (dyn Write + 'a), &OperationLimits) -> Result<(), String>,
{
    check_decode(limits)?;
    let mut header = [0; HEADER];
    exact(&mut source, &mut header)?;
    if &header[..5] != MAGIC || header[5] != VERSION {
        return Err("invalid CIXW1 header".into());
    }
    let cap = u32::from_le_bytes(header[6..10].try_into().unwrap()) as usize;
    if cap == 0 || cap > limits.memory_bytes {
        return Err("CIXW1 window cap exceeds limits".into());
    }
    let mut seq = 0u64;
    let mut total = 0u64;
    let mut archive = HEADER as u64;
    let mut whole = Sha256::new();
    loop {
        check_decode(limits)?;
        let mut tag = [0];
        exact(&mut source, &mut tag)?;
        archive = archive.checked_add(1).ok_or("CIXW1 size overflow")?;
        if tag[0] == END {
            let mut end = [0; 48];
            exact(&mut source, &mut end)?;
            archive = archive.checked_add(48).ok_or("CIXW1 size overflow")?;
            let count = u64::from_le_bytes(end[..8].try_into().unwrap());
            let expected = u64::from_le_bytes(end[8..16].try_into().unwrap());
            if count != seq || expected != total || whole.finalize().as_slice() != &end[16..] {
                return Err("CIXW1 terminal mismatch".into());
            }
            let mut tail = [0];
            if source.read(&mut tail).map_err(|e| e.to_string())? != 0 {
                return Err("trailing CIXW1 bytes".into());
            }
            return Ok(StreamSummary {
                windows: seq,
                input_bytes: total,
                archive_bytes: archive,
                independent_windows: seq,
            });
        }
        if tag[0] != RECORD {
            return Err("unknown CIXW1 record tag".into());
        }
        let mut h = [0; RECORD_HEADER - 1];
        exact(&mut source, &mut h)?;
        archive = archive
            .checked_add(h.len() as u64)
            .ok_or("CIXW1 size overflow")?;
        let number = u64::from_le_bytes(h[..8].try_into().unwrap());
        let plain = u32::from_le_bytes(h[8..12].try_into().unwrap()) as usize;
        let packed = u32::from_le_bytes(h[12..16].try_into().unwrap()) as usize;
        if number != seq
            || plain == 0
            || plain > cap
            || packed > limits.intermediate_bytes
            || packed >= limits.memory_bytes
            || total
                .checked_add(plain as u64)
                .is_none_or(|next| next > limits.output_bytes as u64)
        {
            return Err("CIXW1 record limits".into());
        }
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(packed)
            .map_err(|_| "CIXW1 payload allocation")?;
        // The payload stays live during decoding; charge its actual capacity
        // before initializing or reading a body that cannot be decoded.
        let decoder_memory = limits
            .memory_bytes
            .checked_sub(payload.capacity())
            .ok_or("CIXW1 payload memory limit")?;
        if decoder_memory == 0 {
            return Err("CIXW1 payload leaves no decoder memory".into());
        }
        payload.resize(packed, 0);
        exact(&mut source, &mut payload)?;
        archive = archive
            .checked_add(packed as u64)
            .ok_or("CIXW1 size overflow")?;
        if payload.starts_with(MAGIC) {
            return Err("nested CIXW1 is unsupported".into());
        }
        let nested_limits = OperationLimits {
            // The payload buffer remains live while its archive decoder runs.
            memory_bytes: decoder_memory,
            output_bytes: plain,
            intermediate_bytes: limits.intermediate_bytes,
            deadline: limits.deadline,
            cancellation: limits.cancellation.clone(),
        };
        let mut writer = RecordWriter {
            destination,
            expected: plain,
            written: 0,
            hash: Sha256::new(),
            whole: &mut whole,
        };
        decode_payload(&payload, &mut writer, &nested_limits)?;
        let record_hash = writer.hash.clone().finalize();
        if writer.written != plain || record_hash.as_slice() != &h[16..48] {
            return Err("CIXW1 record checksum or length".into());
        }
        let next_total = total
            .checked_add(plain as u64)
            .ok_or("CIXW1 length overflow")?;
        destination.flush().map_err(|e| e.to_string())?;
        total = next_total;
        seq = seq.checked_add(1).ok_or("CIXW1 sequence overflow")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn writer_emits_independent_records_and_terminal() {
        let limits = EncodeLimits {
            window_bytes: 3,
            memory_bytes: 64,
            output_bytes: 256,
            deadline: None,
            cancellation: None,
        };
        let mut archive = Vec::new();
        let summary = encode(
            Cursor::new(b"abcdef"),
            &mut archive,
            &limits,
            |window, _| Ok(window.to_vec()),
        )
        .unwrap();
        assert_eq!(summary.windows, 2);
        assert!(archive.starts_with(MAGIC));
        assert_eq!(
            *archive.last().unwrap(),
            Sha256::digest(b"abcdef").as_slice()[31]
        );
    }

    #[test]
    fn decoder_rejects_exhausted_payload_budget_before_body_read() {
        struct HeaderOnlyReader {
            headers: Cursor<Vec<u8>>,
            body_reads: usize,
        }
        impl Read for HeaderOnlyReader {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                if self.headers.position() == self.headers.get_ref().len() as u64 {
                    self.body_reads += 1;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "payload body must not be read",
                    ));
                }
                self.headers.read(bytes)
            }
        }
        let limits = OperationLimits {
            memory_bytes: 64,
            output_bytes: 1,
            intermediate_bytes: 65,
            deadline: None,
            cancellation: None,
        };
        for packed in [64u32, 65] {
            let mut headers = Vec::new();
            headers.extend_from_slice(MAGIC);
            headers.push(VERSION);
            headers.extend_from_slice(&1u32.to_le_bytes());
            headers.push(RECORD);
            headers.extend_from_slice(&0u64.to_le_bytes());
            headers.extend_from_slice(&1u32.to_le_bytes());
            headers.extend_from_slice(&packed.to_le_bytes());
            headers.extend_from_slice(&Sha256::digest(b"x"));
            assert_eq!(headers.len(), HEADER + RECORD_HEADER);
            let mut reader = HeaderOnlyReader {
                headers: Cursor::new(headers),
                body_reads: 0,
            };
            let mut destination = b"unchanged".to_vec();
            let mut callbacks = 0;
            let error = decode(&mut reader, &mut destination, &limits, |_, _, _| {
                callbacks += 1;
                Ok(())
            })
            .unwrap_err();
            assert_eq!(error, "CIXW1 record limits");
            assert_eq!(reader.headers.position(), (HEADER + RECORD_HEADER) as u64);
            assert_eq!(reader.body_reads, 0);
            assert_eq!(callbacks, 0);
            assert_eq!(destination, b"unchanged");
        }
    }

    #[test]
    fn decoder_roundtrips_payload_with_remaining_memory() {
        let input = b"abcdefg";
        let encode_limits = EncodeLimits {
            window_bytes: 3,
            memory_bytes: 64,
            output_bytes: 512,
            deadline: None,
            cancellation: None,
        };
        let mut archive = Vec::new();
        let encoded = encode(Cursor::new(input), &mut archive, &encode_limits, |window, _| {
            Ok(window.to_vec())
        })
        .unwrap();
        let decode_limits = OperationLimits {
            memory_bytes: 64,
            output_bytes: input.len(),
            intermediate_bytes: 3,
            deadline: None,
            cancellation: None,
        };
        let mut restored = Vec::new();
        let mut callbacks = 0;
        let decoded = decode(Cursor::new(&archive), &mut restored, &decode_limits, |payload, out, nested| {
            callbacks += 1;
            assert!(payload.len() < decode_limits.memory_bytes);
            assert!(nested.memory_bytes > 0);
            assert!(nested.memory_bytes <= decode_limits.memory_bytes - payload.len());
            assert_eq!(nested.output_bytes, payload.len());
            out.write_all(payload).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(callbacks, 3);
        assert_eq!(restored, input);
        assert_eq!(decoded, encoded);
    }
}
