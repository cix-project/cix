use cix_native::zpaq_context::{Config, Feature, Predictor};

#[test]
fn serialized_fixture_and_stream_features_are_stable() {
    let config = Config {
        bucket_bits: 4,
        max_distance: 64,
    };
    assert_eq!(config.to_bytes().unwrap(), [1, 4, 64, 0, 0, 0, 0]);

    let mut predictor = Predictor::new(config).unwrap();
    let mut features = Vec::new();
    for &byte in b"abracadabra" {
        features.push(predictor.observe(byte));
    }
    assert_eq!(
        features[0],
        Feature {
            repeated: false,
            distance_bucket: 0
        }
    );
    assert_eq!(
        features[3],
        Feature {
            repeated: true,
            distance_bucket: 3
        }
    );
    assert_eq!(
        features[10],
        Feature {
            repeated: true,
            distance_bucket: 3
        }
    );
}

#[test]
fn feature_lookup_does_not_mutate_state() {
    let config = Config {
        bucket_bits: 8,
        max_distance: 1024,
    };
    let mut predictor = Predictor::new(config).unwrap();
    predictor.observe(b'x');
    let before = predictor.position();
    let first = predictor.feature(b'x');
    let second = predictor.feature(b'x');
    assert_eq!(first, second);
    assert_eq!(predictor.position(), before);
}

#[test]
fn saturated_v2_configuration_preserves_distance_two_bucket() {
    let config = Config {
        bucket_bits: 1,
        max_distance: 2,
    };
    assert_eq!(config.to_saturated_bytes().unwrap(), [2, 1, 2, 0, 0, 0, 0]);
    assert_eq!(
        Config::from_saturated_bytes(&config.to_saturated_bytes().unwrap()).unwrap(),
        config
    );

    let mut predictor = Predictor::new_saturated(config).unwrap();
    assert_eq!(
        predictor.observe(b'a'),
        Feature {
            repeated: false,
            distance_bucket: 0
        }
    );
    predictor.observe(b'b');
    assert_eq!(
        predictor.observe(b'a'),
        Feature {
            repeated: true,
            distance_bucket: 1
        }
    );
}
