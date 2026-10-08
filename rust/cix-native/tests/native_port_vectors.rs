//! Tiny generated compatibility vectors, independent of evaluation datasets.
use cix_native::full_engine::{address_relations, graph_residuals};

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/native_ports")
        .join(name);
    std::fs::read(path).unwrap()
}

#[test]
fn graph_frames_match_preserved_reference_for_all_supported_modes() {
    let input = fixture("graph.input");
    for mode in 0..=2 {
        for representation in 0..=1 {
            let expected = fixture(&format!("graph-{mode}-{representation}.frame"));
            assert_eq!(
                graph_residuals::encode(&input, mode, representation).unwrap(),
                expected
            );
            assert_eq!(
                graph_residuals::decode_with_limit(&expected, input.len()).unwrap(),
                input
            );
            assert!(graph_residuals::decode_with_limit(&expected, input.len() - 1).is_err());
        }
    }
}

#[test]
fn pointer_frames_match_preserved_reference_for_all_modes() {
    let input = fixture("pointer.input");
    for mode in 0..=5 {
        let expected = fixture(&format!("pointer-{mode}.frame"));
        assert_eq!(
            address_relations::pointer_delta_transform(&input, mode).unwrap(),
            expected,
            "pointer mode {mode}"
        );
        assert_eq!(
            address_relations::pointer_delta_inverse(&expected).unwrap(),
            input
        );
    }
}

#[test]
fn palette_frames_match_preserved_reference_with_grouped_tokens_and_escapes() {
    let input = fixture("palette.input");
    for mode in 0..=2 {
        for palette_size in [1, 2, 64] {
            let expected = fixture(&format!("palette-{mode}-{palette_size}.frame"));
            assert_eq!(
                graph_residuals::encode_with_palette(&input, mode, 2, palette_size).unwrap(),
                expected,
                "mode {mode}, palette {palette_size}"
            );
            assert_eq!(
                graph_residuals::decode_with_limit(&expected, input.len()).unwrap(),
                input
            );
            assert!(graph_residuals::decode_with_limit(&expected, input.len() - 1).is_err());
        }
    }
}
