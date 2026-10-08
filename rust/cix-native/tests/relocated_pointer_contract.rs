//! ECRP1 reference-byte and framing-logic contracts. Callback payloads below
//! are deliberate unit fixtures, not Brotli/PAQ streams or physical archives.
use cix_native::full_engine::{
    address_relations as address,
    arithmetic::{vdecode, vencode},
    catalogue,
};
use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ecoff_relocated_reference")
            .join(name),
    )
    .unwrap()
}
fn raw(mode: u8, shared: bool) -> Vec<u8> {
    fixture(&format!("mode{mode}-shared{}.raw", u8::from(shared)))
}
fn parts(raw: &[u8]) -> (&[u8], &[u8]) {
    let size = u64::from_le_bytes(raw[raw.len() - 13..raw.len() - 5].try_into().unwrap()) as usize;
    let at = raw.len() - 13 - size;
    (&raw[..at], &raw[at..raw.len() - 13])
}
fn packed(blob: &[u8]) -> Vec<u8> {
    let mut result = vencode(blob.len() as u64);
    result.extend_from_slice(blob);
    result
}
fn unpack(blob: &[u8], at: usize) -> (&[u8], usize) {
    let (n, start) = vdecode(blob, at).unwrap();
    let end = start + n as usize;
    (&blob[start..end], end)
}
fn assemble(skeleton: &[u8], metadata: &[u8]) -> Vec<u8> {
    let mut result = skeleton.to_vec();
    result.extend_from_slice(metadata);
    result.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    result.extend_from_slice(b"ECRP1");
    result
}
fn metadata(mode: u8, shared: bool, flags: &[u8], choices: &[u8], exceptions: &[u8]) -> Vec<u8> {
    let mut result = vec![mode, u8::from(shared)];
    result.extend_from_slice(&packed(flags));
    result.extend_from_slice(&packed(choices));
    result.extend_from_slice(exceptions);
    result
}

#[test]
fn all_twelve_raw_vectors_match_independent_reference_and_restore_exactly() {
    let original = fixture("original.tar");
    for mode in 0..=5 {
        for shared in [false, true] {
            let expected = raw(mode, shared);
            assert_eq!(
                address::relocated_pointer_transform(&original, mode, shared).unwrap(),
                expected
            );
            assert_eq!(
                address::relocated_pointer_inverse(&expected).unwrap(),
                original
            );
        }
    }
}

#[test]
fn local_shared_membership_and_literal_escape_costs_match_the_fixture() {
    let local = raw(1, false);
    let shared = raw(1, true);
    assert_eq!(unpack(parts(&local).1, 2).0, &[0b0001]);
    assert_eq!(unpack(parts(&shared).1, 2).0, &[0b1011]);
    for shared in [false, true] {
        let value = raw(2, shared);
        let (_, meta) = parts(&value);
        let (flags, next) = unpack(meta, 2);
        let (choices, next) = unpack(meta, next);
        assert!(flags.is_empty());
        assert_eq!(
            choices,
            if shared {
                &[0, 1, 3, 1][..]
            } else {
                &[0, 1, 1, 1][..]
            }
        );
        assert_eq!(meta.len() - next, if shared { 8 } else { 24 });
    }
    // The last reference member has an undefined local symbol, but a shared
    // definition rescues it. Modes 4/5 golden bytes also bind the causal context.
    assert_ne!(raw(5, false), raw(5, true));
}

#[test]
fn empty_admission_remains_reversible_in_every_representation() {
    for source in [&b""[..], &b"arbitrary bytes without executable tables"[..]] {
        for mode in 0..=5 {
            for shared in [false, true] {
                let encoded = address::relocated_pointer_transform(source, mode, shared).unwrap();
                assert_eq!(
                    address::relocated_pointer_inverse(&encoded).unwrap(),
                    source
                );
            }
        }
    }
    assert!(address::relocated_pointer_transform(b"", 6, false).is_err());
}

#[test]
fn malformed_trailer_lengths_and_model_tags_return_errors() {
    let valid = raw(2, false);
    for n in 0..17 {
        assert!(address::relocated_pointer_inverse(&valid[..n]).is_err());
    }
    let mut damaged = valid.clone();
    *damaged.last_mut().unwrap() ^= 1;
    assert!(address::relocated_pointer_inverse(&damaged).is_err());
    for size in [0u64, 1, 3, u64::MAX] {
        let mut damaged = valid.clone();
        let n = damaged.len();
        damaged[n - 13..n - 5].copy_from_slice(&size.to_le_bytes());
        assert!(address::relocated_pointer_inverse(&damaged).is_err());
    }
    let (skeleton, meta) = parts(&valid);
    for (at, value) in [(0, 6), (1, 2)] {
        let mut damaged = meta.to_vec();
        damaged[at] = value;
        assert!(address::relocated_pointer_inverse(&assemble(skeleton, &damaged)).is_err());
    }
}

#[test]
fn membership_choice_and_exception_corruption_is_rejected() {
    let valid = raw(2, false);
    let (skeleton, meta) = parts(&valid);
    let (_, next) = unpack(meta, 2);
    let (choices, next) = unpack(meta, next);
    let exceptions = &meta[next..];
    for damaged in [
        metadata(2, false, &[1], choices, exceptions),
        metadata(2, false, &[], &[2, 1, 1, 1], exceptions),
        metadata(2, false, &[], &[0], exceptions),
        metadata(2, false, &[], &[0, 1, 1, 1, 0], exceptions),
        metadata(2, false, &[], choices, &exceptions[..exceptions.len() - 1]),
        {
            let mut m = meta.to_vec();
            m.push(0);
            m
        },
    ] {
        assert!(address::relocated_pointer_inverse(&assemble(skeleton, &damaged)).is_err());
    }
    let literal = raw(0, false);
    assert!(address::relocated_pointer_inverse(&assemble(
        parts(&literal).0,
        &metadata(0, false, &[], &[0], &[])
    ))
    .is_err());
    let membership = raw(1, false);
    assert!(address::relocated_pointer_inverse(&assemble(
        parts(&membership).0,
        &metadata(1, false, &[], &[], &[])
    ))
    .is_err());
}

#[test]
fn nonzero_choice_skeleton_and_invalid_membership_index_are_rejected() {
    // This offset belongs solely to the checked-in synthetic fixture layout:
    // first USTAR payload (512) plus its .rdata pointer field (0x470).
    const FIELD: usize = 512 + 0x470;
    for (mode, value) in [(2, 1u64), (1, u64::MAX)] {
        let valid = raw(mode, false);
        let (skeleton, meta) = parts(&valid);
        let mut inner = address::got_inverse(skeleton).unwrap();
        inner[FIELD..FIELD + 8].copy_from_slice(&value.to_le_bytes());
        let damaged_parent = address::got_transform(&inner, 3, true).unwrap();
        assert!(address::relocated_pointer_inverse(&assemble(&damaged_parent, meta)).is_err());
    }
}

#[test]
fn callback_framing_covers_both_selectors_and_every_internal_representation() {
    let source = fixture("original.tar");
    for mode in 0..=5 {
        for shared in [false, true] {
            for (variant, selector) in [
                (address::BackendVariant::Brotli, 0),
                (address::BackendVariant::PaqV215, 1),
            ] {
                let expected = raw(mode, shared);
                let request = address::BackendRequest {
                    variant,
                    options: "unit".into(),
                    stream: 0,
                };
                let frame = address::encode_transformed_frame(
                    address::HistoricalFrame::Relocated,
                    address::Transform::RelocatedPointers { mode, shared },
                    &source,
                    request,
                    &mut |request, payload| {
                        assert_eq!(request.variant, variant);
                        assert_eq!(payload, expected);
                        Ok(b"unit-callback-payload".to_vec())
                    },
                )
                .unwrap();
                assert_eq!(&frame[..5], b"CIXB\x0a");
                assert_eq!(frame[5], selector);
                assert!(address::recognizes_magic(&frame));
                let restored = address::decode_transformed_frame(
                    address::HistoricalFrame::Relocated,
                    &frame,
                    source.len(),
                    1 << 20,
                    &mut |request, payload, maximum| {
                        assert_eq!(request.variant, variant);
                        assert_eq!(payload, b"unit-callback-payload");
                        assert!(expected.len() <= maximum);
                        Ok(expected.clone())
                    },
                )
                .unwrap();
                assert_eq!(restored, source);
            }
        }
    }
    assert!(!address::recognizes_magic(b"CIXB0a"));
}

#[test]
fn frame_output_memory_backend_and_crc_limits_are_enforced() {
    let source = fixture("original.tar");
    let value = raw(5, true);
    let request = address::BackendRequest {
        variant: address::BackendVariant::Brotli,
        options: "unit".into(),
        stream: 0,
    };
    let frame = address::encode_transformed_frame(
        address::HistoricalFrame::Relocated,
        address::Transform::RelocatedPointers {
            mode: 5,
            shared: true,
        },
        &source,
        request.clone(),
        &mut |_, _| Ok(vec![0]),
    )
    .unwrap();
    for (output, memory) in [
        (source.len() - 1, 1 << 20),
        (source.len(), 0),
        (source.len(), source.len() * 6 - 1),
    ] {
        assert!(address::decode_transformed_frame(
            address::HistoricalFrame::Relocated,
            &frame,
            output,
            memory,
            &mut |_, _, _| panic!("limits must precede backend")
        )
        .is_err());
    }
    let mut damaged = frame.clone();
    damaged[5] = 2;
    assert!(address::decode_transformed_frame(
        address::HistoricalFrame::Relocated,
        &damaged,
        source.len(),
        1 << 20,
        &mut |_, _, _| panic!("selector must precede backend")
    )
    .is_err());
    let (_, crc_at) = vdecode(&frame, 6).unwrap();
    let mut damaged = frame;
    damaged[crc_at] ^= 1;
    assert!(address::decode_transformed_frame(
        address::HistoricalFrame::Relocated,
        &damaged,
        source.len(),
        1 << 20,
        &mut |_, _, _| Ok(value.clone())
    )
    .is_err());
    assert!(address::encode_transformed_frame(
        address::HistoricalFrame::Relocated,
        address::Transform::Got { mode: 3 },
        &source,
        request,
        &mut |_, _| panic!("wrong transform must precede backend")
    )
    .is_err());
}

#[test]
fn catalogue_restores_pair_without_reordering_existing_families_or_using_names() {
    let source = fixture("original.tar");
    let result = catalogue::catalog(&source).unwrap();
    let selected = result
        .candidates
        .iter()
        .filter(|c| c.family == "executable-relocated-pointer")
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        selected,
        [
            "relocated-pointer-mode5-paq",
            "relocated-pointer-mode5-brotli"
        ]
    );
    let previous = [
        "ecoff-got-mode3-paq",
        "ecoff-got-mode3-brotli",
        "ecoff-pointer-mode5-paq",
        "ecoff-pointer-mode5-brotli",
        "ecoff-section-mode4-paq",
        "ecoff-section-mode4-brotli",
        "ecoff-filter-mode0",
        "ecoff-filter-mode1",
        "ecoff-filter-mode2",
        "executable-joint-discount",
    ];
    let indices = result
        .candidates
        .iter()
        .filter_map(|c| previous.iter().position(|id| *id == c.id))
        .collect::<Vec<_>>();
    assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
    let mut renamed = source.clone();
    renamed[..100].fill(0);
    renamed[..11].copy_from_slice(b"renamed.bin");
    renamed[148..156].fill(b' ');
    let checksum: u32 = renamed[..512].iter().map(|b| u32::from(*b)).sum();
    renamed[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    assert_eq!(result, catalogue::catalog(&renamed).unwrap());
    assert_eq!(result, catalogue::catalog(&source.to_vec()).unwrap());
    let unrelated = catalogue::catalog(b"arbitrary CIXB0a ECOFF text").unwrap();
    assert!(!unrelated
        .candidates
        .iter()
        .any(|c| c.family == "executable-relocated-pointer"));
}
