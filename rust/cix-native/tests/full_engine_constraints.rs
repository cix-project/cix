//! Focused CLI contract for whole-input native constraints.

use std::{fs, process::Command};

#[test]
fn forced_native_formats_and_block_remain_decodable() {
    let root = std::env::temp_dir().join(format!("cix-constraints-{}", std::process::id()));
    let input = root.with_extension("input");
    fs::write(&input, b"whole native constraint fixture\n".repeat(64)).unwrap();
    for level in ["--fast", "-2", "-6", "-8", "--best"] {
        for (format, magic) in [
            ("cixg1", b"CIXG1".as_slice()),
            ("cixg2", b"CIXG2".as_slice()),
            ("cixm6", b"CIXM6".as_slice()),
        ] {
            let archive = root.with_extension(format);
            let output = Command::new(env!("CARGO_BIN_EXE_cix"))
                .args([
                    level,
                    "--format",
                    format,
                    "--block-size",
                    "16384",
                    "-o",
                    archive.to_str().unwrap(),
                    input.to_str().unwrap(),
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{format}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                fs::read(&archive).unwrap().starts_with(magic),
                "{format} must not fall back to another wire"
            );
            let decoded = Command::new(env!("CARGO_BIN_EXE_uncix"))
                .args(["-c", archive.to_str().unwrap()])
                .output()
                .unwrap();
            assert!(decoded.status.success());
            assert_eq!(decoded.stdout, fs::read(&input).unwrap());
            let _ = fs::remove_file(&archive);
        }
    }
    let _ = fs::remove_file(input);
}

#[test]
fn native_tuning_constraints_and_explicit_format_remain_decodable() {
    let root = std::env::temp_dir().join(format!("cix-constraint-controls-{}", std::process::id()));
    let input = root.with_extension("input");
    let archive = root.with_extension("cix");
    let source = b"constraint controls\n".repeat(128);
    fs::write(&input, &source).unwrap();
    for (extra, forced_magic) in [
        (vec!["--block-size", "16384"], None),
        (vec!["--backend", "hybrid"], None),
        (
            vec!["--parallelism", "candidates", "--format", "cixg2"],
            Some(b"CIXG2".as_slice()),
        ),
        (vec!["--block-size", "128KiB"], None),
    ] {
        let mut args = vec!["-6"];
        args.extend(extra);
        args.extend(["-o", archive.to_str().unwrap(), input.to_str().unwrap()]);
        let output = Command::new(env!("CARGO_BIN_EXE_cix"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if let Some(magic) = forced_magic {
            assert!(fs::read(&archive).unwrap().starts_with(magic));
        }
        let decoded = Command::new(env!("CARGO_BIN_EXE_uncix"))
            .args(["-c", archive.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(decoded.status.success());
        assert_eq!(decoded.stdout, source);
        let _ = fs::remove_file(&archive);
    }
    let _ = fs::remove_file(input);
}
