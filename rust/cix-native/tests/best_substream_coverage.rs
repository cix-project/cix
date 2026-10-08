//! Newly eligible legal BEST route/backend pairs must actually reconstruct.
use std::{fs, process::Command};
#[test]
fn every_new_best_substream_pair_fresh_decodes() {
    let area = std::env::temp_dir().join(format!("cix-best-pairs-{}", std::process::id()));
    fs::create_dir(&area).unwrap();
    let source = b"input-derived lanes and repeated fragments\0\x01\x02".repeat(8);
    let input = area.join("input");
    fs::write(&input, &source).unwrap();
    for route in ["composition", "lz", "predictor", "bwt", "stride", "phrases"] {
        for backend in [
            "context-range-o1-b4",
            "context-range-o1-b8",
            "context-range-o2-b8",
            "huffman",
        ] {
            let archive = area.join(format!("{route}-{backend}.cix"));
            let out = Command::new(env!("CARGO_BIN_EXE_cix"))
                .args([
                    "--best",
                    "--stream",
                    "--format",
                    "cixg2",
                    "--route",
                    route,
                    "--backend",
                    backend,
                    "--threads",
                    "1",
                    "-o",
                ])
                .arg(&archive)
                .arg(&input)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{route}/{backend}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let restored = Command::new(env!("CARGO_BIN_EXE_uncix"))
                .arg("-c")
                .arg(&archive)
                .output()
                .unwrap();
            assert!(
                restored.status.success(),
                "{route}/{backend}: {}",
                String::from_utf8_lossy(&restored.stderr)
            );
            assert_eq!(restored.stdout, source, "{route}/{backend}");
        }
    }
    fs::remove_dir_all(area).unwrap();
}
