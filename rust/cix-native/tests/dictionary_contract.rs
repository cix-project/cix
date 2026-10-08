// Root schedules these tests against the installed pinned libzstd.
use cix_native::dictionaries::ZstdDictionary;
#[test]
fn dictionary_contract_roundtrip_wrong_identity_and_cap() {
    let owned: Vec<Vec<u8>> = (0..64)
        .map(|n| {
            format!(
                "sample-{n:02}-{}",
                "alphabetic dictionary material ".repeat(24)
            )
            .into_bytes()
        })
        .collect();
    let samples: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
    let d = ZstdDictionary::train(&samples, 1024, 128 << 20).unwrap();
    let p = d
        .encode(b"dictionary sample alpha alpha", 3, 1 << 20, 128 << 20)
        .unwrap();
    assert_eq!(
        d.decode(&p, 1 << 20, 128 << 20, d.identity()).unwrap(),
        b"dictionary sample alpha alpha"
    );
    let mut wrong = d.identity().clone();
    wrong.sha256[0] ^= 1;
    assert!(d.decode(&p, 1 << 20, 128 << 20, &wrong).is_err());
    assert!(d.decode(&p, 1, 128 << 20, d.identity()).is_err());
}
#[test]
fn dictionary_contract_rejects_invalid_training_and_trailing() {
    assert!(ZstdDictionary::train(&[b"tiny".as_slice()], 256, 1024).is_err());
    let owned: Vec<Vec<u8>> = (0..64)
        .map(|n| {
            format!(
                "sample-{n:02}-{}",
                "structured dictionary material ".repeat(24)
            )
            .into_bytes()
        })
        .collect();
    let samples: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
    let d = ZstdDictionary::train(&samples, 1024, 128 << 20).unwrap();
    let mut p = d
        .encode(b"abcdefghabcdefgh", 3, 1 << 20, 128 << 20)
        .unwrap();
    p.push(0);
    assert!(d.decode(&p, 1 << 20, 128 << 20, d.identity()).is_err());
}

#[test]
fn dictionary_contract_charges_trained_retained_capacity() {
    let owned: Vec<Vec<u8>> = (0..64)
        .map(|n| format!("sample-{n:02}-{}", "retained capacity material ".repeat(32)).into_bytes())
        .collect();
    let samples: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
    let dictionary = ZstdDictionary::train(&samples, 4096, 128 << 20).unwrap();
    // Training retains the allocation it reserved before truncating the
    // produced dictionary. Admission must charge that allocation, not merely
    // the used dictionary byte count.
    assert!(dictionary.retained_bytes() >= 4096);
    assert!(dictionary
        .encode(b"x", 3, 1, dictionary.retained_bytes())
        .is_err());
}
