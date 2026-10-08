//! CLI integration for the independent CIXW1 transport.

use std::{
    io::{Read, Write},
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

fn run(binary: &str, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn CIX command");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(input)
        .expect("write source");
    child.wait_with_output().expect("collect CIX command")
}

#[test]
fn stdin_uses_independent_windows_and_shared_decode_test_list_paths() {
    let source = b"independent window CLI fixture\n".repeat(4096);
    let archive = run(env!("CARGO_BIN_EXE_cix"), &["-c", "-"], &source);
    assert!(
        archive.status.success(),
        "{}",
        String::from_utf8_lossy(&archive.stderr)
    );
    assert!(archive.stdout.starts_with(b"CIXW1"));

    let restored = run(env!("CARGO_BIN_EXE_uncix"), &["-c", "-"], &archive.stdout);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(restored.stdout, source);

    let tested = run(env!("CARGO_BIN_EXE_uncix"), &["-t", "-"], &archive.stdout);
    assert!(
        tested.status.success(),
        "{}",
        String::from_utf8_lossy(&tested.stderr)
    );

    let listed = run(env!("CARGO_BIN_EXE_uncix"), &["-l", "-"], &archive.stdout);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert!(String::from_utf8_lossy(&listed.stderr).contains("WindowIncremental"));
}

#[test]
fn explicit_legacy_stream_and_explicit_backend_keep_their_contracts() {
    let source = b"window control preservation\n".repeat(2048);
    let stream = run(env!("CARGO_BIN_EXE_cix"), &["--stream", "-c", "-"], &source);
    assert!(
        stream.status.success(),
        "{}",
        String::from_utf8_lossy(&stream.stderr)
    );
    assert!(stream.stdout.starts_with(b"CIXG") || stream.stdout.starts_with(b"CIXZ1"));
    assert!(!stream.stdout.starts_with(b"CIXW1"));

    let constrained = run(
        env!("CARGO_BIN_EXE_cix"),
        &["--backend", "raw", "-c", "-"],
        &source,
    );
    assert!(
        constrained.status.success(),
        "{}",
        String::from_utf8_lossy(&constrained.stderr)
    );
    assert!(constrained.stdout.starts_with(b"CIXW1"));
    let restored = run(
        env!("CARGO_BIN_EXE_uncix"),
        &["-c", "-"],
        &constrained.stdout,
    );
    assert!(restored.status.success());
    assert_eq!(restored.stdout, source);
}

#[test]
fn completed_window_is_published_before_stdin_eof() {
    // 36,200,000 B admits a 64 KiB selector window but not its 128 KiB
    // predecessor. This keeps the host-level early-output proof modest.
    let source = vec![b'W'; 65 << 10];
    let mut child = Command::new(env!("CARGO_BIN_EXE_cix"))
        .args(["--memory", "36200000B", "-c", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn CIX window encoder");
    let mut stdout = child.stdout.take().expect("child stdout");
    let (record_tx, record_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut prefix = [0u8; 11];
        let result = stdout.read_exact(&mut prefix).map(|_| prefix);
        let _ = record_tx.send(result);
        let mut remainder = Vec::new();
        stdout.read_to_end(&mut remainder).expect("drain archive");
        remainder
    });
    let mut stdin = child.stdin.take().expect("child stdin");
    let (written_tx, written_rx) = mpsc::channel();
    let (close_tx, close_rx) = mpsc::channel();
    let writer = thread::spawn(move || {
        stdin
            .write_all(&source)
            .expect("write first complete window");
        stdin.flush().expect("flush first complete window");
        written_tx.send(()).expect("report written window");
        close_rx.recv().expect("release EOF");
    });

    written_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("writer must reach the first window without pipe deadlock");
    let prefix = record_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("completed CIXW1 record must arrive before EOF")
        .expect("read CIXW1 record prefix");
    assert_eq!(&prefix[..5], b"CIXW1");
    assert_eq!(prefix[10], 1, "first window record tag");

    close_tx.send(()).expect("send EOF");
    writer.join().expect("writer thread");
    let status = child.wait().expect("wait for encoder");
    let archive_tail = reader.join().expect("reader thread");
    let mut archive = prefix.to_vec();
    archive.extend_from_slice(&archive_tail);
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("child stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(status.success(), "{stderr}");
    let restored = run(env!("CARGO_BIN_EXE_uncix"), &["-c", "-"], &archive);
    assert!(restored.status.success());
    assert_eq!(restored.stdout, vec![b'W'; 65 << 10]);
}
