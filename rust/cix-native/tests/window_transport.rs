//! Adversarial framing and resource contracts for the independent window carrier.
use cix_native::full_engine::{
    backend_provider::{BackendProvider, TrustedBackendPaths},
    containers,
    dispatch::{FullEngine, OperationLimits},
    io as engine_io,
    window_stream::{self, EncodeLimits},
};
use std::{
    io::Cursor,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};

fn limits(output: usize) -> OperationLimits {
    OperationLimits {
        memory_bytes: 128 << 20,
        output_bytes: output,
        intermediate_bytes: 1 << 20,
        deadline: None,
        cancellation: None,
    }
}

fn engine() -> FullEngine {
    FullEngine {
        backends: BackendProvider {
            paths: TrustedBackendPaths {
                cix: PathBuf::from("unused"),
                paq_libraries: PathBuf::from("unused"),
                temporary_root: PathBuf::from("unused"),
            },
        },
        images: None,
        spatial: None,
    }
}

fn archive(input: &[u8], window: usize) -> Vec<u8> {
    let mut result = Vec::new();
    let summary = window_stream::encode(
        Cursor::new(input),
        &mut result,
        &EncodeLimits {
            window_bytes: window,
            memory_bytes: 1 << 20,
            output_bytes: 1 << 20,
            deadline: None,
            cancellation: None,
        },
        containers::encode_raw,
    )
    .unwrap();
    assert_eq!(summary.archive_bytes, result.len() as u64);
    result
}

fn decode(input: &[u8], output: &mut Vec<u8>, cap: usize) -> Result<(), String> {
    engine_io::decode(
        &engine(),
        Cursor::new(input),
        output,
        input.len(),
        &limits(cap),
    )
    .map(|_| ())
}

#[test]
fn exact_multiple_windows_empty_and_one_byte() {
    for source in [b"".as_slice(), b"a", b"abcdefghijklmnop"] {
        let encoded = archive(source, 4);
        let mut output = Vec::new();
        decode(&encoded, &mut output, source.len().max(1)).unwrap();
        assert_eq!(output, source);
    }
}

#[test]
fn every_truncation_and_trailing_bytes_are_rejected() {
    let encoded = archive(b"abcdefghij", 4);
    for end in 0..encoded.len() {
        assert!(
            decode(&encoded[..end], &mut Vec::new(), 10).is_err(),
            "cut={end}"
        );
    }
    let mut tail = encoded;
    tail.push(0);
    assert!(decode(&tail, &mut Vec::new(), 10).is_err());
}

#[test]
fn metadata_sequence_hash_and_terminal_are_checked() {
    let encoded = archive(b"abcdefgh", 4);
    // Magic, version, cap, record tag, sequence, raw length, packed length,
    // per-window hash, payload, end count, end total and whole-stream hash.
    for at in [
        0,
        5,
        10,
        11,
        19,
        23,
        27,
        59,
        encoded.len() - 48,
        encoded.len() - 40,
        encoded.len() - 1,
    ] {
        let mut bad = encoded.clone();
        bad[at] ^= 0x80;
        assert!(decode(&bad, &mut Vec::new(), 8).is_err(), "mutation={at}");
    }
    // Raising a window maximum remains a valid header. Zero and maxima that
    // cannot fit the declared decoder budget are invalid instead.
    for cap in [0, u32::MAX] {
        let mut bad = encoded.clone();
        bad[6..10].copy_from_slice(&cap.to_le_bytes());
        assert!(decode(&bad, &mut Vec::new(), 8).is_err());
    }
}

#[test]
fn cumulative_output_is_admitted_before_writing_a_record() {
    let encoded = archive(b"abcdefgh", 4);
    let mut output = Vec::new();
    assert!(decode(&encoded, &mut output, 7).is_err());
    assert_eq!(output, b"abcd");
    let mut output = Vec::new();
    assert!(decode(&encoded, &mut output, 3).is_err());
    assert!(output.is_empty());
}

#[test]
fn cancelled_or_expired_decoders_do_not_write() {
    let encoded = archive(b"abcdefgh", 4);
    for expiry in [false, true] {
        let mut budget = limits(8);
        if expiry {
            budget.deadline = Some(std::time::Instant::now());
        } else {
            budget.cancellation = Some(Arc::new(AtomicBool::new(true)));
        }
        let mut output = Vec::new();
        assert!(engine_io::decode(
            &engine(),
            Cursor::new(&encoded),
            &mut output,
            encoded.len(),
            &budget
        )
        .is_err());
        assert!(output.is_empty());
    }
}

#[test]
fn huge_payload_and_recursive_carrier_are_rejected_before_dispatch() {
    let mut encoded = archive(b"abcd", 4);
    encoded[23..27].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut called = false;
    assert!(window_stream::decode(
        Cursor::new(&encoded),
        &mut Vec::new(),
        &limits(4),
        |_, _, _| {
            called = true;
            Ok(())
        }
    )
    .is_err());
    assert!(!called);
    let mut encoded = archive(b"abcd", 4);
    encoded[59..64].copy_from_slice(b"CIXW1");
    assert!(window_stream::decode(
        Cursor::new(&encoded),
        &mut Vec::new(),
        &limits(4),
        |_, _, _| {
            called = true;
            Ok(())
        }
    )
    .is_err());
    assert!(!called);
}

#[test]
fn encoded_payload_may_exceed_decoded_output_cap() {
    // A one-byte source still needs an inner and outer frame. Input/archive
    // admission must remain distinct from the decoded output limit.
    let encoded = archive(b"z", 4);
    let mut output = Vec::new();
    decode(&encoded, &mut output, 1).unwrap();
    assert_eq!(output, b"z");
}
