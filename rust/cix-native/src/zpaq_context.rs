//! Bounded recurrence-distance context features inspired by ZPAQ's generic
//! distance contexts.
//!
//! This module is intentionally isolated from the production selector.  It
//! exposes a streaming feature extractor that keeps only the last position of
//! each byte value.  A later encoder may feed the resulting bucket into a bit
//! predictor or context mixer.  The feature stream is not itself a compressed
//! representation.

const LEGACY_CONFIG_VERSION: u8 = 1;
const SATURATED_CONFIG_VERSION: u8 = 2;
const MAX_BUCKET_BITS: u8 = 8;

/// Parameters are serialized with every experimental feature stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    /// Number of low bits retained from the logarithmic distance bucket.
    pub bucket_bits: u8,
    /// Distances at or above this value share the final bucket.
    pub max_distance: u32,
}

impl Config {
    pub fn validate(self) -> Result<(), String> {
        if !(1..=MAX_BUCKET_BITS).contains(&self.bucket_bits) {
            return Err("recurrence bucket_bits must be in 1..=8".into());
        }
        if self.max_distance < 2 {
            return Err("recurrence max_distance must be at least 2".into());
        }
        Ok(())
    }

    /// Stable versioned configuration bytes: version, bucket bits, u32 LE
    /// distance cap.  Feature state is deliberately not serialized.
    /// Legacy v1 serialization, retained for already-emitted fixed-mix v2
    /// payloads. Its bucket arithmetic intentionally wraps.
    pub fn to_bytes(self) -> Result<[u8; 7], String> {
        self.to_versioned_bytes(LEGACY_CONFIG_VERSION)
    }

    /// Version 2 serialization for saturated bucket arithmetic. This avoids
    /// the v1 wrap where `bucket_bits=1, max_distance=2` mapped distance two
    /// back to bucket zero.
    pub fn to_saturated_bytes(self) -> Result<[u8; 7], String> {
        self.to_versioned_bytes(SATURATED_CONFIG_VERSION)
    }

    fn to_versioned_bytes(self, version: u8) -> Result<[u8; 7], String> {
        self.validate()?;
        let mut out = [0u8; 7];
        out[0] = version;
        out[1] = self.bucket_bits;
        out[2..6].copy_from_slice(&self.max_distance.to_le_bytes());
        // Reserved byte makes extension validation explicit.
        out[6] = 0;
        Ok(out)
    }

    /// Parse legacy v1 configuration bytes only.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        Self::from_versioned_bytes(bytes, LEGACY_CONFIG_VERSION)
    }

    /// Parse saturated v2 configuration bytes only.
    pub fn from_saturated_bytes(bytes: &[u8]) -> Result<Self, String> {
        Self::from_versioned_bytes(bytes, SATURATED_CONFIG_VERSION)
    }

    fn from_versioned_bytes(bytes: &[u8], version: u8) -> Result<Self, String> {
        if bytes.len() != 7 || bytes[0] != version || bytes[6] != 0 {
            return Err("invalid recurrence configuration".into());
        }
        let max_distance = u32::from_le_bytes(bytes[2..6].try_into().unwrap());
        let config = Self {
            bucket_bits: bytes[1],
            max_distance,
        };
        config.validate()?;
        Ok(config)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BucketSemantics {
    LegacyWrapping,
    Saturated,
}

/// A feature available before observing the current symbol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Feature {
    /// `true` when this byte has occurred previously in the stream.
    pub repeated: bool,
    /// Logarithmic distance bucket; zero denotes an unseen byte.
    pub distance_bucket: u8,
}

/// Fixed-memory streaming recurrence state.
pub struct Predictor {
    config: Config,
    position: u64,
    last_position: [u64; 256],
    seen: [bool; 256],
    semantics: BucketSemantics,
}

impl Predictor {
    pub fn new(config: Config) -> Result<Self, String> {
        Self::with_semantics(config, BucketSemantics::LegacyWrapping)
    }

    /// Corrected bucket semantics for fixed-mix payload v3 and later.
    pub fn new_saturated(config: Config) -> Result<Self, String> {
        Self::with_semantics(config, BucketSemantics::Saturated)
    }

    fn with_semantics(config: Config, semantics: BucketSemantics) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            config,
            position: 0,
            last_position: [0; 256],
            seen: [false; 256],
            semantics,
        })
    }

    pub fn config(&self) -> Config {
        self.config
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    /// Predict a symbol's recurrence feature without changing state.
    pub fn feature(&self, symbol: u8) -> Feature {
        let index = symbol as usize;
        if !self.seen[index] {
            return Feature {
                repeated: false,
                distance_bucket: 0,
            };
        }
        let distance = self.position.saturating_sub(self.last_position[index]);
        Feature {
            repeated: true,
            distance_bucket: distance_bucket(distance, self.config, self.semantics),
        }
    }

    /// Observe one reconstructed byte.  The position advances exactly once.
    pub fn observe(&mut self, symbol: u8) -> Feature {
        let feature = self.feature(symbol);
        let index = symbol as usize;
        self.seen[index] = true;
        self.last_position[index] = self.position;
        self.position = self.position.saturating_add(1);
        feature
    }
}

fn distance_bucket(distance: u64, config: Config, semantics: BucketSemantics) -> u8 {
    let capped = distance.min(u64::from(config.max_distance));
    // One bucket per power-of-two interval.  The cap is included in the last
    // bucket, making the representation independent of stream length.
    let mut power = 0u8;
    let mut bound = 1u64;
    while bound < capped && power < 63 {
        bound <<= 1;
        power += 1;
    }
    let bucket = u16::from(power.saturating_add(1));
    match semantics {
        BucketSemantics::LegacyWrapping => {
            let mask = (1u16 << config.bucket_bits) - 1;
            (bucket & mask) as u8
        }
        BucketSemantics::Saturated => bucket.min((1u16 << config.bucket_bits) - 1) as u8,
    }
}

/// Extract features in one pass. The output length is exactly `data.len()`;
/// `Predictor` itself retains roughly 2.3 KiB of fixed state regardless of
/// input length. This helper materializes its returned feature vector and is
/// therefore not the streaming interface; streaming callers should use
/// `Predictor` directly.
pub fn extract(data: &[u8], config: Config) -> Result<Vec<Feature>, String> {
    let mut predictor = Predictor::new(config)?;
    let mut out = Vec::with_capacity(data.len());
    for &symbol in data {
        out.push(predictor.observe(symbol));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_round_trips() {
        let config = Config {
            bucket_bits: 6,
            max_distance: 4096,
        };
        let bytes = config.to_bytes().unwrap();
        assert_eq!(Config::from_bytes(&bytes).unwrap(), config);
    }

    #[test]
    fn fixture_is_deterministic_and_causal() {
        let config = Config {
            bucket_bits: 4,
            max_distance: 64,
        };
        let input = b"abca";
        let expected = [
            Feature {
                repeated: false,
                distance_bucket: 0,
            },
            Feature {
                repeated: false,
                distance_bucket: 0,
            },
            Feature {
                repeated: false,
                distance_bucket: 0,
            },
            Feature {
                repeated: true,
                distance_bucket: 3,
            },
        ];
        assert_eq!(extract(input, config).unwrap(), expected);
        assert_eq!(extract(input, config).unwrap(), expected);
    }

    #[test]
    fn state_is_bounded_for_long_input() {
        let config = Config {
            bucket_bits: 8,
            max_distance: 32,
        };
        let mut predictor = Predictor::new(config).unwrap();
        for i in 0..1_000_000u32 {
            predictor.observe((i % 251) as u8);
        }
        assert_eq!(predictor.position(), 1_000_000);
        assert_eq!(predictor.config(), config);
        assert!(std::mem::size_of::<Predictor>() <= 3 * 1024);
    }

    #[test]
    fn rejects_invalid_parameters_and_bytes() {
        assert!(Config {
            bucket_bits: 0,
            max_distance: 2
        }
        .to_bytes()
        .is_err());
        assert!(Config {
            bucket_bits: 9,
            max_distance: 2
        }
        .to_bytes()
        .is_err());
        assert!(Config {
            bucket_bits: 4,
            max_distance: 1
        }
        .to_bytes()
        .is_err());
        assert!(Config::from_bytes(&[LEGACY_CONFIG_VERSION, 4, 2, 0, 0, 0, 1]).is_err());
    }
}
