//! Direct whole-input wrappers keep canonical bytes while admitting small sources.
use sha2::{Digest, Sha256};
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run(binary: &str, args: &[&str], data: &[u8]) -> Output {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for part in data.chunks(997) {
        input.write_all(part).unwrap();
    }
    drop(input);
    child.wait_with_output().unwrap()
}

#[test]
fn small_sources_use_canonical_external_archives_below_former_reservation() {
    let directory = std::env::temp_dir().join(format!("cix-source-buffer-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("input");
    for backend in ["gzip", "bzip2"] {
        for data in [
            Vec::new(),
            b"generic source buffer test\0".repeat(80),
            vec![b'X'; 65_537],
        ] {
            std::fs::write(&path, &data).unwrap();
            let encoded = run(
                env!("CARGO_BIN_EXE_cix"),
                &[
                    "--external",
                    backend,
                    "--memory",
                    "64MiB",
                    "-c",
                    path.to_str().unwrap(),
                ],
                &[],
            );
            assert!(
                encoded.status.success(),
                "{}",
                String::from_utf8_lossy(&encoded.stderr)
            );
            let expected = cix_native::external::encode(backend, "default", &data).unwrap();
            assert_eq!(
                encoded.stdout, expected,
                "{backend}: canonical complete archive"
            );
            println!("backend={backend} input_bytes={} input_sha={:x} complete_bytes={} archive_sha={:x}", data.len(), Sha256::digest(&data), encoded.stdout.len(), Sha256::digest(&encoded.stdout));
            let restored = run(
                env!("CARGO_BIN_EXE_uncix"),
                &["--memory", "64MiB", "-c", "-"],
                &encoded.stdout,
            );
            assert!(
                restored.status.success(),
                "{}",
                String::from_utf8_lossy(&restored.stderr)
            );
            assert_eq!(restored.stdout, data);
            let truncated = run(
                env!("CARGO_BIN_EXE_uncix"),
                &["--memory", "64MiB", "-c", "-"],
                &encoded.stdout[..encoded.stdout.len() - 1],
            );
            assert_eq!(truncated.status.code(), Some(2));
        }
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
