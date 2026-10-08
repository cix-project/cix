//! Frozen complete Python-reference archives; no Python is used by this test.
use cix_native::full_engine::graph_residuals as graph;
use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/graph_residual_parity").join(name)).unwrap()
}
fn publish(name: &str, frame: &[u8]) {
    if let Some(dir) = std::env::var_os("CIX_GRAPH_NATIVE_OUTPUT") {
        std::fs::write(Path::new(&dir).join(name), frame).unwrap();
    }
}
#[test]
fn six_graph_complete_frames_and_independent_decoder_limits() {
    let input = fixture("graph.input");
    for mode in 0..=2 {
        for representation in 0..=1 {
            let name = format!("graph-{mode}-{representation}.frame");
            let expected = fixture(&name);
            let encoded = graph::encode(&input, mode, representation).unwrap();
            assert_eq!(encoded, expected, "{name}");
            assert_eq!(graph::decode_with_limit(&expected, input.len()).unwrap(), input);
            assert!(graph::decode_with_limit(&expected, input.len() - 1).is_err());
            assert!(graph::decode_with_limit(&expected, 0).is_err());
            publish(&name, &encoded);
        }
    }
}
#[test]
fn nine_palette_complete_frames_preserve_order_grouped_tokens_and_escapes() {
    let input = fixture("palette.input");
    for mode in 0..=2 {
        for palette_size in [1, 2, 64] {
            let name = format!("palette-{mode}-{palette_size}.frame");
            let expected = fixture(&name);
            let encoded = graph::encode_with_palette(&input, mode, 2, palette_size).unwrap();
            assert_eq!(encoded, expected, "{name}");
            assert_eq!(graph::decode_with_limit(&expected, input.len()).unwrap(), input);
            assert!(graph::decode_with_limit(&expected, input.len() - 1).is_err());
            publish(&name, &encoded);
        }
    }
}
#[test]
fn twelve_hydrogen_complete_frames_match_all_models_and_representations() {
    let input = fixture("hydrogen.input");
    for model in 0..=2 {
        for representation in 0..=3 {
            let name = format!("hydrogen-{model}-{representation}.frame");
            let expected = fixture(&name);
            let encoded = graph::encode_hydrogen_with_options(&input, model, representation, 6200).unwrap();
            assert_eq!(encoded, expected, "{name}");
            assert_eq!(graph::decode_hydrogen_with_limit(&expected, input.len()).unwrap(), input);
            assert!(graph::decode_hydrogen_with_limit(&expected, input.len() - 1).is_err());
            assert!(graph::decode_hydrogen_with_limit(&expected, 0).is_err());
            publish(&name, &encoded);
        }
    }
}
#[test]
fn literal_records_mixed_endings_and_signed_zero_roundtrip() {
    let original = fixture("hydrogen.input");
    let mut mixed = Vec::new();
    for (i, line) in original.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() { continue; }
        mixed.extend_from_slice(line);
        mixed.extend_from_slice([b"\r\n".as_slice(), b"\r".as_slice(), b"\n".as_slice()][i % 3]);
    }
    let at = mixed.windows(10).position(|s| s == b"    0.0000").unwrap();
    mixed[at..at + 10].copy_from_slice(b"   -0.0000");
    mixed.extend_from_slice(b"$$$$\nliteral tail\r\n\0\xff");
    for model in 0..=2 {
        for representation in 0..=3 {
            let frame = graph::encode_hydrogen_with_options(&mixed, model, representation, 6200).unwrap();
            assert_eq!(graph::decode_hydrogen_with_limit(&frame, mixed.len()).unwrap(), mixed);
        }
    }
    let literal = b"literal\r\n\0\xff$$$$\nunparsed tail";
    for mode in 0..=2 {
        for representation in 0..=1 {
            let frame = graph::encode(literal, mode, representation).unwrap();
            assert_eq!(graph::decode_with_limit(&frame, literal.len()).unwrap(), literal);
        }
    }
}
#[test]
fn malformed_headers_truncation_and_trailing_payloads_are_rejected() {
    for name in ["graph-2-0.frame", "graph-2-1.frame", "palette-2-64.frame"] {
        let frame = fixture(name);
        // Binary/palette streams require their final encoded component.
        // Decimal text's optional final LF is checked separately below.
        if name != "graph-2-1.frame" {
            assert!(graph::decode_with_limit(&frame[..frame.len()-1], 1_000_000).is_err(), "truncated {name}");
        }
        let mut trailing = frame.clone(); trailing.push(0);
        assert!(graph::decode_with_limit(&trailing, 1_000_000).is_err(), "trailing {name}");
        let mut bad_mode = frame.clone(); bad_mode[5] = 255;
        assert!(graph::decode_with_limit(&bad_mode, 1_000_000).is_err(), "mode {name}");
        let mut bad_repr = frame; bad_repr[6] = 255;
        assert!(graph::decode_with_limit(&bad_repr, 1_000_000).is_err(), "representation {name}");
    }
    for model in 0..=2 {
        for representation in 0..=3 {
            let frame = fixture(&format!("hydrogen-{model}-{representation}.frame"));
            // Every hydrogen choice representation has a decimal residual tail.
            // Optional LF and genuinely incomplete vectors are checked below.
            let mut trailing = frame.clone(); trailing.push(0);
            assert!(graph::decode_hydrogen_with_limit(&trailing, 1_000_000).is_err());
            let mut bad_mode = frame.clone(); bad_mode[5] = 255;
            assert!(graph::decode_hydrogen_with_limit(&bad_mode, 1_000_000).is_err());
            let mut bad_repr = frame; bad_repr[6] = 255;
            assert!(graph::decode_hydrogen_with_limit(&bad_repr, 1_000_000).is_err());
        }
    }
}

#[test]
fn declared_graph_coordinate_count_must_match_the_skeleton() {
    use cix_native::full_engine::arithmetic::vdecode;
    let frame = fixture("graph-2-0.frame");
    let (_, after_records) = vdecode(&frame, 7).unwrap();
    let (skeleton_len, skeleton_start) = vdecode(&frame, after_records + 1).unwrap();
    let count_at = skeleton_start + skeleton_len as usize;
    let (count, _) = vdecode(&frame, count_at).unwrap();
    assert!(count > 0 && count < 127);
    for wrong_count in [count - 1, count + 1] {
        let mut altered = frame.clone();
        altered[count_at] = wrong_count as u8;
        assert!(graph::decode_with_limit(&altered, 1_000_000).is_err());
    }
}

#[test]
fn palette_sixty_fifth_unique_vector_escapes_and_frequency_ties_keep_first_seen_order() {
    let input = fixture("palette-boundary.input");
    let expected = fixture("palette-boundary-0-64.frame");
    let encoded = graph::encode_with_palette(&input, 0, 2, 64).unwrap();
    assert_eq!(encoded, expected);
    assert_eq!(graph::decode_with_limit(&expected, input.len()).unwrap(), input);
    assert!(graph::decode_with_limit(&expected, input.len() - 1).is_err());
    publish("palette-boundary-0-64.frame", &encoded);
}

// These operations change only the decimal residual tail, not the skeleton or
// length-bound choice stream. Fixture guards make the intended mutation explicit.
fn decimal_tail_variants(frame: &[u8], payload_start: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    assert_eq!(frame.last(), Some(&b'\n'));
    let without_lf = &frame[..frame.len() - 1];
    let final_line_start = without_lf[payload_start..].iter().rposition(|b| *b == b'\n')
        .map_or(payload_start, |at| payload_start + at + 1);
    let final_line = &without_lf[final_line_start..];
    assert_eq!(final_line.iter().filter(|b| **b == b',').count(), 2);
    let last_comma = without_lf.iter().rposition(|b| *b == b',').unwrap();
    assert!(last_comma + 1 < without_lf.len());
    (without_lf.to_vec(), without_lf[..last_comma + 1].to_vec(),
     without_lf[..final_line_start].to_vec())
}

#[test]
fn text_delimiter_identity_and_incomplete_required_vectors() {
    let graph_input = fixture("graph.input");
    for mode in 0..=2 {
        let name = format!("graph-{mode}-1.frame");
        let (without_lf, empty_component, missing_vector) = decimal_tail_variants(&fixture(&name), decimal_payload_start(&fixture(&name), false));
        assert_eq!(graph::decode_with_limit(&without_lf, graph_input.len()).unwrap(), graph_input, "optional LF {name}");
        assert!(graph::decode_with_limit(&empty_component, graph_input.len()).is_err(), "empty final component {name}");
        assert!(graph::decode_with_limit(&missing_vector, graph_input.len()).is_err(), "missing required vector {name}");
        assert!(graph::decode_with_limit(&without_lf, graph_input.len() - 1).is_err(), "bounded expansion {name}");
    }
    let hydrogen_input = fixture("hydrogen.input");
    for model in 0..=2 {
        for representation in 0..=3 {
            let name = format!("hydrogen-{model}-{representation}.frame");
            let (without_lf, empty_component, missing_vector) = decimal_tail_variants(&fixture(&name), decimal_payload_start(&fixture(&name), true));
            assert_eq!(graph::decode_hydrogen_with_limit(&without_lf, hydrogen_input.len()).unwrap(), hydrogen_input, "optional LF {name}");
            assert!(graph::decode_hydrogen_with_limit(&empty_component, hydrogen_input.len()).is_err(), "empty final component {name}");
            assert!(graph::decode_hydrogen_with_limit(&missing_vector, hydrogen_input.len()).is_err(), "missing required vector {name}");
            assert!(graph::decode_hydrogen_with_limit(&without_lf, hydrogen_input.len() - 1).is_err(), "bounded expansion {name}");
        }
    }
}

fn decimal_payload_start(frame: &[u8], hydrogen: bool) -> usize {
    use cix_native::full_engine::arithmetic::vdecode;
    let mut at = 7;
    if hydrogen { at = vdecode(frame, at).unwrap().1; } // radius
    let (records, after_records) = vdecode(frame, at).unwrap();
    at = after_records + (records as usize + 7) / 8;
    let (skeleton_len, skeleton_start) = vdecode(frame, at).unwrap();
    at = skeleton_start + skeleton_len as usize;
    let (count_or_choices, start) = vdecode(frame, at).unwrap();
    if hydrogen { start + count_or_choices as usize } else { start }
}
