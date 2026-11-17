//! Named outputs must not replace an input through an alternate path spelling.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static UNIQUE: AtomicUsize = AtomicUsize::new(0);
const INPUT: &[u8] = b"synthetic path-identity input\n";
const EXISTING: &[u8] = b"synthetic existing output\n";
const MODES: [&[&str]; 2] = [&["--stream", "--fast"], &["--output-format", "gzip"]];

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let serial = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cix-path-identity-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create owned synthetic directory");
        Self(path)
    }

    fn names(&self) -> Vec<std::ffi::OsString> {
        let mut names: Vec<_> = fs::read_dir(&self.0)
            .expect("read synthetic directory")
            .map(|entry| entry.expect("read synthetic entry").file_name())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(directory: &Path, mode: &[&str], args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cix"))
        .current_dir(directory)
        .args(mode)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cix in synthetic directory");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(input)
        .expect("write bounded synthetic stdin");
    child.wait_with_output().expect("collect cix output")
}

fn assert_alias_refused(directory: &Directory, mode: &[&str], output_path: &str) {
    let output = run(&directory.0, mode, &["-f", "-o", output_path, "sample"], b"");
    assert!(
        !output.status.success(),
        "mode={mode:?}, output={output_path}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("input and output paths must differ"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(directory.0.join("sample")).expect("read input"),
        INPUT
    );
    assert_eq!(directory.names(), vec![std::ffi::OsString::from("sample")]);
}

#[test]
fn cli_path_identity_refuses_forced_relative_alias_before_writes() {
    for mode in MODES {
        let directory = Directory::new();
        fs::write(directory.0.join("sample"), INPUT).expect("write input");
        assert_alias_refused(&directory, mode, "./sample");
    }
}

#[test]
fn cli_path_identity_refuses_forced_absolute_alias_before_writes() {
    for mode in MODES {
        let directory = Directory::new();
        fs::write(directory.0.join("sample"), INPUT).expect("write input");
        let absolute = fs::canonicalize(directory.0.join("sample")).expect("resolve input");
        assert_alias_refused(
            &directory,
            mode,
            absolute.to_str().expect("synthetic path UTF-8"),
        );
    }
}

#[test]
fn cli_path_identity_force_replaces_distinct_output_but_keeps_input() {
    let directory = Directory::new();
    fs::write(directory.0.join("sample"), INPUT).expect("write input");
    fs::write(directory.0.join("archive"), EXISTING).expect("write output");
    let output = run(
        &directory.0,
        MODES[1],
        &["-f", "-o", "archive", "sample"],
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(directory.0.join("sample")).expect("read input"),
        INPUT
    );
    let archive = fs::read(directory.0.join("archive")).expect("read output");
    assert!(archive.starts_with(&[0x1f, 0x8b]), "expected gzip output");
    assert_ne!(archive, EXISTING);
    assert_eq!(
        directory.names(),
        vec![
            std::ffi::OsString::from("archive"),
            std::ffi::OsString::from("sample"),
        ]
    );
}

#[test]
fn cli_path_identity_without_force_preserves_distinct_existing_output() {
    for mode in MODES {
        let directory = Directory::new();
        fs::write(directory.0.join("sample"), INPUT).expect("write input");
        fs::write(directory.0.join("archive"), EXISTING).expect("write output");
        let output = run(&directory.0, mode, &["-o", "archive", "sample"], b"");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("refusing to overwrite"), "{stderr}");
        assert_eq!(
            fs::read(directory.0.join("sample")).expect("read input"),
            INPUT
        );
        assert_eq!(
            fs::read(directory.0.join("archive")).expect("read output"),
            EXISTING
        );
        assert_eq!(
            directory.names(),
            vec![
                std::ffi::OsString::from("archive"),
                std::ffi::OsString::from("sample"),
            ]
        );
    }
}

#[test]
fn cli_path_identity_stdin_dash_is_not_a_named_input_alias() {
    for mode in MODES {
        let directory = Directory::new();
        fs::write(directory.0.join("-"), EXISTING).expect("write named output");
        let output = run(&directory.0, mode, &["-f", "-o", "./-", "-"], INPUT);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        let archive = fs::read(directory.0.join("-")).expect("read named output");
        assert!(!archive.is_empty());
        assert_ne!(archive, EXISTING);
        assert_eq!(directory.names(), vec![std::ffi::OsString::from("-")]);
    }
}
