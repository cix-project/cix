use cix_native::{fixedmix, zpaq_context::Config};

const FIXTURE: &[u8] = b"fixed-mix v1 canonical fixture: abracadabra abracadabra\n";

fn config() -> Config {
    Config {
        bucket_bits: 5,
        max_distance: 4096,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn recurrence_v3_round_trips_with_prefix_history() {
    let prefix = b"prefix history has repeatable records: aa bb aa bb ";
    let input = b"aa bb aa bb recurrence expert remains causal across this block";
    let encoded = fixedmix::encode_with_recurrence_eta(input, prefix, 6, config()).unwrap();
    assert_eq!(encoded[0], 3);
    assert_eq!(&encoded[2..9], &config().to_saturated_bytes().unwrap());
    assert_eq!(
        fixedmix::decode_with_history(&encoded, input.len(), prefix).unwrap(),
        input
    );
}

#[test]
fn recurrence_v3_rejects_malformed_truncated_and_corrupted_payloads() {
    let encoded = fixedmix::encode_with_recurrence_eta(FIXTURE, &[], 5, config()).unwrap();

    let mut malformed = encoded.clone();
    malformed[8] = 1; // recurrence configuration's reserved byte
    assert!(fixedmix::decode(&malformed, FIXTURE.len()).is_err());

    let mut invalid_bucket_count = encoded.clone();
    invalid_bucket_count[3] = 0;
    assert!(fixedmix::decode(&invalid_bucket_count, FIXTURE.len()).is_err());

    for truncated in [&encoded[..2], &encoded[..8], &encoded[..encoded.len() - 1]] {
        assert!(fixedmix::decode(truncated, FIXTURE.len()).is_err());
    }

    let mut corrupted = encoded.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0x80;
    match fixedmix::decode(&corrupted, FIXTURE.len()) {
        Err(_) => {}
        Ok(restored) => assert_ne!(restored, FIXTURE),
    }
}

#[test]
fn legacy_v2_fixture_retains_its_decoder_semantics() {
    let encoded = fixedmix::encode_with_legacy_recurrence_eta(FIXTURE, &[], 5, config()).unwrap();
    assert_eq!(encoded[0], 2);
    assert_eq!(
        hex(&encoded),
        "020001050010000000a802666bfd240a8640932793288b958e1f69782cb16f601a77f6fdf6edc97fe11122ddebb1af2f",
        "v2 bytes are retained solely for archive compatibility"
    );
    assert_eq!(fixedmix::decode(&encoded, FIXTURE.len()).unwrap(), FIXTURE);
}

#[test]
fn v3_uses_saved_causal_distance_features_not_legacy_distance_one_lookup() {
    let input = b"abcdefgaabcdefgaabcdefgaabcdefga";
    let legacy = fixedmix::encode_with_legacy_recurrence_eta(input, &[], 6, config()).unwrap();
    let corrected = fixedmix::encode_with_recurrence_eta(input, &[], 6, config()).unwrap();
    assert_eq!(legacy[0], 2);
    assert_eq!(corrected[0], 3);
    // Headers differ by version/config marker, but this checks that the
    // arithmetic stream also changes when the saved feature is consumed.
    assert_ne!(&legacy[9..], &corrected[9..]);
    assert_eq!(fixedmix::decode(&legacy, input.len()).unwrap(), input);
    assert_eq!(fixedmix::decode(&corrected, input.len()).unwrap(), input);
}

#[test]
fn v1_canonical_fixture_is_unchanged() {
    let encoded = fixedmix::encode_with_history_eta(FIXTURE, &[], 6).unwrap();
    assert_eq!(encoded[0], 1);
    assert_eq!(
        hex(&encoded),
        "0101b602666bd260a6fbef2d9c6198adee64e0e6c7a247a3ad4d99fa4bcc1a920fa354759d5424681d85c8",
        "version-1 bytes are a compatibility contract"
    );
    assert_eq!(fixedmix::decode(&encoded, FIXTURE.len()).unwrap(), FIXTURE);
}
