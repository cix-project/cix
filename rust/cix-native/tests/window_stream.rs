//! CIXW1 framing contract.  The integrated io test suite adds provider routes.

use cix_native::full_engine::window_stream::{encode, EncodeLimits, MAGIC};
use std::io::Cursor;

#[test]
fn bounded_writer_emits_records_before_eof_and_terminal() {
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
    assert_eq!(summary.input_bytes, 6);
    assert_eq!(summary.archive_bytes as usize, archive.len());
    assert!(archive.starts_with(MAGIC));
}

#[test]
fn writer_rejects_cumulative_archive_cap_before_terminal() {
    let limits = EncodeLimits {
        window_bytes: 3,
        memory_bytes: 64,
        output_bytes: 12,
        deadline: None,
        cancellation: None,
    };
    assert!(encode(
        Cursor::new(b"abc"),
        &mut Vec::new(),
        &limits,
        |window, _| Ok(window.to_vec())
    )
    .is_err());
}
