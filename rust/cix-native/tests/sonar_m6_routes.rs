//! Test-only CIXM6 frame construction for legacy mode regression checks.

#[path = "../src/bwt_legacy.rs"]
mod bwt_legacy;
#[path = "../src/combinatorics.rs"]
mod combinatorics;
#[path = "../src/sparse_runs.rs"]
mod sparse_runs;

use cix_native::{adaptive, limits, ppm, rank};
use std::path::Path;

fn put_varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

fn frame(mode: u8, source: &[u8], payload: Vec<u8>) -> Vec<u8> {
    let mut archive = b"CIXM6".to_vec();
    put_varint(1 << 20, &mut archive);
    archive.push(mode);
    put_varint(source.len(), &mut archive);
    put_varint(payload.len(), &mut archive);
    archive.extend(payload);
    archive.push(255);
    put_varint(source.len(), &mut archive);
    archive.extend_from_slice(&crc32fast::hash(source).to_le_bytes());
    archive
}

fn emit(path: &Path, name: &str, mode: u8, source: Vec<u8>, payload: Vec<u8>) {
    std::fs::write(path.join(format!("{name}.source")), &source).unwrap();
    std::fs::write(path.join(format!("{name}.payload")), &payload).unwrap();
    std::fs::write(
        path.join(format!("{name}.cix")),
        frame(mode, &source, payload),
    )
    .unwrap();
}

#[test]
fn emit_complete_legacy_mode_archives() {
    let directory = std::env::var_os("CIX_M6_FIXTURE_DIR").map(std::path::PathBuf::from);
    let Some(directory) = directory else {
        return;
    };
    std::fs::create_dir_all(&directory).unwrap();

    let bwt = b"banana_bandana::abracadabra::".repeat(1500);
    emit(
        &directory,
        "mode4",
        4,
        bwt.clone(),
        bwt_legacy::encode(&bwt).unwrap().0,
    );

    let mut sparse = vec![0u8; 65_536];
    for (index, value) in (0..sparse.len()).step_by(977).enumerate() {
        sparse[value] = (index as u8).wrapping_add(1);
    }
    emit(
        &directory,
        "mode11",
        11,
        sparse.clone(),
        sparse_runs::encode_sparse(&sparse).unwrap(),
    );

    let mut runs = vec![b'A'; 30_000];
    runs.extend(vec![b'B'; 20_000]);
    runs.extend(vec![b'C'; 10_000]);
    emit(
        &directory,
        "mode12",
        12,
        runs.clone(),
        sparse_runs::encode_runs(&runs).unwrap(),
    );
}
