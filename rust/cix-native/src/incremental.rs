//! Caller-driven independent-block CIXG1 streaming.
//!
//! One bounded input block and one output frame provide backpressure. Frames
//! use the existing native selector and decoder. This interface deliberately
//! excludes history-bearing streams and full-engine specialist archives.
//! Memory admission charges owned buffers before giving the codec its remaining
//! budget. It is not a process RSS ceiling; caller input/output storage is owned
//! and budgeted by the caller. Deadlines apply to a complete stream until reset.
use super::{codec_error, level, validate, LimitedWriter, NativeError, NativeOptions};
use crate::core::engine;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::sync::atomic::Ordering;
use std::time::Instant;

const BLOCK: usize = 65_536;
const HEADER: usize = 13;
const FRAME_HEADER: usize = 41;
const WRAPPER: usize = HEADER + FRAME_HEADER * 2;
const STATE_ALLOWANCE: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IncrementalState {
    NeedsInput,
    NeedsOutput,
    Finished,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IncrementalProgress {
    pub consumed: usize,
    pub produced: usize,
    pub state: IncrementalState,
}

fn error(message: &'static str) -> NativeError {
    NativeError::Codec(message.into())
}
fn buffer(capacity: usize) -> Result<Vec<u8>, NativeError> {
    let mut value = Vec::new();
    value
        .try_reserve_exact(capacity)
        .map_err(|_| error("incremental allocation failed"))?;
    Ok(value)
}
fn deadline(options: &NativeOptions) -> Result<Option<Instant>, NativeError> {
    options
        .deadline
        .map(|duration| {
            Instant::now()
                .checked_add(duration)
                .ok_or(NativeError::InvalidOptions("stream deadline is too large"))
        })
        .transpose()
}
struct Control {
    options: NativeOptions,
    deadline: Option<Instant>,
    failed: bool,
    emitted: usize,
}
impl Control {
    fn new(options: NativeOptions) -> Result<Self, NativeError> {
        validate(&options)?;
        if options.memory_limit < BLOCK * 3 + WRAPPER + STATE_ALLOWANCE {
            return Err(NativeError::InvalidOptions(
                "incremental streams require at least 197727 bytes of memory",
            ));
        }
        Ok(Self {
            deadline: deadline(&options)?,
            options,
            failed: false,
            emitted: 0,
        })
    }
    fn check(&self) -> Result<(), NativeError> {
        if self.failed {
            return Err(error("incremental stream must be reset after an error"));
        }
        if self
            .options
            .cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            return Err(error("incremental stream cancelled"));
        }
        if self.deadline.is_some_and(|at| Instant::now() >= at) {
            return Err(error("incremental stream deadline exceeded"));
        }
        Ok(())
    }
    fn finish_call<T>(&mut self, result: Result<T, NativeError>) -> Result<T, NativeError> {
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn codec_budget(&self, buffers: &[usize]) -> Result<usize, NativeError> {
        let retained = buffers
            .iter()
            .try_fold(STATE_ALLOWANCE, |n, &v| n.checked_add(v))
            .ok_or_else(|| error("incremental buffer size overflow"))?;
        self.options
            .memory_limit
            .checked_sub(retained)
            .filter(|&n| n > 0)
            .ok_or_else(|| error("incremental simultaneous-buffer memory limit"))
    }
    fn remaining_time(&self) -> Option<std::time::Duration> {
        self.deadline
            .map(|at| at.saturating_duration_since(Instant::now()))
    }
    fn reset(&mut self) -> Result<(), NativeError> {
        self.deadline = deadline(&self.options)?;
        self.failed = false;
        self.emitted = 0;
        Ok(())
    }
}
#[derive(Default)]
struct Pending {
    bytes: Vec<u8>,
    position: usize,
    end: usize,
}
impl Pending {
    fn empty(&self) -> bool {
        self.position == self.end
    }
    fn set(&mut self, bytes: Vec<u8>, position: usize, end: usize) {
        debug_assert!(self.empty() && position <= end && end <= bytes.len());
        *self = Self {
            bytes,
            position,
            end,
        };
    }
    fn drain(&mut self, output: &mut [u8], control: &mut Control) -> usize {
        let count = output.len().min(self.end - self.position);
        output[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        control.emitted += count; // queue/decode admission checked the total first
        if self.empty() {
            *self = Self::default();
        }
        count
    }
    fn progress(&self, consumed: usize, produced: usize, finished: bool) -> IncrementalProgress {
        IncrementalProgress {
            consumed,
            produced,
            state: if !self.empty() {
                IncrementalState::NeedsOutput
            } else if finished {
                IncrementalState::Finished
            } else {
                IncrementalState::NeedsInput
            },
        }
    }
}
fn append_frame(target: &mut Vec<u8>, route: u8, n: u32, payload: u32, hash: &[u8; 32]) {
    target.push(route);
    target.extend_from_slice(&n.to_le_bytes());
    target.extend_from_slice(&payload.to_le_bytes());
    target.extend_from_slice(hash);
}

/// A process-free native encoder with independent 64-KiB blocks. Flush closes
/// a partial block; there is no persistent model history in this profile.
pub struct IncrementalEncoder {
    control: Control,
    input: Vec<u8>,
    pending: Pending,
    whole: Sha256,
    started: bool,
    finishing: bool,
    finished: bool,
}
impl IncrementalEncoder {
    pub fn new(options: NativeOptions) -> Result<Self, NativeError> {
        Ok(Self {
            control: Control::new(options)?,
            input: buffer(BLOCK)?,
            pending: Pending::default(),
            whole: Sha256::new(),
            started: false,
            finishing: false,
            finished: false,
        })
    }
    pub fn process(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<IncrementalProgress, NativeError> {
        let result = self.process_inner(input, output);
        self.control.finish_call(result)
    }
    fn process_inner(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<IncrementalProgress, NativeError> {
        self.control.check()?;
        if self.finishing {
            return Err(error("use finish to drain a finishing encoder"));
        }
        let mut produced = self.pending.drain(output, &mut self.control);
        if !self.pending.empty() {
            return Ok(self.pending.progress(0, produced, self.finished));
        }
        self.start()?;
        produced += self
            .pending
            .drain(&mut output[produced..], &mut self.control);
        if !self.pending.empty() {
            return Ok(self.pending.progress(0, produced, false));
        }
        let consumed = input.len().min(BLOCK - self.input.len());
        self.input.extend_from_slice(&input[..consumed]);
        if self.input.len() == BLOCK {
            self.submit()?;
        }
        produced += self
            .pending
            .drain(&mut output[produced..], &mut self.control);
        Ok(self.pending.progress(consumed, produced, false))
    }
    pub fn flush(&mut self, output: &mut [u8]) -> Result<IncrementalProgress, NativeError> {
        let result = self.flush_inner(output, false);
        self.control.finish_call(result)
    }
    pub fn finish(&mut self, output: &mut [u8]) -> Result<IncrementalProgress, NativeError> {
        let result = self.flush_inner(output, true);
        self.control.finish_call(result)
    }
    fn flush_inner(
        &mut self,
        output: &mut [u8],
        finish: bool,
    ) -> Result<IncrementalProgress, NativeError> {
        self.control.check()?;
        if self.finishing && !finish {
            return Err(error("use finish to drain a finishing encoder"));
        }
        self.finishing |= finish;
        let mut produced = self.pending.drain(output, &mut self.control);
        if !self.pending.empty() || self.finished {
            return Ok(self.pending.progress(0, produced, self.finished));
        }
        self.start()?;
        produced += self
            .pending
            .drain(&mut output[produced..], &mut self.control);
        if !self.pending.empty() {
            return Ok(self.pending.progress(0, produced, false));
        }
        if !self.input.is_empty() {
            self.submit()?;
            produced += self
                .pending
                .drain(&mut output[produced..], &mut self.control);
            if !self.pending.empty() {
                return Ok(self.pending.progress(0, produced, false));
            }
        }
        if finish {
            let mut end = buffer(FRAME_HEADER)?;
            append_frame(&mut end, 255, 0, 0, &self.whole.clone().finalize().into());
            self.queue(end, 0, FRAME_HEADER)?;
            self.finished = true;
            produced += self
                .pending
                .drain(&mut output[produced..], &mut self.control);
        }
        Ok(self.pending.progress(0, produced, self.finished))
    }
    fn start(&mut self) -> Result<(), NativeError> {
        if !self.started {
            let mut header = buffer(HEADER)?;
            header.extend_from_slice(b"CIXG1");
            header.extend_from_slice(&(BLOCK as u32).to_le_bytes());
            header.extend_from_slice(&0u32.to_le_bytes());
            self.queue(header, 0, HEADER)?;
            self.started = true;
        }
        Ok(())
    }
    fn queue(&mut self, bytes: Vec<u8>, start: usize, end: usize) -> Result<(), NativeError> {
        if self
            .control
            .emitted
            .checked_add(end - start)
            .is_none_or(|n| n > self.control.options.output_limit)
        {
            return Err(NativeError::OutputLimit);
        }
        self.control
            .codec_budget(&[self.input.capacity(), bytes.capacity()])?;
        self.pending.set(bytes, start, end);
        Ok(())
    }
    fn submit(&mut self) -> Result<(), NativeError> {
        let mut archive = buffer(BLOCK + WRAPPER)?;
        let budget = self
            .control
            .codec_budget(&[self.input.capacity(), archive.capacity()])?;
        let _library = engine::limits::LibraryGuard::new();
        let _cancel = self
            .control
            .options
            .cancellation
            .clone()
            .map(engine::limits::CancellationGuard::new);
        let _deadline = self
            .control
            .remaining_time()
            .map(engine::limits::DeadlineGuard::new);
        let mut bounded = LimitedWriter {
            inner: &mut archive,
            remaining: BLOCK + WRAPPER,
        };
        engine::encode(
            Cursor::new(&self.input),
            &mut bounded,
            engine::EncodeOptions {
                block: BLOCK as u32,
                level: level(self.control.options.profile),
                forced: None,
                backend: "native",
                backend_set: false,
                format: "cixg1",
                input_fd: -1,
                flush_interval: None,
                memory: budget,
                workers: self.control.options.workers,
                explain: false,
                verbose: false,
                strategy: "blocks",
            },
        )
        .map_err(codec_error)?;
        if archive.len() < WRAPPER || &archive[..5] != b"CIXG1" {
            return Err(error("invalid independent native archive"));
        }
        let payload =
            u32::from_le_bytes(archive[HEADER + 5..HEADER + 9].try_into().unwrap()) as usize;
        let end = archive.len() - FRAME_HEADER;
        if archive[HEADER] == 255
            || HEADER + FRAME_HEADER + payload != end
            || archive[end] != 255
            || archive[end + 1..end + 9] != [0; 8]
        {
            return Err(error(
                "native encoder did not produce exactly one independent block",
            ));
        }
        self.queue(archive, HEADER, end)?;
        self.whole.update(&self.input);
        self.input.clear();
        Ok(())
    }
    /// Discard the current stream and start a fresh deadline and archive.
    pub fn reset(&mut self) -> Result<(), NativeError> {
        self.control.reset()?;
        self.input.clear();
        self.pending = Pending::default();
        self.whole = Sha256::new();
        self.started = false;
        self.finishing = false;
        self.finished = false;
        Ok(())
    }
}

/// Incremental decoder for independent CIXG1 blocks up to 64 KiB. Other native
/// wire versions and history profiles remain available through `core::decode`.
pub struct IncrementalDecoder {
    control: Control,
    header: [u8; HEADER],
    header_len: usize,
    frame: [u8; FRAME_HEADER],
    frame_len: usize,
    member: Vec<u8>,
    payload: usize,
    received: usize,
    source: usize,
    prepared: bool,
    pending: Pending,
    whole: Sha256,
    finished: bool,
}
impl IncrementalDecoder {
    pub fn new(options: NativeOptions) -> Result<Self, NativeError> {
        Ok(Self {
            control: Control::new(options)?,
            header: [0; HEADER],
            header_len: 0,
            frame: [0; FRAME_HEADER],
            frame_len: 0,
            member: Vec::new(),
            payload: 0,
            received: 0,
            source: 0,
            prepared: false,
            pending: Pending::default(),
            whole: Sha256::new(),
            finished: false,
        })
    }
    pub fn process(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<IncrementalProgress, NativeError> {
        let result = self.process_inner(input, output);
        self.control.finish_call(result)
    }
    fn process_inner(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<IncrementalProgress, NativeError> {
        self.control.check()?;
        let mut produced = self.pending.drain(output, &mut self.control);
        if !self.pending.empty() {
            return Ok(self.pending.progress(0, produced, self.finished));
        }
        if self.finished {
            if !input.is_empty() {
                return Err(error("trailing bytes after incremental archive"));
            }
            return Ok(self.pending.progress(0, produced, true));
        }
        let mut consumed = 0;
        while consumed < input.len() && self.pending.empty() && !self.finished {
            if self.header_len < HEADER {
                let take = (HEADER - self.header_len).min(input.len() - consumed);
                self.header[self.header_len..self.header_len + take]
                    .copy_from_slice(&input[consumed..consumed + take]);
                self.header_len += take;
                consumed += take;
                if self.header_len == HEADER {
                    let block = u32::from_le_bytes(self.header[5..9].try_into().unwrap()) as usize;
                    if &self.header[..5] != b"CIXG1"
                        || self.header[9..13] != [0; 4]
                        || block == 0
                        || block > BLOCK
                    {
                        return Err(error(
                            "incremental decoder requires independent CIXG1 blocks up to 64 KiB",
                        ));
                    }
                }
                continue;
            }
            if self.frame_len < FRAME_HEADER {
                let take = (FRAME_HEADER - self.frame_len).min(input.len() - consumed);
                self.frame[self.frame_len..self.frame_len + take]
                    .copy_from_slice(&input[consumed..consumed + take]);
                self.frame_len += take;
                consumed += take;
                if self.frame_len == FRAME_HEADER {
                    self.prepare_frame()?;
                    if !self.finished && self.payload == 0 {
                        self.complete_frame()?;
                    }
                }
                continue;
            }
            debug_assert!(self.prepared);
            let take = (self.payload - self.received).min(input.len() - consumed);
            self.member
                .extend_from_slice(&input[consumed..consumed + take]);
            consumed += take;
            self.received += take;
            if self.received == self.payload {
                self.complete_frame()?;
            }
        }
        if self.finished && consumed < input.len() {
            return Err(error("trailing bytes after incremental archive"));
        }
        produced += self
            .pending
            .drain(&mut output[produced..], &mut self.control);
        Ok(self.pending.progress(consumed, produced, self.finished))
    }
    fn prepare_frame(&mut self) -> Result<(), NativeError> {
        self.source = u32::from_le_bytes(self.frame[1..5].try_into().unwrap()) as usize;
        self.payload = u32::from_le_bytes(self.frame[5..9].try_into().unwrap()) as usize;
        let hash: [u8; 32] = self.frame[9..41].try_into().unwrap();
        if self.frame[0] == 255 {
            let actual: [u8; 32] = self.whole.clone().finalize().into();
            if self.source != 0 || self.payload != 0 || hash != actual {
                return Err(error("invalid incremental archive end frame or checksum"));
            }
            self.finished = true;
            return Ok(());
        }
        let block = u32::from_le_bytes(self.header[5..9].try_into().unwrap()) as usize;
        if self.source == 0 || self.source > block {
            return Err(error(
                "incremental block source length exceeds paid block limit",
            ));
        }
        if self
            .control
            .emitted
            .checked_add(self.source)
            .is_none_or(|n| n > self.control.options.output_limit)
        {
            return Err(NativeError::OutputLimit);
        }
        let size = self
            .payload
            .checked_add(WRAPPER)
            .ok_or_else(|| error("incremental frame overflow"))?;
        self.control.codec_budget(&[size, self.source])?;
        self.member = buffer(size)?;
        self.control
            .codec_budget(&[self.member.capacity(), self.source])?;
        self.member.extend_from_slice(&self.header);
        self.member.extend_from_slice(&self.frame);
        self.received = 0;
        self.prepared = true;
        Ok(())
    }
    fn complete_frame(&mut self) -> Result<(), NativeError> {
        let hash: [u8; 32] = self.frame[9..41].try_into().unwrap();
        append_frame(&mut self.member, 255, 0, 0, &hash);
        let mut restored = buffer(self.source)?;
        let budget = self
            .control
            .codec_budget(&[self.member.capacity(), restored.capacity()])?;
        let _library = engine::limits::LibraryGuard::new();
        let _cancel = self
            .control
            .options
            .cancellation
            .clone()
            .map(engine::limits::CancellationGuard::new);
        let _deadline = self
            .control
            .remaining_time()
            .map(engine::limits::DeadlineGuard::new);
        let mut bounded = LimitedWriter {
            inner: &mut restored,
            remaining: self.source,
        };
        engine::decode(
            Cursor::new(&self.member),
            &mut bounded,
            false,
            false,
            -1,
            budget,
        )
        .map_err(codec_error)?;
        if restored.len() != self.source {
            return Err(error("incremental decoded length mismatch"));
        }
        self.whole.update(&restored);
        self.member = Vec::new();
        self.pending.set(restored, 0, self.source);
        self.frame_len = 0;
        self.prepared = false;
        Ok(())
    }
    pub fn finish(&mut self, output: &mut [u8]) -> Result<IncrementalProgress, NativeError> {
        let result = self.finish_inner(output);
        self.control.finish_call(result)
    }
    fn finish_inner(&mut self, output: &mut [u8]) -> Result<IncrementalProgress, NativeError> {
        self.control.check()?;
        let produced = self.pending.drain(output, &mut self.control);
        if !self.pending.empty() {
            return Ok(self.pending.progress(0, produced, false));
        }
        if !self.finished {
            return Err(error("truncated incremental archive"));
        }
        Ok(self.pending.progress(0, produced, true))
    }
    /// Discard pending plaintext and start a new independent stream.
    pub fn reset(&mut self) -> Result<(), NativeError> {
        self.control.reset()?;
        self.header_len = 0;
        self.frame_len = 0;
        self.member = Vec::new();
        self.payload = 0;
        self.received = 0;
        self.source = 0;
        self.prepared = false;
        self.pending = Pending::default();
        self.whole = Sha256::new();
        self.finished = false;
        Ok(())
    }
}
