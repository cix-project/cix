//! Multiple named-input CLI behavior, including pre-write safety checks.

use std::fs;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static UNIQUE: AtomicUsize = AtomicUsize::new(0);

fn directory() -> std::path::PathBuf {
    let serial = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "cix-multiple-files-{}-{serial}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create temporary directory");
    directory
}

fn command(binary: &str, directory: &std::path::Path, args: &[&str]) -> Output {
    Command::new(binary)
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run {binary}: {error}"))
}

fn cix(directory: &std::path::Path, args: &[&str]) -> Output {
    command(env!("CARGO_BIN_EXE_cix"), directory, args)
}

fn uncix(directory: &std::path::Path, args: &[&str]) -> Output {
    command(env!("CARGO_BIN_EXE_uncix"), directory, args)
}

#[test]
fn named_files_compress_and_decompress_independently() {
    let directory = directory();
    let first = b"first multiple-file fixture\n".repeat(128);
    let second = b"second multiple-file fixture\n".repeat(96);
    fs::write(directory.join("first"), &first).expect("write first input");
    fs::write(directory.join("second"), &second).expect("write second input");

    let compressed = cix(&directory, &["--fast", "first", "second"]);
    assert!(
        compressed.status.success(),
        "{}",
        String::from_utf8_lossy(&compressed.stderr)
    );
    assert!(directory.join("first.cix").is_file());
    assert!(directory.join("second.cix").is_file());

    let restored = uncix(&directory, &["-f", "first.cix", "second.cix"]);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(
        fs::read(directory.join("first")).expect("read first"),
        first
    );
    assert_eq!(
        fs::read(directory.join("second")).expect("read second"),
        second
    );
    fs::remove_dir_all(&directory).expect("remove temporary directory");
}

#[test]
fn multiple_file_errors_are_reported_after_other_inputs_run() {
    let directory = directory();
    let bytes = b"multiple-file verification fixture\n".repeat(64);
    fs::write(directory.join("good"), bytes).expect("write input");
    let compressed = cix(&directory, &["--fast", "good"]);
    assert!(compressed.status.success());

    let verified = uncix(&directory, &["-dt", "good.cix", "missing.cix"]);
    assert!(!verified.status.success());
    let stderr = String::from_utf8_lossy(&verified.stderr);
    assert!(stderr.contains("archive OK"), "{stderr}");
    assert!(stderr.contains("missing.cix"), "{stderr}");
    assert!(stderr.contains("1 of 2 input files failed"), "{stderr}");
    fs::remove_dir_all(&directory).expect("remove temporary directory");
}

#[test]
fn ambiguous_or_unsafe_multi_file_plans_fail_before_writes() {
    let directory = directory();
    fs::write(directory.join("one"), b"one").expect("write first input");
    fs::write(directory.join("two"), b"two").expect("write second input");

    let named_output = cix(&directory, &["-o", "archive.cix", "one", "two"]);
    assert!(!named_output.status.success());
    assert!(!directory.join("archive.cix").exists());

    let mixed_stdin = cix(&directory, &["-", "one"]);
    assert!(!mixed_stdin.status.success());
    assert!(!directory.join("one.cix").exists());

    // The derived output for `one` would overwrite the second input.  This
    // must fail as a complete plan before creating an archive for either file.
    fs::write(directory.join("one.cix"), b"must remain an input").expect("write colliding input");
    let collision = cix(&directory, &["one", "one.cix"]);
    assert!(!collision.status.success());
    assert_eq!(
        fs::read(directory.join("one.cix")).expect("read colliding input"),
        b"must remain an input"
    );
    fs::remove_dir_all(&directory).expect("remove temporary directory");
}

#[test]
fn multiple_files_preserve_existing_outputs_without_force() {
    let directory = directory();
    fs::write(directory.join("first"), b"first input").expect("write first input");
    fs::write(directory.join("second"), b"second input").expect("write second input");
    fs::write(directory.join("first.cix"), b"existing archive").expect("write existing archive");

    let output = cix(&directory, &["--fast", "first", "second"]);
    assert!(!output.status.success());
    assert_eq!(
        fs::read(directory.join("first.cix")).expect("read existing archive"),
        b"existing archive"
    );
    assert!(directory.join("second.cix").is_file());
    assert!(String::from_utf8_lossy(&output.stderr).contains("1 of 2 input files failed"));
    fs::remove_dir_all(&directory).expect("remove temporary directory");
}

#[test]
fn option_terminator_accepts_multiple_dash_prefixed_names() {
    let directory = directory();
    let first = b"dash-prefixed first\n".repeat(64);
    let second = b"dash-prefixed second\n".repeat(48);
    fs::write(directory.join("-first"), &first).expect("write first input");
    fs::write(directory.join("-second"), &second).expect("write second input");

    let compressed = cix(&directory, &["--fast", "--", "-first", "-second"]);
    assert!(
        compressed.status.success(),
        "{}",
        String::from_utf8_lossy(&compressed.stderr)
    );
    let restored = uncix(&directory, &["-f", "--", "-first.cix", "-second.cix"]);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(
        fs::read(directory.join("-first")).expect("read first"),
        first
    );
    assert_eq!(
        fs::read(directory.join("-second")).expect("read second"),
        second
    );
    fs::remove_dir_all(&directory).expect("remove temporary directory");
}
