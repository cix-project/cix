//! Generated compatibility checks for the ordinary executable boundary.
use cix_native::full_engine::{containers, mixed};
use std::{
    fs,
    io::Write,
    ops::Range,
    path::PathBuf,
    process::{Command, Output, Stdio},
    slice,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Work(PathBuf);
impl Work {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cix-cli-full-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Work {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn cli(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cix"))
        .args(args)
        .env_remove("HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}
fn successful(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn raw_frame_decodes_and_lists_without_optional_providers_or_home() {
    let data = b"ordinary full-engine CLI\0\xff";
    let archive = containers::encode_raw(data, 4096).unwrap();
    let output = cli(&["-dc", "--memory", "8M"], &archive);
    successful(&output);
    assert_eq!(output.stdout, data);
    let listed = cli(&["-l", "--memory", "8M"], &archive);
    successful(&listed);
    assert!(listed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&listed.stderr).contains("verified=true"));
    let tested = cli(&["-t", "--memory", "8M"], &archive);
    successful(&tested);
    assert!(tested.stdout.is_empty());
}

#[test]
fn mixed_raw_member_needs_no_image_provider() {
    let data = b"a nested arbitrary region";
    let whole: Range<usize> = 0..data.len();
    let archive = mixed::encode_with(data, slice::from_ref(&whole), 4096, 4 << 20, |region, _| {
        Ok(mixed::RegionChoice {
            backend: mixed::RegionBackend::Specialist,
            parameters: b"raw-child".to_vec(),
            payload: containers::encode_raw(region, 4096)?,
        })
    })
    .unwrap();
    let output = cli(&["-dc", "--memory", "8M"], &archive);
    successful(&output);
    assert_eq!(output.stdout, data);
}

#[test]
fn named_default_reaches_selector_and_independent_cli_decode() {
    let work = Work::new();
    let data = b"named input automatic selection\n".repeat(20);
    let source = work.file("arbitrary-input", &data);
    let output = cli(
        &[
            "-c",
            "--explain",
            "--threads",
            "1",
            source.to_str().unwrap(),
        ],
        &[],
    );
    successful(&output);
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(report.contains("full_engine selected="), "{report}");
    assert!(report.contains("heterogeneous-cixh1-v1"), "{report}");
    assert_eq!(fs::read(source).unwrap(), data);
    let restored = cli(&["-dc"], &output.stdout);
    successful(&restored);
    assert_eq!(restored.stdout, data);
}

#[test]
fn damaged_full_frame_does_not_publish_atomic_output_or_remove_input() {
    let work = Work::new();
    let mut archive = containers::encode_raw(b"checksum protected", 4096).unwrap();
    *archive.last_mut().unwrap() ^= 1;
    let source = work.file("corrupt.cix", &archive);
    let destination = work.0.join("restored");
    let output = cli(
        &[
            "-d",
            "-o",
            destination.to_str().unwrap(),
            source.to_str().unwrap(),
        ],
        &[],
    );
    assert!(!output.status.success());
    assert!(!destination.exists());
    assert_eq!(fs::read(source).unwrap(), archive);
    assert_eq!(fs::read_dir(&work.0).unwrap().count(), 1);
}

#[test]
fn native_stream_remains_available_and_ignores_unused_bad_image_libraries() {
    let work = Work::new();
    work.file("libcix_jxl_bridge.so", b"invalid shared library");
    work.file("libcix_spatial_bridge.so", b"invalid shared library");
    let data = b"streamed bytes\n".repeat(30);
    let output = cli(&["-c", "--stream", "--fast"], &data);
    successful(&output);
    let restored = cli(
        &["-dc", "--private-library-dir", work.0.to_str().unwrap()],
        &output.stdout,
    );
    successful(&restored);
    assert_eq!(restored.stdout, data);
}

#[test]
fn native_and_raw_decoding_ignore_invalid_home() {
    let data = b"provider-free decoding";
    let archive = containers::encode_raw(data, 4096).unwrap();
    let work = Work::new();
    let input = work.file("raw.cix", &archive);
    let output = Command::new(env!("CARGO_BIN_EXE_cix"))
        .args(["-dc", input.to_str().unwrap()])
        .env("HOME", "relative-and-unusable")
        .output()
        .unwrap();
    successful(&output);
    assert_eq!(output.stdout, data);
    let failed = cli(&["-dcQ"], &[]);
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
}

#[test]
fn small_memory_falls_back_to_bounded_native_streaming() {
    let work = Work::new();
    let data = b"a small bounded-memory source\n".repeat(10);
    let source = work.file("source", &data);
    let output = cli(
        &[
            "-c",
            "--fast",
            "--explain",
            "--memory",
            "8M",
            source.to_str().unwrap(),
        ],
        &[],
    );
    successful(&output);
    assert!(String::from_utf8_lossy(&output.stderr).contains("full_engine=not_admitted"));
    let restored = cli(&["-dc"], &output.stdout);
    successful(&restored);
    assert_eq!(restored.stdout, data);
}
