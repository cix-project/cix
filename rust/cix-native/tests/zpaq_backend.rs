use cix_native::external::{decode, encode_zpaq};

#[test]
fn pinned_zpaq715_level5_round_trips_large_fixture_and_rejects_trailing_data() {
    // More than one MiB exercises libzpaq's block/model path rather than only
    // a tiny header fixture. The CIX envelope supplies the independent SHA-256
    // check in addition to ZPAQ's own block checksum.
    let input = b"ZPAQ level five CIX bounded native fixture 0123456789\n".repeat(32_768);
    let archive = encode_zpaq("size", &input, 5).expect("encode pinned ZPAQ");
    assert_eq!(decode(&archive, 2 * 1024 * 1024 * 1024).unwrap(), input);

    let mut trailing = archive;
    trailing.push(0);
    let payload_size = u32::try_from(trailing.len() - 47).unwrap();
    trailing[11..15].copy_from_slice(&payload_size.to_le_bytes());
    assert!(decode(&trailing, 2 * 1024 * 1024 * 1024).is_err());

    // libzpaq emits no frame for empty input; that candidate is rejected.
    // A forged zero-payload envelope must not become an alternate spelling.
    assert!(encode_zpaq("size", &[], 5)
        .unwrap_err()
        .starts_with("ZPAQ candidate rejected:"));
    let mut missing_payload = vec![0u8; 47];
    missing_payload[..5].copy_from_slice(b"CIXB1");
    missing_payload[5] = 6;
    missing_payload[6] = 3;
    use sha2::{Digest, Sha256};
    missing_payload[15..47].copy_from_slice(&Sha256::digest([]));
    missing_payload.truncate(47);
    missing_payload[11..15].copy_from_slice(&0u32.to_le_bytes());
    assert!(decode(&missing_payload, 2 * 1024 * 1024 * 1024).is_err());
}
