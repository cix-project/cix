use cix_native::huffman;

fn encoded(input: &[u8]) -> Vec<u8> {
    huffman::encode(input).expect("encode")
}

#[test]
fn round_trips_empty_single_and_full_byte_alphabet() {
    for input in [
        Vec::new(),
        vec![42; 4097],
        (0u8..=255).cycle().take(4099).collect(),
        b"canonical Huffman framing must remain deterministic".to_vec(),
    ] {
        let first = encoded(&input);
        assert_eq!(first, encoded(&input));
        assert_eq!(huffman::decode(&first).unwrap(), input);
    }
}

#[test]
fn rejects_truncation_trailing_data_and_padding_bits() {
    let frame = encoded(b"a very small non-byte-aligned Huffman payload");
    assert!(huffman::decode(&frame[..frame.len() - 1]).is_err());
    let mut trailing = frame.clone();
    trailing.push(0);
    assert!(huffman::decode(&trailing).is_err());
    let mut dirty_padding = frame;
    let bit_len = u64::from_le_bytes(dirty_padding[11..19].try_into().unwrap());
    assert_ne!(bit_len % 8, 0);
    *dirty_padding.last_mut().unwrap() |= 1;
    assert!(huffman::decode(&dirty_padding).is_err());
}

#[test]
fn rejects_invalid_length_tables() {
    // Header: version, count, output length, bit length; then `(symbol,length)`.
    // Three 1-bit codes are oversubscribed.
    let mut oversubscribed = vec![huffman::VERSION];
    oversubscribed.extend_from_slice(&3u16.to_le_bytes());
    oversubscribed.extend_from_slice(&1u64.to_le_bytes());
    oversubscribed.extend_from_slice(&1u64.to_le_bytes());
    oversubscribed.extend_from_slice(&[0, 1, 1, 1, 2, 1, 0]);
    assert!(huffman::decode(&oversubscribed).is_err());

    // Two 2-bit codes leave half of the prefix space unused.
    let mut incomplete = vec![huffman::VERSION];
    incomplete.extend_from_slice(&2u16.to_le_bytes());
    incomplete.extend_from_slice(&1u64.to_le_bytes());
    incomplete.extend_from_slice(&2u64.to_le_bytes());
    incomplete.extend_from_slice(&[0, 2, 1, 2, 0]);
    assert!(huffman::decode(&incomplete).is_err());

    // A single symbol has the explicit, unambiguous one-bit convention.
    let mut invalid_single = vec![huffman::VERSION];
    invalid_single.extend_from_slice(&1u16.to_le_bytes());
    invalid_single.extend_from_slice(&2u64.to_le_bytes());
    invalid_single.extend_from_slice(&4u64.to_le_bytes());
    invalid_single.extend_from_slice(&[7, 2, 0]);
    assert!(huffman::decode(&invalid_single).is_err());
}

#[test]
fn rejects_unsorted_metadata_and_mid_code_end() {
    let mut unsorted = encoded(b"abcabcabcabc");
    // The first two metadata pairs follow the fixed 19-byte header.
    unsorted.swap(19, 21);
    unsorted.swap(20, 22);
    assert!(huffman::decode(&unsorted).is_err());

    let mut mid_code = encoded(b"abcabcabcabc");
    let bits = u64::from_le_bytes(mid_code[11..19].try_into().unwrap());
    mid_code[11..19].copy_from_slice(&(bits - 1).to_le_bytes());
    assert!(huffman::decode(&mid_code).is_err());
}
