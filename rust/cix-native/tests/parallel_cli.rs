#![cfg(unix)]

//! Byte-level equivalence between the serial and ordered-block CLI paths.
//! These use forced routes so scheduling cannot legitimately alter a selector
//! decision; a changed archive would therefore be a framing/order regression.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct ChildGuard {
    child: Child,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before Unix epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("cix-parallel-cli-{}-{stamp}", std::process::id()));
        fs::create_dir(&path).expect("create test directory");
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn encode(input: &[u8], route: &str, threads: usize) -> Vec<u8> {
    let threads = threads.to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cix"))
        .args([
            "--stream",
            "--format",
            "cixg1",
            "--route",
            route,
            "--block-size",
            "4KiB",
            "--threads",
            threads.as_str(),
            "--parallelism",
            "blocks",
            "-c",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cix");
    child
        .stdin
        .take()
        .expect("cix stdin")
        .write_all(input)
        .expect("write input");
    let output = child.wait_with_output().expect("wait cix");
    assert!(
        output.status.success(),
        "cix {route} threads={threads} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn decode(archive: &[u8]) -> Vec<u8> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_uncix"))
        .args(["--stream", "-c", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn uncix");
    child
        .stdin
        .take()
        .expect("uncix stdin")
        .write_all(archive)
        .expect("write archive");
    let output = child.wait_with_output().expect("wait uncix");
    assert!(
        output.status.success(),
        "uncix failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn run_heavy_input() -> Vec<u8> {
    let mut input = Vec::with_capacity(48 * 1024);
    for block in 0..12 {
        input.extend(std::iter::repeat_n((block % 3) as u8 + b'A', 2048));
        input.extend(std::iter::repeat_n((block % 5) as u8 + b'K', 2048));
    }
    input
}

fn has_complete_end_frame(archive: &[u8]) -> bool {
    if archive.len() < 13 || !matches!(&archive[..5], b"CIXG1" | b"CIXG2") {
        return false;
    }
    let mut pos = 13usize;
    while pos.saturating_add(41) <= archive.len() {
        let route = archive[pos];
        let source = u32::from_le_bytes(archive[pos + 1..pos + 5].try_into().unwrap());
        let payload = u32::from_le_bytes(archive[pos + 5..pos + 9].try_into().unwrap()) as usize;
        let end = match pos
            .checked_add(41)
            .and_then(|offset| offset.checked_add(payload))
        {
            Some(end) if end <= archive.len() => end,
            _ => return false,
        };
        if route == 255 && source == 0 && payload == 0 {
            return true;
        }
        pos = end;
    }
    false
}

fn temporary_archive_has_frame(parent: &Path, target_name: &str) -> bool {
    fs::read_dir(parent)
        .expect("read test directory")
        .filter_map(Result::ok)
        .any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&format!(".{target_name}.cix-tmp-"))
                && entry
                    .metadata()
                    .map(|metadata| metadata.len() >= 54)
                    .unwrap_or(false)
        })
}

fn no_temporary_archives(parent: &Path, target_name: &str) -> bool {
    fs::read_dir(parent)
        .expect("read test directory")
        .filter_map(Result::ok)
        .all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!(".{target_name}.cix-tmp-"))
        })
}

#[test]
fn forced_raw_and_runs_are_byte_identical_across_block_workers() {
    for (route, input, expected_route) in [
        ("raw", run_heavy_input(), 0u8),
        ("runs", vec![b'R'; 48 * 1024], 1u8),
    ] {
        let serial = encode(&input, route, 1);
        assert_eq!(
            serial[13], expected_route,
            "forced {route} first frame route"
        );
        assert_eq!(decode(&serial), input, "serial {route} round trip");
        for workers in [2, 4] {
            let parallel = encode(&input, route, workers);
            assert_eq!(
                parallel, serial,
                "forced {route} archive differed with {workers} block workers"
            );
            assert_eq!(decode(&parallel), input, "parallel {route} round trip");
        }
    }
}

#[test]
fn explicit_twenty_block_workers_preserve_order_and_round_trip() {
    // More 4 KiB blocks than workers proves the ordered pipeline accepts a
    // real 20-worker request rather than only parsing the option.
    let mut input = Vec::with_capacity(96 * 1024);
    for block in 0..24u8 {
        input.extend(std::iter::repeat_n(block, 4096));
    }
    let serial = encode(&input, "raw", 1);
    let parallel = encode(&input, "raw", 20);
    assert_eq!(parallel, serial, "20 workers changed framed output order");
    assert_eq!(decode(&parallel), input, "20 workers did not restore input");
}

#[test]
fn sigint_with_stalled_stdout_exits_without_a_complete_footer() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cix"))
        .args([
            "--stream",
            "--threads",
            "2",
            "--parallelism",
            "blocks",
            "--route",
            "raw",
            "--block-size",
            "4KiB",
            "-c",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cix");
    let written = Arc::new(AtomicUsize::new(0));
    let producer_written = Arc::clone(&written);
    let mut stdin = child.stdin.take().expect("cix stdin");
    thread::spawn(move || {
        let chunk = [b'X'; 8192];
        for _ in 0..2048 {
            if stdin.write_all(&chunk).is_err() {
                break;
            }
            producer_written.fetch_add(chunk.len(), Ordering::Release);
        }
    });

    // Deliberately do not read stdout: CIX must be able to leave a blocked
    // application write when interrupted rather than publish a footer later.
    let ready_by = Instant::now() + Duration::from_secs(2);
    while written.load(Ordering::Acquire) == 0 && Instant::now() < ready_by {
        thread::yield_now();
    }
    assert!(
        written.load(Ordering::Acquire) != 0,
        "producer never reached cix"
    );
    thread::sleep(Duration::from_millis(150));
    assert!(
        child.try_wait().expect("poll cix").is_none(),
        "cix ended before SIGINT"
    );
    unsafe {
        assert_eq!(
            libc::kill(child.id() as i32, libc::SIGINT),
            0,
            "send SIGINT"
        );
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait().expect("poll interrupted cix") {
            Some(status) => break status,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("cix did not leave stalled stdout after SIGINT");
            }
        }
    };
    let mut archive = Vec::new();
    child
        .stdout
        .take()
        .expect("cix stdout")
        .read_to_end(&mut archive)
        .expect("read interrupted archive");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("cix stderr")
        .read_to_string(&mut stderr)
        .expect("read interrupted diagnostic");
    assert!(!status.success(), "SIGINT archive unexpectedly succeeded");
    assert!(!stderr.is_empty(), "SIGINT produced no diagnostic");
    assert!(
        !has_complete_end_frame(&archive),
        "interrupted stream must not publish a complete integrity footer"
    );
}

#[test]
fn sigterm_removes_atomic_temporary_output_and_never_publishes_target() {
    let directory = TestDir::new();
    let target = directory.path.join("archive.cix");
    let target_text = target.to_str().expect("temporary path is UTF-8");
    let mut process = ChildGuard {
        child: Command::new(env!("CARGO_BIN_EXE_cix"))
            .args([
                "--stream",
                "--threads",
                "2",
                "--parallelism",
                "blocks",
                "--route",
                "raw",
                "--block-size",
                "4KiB",
                "-f",
                "-o",
                target_text,
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn cix"),
    };
    let mut stdin = process.child.stdin.take().expect("cix stdin");
    stdin
        .write_all(&vec![b'Q'; 8192])
        .expect("write framed input");
    stdin.flush().expect("flush framed input");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !temporary_archive_has_frame(&directory.path, "archive.cix") && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        temporary_archive_has_frame(&directory.path, "archive.cix"),
        "CIX did not produce framed temporary output before SIGTERM"
    );
    assert!(!target.exists(), "atomic target appeared before completion");
    unsafe {
        assert_eq!(
            libc::kill(process.child.id() as i32, libc::SIGTERM),
            0,
            "send SIGTERM"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match process.child.try_wait().expect("poll terminated cix") {
            Some(status) => break status,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => panic!("cix did not terminate after SIGTERM"),
        }
    };
    drop(stdin);
    let mut stderr = String::new();
    process
        .child
        .stderr
        .take()
        .expect("cix stderr")
        .read_to_string(&mut stderr)
        .expect("read SIGTERM diagnostic");
    assert!(!status.success(), "SIGTERM encoding unexpectedly succeeded");
    assert!(!stderr.is_empty(), "SIGTERM produced no diagnostic");
    assert!(
        !target.exists(),
        "SIGTERM published the final atomic target"
    );
    assert!(
        no_temporary_archives(&directory.path, "archive.cix"),
        "SIGTERM left a temporary archive behind"
    );
}
