#![cfg(unix)]

use cix_native::{limits, stdout_writer::StdoutWriter};
use std::fs::{read, remove_file, File};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

fn interrupt_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn pipe() -> (File, File) {
    let mut fds = [-1; 2];
    // SAFETY: fds points at two writable c_int slots.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: pipe returned two fresh owned file descriptors.
    unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
}

#[test]
fn writes_complete_buffer_through_partial_nonblocking_pipe_writes() {
    let _serial = interrupt_lock().lock().unwrap();
    limits::INTERRUPTED.store(false, std::sync::atomic::Ordering::Relaxed);
    let (reader, writer_fd) = pipe();
    let writer = StdoutWriter::from_fd(writer_fd.as_raw_fd()).unwrap();
    let payload = (0..512 * 1024).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    let expected = payload.clone();

    let producer = thread::spawn(move || {
        let mut writer = writer;
        writer.write_all(&payload).unwrap();
    });
    // The writer owns a duplicate. Closing the original here lets the reader
    // see EOF exactly when the producer drops that duplicate.
    drop(writer_fd);
    let mut received = Vec::new();
    let reader_thread = thread::spawn(move || {
        let mut chunk = [0u8; 997];
        loop {
            // SAFETY: the slice is valid for this read call.
            let count = unsafe {
                libc::read(
                    reader.as_raw_fd(),
                    chunk.as_mut_ptr().cast::<libc::c_void>(),
                    chunk.len(),
                )
            };
            if count == 0 {
                break;
            }
            assert!(count > 0);
            received.extend_from_slice(&chunk[..count as usize]);
            thread::sleep(Duration::from_millis(1));
        }
        received
    });
    producer.join().unwrap();
    let received = reader_thread.join().unwrap();
    assert_eq!(received, expected);
}

#[test]
fn stopped_pipe_is_interrupted_within_bounded_poll_interval() {
    let _serial = interrupt_lock().lock().unwrap();
    limits::INTERRUPTED.store(false, std::sync::atomic::Ordering::Relaxed);
    let (_reader, writer_fd) = pipe();
    let writer = StdoutWriter::from_fd(writer_fd.as_raw_fd()).unwrap();
    let started = Instant::now();
    let worker = thread::spawn(move || {
        let mut writer = writer;
        writer.write_all(&vec![0x5a; 1024 * 1024])
    });
    thread::sleep(Duration::from_millis(20));
    limits::INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
    let error = worker.join().unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(started.elapsed() < Duration::from_millis(250));
    limits::INTERRUPTED.store(false, std::sync::atomic::Ordering::Relaxed);
    drop(writer_fd);
}

#[test]
fn stalled_pipe_observes_pipeline_peer_cancellation() {
    let _serial = interrupt_lock().lock().unwrap();
    limits::INTERRUPTED.store(false, Ordering::Relaxed);
    let (_reader, writer_fd) = pipe();
    let writer = StdoutWriter::from_fd(writer_fd.as_raw_fd()).unwrap();
    let peer_cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&peer_cancelled);
    let started = Instant::now();
    let worker = thread::spawn(move || {
        let _guard = limits::CancellationGuard::new(worker_cancelled);
        let mut writer = writer;
        writer.write_all(&vec![0xa5; 1024 * 1024])
    });
    thread::sleep(Duration::from_millis(20));
    peer_cancelled.store(true, Ordering::Relaxed);
    let error = worker.join().unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(started.elapsed() < Duration::from_millis(250));
    drop(writer_fd);
}

#[test]
fn restores_nonblocking_state_and_writes_regular_file() {
    let _serial = interrupt_lock().lock().unwrap();
    limits::INTERRUPTED.store(false, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cix-stdout-writer-{}-{}.tmp",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let file = File::create(&path).unwrap();
    let original = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    assert!(original >= 0);
    {
        let mut writer = StdoutWriter::from_fd(file.as_raw_fd()).unwrap();
        let changed = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(changed & libc::O_NONBLOCK, 0);
        writer.write_all(b"regular-output").unwrap();
        writer.flush().unwrap();
    }
    let restored = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    assert_eq!(restored & libc::O_NONBLOCK, original & libc::O_NONBLOCK);
    drop(file);
    assert_eq!(read(&path).unwrap(), b"regular-output");
    remove_file(path).unwrap();
}
