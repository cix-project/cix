use std::io::Write;
use std::process::{Command, Stdio};

fn run(binary: &str, args: &[&str], input: &[u8]) -> std::process::Output {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn explicit_standard_format_roundtrips_without_cix_envelope() {
    let input = b"CLI standard stream\n".repeat(500);
    let encoded = run(
        env!("CARGO_BIN_EXE_cix"),
        &["--output-format", "gzip", "-c", "-"],
        &input,
    );
    assert!(
        encoded.status.success(),
        "{}",
        String::from_utf8_lossy(&encoded.stderr)
    );
    assert!(encoded.stdout.starts_with(b"\x1f\x8b"));
    assert!(!encoded.stdout.starts_with(b"CIX"));
    let decoded = run(
        env!("CARGO_BIN_EXE_uncix"),
        &["--input-format", "gzip", "-c", "-"],
        &encoded.stdout,
    );
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_eq!(decoded.stdout, input);
    let tested = run(
        env!("CARGO_BIN_EXE_uncix"),
        &["--input-format", "gzip", "-t", "-"],
        &encoded.stdout,
    );
    assert!(
        tested.status.success(),
        "{}",
        String::from_utf8_lossy(&tested.stderr)
    );
}

#[test]
fn small_standard_memory_fails_before_large_buffer_admission() {
    let result = run(
        env!("CARGO_BIN_EXE_cix"),
        &["--output-format", "gzip", "--memory", "1KiB", "-c", "-"],
        b"x",
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("standard"));
}

#[test]
fn raw_deflate_requires_explicit_input_format_and_cix_overrides_are_rejected() {
    let input = b"explicit format";
    let encoded = run(
        env!("CARGO_BIN_EXE_cix"),
        &["--output-format", "deflate", "-c", "-"],
        input,
    );
    assert!(encoded.status.success());
    let guessed = run(env!("CARGO_BIN_EXE_uncix"), &["-c", "-"], &encoded.stdout);
    assert!(!guessed.status.success());
    let conflict = run(
        env!("CARGO_BIN_EXE_cix"),
        &["--output-format", "gzip", "--route", "lz", "-c", "-"],
        input,
    );
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("standard formats conflict"));
}
