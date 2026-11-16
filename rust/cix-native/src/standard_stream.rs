//! Bounded I/O for genuine gzip, zlib and raw DEFLATE streams.
//!
//! Input/output limits count bytes, not memory or process RSS. Two fixed 8 KiB
//! scratch buffers are used; flate2/zlib retain their own native allocations.
//! Exactly one stream/member is accepted. Decode requires physical EOF after
//! StreamEnd and rejects trailing or concatenated data.
//!
//! A source may be read one byte beyond input_limit to distinguish EOF from
//! excess input. That byte is counted but never supplied to the codec.
//! Output is never written beyond output_limit. On any error, discard any
//! output prefix already written. Successful return includes codec completion,
//! strict input EOF and an explicit successful sink flush.
//!
//! Caller-owned source/sink remain borrowed. Cancellation is cooperative
//! between calls and cannot interrupt a blocking Read/Write or native call.
//! The opt-in eager entry points expose each read batch before the next read;
//! this is per-read flushing, not a timer or an interruptible idle-read API.

use crate::standard_formats::{StandardError, StandardFormat};
use flate2::{
    Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status,
};
use std::{
    fmt,
    io::{self, Read, Write},
    sync::atomic::{AtomicBool, Ordering},
};

pub const SCRATCH_BYTES: usize = 8 << 10;

pub struct StreamOptions<'a> {
    pub level: u8,
    pub input_limit: u64,
    pub output_limit: u64,
    pub cancellation: Option<&'a AtomicBool>,
}

impl StreamOptions<'_> {
    fn validate(&self) -> Result<(), StreamFailure> {
        if self.level > 9 {
            return Err(StreamFailure::Format(StandardError::InvalidOptions(
                "standard-stream level must be 0..=9",
            )));
        }
        // Leave representable space for the single excess/EOF probe.
        if self.input_limit == u64::MAX || self.output_limit == u64::MAX {
            return Err(StreamFailure::Format(StandardError::InvalidOptions(
                "standard-stream byte limits must be less than u64::MAX",
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamProgress {
    /// Successful source reads, including any excess/trailing probe byte.
    pub input_read: u64,
    /// Bytes actually consumed by the codec.
    pub input_consumed: u64,
    /// Bytes produced by the codec, including a refused overflow probe.
    pub output_produced: u64,
    /// Bytes accepted by successful sink writes.
    pub output_written: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoStage {
    Read,
    Write,
    Flush,
}

#[derive(Debug)]
pub enum StreamFailure {
    UnsupportedFormat(StandardFormat),
    Format(StandardError),
    InputLimit,
    OutputLimit,
    Truncated,
    TrailingData,
    Cancelled,
    CounterOverflow,
    InvalidIoCount(IoStage),
    InvalidCodecProgress,
    Io { stage: IoStage, error: io::Error },
}

#[derive(Debug)]
pub struct StreamError {
    pub failure: StreamFailure,
    pub progress: StreamProgress,
}

impl fmt::Display for StreamError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "standard stream failed: {:?}; progress {:?}",
            self.failure, self.progress
        )
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.failure {
            StreamFailure::Format(error) => Some(error),
            StreamFailure::Io { error, .. } => Some(error),
            _ => None,
        }
    }
}

fn failed(failure: StreamFailure, progress: StreamProgress) -> StreamError {
    StreamError { failure, progress }
}

fn checkpoint(
    options: &StreamOptions<'_>,
    progress: StreamProgress,
) -> Result<(), StreamError> {
    if options
        .cancellation
        .is_some_and(|flag| flag.load(Ordering::Acquire))
    {
        Err(failed(StreamFailure::Cancelled, progress))
    } else {
        Ok(())
    }
}

fn add(
    value: u64,
    count: usize,
    progress: StreamProgress,
) -> Result<u64, StreamError> {
    value
        .checked_add(count as u64)
        .ok_or_else(|| failed(StreamFailure::CounterOverflow, progress))
}

fn read_once<R: Read>(
    source: &mut R,
    buffer: &mut [u8],
    options: &StreamOptions<'_>,
    progress: StreamProgress,
) -> Result<usize, StreamError> {
    loop {
        checkpoint(options, progress)?;
        match source.read(buffer) {
            Ok(count) if count <= buffer.len() => return Ok(count),
            Ok(_) => {
                return Err(failed(
                    StreamFailure::InvalidIoCount(IoStage::Read),
                    progress,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(failed(
                    StreamFailure::Io { stage: IoStage::Read, error },
                    progress,
                ));
            }
        }
    }
}

fn read_capped<R: Read>(
    source: &mut R,
    buffer: &mut [u8],
    options: &StreamOptions<'_>,
    progress: &mut StreamProgress,
) -> Result<usize, StreamError> {
    let remaining = options.input_limit - progress.input_read;
    if remaining == 0 {
        let mut probe = [0_u8; 1];
        let count = read_once(source, &mut probe, options, *progress)?;
        progress.input_read = add(progress.input_read, count, *progress)?;
        return if count == 0 {
            Ok(0)
        } else {
            Err(failed(StreamFailure::InputLimit, *progress))
        };
    }
    let capacity = remaining.min(buffer.len() as u64) as usize;
    let count = read_once(source, &mut buffer[..capacity], options, *progress)?;
    progress.input_read = add(progress.input_read, count, *progress)?;
    Ok(count)
}

fn require_eof<R: Read>(
    source: &mut R,
    options: &StreamOptions<'_>,
    progress: &mut StreamProgress,
) -> Result<(), StreamError> {
    let before = progress.input_read;
    let mut probe = [0_u8; 1];
    let count = read_once(source, &mut probe, options, *progress)?;
    progress.input_read = add(progress.input_read, count, *progress)?;
    if count == 0 {
        Ok(())
    } else if before == options.input_limit {
        Err(failed(StreamFailure::InputLimit, *progress))
    } else {
        Err(failed(StreamFailure::TrailingData, *progress))
    }
}

fn write_chunk<W: Write>(
    sink: &mut W,
    bytes: &[u8],
    options: &StreamOptions<'_>,
    progress: &mut StreamProgress,
) -> Result<(), StreamError> {
    let mut offset = 0;
    while offset < bytes.len() {
        checkpoint(options, *progress)?;
        match sink.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(failed(
                    StreamFailure::Io {
                        stage: IoStage::Write,
                        error: io::Error::new(
                            io::ErrorKind::WriteZero,
                            "standard-stream sink made no progress",
                        ),
                    },
                    *progress,
                ));
            }
            Ok(count) if count <= bytes.len() - offset => {
                progress.output_written =
                    add(progress.output_written, count, *progress)?;
                offset += count;
            }
            Ok(_) => {
                return Err(failed(
                    StreamFailure::InvalidIoCount(IoStage::Write),
                    *progress,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(failed(
                    StreamFailure::Io { stage: IoStage::Write, error },
                    *progress,
                ));
            }
        }
    }
    Ok(())
}

fn flush_sink<W: Write>(
    sink: &mut W,
    options: &StreamOptions<'_>,
    progress: StreamProgress,
) -> Result<(), StreamError> {
    loop {
        checkpoint(options, progress)?;
        match sink.flush() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(failed(
                    StreamFailure::Io { stage: IoStage::Flush, error },
                    progress,
                ));
            }
        }
    }
}

enum Codec {
    Encoder(Compress),
    Decoder(Decompress),
}

struct Step {
    consumed: usize,
    produced: usize,
    status: Result<Status, String>,
}

impl Codec {
    fn totals(&self) -> (u64, u64) {
        match self {
            Self::Encoder(c) => (c.total_in(), c.total_out()),
            Self::Decoder(c) => (c.total_in(), c.total_out()),
        }
    }

    fn step(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        eof: bool,
        sync_flush: bool,
    ) -> Result<Step, StreamFailure> {
        let (before_in, before_out) = self.totals();
        let status = match self {
            Self::Encoder(codec) => codec
                .compress(
                    input,
                    output,
                    if eof {
                        FlushCompress::Finish
                    } else if sync_flush {
                        FlushCompress::Sync
                    } else {
                        FlushCompress::None
                    },
                )
                .map_err(|error| error.to_string()),
            // Finish assumes enough destination space for remaining decoded
            // data. Fixed scratch does not satisfy that assumption: use None
            // and require an actual StreamEnd instead.
            Self::Decoder(codec) => codec
                .decompress(input, output, FlushDecompress::None)
                .map_err(|error| error.to_string()),
        };
        let (after_in, after_out) = self.totals();
        let consumed = after_in
            .checked_sub(before_in)
            .and_then(|count| usize::try_from(count).ok())
            .ok_or(StreamFailure::InvalidCodecProgress)?;
        let produced = after_out
            .checked_sub(before_out)
            .and_then(|count| usize::try_from(count).ok())
            .ok_or(StreamFailure::InvalidCodecProgress)?;
        if consumed > input.len() || produced > output.len() {
            return Err(StreamFailure::InvalidCodecProgress);
        }
        Ok(Step { consumed, produced, status })
    }
}

fn run<R: Read, W: Write>(
    mut codec: Codec,
    source: &mut R,
    sink: &mut W,
    options: &StreamOptions<'_>,
    eager: bool,
) -> Result<StreamProgress, StreamError> {
    let decoding = matches!(&codec, Codec::Decoder(_));
    let mut progress = StreamProgress::default();
    let mut input = [0_u8; SCRATCH_BYTES];
    let mut output = [0_u8; SCRATCH_BYTES];
    let mut offset = 0;
    let mut length = 0;
    let mut eof = false;
    let mut syncing = false;
    loop {
        checkpoint(options, progress)?;
        let remaining = options.output_limit - progress.output_written;
        // One extra native-output byte detects overflow, including a decoder
        // that still needs to consume a trailer after reaching its exact cap.
        // Sync needs more than six output bytes of room. Keep the full fixed
        // scratch buffer during this operation; the overflow check below still
        // refuses the entire overflowing chunk before any sink write.
        let capacity = if syncing || remaining >= SCRATCH_BYTES as u64 {
            SCRATCH_BYTES
        } else {
            remaining as usize + 1
        };
        let step = codec
            .step(&input[offset..length], &mut output[..capacity], eof, syncing)
            .map_err(|failure| failed(failure, progress))?;
        progress.input_consumed =
            add(progress.input_consumed, step.consumed, progress)?;
        progress.output_produced =
            add(progress.output_produced, step.produced, progress)?;
        offset += step.consumed;
        let status = step.status.map_err(|message| {
            failed(StreamFailure::Format(StandardError::Codec(message)), progress)
        })?;
        if step.produced as u64 > remaining {
            // Do not write any bytes from this overflowing chunk.
            return Err(failed(StreamFailure::OutputLimit, progress));
        }
        write_chunk(sink, &output[..step.produced], options, &mut progress)?;
        // Flush before any subsequent source read, including strict EOF probing.
        // Decoder prefixes remain provisional until trailer/EOF validation.
        if eager && step.produced != 0 {
            flush_sink(sink, options, progress)?;
        }
        // Keep Sync across output-full calls. A non-full destination with all
        // input consumed ends this flush; do not issue another empty Sync.
        if syncing && offset == length && step.produced < capacity {
            syncing = false;
        }
        if status == Status::StreamEnd {
            if decoding {
                if offset != length {
                    return Err(failed(StreamFailure::TrailingData, progress));
                }
                require_eof(source, options, &mut progress)?;
            } else if !eof || offset != length {
                return Err(failed(StreamFailure::InvalidCodecProgress, progress));
            }
            flush_sink(sink, options, progress)?;
            return Ok(progress);
        }
        if step.consumed == 0 && step.produced == 0 {
            if eof {
                return Err(failed(
                    if decoding {
                        StreamFailure::Truncated
                    } else {
                        StreamFailure::InvalidCodecProgress
                    },
                    progress,
                ));
            }
            let retained = length - offset;
            if retained == SCRATCH_BYTES {
                return Err(failed(StreamFailure::InvalidCodecProgress, progress));
            }
            input.copy_within(offset..length, 0);
            let count = read_capped(
                source,
                &mut input[retained..],
                options,
                &mut progress,
            )?;
            offset = 0;
            length = retained + count;
            eof = count == 0;
            syncing = eager && !decoding && !eof;
        }
    }
}

fn admit(
    format: StandardFormat,
    options: &StreamOptions<'_>,
) -> Result<(), StreamError> {
    let progress = StreamProgress::default();
    options.validate().map_err(|failure| failed(failure, progress))?;
    match format {
        StandardFormat::Gzip | StandardFormat::Zlib | StandardFormat::Deflate => {}
        _ => {
            return Err(failed(
                StreamFailure::UnsupportedFormat(format),
                progress,
            ));
        }
    }
    checkpoint(options, progress)
}

/// Encode one genuine standard stream, completing its trailer before success.
pub fn encode_stream<R: Read, W: Write>(
    format: StandardFormat,
    source: &mut R,
    sink: &mut W,
    options: &StreamOptions<'_>,
) -> Result<StreamProgress, StreamError> {
    admit(format, options)?;
    let level = Compression::new(options.level as u32);
    let codec = match format {
        StandardFormat::Gzip => Compress::new_gzip(level, 15),
        StandardFormat::Zlib => Compress::new(level, true),
        StandardFormat::Deflate => Compress::new(level, false),
        _ => unreachable!("admit rejects unsupported formats"),
    };
    run(Codec::Encoder(codec), source, sink, options, false)
}

/// Decode one stream/member; concatenated members and trailing bytes refuse.
pub fn decode_stream<R: Read, W: Write>(
    format: StandardFormat,
    source: &mut R,
    sink: &mut W,
    options: &StreamOptions<'_>,
) -> Result<StreamProgress, StreamError> {
    admit(format, options)?;
    let codec = match format {
        StandardFormat::Gzip => Decompress::new_gzip(15),
        StandardFormat::Zlib => Decompress::new(true),
        StandardFormat::Deflate => Decompress::new(false),
        _ => unreachable!("admit rejects unsupported formats"),
    };
    run(Codec::Decoder(codec), source, sink, options, false)
}

/// Encode with a Sync flush after each successful source read.
///
/// Each batch is drained and sink-flushed before the next potentially blocking
/// read. No whole-input spool or extra queue is introduced. Frequent short reads
/// increase wire overhead; this opt-in path does not preserve compressed bytes.
/// Blocking providers remain cooperative. Success still requires Finish and EOF.
pub fn encode_stream_eager<R: Read, W: Write>(
    format: StandardFormat,
    source: &mut R,
    sink: &mut W,
    options: &StreamOptions<'_>,
) -> Result<StreamProgress, StreamError> {
    admit(format, options)?;
    let level = Compression::new(options.level as u32);
    let codec = match format {
        StandardFormat::Gzip => Compress::new_gzip(level, 15),
        StandardFormat::Zlib => Compress::new(level, true),
        StandardFormat::Deflate => Compress::new(level, false),
        _ => unreachable!("admit rejects unsupported formats"),
    };
    run(Codec::Encoder(codec), source, sink, options, true)
}

/// Decode with each produced output chunk sink-flushed before another read.
///
/// Output is provisional: discard the entire prefix on any error, including a
/// later checksum, truncation, trailing-data or physical-EOF failure. No timer,
/// interrupted blocking read, codec allocation cap or aggregate RSS is promised.
pub fn decode_stream_eager<R: Read, W: Write>(
    format: StandardFormat,
    source: &mut R,
    sink: &mut W,
    options: &StreamOptions<'_>,
) -> Result<StreamProgress, StreamError> {
    admit(format, options)?;
    let codec = match format {
        StandardFormat::Gzip => Decompress::new_gzip(15),
        StandardFormat::Zlib => Decompress::new(true),
        StandardFormat::Deflate => Decompress::new(false),
        _ => unreachable!("admit rejects unsupported formats"),
    };
    run(Codec::Decoder(codec), source, sink, options, true)
}
