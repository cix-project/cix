//! Local promotion gate for the high-resource CIXB1 BEST candidates.
//!
//! These are exact archive-only checks: each candidate pays its CIXB1 header,
//! payload length and SHA-256 before the decoder receives a fresh archive.

use cix_native::external::{best_candidates, decode, CandidateConfig};

fn fixture() -> Vec<u8> {
    let mut input = Vec::with_capacity(384 * 1024);
    for row in 0u32..8192 {
        input.extend_from_slice(b"CIX heavy backend qualification record=");
        input.extend_from_slice(row.to_string().as_bytes());
        input.extend_from_slice(b" lane=");
        input.extend_from_slice((row % 17).to_string().as_bytes());
        input.extend_from_slice(b" repeating payload: 0123456789abcdef\n");
    }
    input
}

#[test]
fn best_heavy_brotli_and_xz_candidates_restore_complete_archives() {
    let source = fixture();
    let required = [
        "brotli-size-q11-generic-w22",
        "xz-dict128-size-9e",
        "xz-dict128-preset6",
    ];
    for id in required {
        let descriptor = best_candidates()
            .iter()
            .find(|candidate| candidate.id == id)
            .unwrap_or_else(|| panic!("BEST must include {id}"));
        assert!(matches!(
            descriptor.config,
            CandidateConfig::BrotliQuality { .. } | CandidateConfig::XzDictionary { .. }
        ));
        let archive = descriptor
            .full_archive_encode(&source)
            .unwrap_or_else(|error| panic!("{id} encode failed: {error}"));
        assert_eq!(
            decode(&archive, 1024 * 1024 * 1024)
                .unwrap_or_else(|error| { panic!("{id} archive-only decode failed: {error}") }),
            source,
            "{id} did not restore the exact source"
        );
    }
}
