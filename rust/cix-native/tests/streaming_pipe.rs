#![cfg(unix)]

//! Process-level streaming checks.  These deliberately exercise stdin/stdout
//! pipes rather than calling the encoder directly: a frame has to cross an OS
//! pipe before the producer closes its write end.

use std::{
    io::{Read, Write},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const STARTUP: Duration = Duration::from_secs(5);
const COMPLETE: Duration = Duration::from_secs(15);

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

fn spawn(binary: &str, args: &[&str]) -> ChildGuard {
    let child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {binary}: {error}"));
    ChildGuard { child }
}

fn cix(args: &[&str]) -> ChildGuard {
    spawn(env!("CARGO_BIN_EXE_cix"), args)
}

fn uncix(args: &[&str]) -> ChildGuard {
    spawn(env!("CARGO_BIN_EXE_uncix"), args)
}

fn take_stdin(process: &mut ChildGuard) -> ChildStdin {
    process.child.stdin.take().expect("child stdin")
}

fn take_stdout(process: &mut ChildGuard) -> ChildStdout {
    process.child.stdout.take().expect("child stdout")
}

fn take_stderr(process: &mut ChildGuard) -> ChildStderr {
    process.child.stderr.take().expect("child stderr")
}

fn drain(reader: impl Read + Send + 'static, delay: Duration) -> Receiver<Vec<u8>> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0u8; 257];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if send.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                    if !delay.is_zero() {
                        thread::sleep(delay);
                    }
                }
                Err(error) => panic!("pipe read: {error}"),
            }
        }
    });
    receive
}

fn wait(process: &mut ChildGuard, timeout: Duration) -> (ExitStatus, String) {
    let started = Instant::now();
    loop {
        match process.child.try_wait() {
            Ok(Some(status)) => {
                let mut stderr = String::new();
                take_stderr(process)
                    .read_to_string(&mut stderr)
                    .expect("read child stderr");
                return (status, stderr);
            }
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = process.child.kill();
                let _ = process.child.wait();
                panic!("child did not exit within {:?}", timeout);
            }
            Err(error) => panic!("wait child: {error}"),
        }
    }
}

fn collect(receiver: &Receiver<Vec<u8>>, timeout: Duration) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        match receiver.recv_timeout(remaining) {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(mpsc::RecvTimeoutError::Disconnected) => return bytes,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("stdout did not close within {:?}", timeout)
            }
        }
    }
}

fn first_chunk(receiver: &Receiver<Vec<u8>>, timeout: Duration) -> Vec<u8> {
    receiver
        .recv_timeout(timeout)
        .unwrap_or_else(|_| panic!("no stdout before producer EOF within {:?}", timeout))
}

/// Accumulate enough pipe reads to prove one entire CIXG frame has arrived.
/// A pipe is allowed to split a single encoder write, so one `read` is not a
/// framing boundary.
fn first_complete_frame(receiver: &Receiver<Vec<u8>>, timeout: Duration) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= 22 {
            let payload =
                u32::from_le_bytes(bytes[18..22].try_into().expect("frame payload length"))
                    as usize;
            let end = 54usize.checked_add(payload).expect("frame length overflow");
            if bytes.len() >= end {
                return bytes;
            }
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        match receiver.recv_timeout(remaining) {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(_) => panic!(
                "no complete CIX frame before producer EOF within {:?}",
                timeout
            ),
        }
    }
}

fn write_irregular(stdin: &mut ChildStdin, input: &[u8]) {
    let chunks = [1usize, 19, 3, 241, 17, 1021, 11, 509];
    let mut offset = 0;
    let mut index = 0;
    while offset < input.len() {
        let end = (offset + chunks[index % chunks.len()]).min(input.len());
        stdin
            .write_all(&input[offset..end])
            .expect("write irregular producer chunk");
        stdin.flush().expect("flush irregular producer chunk");
        offset = end;
        index += 1;
    }
}

fn decode_all(archive: &[u8]) -> Vec<u8> {
    let mut process = uncix(&["--stream", "-c", "-"]);
    let mut stdin = take_stdin(&mut process);
    let stdout = drain(take_stdout(&mut process), Duration::ZERO);
    write_irregular(&mut stdin, archive);
    drop(stdin);
    let (status, stderr) = wait(&mut process, COMPLETE);
    assert!(status.success(), "uncix failed: {stderr}");
    collect(&stdout, COMPLETE)
}

fn raw_stream_archive(input: &[u8], block_size: &str) -> Vec<u8> {
    let mut process = cix(&[
        "--stream",
        "--threads",
        "2",
        "--parallelism",
        "blocks",
        "--route",
        "raw",
        "--block-size",
        block_size,
        "-c",
        "-",
    ]);
    let mut stdin = take_stdin(&mut process);
    let stdout = drain(take_stdout(&mut process), Duration::ZERO);
    write_irregular(&mut stdin, input);
    drop(stdin);
    let (status, stderr) = wait(&mut process, COMPLETE);
    assert!(status.success(), "cix failed: {stderr}");
    collect(&stdout, COMPLETE)
}

fn first_frame_end(archive: &[u8]) -> usize {
    assert!(archive.len() >= 54, "archive lacks a CIX header and frame");
    assert!(matches!(&archive[..5], b"CIXG1" | b"CIXG2"));
    let payload =
        u32::from_le_bytes(archive[18..22].try_into().expect("frame payload length")) as usize;
    let end = 54usize.checked_add(payload).expect("frame length overflow");
    assert!(archive.len() >= end, "archive truncates the first frame");
    end
}

fn synthetic(length: usize) -> Vec<u8> {
    let mut state = 0x51a7_9e23u32;
    (0..length)
        .map(|index| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as u8).wrapping_add((index % 29) as u8)
        })
        .collect()
}

#[test]
fn nonseekable_irregular_producer_emits_a_frame_before_eof_and_round_trips() {
    let input = synthetic(12 * 1024 + 73);
    let mut process = cix(&[
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
    ]);
    let mut stdin = take_stdin(&mut process);
    let stdout = drain(take_stdout(&mut process), Duration::ZERO);

    // Keep stdin open after exactly two full blocks.  The output event proves
    // the encoder did not wait for a length or EOF.
    write_irregular(&mut stdin, &input[..8 * 1024]);
    let mut archive = first_complete_frame(&stdout, STARTUP);
    stdin
        .write_all(&input[8 * 1024..])
        .expect("write final producer bytes");
    drop(stdin);

    let (status, stderr) = wait(&mut process, COMPLETE);
    assert!(status.success(), "cix failed: {stderr}");
    archive.extend_from_slice(&collect(&stdout, COMPLETE));
    assert_eq!(decode_all(&archive), input);
}

#[test]
fn idle_input_flushes_a_partial_frame_before_eof() {
    let input = b"an idle non-seekable producer still needs a framed flush";
    let mut process = cix(&[
        "--stream",
        "--threads",
        "2",
        "--parallelism",
        "blocks",
        "--route",
        "raw",
        "--block-size",
        "64KiB",
        "--flush-interval",
        "100ms",
        "-c",
        "-",
    ]);
    let mut stdin = take_stdin(&mut process);
    let stdout = drain(take_stdout(&mut process), Duration::ZERO);
    stdin.write_all(input).expect("write idle producer bytes");
    stdin.flush().expect("flush idle producer bytes");

    let mut archive = first_complete_frame(&stdout, STARTUP);
    drop(stdin);
    let (status, stderr) = wait(&mut process, COMPLETE);
    assert!(status.success(), "cix idle flush failed: {stderr}");
    archive.extend_from_slice(&collect(&stdout, COMPLETE));
    assert_eq!(decode_all(&archive), input);
}

#[test]
fn decoder_reconstructs_a_complete_frame_before_archive_eof() {
    let input = synthetic(10 * 1024 + 101);
    let archive = raw_stream_archive(&input, "4KiB");
    let first = first_frame_end(&archive);
    assert!(first < archive.len(), "fixture unexpectedly has no footer");

    let mut process = uncix(&["--stream", "-c", "-"]);
    let mut stdin = take_stdin(&mut process);
    let stdout = drain(take_stdout(&mut process), Duration::ZERO);
    write_irregular(&mut stdin, &archive[..first]);
    let mut restored = first_chunk(&stdout, STARTUP);
    assert!(
        !restored.is_empty(),
        "uncix did not reconstruct the first complete frame before archive EOF"
    );
    stdin
        .write_all(&archive[first..])
        .expect("write archive remainder");
    drop(stdin);
    let (status, stderr) = wait(&mut process, COMPLETE);
    assert!(status.success(), "uncix failed: {stderr}");
    restored.extend_from_slice(&collect(&stdout, COMPLETE));
    assert_eq!(restored, input);
}

#[test]
fn empty_truncated_and_corrupt_streams_have_defined_pipe_behaviour() {
    let archive = raw_stream_archive(&[], "4KiB");
    assert_eq!(decode_all(&archive), b"");

    for (label, broken) in [
        ("truncated", archive[..archive.len() - 1].to_vec()),
        ("corrupt", {
            let source = synthetic(4096);
            let mut bytes = raw_stream_archive(&source, "4KiB");
            let payload_start = 54;
            bytes[payload_start] ^= 0x80;
            bytes
        }),
    ] {
        let mut process = uncix(&["--stream", "-c", "-"]);
        let mut stdin = take_stdin(&mut process);
        let stdout = drain(take_stdout(&mut process), Duration::ZERO);
        write_irregular(&mut stdin, &broken);
        drop(stdin);
        let (status, stderr) = wait(&mut process, COMPLETE);
        assert!(!status.success(), "{label} archive unexpectedly accepted");
        assert!(!stderr.is_empty(), "{label} archive had no diagnostic");
        let _ = collect(&stdout, COMPLETE);
    }
}

#[test]
fn slow_downstream_consumer_preserves_bytes_without_deadlock() {
    let input = synthetic(160 * 1024 + 313);
    let mut process = cix(&[
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
    ]);
    let mut stdin = take_stdin(&mut process);
    // A deliberately slow reader forces the encoder to observe pipe
    // backpressure.  The test asserts completion and exact reconstruction,
    // not a timing threshold.
    let stdout = drain(take_stdout(&mut process), Duration::from_millis(2));
    write_irregular(&mut stdin, &input);
    drop(stdin);
    let (status, stderr) = wait(&mut process, COMPLETE);
    assert!(
        status.success(),
        "cix stalled or failed with slow consumer: {stderr}"
    );
    let archive = collect(&stdout, COMPLETE);
    assert_eq!(decode_all(&archive), input);
}

#[test]
fn parallel_stream_can_exceed_its_working_memory_budget() {
    // `--memory` admits only the bounded worker/result buffers. The source is
    // deliberately larger, proving the pipe reader did not silently spool the
    // complete input before it emitted or decoded frames.
    let input = synthetic(10 * 1024 * 1024 + 313);
    let mut process = cix(&[
        "--stream",
        "--threads",
        "2",
        "--parallelism",
        "blocks",
        "--route",
        "raw",
        "--block-size",
        "4KiB",
        "--memory",
        "9MiB",
        "-c",
        "-",
    ]);
    let mut stdin = take_stdin(&mut process);
    let stdout = drain(take_stdout(&mut process), Duration::ZERO);
    write_irregular(&mut stdin, &input);
    drop(stdin);
    let (status, stderr) = wait(&mut process, Duration::from_secs(30));
    assert!(status.success(), "parallel bounded stream failed: {stderr}");
    let archive = collect(&stdout, Duration::from_secs(30));
    assert_eq!(decode_all(&archive), input);
}

#[test]
fn fast_lz_low_memory_stream_emits_before_eof_and_exceeds_budget() {
    let input = synthetic(20 * 1024 * 1024 + 313);
    let mut process = cix(&[
        "--stream",
        "--fast",
        "--threads",
        "1",
        "--route",
        "lz",
        "--memory",
        "8MiB",
        "--flush-interval",
        "50ms",
        "-c",
        "-",
    ]);
    let mut stdin = take_stdin(&mut process);
    let stdout = drain(take_stdout(&mut process), Duration::ZERO);
    // A partial FAST block must cross the pipe while the producer stays open.
    write_irregular(&mut stdin, &input[..2048]);
    let mut archive = first_complete_frame(&stdout, STARTUP);
    write_irregular(&mut stdin, &input[2048..]);
    drop(stdin);
    let (status, stderr) = wait(&mut process, Duration::from_secs(30));
    assert!(status.success(), "FAST bounded LZ failed: {stderr}");
    archive.extend_from_slice(&collect(&stdout, Duration::from_secs(30)));
    assert_eq!(decode_all(&archive), input);
}
