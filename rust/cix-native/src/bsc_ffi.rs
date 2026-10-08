//! Thin, bounded wrapper around vendored libbsc.
//!
//! This module deliberately owns only libbsc's *memory block* API.  CIX owns
//! framing, block boundaries, checksums and scheduling, so it never writes a
//! libbsc file stream and never permits libbsc's OpenMP implementation to
//! oversubscribe the CIX worker pool.
//!
//! `build.rs` must compile the Apache-2.0 libbsc 3.3.12 source snapshot into
//! the `bsc` static library before this module is enabled from `lib.rs`.

use std::fmt;
use std::sync::OnceLock;

const HEADER_BYTES: usize = 28;
pub const LIBBSC_VERSION: &str = "3.3.12";
// libbsc's distinct-buffer bsc_compress limit, rather than the wider signed
// integer domain accepted by some other entry points.
const MAX_BLOCK_BYTES: usize = 1_073_741_824;
const NO_ERROR: i32 = 0;
const BAD_PARAMETER: i32 = -1;
const UNEXPECTED_EOB: i32 = -5;
const DATA_CORRUPT: i32 = -6;

// Values from libbsc/libbsc.h.  Keep these here rather than importing a C
// header at runtime so an archive decoder has no toolchain dependency.
const BLOCKSORTER_BWT: i32 = 1;
const CODER_QLFC_STATIC: i32 = 1;
const CODER_QLFC_ADAPTIVE: i32 = 2;
const FEATURE_NONE: i32 = 0;
const FEATURE_FASTMODE: i32 = 1;

// `build.rs` deliberately prefixes vendored libraries with `cix_` so a host
// libbsc installation can neither replace nor silently change CIX decoding.
#[link(name = "cix_bsc", kind = "static")]
unsafe extern "C" {
    fn bsc_init(features: i32) -> i32;
    fn bsc_compress(
        input: *const u8,
        output: *mut u8,
        input_len: i32,
        lzp_hash_size: i32,
        lzp_min_len: i32,
        block_sorter: i32,
        coder: i32,
        features: i32,
    ) -> i32;
    fn bsc_block_info(
        block_header: *const u8,
        header_size: i32,
        compressed_len: *mut i32,
        restored_len: *mut i32,
        features: i32,
    ) -> i32;
    fn bsc_decompress(
        input: *const u8,
        compressed_len: i32,
        output: *mut u8,
        restored_len: i32,
        features: i32,
    ) -> i32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BscError {
    InputTooLarge,
    EmptyBlock,
    Init(i32),
    Compress(i32),
    InvalidHeader(i32),
    Truncated { declared: usize, actual: usize },
    LengthMismatch { declared: usize, expected: usize },
    Decode(i32),
}

impl fmt::Display for BscError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InputTooLarge => write!(
                f,
                "libbsc accepts distinct input/output blocks of at most 1 GiB"
            ),
            Self::EmptyBlock => write!(f, "libbsc does not encode an empty memory block"),
            Self::Init(code) => write!(f, "libbsc initialization failed ({code})"),
            Self::Compress(code) => write!(f, "libbsc compression failed ({code})"),
            Self::InvalidHeader(code) => write!(f, "invalid libbsc block header ({code})"),
            Self::Truncated { declared, actual } => {
                write!(
                    f,
                    "truncated libbsc block: header declares {declared} bytes, frame has {actual}"
                )
            }
            Self::LengthMismatch { declared, expected } => {
                write!(
                    f,
                    "libbsc restored-length mismatch: header {declared}, CIX frame {expected}"
                )
            }
            Self::Decode(code) => write!(f, "libbsc decompression failed ({code})"),
        }
    }
}

impl std::error::Error for BscError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BscOptions {
    /// 0 disables LZP.  Values 10..=28 are accepted by libbsc.
    pub lzp_hash_size: u8,
    /// 0 disables LZP.  Values 4..=255 are accepted by libbsc.
    pub lzp_min_len: u8,
    pub adaptive_coder: bool,
    pub fast_mode: bool,
}

impl Default for BscOptions {
    fn default() -> Self {
        // Matches the upstream bsc defaults other than its process-global
        // threading feature; CIX, rather than libbsc, supplies parallelism.
        Self {
            lzp_hash_size: 15,
            lzp_min_len: 72,
            adaptive_coder: false,
            fast_mode: true,
        }
    }
}

impl BscOptions {
    fn validate(self) -> Result<(), BscError> {
        let hash_ok = self.lzp_hash_size == 0 || (10..=28).contains(&self.lzp_hash_size);
        let len_ok = self.lzp_min_len == 0 || (4..=255).contains(&self.lzp_min_len);
        if hash_ok && len_ok && ((self.lzp_hash_size == 0) == (self.lzp_min_len == 0)) {
            Ok(())
        } else {
            Err(BscError::Compress(BAD_PARAMETER))
        }
    }

    fn features(self) -> i32 {
        if self.fast_mode {
            FEATURE_FASTMODE
        } else {
            FEATURE_NONE
        }
    }
}

// libbsc requires initialization before every other API.  We initialize once
// with no process-global worker threads.  Calling this concurrently is safe
// after the OnceLock publication.
static INITIALIZED: OnceLock<Result<(), BscError>> = OnceLock::new();

fn initialize() -> Result<(), BscError> {
    *INITIALIZED.get_or_init(|| {
        // SAFETY: no pointers are passed and FEATURE_NONE disables libbsc's
        // OpenMP/CUDA/large-page process-global optional paths.
        let result = unsafe { bsc_init(FEATURE_NONE) };
        if result == NO_ERROR {
            Ok(())
        } else {
            Err(BscError::Init(result))
        }
    })
}

fn checked_i32(len: usize) -> Result<i32, BscError> {
    if len > MAX_BLOCK_BYTES {
        return Err(BscError::InputTooLarge);
    }
    i32::try_from(len).map_err(|_| BscError::InputTooLarge)
}

/// libbsc reads and writes its 28-byte header with `int *` casts. CIX frame
/// payloads are byte-aligned and may start at an arbitrary offset, so never
/// pass them directly into that API. This owned scratch is explicitly aligned
/// for `u32`/C `int` and avoids undefined behaviour on strict-alignment CPUs.
struct AlignedBlock {
    words: Vec<u32>,
    byte_len: usize,
}

impl AlignedBlock {
    fn zeroed(byte_len: usize) -> Result<Self, BscError> {
        let word_len = byte_len.checked_add(3).ok_or(BscError::InputTooLarge)? / 4;
        Ok(Self {
            words: vec![0; word_len],
            byte_len,
        })
    }

    fn copied(input: &[u8]) -> Result<Self, BscError> {
        let mut block = Self::zeroed(input.len())?;
        block.bytes_mut().copy_from_slice(input);
        Ok(block)
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: u8 may view any initialized allocation. `zeroed` initializes
        // every u32; this slice is constrained to the requested byte length.
        unsafe { std::slice::from_raw_parts(self.words.as_ptr().cast::<u8>(), self.byte_len) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: the u8 view remains unique for this borrow and the backing
        // allocation is fully initialized/aligned for every C int header load.
        unsafe {
            std::slice::from_raw_parts_mut(self.words.as_mut_ptr().cast::<u8>(), self.byte_len)
        }
    }

    fn as_ptr(&self) -> *const u8 {
        self.words.as_ptr().cast::<u8>()
    }
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.words.as_mut_ptr().cast::<u8>()
    }
}

/// Encode one nonempty input-derived CIX block as a self-describing libbsc
/// block. The returned bytes have no CIX framing; callers must account for
/// their own route metadata and checksum bytes when comparing candidates.
pub fn compress(input: &[u8], options: BscOptions) -> Result<Vec<u8>, BscError> {
    if input.is_empty() {
        return Err(BscError::EmptyBlock);
    }
    options.validate()?;
    initialize()?;
    let input_len = checked_i32(input.len())?;
    let capacity = input
        .len()
        .checked_add(HEADER_BYTES)
        .ok_or(BscError::InputTooLarge)?;
    let input = AlignedBlock::copied(input)?;
    let mut output = AlignedBlock::zeroed(capacity)?;
    let coder = if options.adaptive_coder {
        CODER_QLFC_ADAPTIVE
    } else {
        CODER_QLFC_STATIC
    };
    // SAFETY: both slices are live for the call, non-overlapping and libbsc
    // documents exactly `input_len + LIBBSC_HEADER_SIZE` output capacity.
    let written = unsafe {
        bsc_compress(
            input.as_ptr(),
            output.as_mut_ptr(),
            input_len,
            i32::from(options.lzp_hash_size),
            i32::from(options.lzp_min_len),
            BLOCKSORTER_BWT,
            coder,
            options.features(),
        )
    };
    if written < NO_ERROR {
        return Err(BscError::Compress(written));
    }
    let written = usize::try_from(written).map_err(|_| BscError::Compress(written))?;
    if written > output.byte_len {
        return Err(BscError::Compress(BAD_PARAMETER));
    }
    Ok(output.bytes()[..written].to_vec())
}

/// Decode exactly one libbsc block and require the CIX frame's uncompressed
/// length to agree with its self-description.  This rejects trailing or
/// truncated native payloads before a caller emits reconstructed bytes.
pub fn decompress(payload: &[u8], expected_len: usize) -> Result<Vec<u8>, BscError> {
    if payload.len() < HEADER_BYTES {
        return Err(BscError::InvalidHeader(UNEXPECTED_EOB));
    }
    initialize()?;
    let payload = AlignedBlock::copied(payload)?;
    let mut declared_compressed = 0i32;
    let mut declared_restored = 0i32;
    // SAFETY: payload has at least libbsc's documented fixed header length;
    // output pointers refer to initialized, writable i32 values.
    let info = unsafe {
        bsc_block_info(
            payload.as_ptr(),
            HEADER_BYTES as i32,
            &mut declared_compressed,
            &mut declared_restored,
            FEATURE_NONE,
        )
    };
    if info != NO_ERROR || declared_compressed < 0 || declared_restored < 0 {
        return Err(BscError::InvalidHeader(if info == NO_ERROR {
            DATA_CORRUPT
        } else {
            info
        }));
    }
    let compressed = declared_compressed as usize;
    let restored = declared_restored as usize;
    if compressed != payload.byte_len {
        return Err(BscError::Truncated {
            declared: compressed,
            actual: payload.byte_len,
        });
    }
    if restored != expected_len {
        return Err(BscError::LengthMismatch {
            declared: restored,
            expected: expected_len,
        });
    }
    let mut output = AlignedBlock::zeroed(restored)?;
    // SAFETY: bsc_block_info authenticated the input and output extents;
    // output has precisely the decoded length that the header declared.
    let result = unsafe {
        bsc_decompress(
            payload.as_ptr(),
            checked_i32(compressed)?,
            output.as_mut_ptr(),
            checked_i32(restored)?,
            FEATURE_NONE,
        )
    };
    if result != NO_ERROR {
        return Err(BscError::Decode(result));
    }
    Ok(output.bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_lzp_pair_is_rejected_before_ffi() {
        assert_eq!(
            BscOptions {
                lzp_hash_size: 0,
                lzp_min_len: 72,
                ..BscOptions::default()
            }
            .validate(),
            Err(BscError::Compress(BAD_PARAMETER))
        );
    }

    #[test]
    fn native_block_round_trips_and_rejects_truncation() {
        let input =
            b"bounded native BSC payload with an input-derived repeated phrase. ".repeat(1024);
        let encoded = compress(&input, BscOptions::default()).expect("compressible BSC fixture");
        assert!(encoded.len() < input.len());
        assert_eq!(
            decompress(&encoded, input.len()).expect("fresh BSC decode"),
            input
        );
        let truncated = &encoded[..encoded.len() - 1];
        assert!(matches!(
            decompress(truncated, input.len()),
            Err(BscError::Truncated { .. })
                | Err(BscError::Decode(_))
                | Err(BscError::InvalidHeader(_))
        ));
    }
}
