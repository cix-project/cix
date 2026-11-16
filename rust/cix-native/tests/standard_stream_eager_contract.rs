//! AUTHORED ONLY. No compile, codec call or test has been executed for this file.
//! Ordering fixtures are not actual pipe, elapsed-idle or measured-memory proof.
use cix_native::{
    standard_formats::{self, StandardFormat, StandardOptions},
    standard_stream::{
        decode_stream_eager, encode_stream_eager, IoStage, StreamFailure,
        StreamOptions, SCRATCH_BYTES,
    },
};
use flate2::{Decompress, FlushDecompress, Status};
use std::{cell::RefCell, io::{self, Cursor, Read, Write}, rc::Rc};

const FORMATS: [StandardFormat; 3] = [
    StandardFormat::Gzip, StandardFormat::Zlib, StandardFormat::Deflate,
];

fn options(input: u64, output: u64) -> StreamOptions<'static> {
    StreamOptions { level: 6, input_limit: input, output_limit: output, cancellation: None }
}

#[derive(Default)]
struct Seen { bytes: Vec<u8>, visible: usize, writes: usize, flushes: usize }

struct Sink { seen: Rc<RefCell<Seen>>, fail_flush: bool }
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = bytes.len().min(3);
        let mut seen = self.seen.borrow_mut();
        seen.bytes.extend_from_slice(&bytes[..count]);
        seen.writes += 1;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut seen = self.seen.borrow_mut();
        seen.flushes += 1;
        if self.fail_flush { return Err(io::Error::other("selected eager flush failure")); }
        seen.visible = seen.bytes.len();
        Ok(())
    }
}

struct Gate<'a> {
    bytes: &'a [u8], position: usize, calls: usize,
    seen: Rc<RefCell<Seen>>, format: StandardFormat, encoding: bool,
}
impl Read for Gate<'_> {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        assert!(destination.len() <= SCRATCH_BYTES);
        if self.position != 0 {
            let seen = self.seen.borrow();
            assert_eq!(seen.visible, seen.bytes.len(), "sink must flush before next read");
            assert!(seen.visible > 0);
            if self.encoding {
                // Assert decoded payload, not merely a gzip/zlib header.
                let mut decoder = match self.format {
                    StandardFormat::Gzip => Decompress::new_gzip(15),
                    StandardFormat::Zlib => Decompress::new(true),
                    _ => Decompress::new(false),
                };
                let mut restored = [0_u8; 256];
                let status = decoder.decompress(&seen.bytes, &mut restored, FlushDecompress::None).unwrap();
                assert_ne!(status, Status::StreamEnd, "EOF/Finish has not been supplied");
                assert_eq!(decoder.total_out() as usize, self.position);
                assert_eq!(&restored[..self.position], &self.bytes[..self.position]);
            } else {
                assert_eq!(seen.bytes, b"partial output before EOF");
            }
        }
        self.calls += 1;
        let count = destination.len().min(self.bytes.len() - self.position);
        destination[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

#[test]
fn encoded_payload_is_decodable_and_flushed_before_eof_read() {
    let input = b"partial output before EOF";
    for format in FORMATS {
        let seen = Rc::new(RefCell::new(Seen::default()));
        let mut source = Gate { bytes: input, position: 0, calls: 0, seen: seen.clone(), format, encoding: true };
        let mut sink = Sink { seen: seen.clone(), fail_flush: false };
        let progress = encode_stream_eager(format, &mut source, &mut sink, &options(input.len() as u64, 1024)).unwrap();
        assert_eq!(source.calls, 2);
        assert_eq!(progress.input_consumed, input.len() as u64);
        let seen = seen.borrow();
        assert_eq!(progress.output_written, seen.bytes.len() as u64);
        assert!(seen.writes > 1, "selected short-write consumer exercised");
        assert_eq!(standard_formats::decode(format, &seen.bytes, &StandardOptions { level: 6, output_limit: 256, memory_limit: 8 << 20 }).unwrap(), input);
    }
}

#[test]
fn decoder_flushes_payload_before_strict_physical_eof_probe() {
    let input = b"partial output before EOF";
    for format in FORMATS {
        let archive = standard_formats::encode(format, input, &StandardOptions { level: 6, output_limit: 1024, memory_limit: 8 << 20 }).unwrap();
        let seen = Rc::new(RefCell::new(Seen::default()));
        let mut source = Gate { bytes: &archive, position: 0, calls: 0, seen: seen.clone(), format, encoding: false };
        let mut sink = Sink { seen: seen.clone(), fail_flush: false };
        let progress = decode_stream_eager(format, &mut source, &mut sink, &options(archive.len() as u64, input.len() as u64)).unwrap();
        assert_eq!(source.calls, 2);
        assert_eq!(progress.output_written, input.len() as u64);
        assert_eq!(seen.borrow().bytes, input);
    }
}

#[test]
fn eager_flush_failure_stops_before_another_source_read() {
    let input = b"partial output before EOF";
    // Raw DEFLATE emits no initial wrapper header before the first input read.
    let format = StandardFormat::Deflate;
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut source = Gate { bytes: input, position: 0, calls: 0, seen: seen.clone(), format, encoding: true };
    let mut sink = Sink { seen: seen.clone(), fail_flush: true };
    let error = encode_stream_eager(format, &mut source, &mut sink, &options(input.len() as u64, 1024)).unwrap_err();
    assert!(matches!(error.failure, StreamFailure::Io { stage: IoStage::Flush, .. }));
    assert_eq!(source.calls, 1);
    assert_eq!(error.progress.input_consumed, input.len() as u64);
    assert_eq!(error.progress.output_written, seen.borrow().bytes.len() as u64);
}

#[test]
fn small_eager_output_caps_refuse_without_overflowing_sink() {
    for cap in 0..=6 {
        let seen = Rc::new(RefCell::new(Seen::default()));
        let mut sink = Sink { seen: seen.clone(), fail_flush: false };
        let error = encode_stream_eager(StandardFormat::Deflate, &mut Cursor::new(b"cap boundary"), &mut sink, &options(12, cap)).unwrap_err();
        assert!(matches!(error.failure, StreamFailure::OutputLimit));
        assert!(error.progress.output_produced > cap);
        assert!(error.progress.output_written <= cap);
        assert!(seen.borrow().bytes.len() as u64 <= cap);
    }
}

#[test]
fn eager_decode_truncation_and_trailing_data_never_report_success() {
    for format in FORMATS {
        let archive = standard_formats::encode(format, b"partial output before EOF", &StandardOptions { level: 6, output_limit: 1024, memory_limit: 8 << 20 }).unwrap();
        let mut trailing = archive.clone(); trailing.push(0x7f);
        for malformed in [&archive[..archive.len() - 1], &trailing[..]] {
            let seen = Rc::new(RefCell::new(Seen::default()));
            let mut sink = Sink { seen, fail_flush: false };
            let error = decode_stream_eager(format, &mut Cursor::new(malformed), &mut sink, &options(malformed.len() as u64, 256)).unwrap_err();
            assert!(matches!(error.failure, StreamFailure::Truncated | StreamFailure::TrailingData | StreamFailure::Format(_)));
            // Eager prefixes may have been flushed: callers must discard them.
        }
    }
}
