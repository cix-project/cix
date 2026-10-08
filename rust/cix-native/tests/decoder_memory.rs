//! CIXG decoder admission tests for payload/output buffers and adaptive-route
//! model state.  These run a fresh `uncix` process so the public decoder path,
//! including command-line memory parsing, is exercised.

use cix_native::{adaptive, fixedmix, ppm};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::process::{Command, Output, Stdio};

const CIXG1: &[u8; 5] = b"CIXG1";
const END: u8 = 255;

fn archive_with_magic(magic: &[u8; 5], route: u8, source: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(magic);
    out.extend_from_slice(&(source.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(route);
    out.extend_from_slice(&(source.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&Sha256::digest(source));
    out.extend_from_slice(payload);
    out.push(END);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&Sha256::digest(source));
    out
}

fn archive(route: u8, source: &[u8], payload: &[u8]) -> Vec<u8> {
    archive_with_magic(CIXG1, route, source, payload)
}

fn uncix(bytes: &[u8], memory: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_uncix"))
        .args(["-c", "--memory", memory, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn uncix");
    child
        .stdin
        .take()
        .expect("uncix stdin")
        .write_all(bytes)
        .expect("write archive to uncix");
    child.wait_with_output().expect("collect uncix output")
}

fn cix_bwt(source: &[u8]) -> Vec<u8> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cix"))
        .args([
            "-c", "--format", "cixg1", "--route", "bwt", "--memory", "128MiB", "-",
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
        .write_all(source)
        .expect("write source to cix");
    let output = child.wait_with_output().expect("collect cix output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.get(13).copied(), Some(5));
    output.stdout
}

#[test]
fn ppm_admission_charges_frame_buffers_and_model_state() {
    let source = vec![b'P'; 512];
    let payload = ppm::encode(&source, 3).expect("encode PPM fixture");
    let archive = archive(8, &source, &payload);

    let rejected = uncix(&archive, "9MiB");
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("including frame buffers"));

    // This is deliberately below the former 64 MiB threshold.  Admission is
    // based on the actual frame and a conservative route-specific estimate.
    let accepted = uncix(&archive, "10MiB");
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(accepted.stdout, source);
}

#[test]
fn mixture_admission_adds_its_dynamic_state_to_live_frame_buffers() {
    let source = vec![b'M'; 256];
    let payload = fixedmix::encode(&source);
    let archive = archive(9, &source, &payload);

    let rejected = uncix(&archive, "9MiB");
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("including frame buffers"));

    let accepted = uncix(&archive, "10MiB");
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(accepted.stdout, source);
}

#[test]
fn context_range_substream_reserves_its_model_before_decode() {
    let source = vec![b'C'; 4096];
    let context =
        adaptive::encode_general(&source, 1, 8, &[], 256).expect("encode context-range fixture");
    let mut stream = vec![1, 6];
    stream.extend_from_slice(&(source.len() as u32).to_le_bytes());
    stream.extend_from_slice(&(context.len() as u32).to_le_bytes());
    stream.extend_from_slice(&context);
    let archive = archive_with_magic(b"CIXG2", 2, &source, &stream);

    let rejected = uncix(&archive, "1MiB");
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("composition block needs"));
    assert!(rejected.stdout.is_empty());

    let accepted = uncix(&archive, "2MiB");
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(accepted.stdout, source);
}

#[test]
fn malformed_ppm_headers_fail_before_model_construction() {
    let source = b"x";
    for payload in [vec![6, 0, 0], vec![3, 9, 0], vec![3]] {
        let output = uncix(&archive(8, source, &payload), "16MiB");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }

    // CIXG1 retains its version boundary even when memory is otherwise ample.
    let output = uncix(&archive(8, source, &[3, 4, 0]), "16MiB");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("extended PPM orders"));
}

#[test]
fn truncated_payload_is_rejected_without_partial_output() {
    let source = b"x";
    let mut truncated = archive(8, source, &[3, 0, 0]);
    truncated.truncate(13 + 1 + 4 + 4 + 32 + 1);
    let output = uncix(&truncated, "16MiB");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn small_bwt_payload_cannot_bypass_its_inverse_table_budget() {
    let source: Vec<u8> = (0..64 * 1024)
        .map(|index| b"abracadabra"[index % b"abracadabra".len()])
        .collect();
    let archive = cix_bwt(&source);

    let rejected = uncix(&archive, "128KiB");
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("bwt block needs"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(rejected.stdout.is_empty());

    let accepted = uncix(&archive, "4MiB");
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(accepted.stdout, source);
}
