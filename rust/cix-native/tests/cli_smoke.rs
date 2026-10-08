//! Public CLI parsing and pipe behavior for the optional `[FILE]` interface.

use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static UNIQUE: AtomicUsize = AtomicUsize::new(0);

fn run(binary: &str, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {binary}: {error}"));
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(input)
        .expect("write child stdin");
    child.wait_with_output().expect("collect child output")
}

fn cix(args: &[&str], input: &[u8]) -> Output {
    run(env!("CARGO_BIN_EXE_cix"), args, input)
}

fn uncix(args: &[&str], input: &[u8]) -> Output {
    run(env!("CARGO_BIN_EXE_uncix"), args, input)
}

#[test]
fn implicit_stdin_stream_round_trips_and_matches_explicit_dash() {
    let input = b"implicit stdin input\n".repeat(512);
    let implicit = cix(&["--stream", "--fast", "-c"], &input);
    assert!(
        implicit.status.success(),
        "{}",
        String::from_utf8_lossy(&implicit.stderr)
    );
    let explicit = cix(&["--stream", "--fast", "-c", "-"], &input);
    assert!(
        explicit.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert_eq!(implicit.stdout, explicit.stdout);

    let restored = uncix(&["--stream", "-c"], &implicit.stdout);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(restored.stdout, input);
}

#[test]
fn implicit_stdin_decoder_rejects_a_missing_footer() {
    let input = b"footer integrity fixture\n".repeat(64);
    let archive = cix(&["--stream", "--fast", "-c"], &input);
    assert!(archive.status.success());
    let mut truncated = archive.stdout;
    truncated.truncate(truncated.len().saturating_sub(1));

    let output = uncix(&["--stream", "-c"], &truncated);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("truncated archive"));
}

#[test]
fn dash_prefixed_filename_requires_and_accepts_option_terminator() {
    let serial = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let directory =
        std::env::temp_dir().join(format!("cix-implicit-cli-{}-{serial}", std::process::id()));
    fs::create_dir(&directory).expect("create temporary directory");
    let input_path = directory.join("-input");
    fs::write(&input_path, b"dash filename fixture\n").expect("write dash filename fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_cix"))
        .current_dir(&directory)
        .args(["--fast", "-c", "--", "-input"])
        .output()
        .expect("run cix with option terminator");
    let _ = fs::remove_file(&input_path);
    let _ = fs::remove_dir(&directory);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Automatic whole-input selection may legitimately choose stored CIXR1.
    // The option-terminator contract is successful exact restoration, not a
    // particular automatically selected wire family.
    let restored = uncix(&["-c"], &output.stdout);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(restored.stdout, b"dash filename fixture\n");
}

#[test]
fn stdout_and_named_output_conflict_before_processing_input() {
    let output = cix(&["-c", "-o", "named.cix"], b"ignored");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("conflicts with a named --output"));
}
