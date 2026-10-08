//! Tiny, complete-archive oracle for the public CLI.
//!
//! This deliberately has a much narrower universe than an unconstrained
//! compression search: one CIXG1 frame, synthetic inputs of at most 256
//! bytes, every exposed CIXG1 route, and the five nested coders which the
//! CIXG1 CLI accepts for stream-based routes.  It counts the physical
//! header, frame, footer, and payload because it compares the child process
//! output byte-for-byte as an archive.
//!
//! Explicit exclusions: CIXG2/CIXM6/CIXF1 packages; CIXG2 context-range
//! coders; arbitrary segmentation, persistent history, and independent block
//! policies; external CIXB1 codecs; unsupported CLI overrides; and all inputs
//! larger than the fixed synthetic fixtures below.  This is a correctness
//! oracle, not a global encoding-space exhaustive search or a benchmark.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const ROUTES: [(&str, u8); 11] = [
    ("raw", 0),
    ("runs", 1),
    ("composition", 2),
    ("lz", 3),
    ("predictor", 4),
    ("bwt", 5),
    ("stride", 6),
    ("phrases", 7),
    ("ppm", 8),
    ("mixture", 9),
    ("deflate", 10),
];

const STREAM_ROUTES: [&str; 6] = ["composition", "lz", "predictor", "bwt", "stride", "phrases"];
const CIXG1_NESTED_BACKENDS: [&str; 5] = ["cix", "deflate", "hybrid", "range", "count-range"];
const CHILD_TIMEOUT: Duration = Duration::from_secs(10);
// Two fixture families: (2 intrinsic raw/runs + 6*5 nested stream routes +
// 3 intrinsic PPM/mixture/deflate + 1 auto) archives each, encoded and freshly
// decoded.  Keep the cap explicit so future fixture expansion cannot quietly
// turn this synthetic test into a subprocess farm.
const MAX_CHILD_PROCESSES: usize = 160;
static UNIQUE: AtomicUsize = AtomicUsize::new(0);

struct TempFile(PathBuf);

impl TempFile {
    fn new(label: &str, bytes: &[u8]) -> Self {
        let serial = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cix-cli-archive-oracle-{}-{}-{serial}-{label}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, bytes).expect("write temporary CIX oracle fixture");
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct ChildOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn binary(name: &str) -> &'static str {
    match name {
        "cix" => env!("CARGO_BIN_EXE_cix"),
        "uncix" => env!("CARGO_BIN_EXE_uncix"),
        _ => unreachable!("test only invokes CIX binaries"),
    }
}

fn run_child(name: &str, args: &[&str], child_count: &mut usize) -> ChildOutput {
    *child_count += 1;
    assert!(
        *child_count <= MAX_CHILD_PROCESSES,
        "cli archive oracle exceeded its subprocess cap"
    );
    let mut child = Command::new(binary(name))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {name}: {error}"));
    let mut stdout = child.stdout.take().expect("child stdout");
    let mut stderr = child.stderr.take().expect("child stderr");
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).expect("read child stdout");
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).expect("read child stderr");
        bytes
    });
    let deadline = Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        match child.try_wait().expect("poll child") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{name} exceeded {CHILD_TIMEOUT:?}: {args:?}");
            }
            None => thread::sleep(Duration::from_millis(2)),
        }
    };
    ChildOutput {
        status,
        stdout: stdout_reader.join().expect("join stdout reader"),
        stderr: stderr_reader.join().expect("join stderr reader"),
    }
}

#[derive(Clone, Debug)]
struct ArchiveCandidate {
    requested_route: &'static str,
    backend: &'static str,
    selected_route: &'static str,
    archive: Vec<u8>,
}

fn encode(
    input: &Path,
    route: &'static str,
    backend: Option<&'static str>,
    explain: bool,
    child_count: &mut usize,
) -> (Vec<u8>, String) {
    let mut args = vec![
        "--format",
        "cixg1",
        "--stream",
        "--block-size",
        "256B",
        "--memory",
        "128MiB",
        "--threads",
        "1",
        "--best",
        "--route",
        route,
    ];
    if let Some(backend) = backend {
        args.extend(["--backend", backend]);
    }
    if explain {
        args.push("--explain");
    }
    args.extend(["-c", input.to_str().expect("temporary UTF-8 path")]);
    let output = run_child("cix", &args, child_count);
    assert!(
        output.status.success(),
        "cix route={route} backend={backend:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.starts_with(b"CIXG1"));
    assert!(
        output.stdout.len() >= 13 + 41 + 41,
        "complete CIXG1 archive"
    );
    (
        output.stdout,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn fresh_decode(label: &str, archive: &[u8], expected: &[u8], child_count: &mut usize) {
    let file = TempFile::new("archive.cix", archive);
    let args = [
        "-c",
        "--memory",
        "128MiB",
        file.0.to_str().expect("temporary UTF-8 archive path"),
    ];
    let output = run_child("uncix", &args, child_count);
    assert!(
        output.status.success(),
        "fresh uncix failed for {label}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, expected, "fresh process reconstruction");
}

fn frame_route(archive: &[u8]) -> u8 {
    assert!(archive.starts_with(b"CIXG1"));
    // Five magic bytes, then the eight-byte CIXG1 stream header.
    archive[13]
}

fn route_name(id: u8) -> &'static str {
    ROUTES
        .iter()
        .find_map(|&(name, candidate_id)| (candidate_id == id).then_some(name))
        .expect("CIXG1 frame route belongs to the declared route set")
}

fn enumerate_cixg1(
    input: &Path,
    expected: &[u8],
    child_count: &mut usize,
) -> Vec<ArchiveCandidate> {
    let mut candidates = Vec::new();
    for &(route, expected_id) in &ROUTES {
        if STREAM_ROUTES.contains(&route) {
            for &backend in &CIXG1_NESTED_BACKENDS {
                let (archive, _) = encode(input, route, Some(backend), false, child_count);
                let selected_id = frame_route(&archive);
                // A route constraint keeps its non-raw candidates, while a
                // legal raw fallback is written as route 0.  It must never
                // claim the requested route while carrying raw bytes.
                assert!(
                    selected_id == expected_id || selected_id == 0,
                    "route constraint {route}:{backend} emitted unrelated route {selected_id}"
                );
                fresh_decode(
                    &format!("{route}:{backend}"),
                    &archive,
                    expected,
                    child_count,
                );
                candidates.push(ArchiveCandidate {
                    requested_route: route,
                    backend,
                    selected_route: route_name(selected_id),
                    archive,
                });
            }
        } else {
            // raw and runs have no nested stream coder.  PPM owns its model
            // (CIXG1 permits the internally selected orders 0..=3), mixture
            // owns its mixer, and deflate owns its DEFLATE stream.
            let backend = match route {
                "ppm" => "ppm",
                "mixture" => "fixedmix",
                "deflate" => "deflate",
                _ => "intrinsic",
            };
            let (archive, _) = encode(input, route, None, false, child_count);
            let selected_id = frame_route(&archive);
            assert!(
                selected_id == expected_id || selected_id == 0,
                "route constraint {route} emitted unrelated route {selected_id}"
            );
            fresh_decode(
                &format!("{route}:{backend}"),
                &archive,
                expected,
                child_count,
            );
            candidates.push(ArchiveCandidate {
                requested_route: route,
                backend,
                selected_route: route_name(selected_id),
                archive,
            });
        }
    }
    assert_eq!(candidates.len(), 35, "complete bounded CIXG1 candidate set");
    candidates
}

fn automatic_best(input: &Path, child_count: &mut usize) -> (Vec<u8>, String) {
    let args = [
        "--format",
        "cixg1",
        "--stream",
        "--block-size",
        "256B",
        "--memory",
        "128MiB",
        "--threads",
        "1",
        "--best",
        "--explain",
        "-c",
        input.to_str().expect("temporary UTF-8 path"),
    ];
    let output = run_child("cix", &args, child_count);
    assert!(
        output.status.success(),
        "automatic CIXG1 BEST failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.starts_with(b"CIXG1"));
    (
        output.stdout,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn complete_cixg1_archive_oracle_classifies_best_selector_on_tiny_synthetic_inputs() {
    let fixtures: [(&str, Vec<u8>); 2] = [
        ("run-rich", vec![b'Z'; 192]),
        (
            "deterministic-mixed",
            (0..192)
                .map(|index| ((index * 73 + (index / 7) * 19) & 0xff) as u8)
                .collect(),
        ),
    ];
    let mut child_count = 0;
    for (label, bytes) in fixtures {
        assert!(bytes.len() <= 256, "fixture bound for {label}");
        let input = TempFile::new(&format!("{label}.bin"), &bytes);
        let candidates = enumerate_cixg1(&input.0, &bytes, &mut child_count);
        let minimum = candidates
            .iter()
            .map(|candidate| candidate.archive.len())
            .min()
            .expect("bounded candidate universe");
        let winners: Vec<_> = candidates
            .iter()
            .filter(|candidate| candidate.archive.len() == minimum)
            .collect();

        let (automatic, explain) = automatic_best(&input.0, &mut child_count);
        fresh_decode("automatic", &automatic, &bytes, &mut child_count);
        let auto_route = route_name(frame_route(&automatic));
        if automatic.len() != minimum {
            let classification = if winners
                .iter()
                .any(|winner| winner.selected_route == auto_route)
            {
                "same-route scorer/tie issue"
            } else {
                "shortlist miss"
            };
            panic!(
                "{label}: automatic complete archive={}B, bounded complete minimum={}B; {classification}; auto route={auto_route}; bounded winners={:?}; explain:\\n{explain}",
                automatic.len(),
                minimum,
                winners
                    .iter()
                    .map(|winner| format!(
                        "{}:{}=>{}",
                        winner.requested_route, winner.backend, winner.selected_route
                    ))
                    .collect::<Vec<_>>(),
            );
        }
    }
    assert!(child_count <= MAX_CHILD_PROCESSES);
}
