use cix_native::core::{
    self,
    incremental::{IncrementalDecoder, IncrementalEncoder, IncrementalState},
    NativeOptions, NativeProfile,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

fn options() -> NativeOptions {
    NativeOptions {
        profile: NativeProfile::Fast,
        ..NativeOptions::default()
    }
}
fn encode(source: &[u8]) -> Vec<u8> {
    let mut encoder = IncrementalEncoder::new(options()).unwrap();
    let mut result = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let end = source.len().min(at + 137);
        let mut out = [0; 97];
        let p = encoder.process(&source[at..end], &mut out).unwrap();
        assert!(p.consumed + p.produced > 0);
        at += p.consumed;
        result.extend_from_slice(&out[..p.produced]);
    }
    loop {
        let mut out = [0; 101];
        let p = encoder.finish(&mut out).unwrap();
        result.extend_from_slice(&out[..p.produced]);
        if p.state == IncrementalState::Finished {
            break;
        }
        assert!(p.produced > 0);
    }
    result
}
fn decode_fragments(archive: &[u8]) -> Vec<u8> {
    let mut decoder = IncrementalDecoder::new(options()).unwrap();
    let mut result = Vec::new();
    let mut at = 0;
    while at < archive.len() {
        let end = archive.len().min(at + 3);
        let mut out = [0; 83];
        let p = decoder.process(&archive[at..end], &mut out).unwrap();
        assert!(p.consumed + p.produced > 0);
        at += p.consumed;
        result.extend_from_slice(&out[..p.produced]);
    }
    loop {
        let mut out = [0; 83];
        let p = decoder.finish(&mut out).unwrap();
        result.extend_from_slice(&out[..p.produced]);
        if p.state == IncrementalState::Finished {
            break;
        }
        assert!(p.produced > 0);
    }
    result
}
#[test]
fn fragmented_streams_cross_decode_with_ordinary_core() {
    for length in [0, 1, 63, 65_536, 65_537] {
        let source: Vec<u8> = (0..length).map(|i| (i % 31) as u8).collect();
        let archive = encode(&source);
        assert_eq!(core::decode_buffer(&archive, &options()).unwrap(), source);
        assert_eq!(decode_fragments(&archive), source);
    }
}
#[test]
fn data_frame_is_available_before_finish_and_flush_preserves_input() {
    let source = vec![42; 65_536];
    let mut encoder = IncrementalEncoder::new(options()).unwrap();
    let mut out = vec![0; 70_000];
    let first = encoder.process(&source, &mut out).unwrap();
    assert_eq!(first.consumed, source.len());
    assert!(first.produced > 13);
    let mut archive = out[..first.produced].to_vec();
    let p = encoder.process(b"tail", &mut out).unwrap();
    archive.extend_from_slice(&out[..p.produced]);
    let p = encoder.flush(&mut out).unwrap();
    assert!(p.produced > 0);
    archive.extend_from_slice(&out[..p.produced]);
    let p = encoder.finish(&mut out).unwrap();
    assert_eq!(p.state, IncrementalState::Finished);
    archive.extend_from_slice(&out[..p.produced]);
    let mut expected = source;
    expected.extend_from_slice(b"tail");
    assert_eq!(core::decode_buffer(&archive, &options()).unwrap(), expected);
}
#[test]
fn checksummed_end_and_trailing_bytes_are_required() {
    let good = encode(b"");
    let mut corrupt = good.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    for bytes in [corrupt, {
        let mut value = good.clone();
        value.push(0);
        value
    }] {
        let mut decoder = IncrementalDecoder::new(options()).unwrap();
        assert!(decoder.process(&bytes, &mut [0; 8]).is_err());
        assert!(decoder.process(&good, &mut [0; 8]).is_err());
        decoder.reset().unwrap();
        assert_eq!(
            decoder.process(&good, &mut [0; 8]).unwrap().state,
            IncrementalState::Finished
        );
        assert!(decoder.process(b"trailing", &mut [0; 8]).is_err());
    }
    let mut decoder = IncrementalDecoder::new(options()).unwrap();
    decoder.process(&good[..good.len() - 1], &mut []).unwrap();
    assert!(decoder.finish(&mut []).is_err());
    assert!(decoder.process(&good, &mut []).is_err());
}
#[test]
fn zero_output_backpressure_cancellation_deadline_and_error_reset() {
    let mut encoder = IncrementalEncoder::new(options()).unwrap();
    let p = encoder.process(b"some bytes", &mut []).unwrap();
    assert_eq!(
        (p.consumed, p.produced, p.state),
        (0, 0, IncrementalState::NeedsOutput)
    );
    let cancel = Arc::new(AtomicBool::new(true));
    let mut configured = options();
    configured.cancellation = Some(cancel.clone());
    let mut encoder = IncrementalEncoder::new(configured.clone()).unwrap();
    let mut decoder = IncrementalDecoder::new(configured).unwrap();
    assert!(encoder.process(b"x", &mut [0; 64]).is_err());
    assert!(decoder.process(b"C", &mut [0; 64]).is_err());
    cancel.store(false, Ordering::Release);
    assert!(encoder.finish(&mut [0; 64]).is_err());
    assert!(decoder.finish(&mut [0; 64]).is_err());
    encoder.reset().unwrap();
    decoder.reset().unwrap();
    assert!(encoder.process(b"x", &mut [0; 64]).is_ok());
    assert!(decoder.process(b"C", &mut [0; 64]).is_ok());
    let mut timed = options();
    timed.deadline = Some(Duration::ZERO);
    assert!(IncrementalEncoder::new(timed.clone())
        .unwrap()
        .process(b"", &mut [])
        .is_err());
    assert!(IncrementalDecoder::new(timed)
        .unwrap()
        .process(b"", &mut [])
        .is_err());
}
#[test]
fn paid_lengths_and_output_limits_are_checked_before_payload() {
    let mut low = options();
    low.output_limit = 0;
    let archive = encode(b"x");
    let mut decoder = IncrementalDecoder::new(low).unwrap();
    assert!(decoder.process(&archive[..54], &mut []).is_err());
    let mut damaged = archive[..54].to_vec();
    damaged[18..22].fill(255);
    let mut decoder = IncrementalDecoder::new(options()).unwrap();
    assert!(decoder.process(&damaged, &mut []).is_err());
    let mut low = options();
    low.output_limit = 12;
    let mut encoder = IncrementalEncoder::new(low).unwrap();
    assert!(encoder.process(b"x", &mut [0; 64]).is_err());
    assert!(encoder.finish(&mut [0; 64]).is_err());
}
