//! Reusable, process-free entry points for the native CIX container.
//!
//! These functions do not install signal handlers, print diagnostics, alter
//! process limits, or start external programs.  They expose the existing
//! native CIX routes only; later full-engine dispatch is a separate capability.
use std::fmt;
use std::io::{Read, Write};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;

#[path = "incremental.rs"]
pub mod incremental;

// The reconciled native codec/container implementation is intentionally owned
// below this library module.  `main.rs` is now only a CLI entry shim.
#[path = "engine.rs"]
pub(crate) mod engine;
// Keep compatibility at the crate root through the codec primitives below;
// do not expose the implementation module itself, which also hosts private
// full-engine helpers used by the executable adapter.
pub use engine::cix_squash_splice;
pub use engine::{
    adaptive, bsc_ffi, combinatorics, external, fixedmix, huffman, legacy_lz, limits, parallel,
    ppm, rank, resources, stdout_writer, zpaq_context,
};

/// Native container operating profile.  It is intentionally distinct from a
/// future full-engine specialist catalogue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeProfile {
    Fast,
    Default,
    Best,
}

/// Explicit caller-owned resource and route controls.
#[derive(Clone, Debug)]
pub struct NativeOptions {
    pub profile: NativeProfile,
    pub output_limit: usize,
    pub memory_limit: usize,
    pub workers: usize,
    pub deadline: Option<Duration>,
    pub cancellation: Option<Arc<AtomicBool>>,
}

impl Default for NativeOptions {
    fn default() -> Self {
        Self {
            profile: NativeProfile::Default,
            output_limit: usize::MAX,
            memory_limit: 128 << 20,
            workers: 1,
            deadline: None,
            cancellation: None,
        }
    }
}

#[derive(Debug)]
pub enum NativeError {
    InvalidOptions(&'static str),
    Codec(String),
    OutputLimit,
}
impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOptions(x) => write!(f, "{x}"),
            Self::Codec(x) => f.write_str(x),
            Self::OutputLimit => f.write_str("CIX output exceeds caller limit"),
        }
    }
}
impl std::error::Error for NativeError {}
fn codec_error(error: String) -> NativeError {
    if error.contains("CIX output exceeds caller limit") {
        NativeError::OutputLimit
    } else {
        NativeError::Codec(error)
    }
}

fn level(profile: NativeProfile) -> u8 {
    match profile {
        NativeProfile::Fast => 1,
        NativeProfile::Default => 6,
        NativeProfile::Best => 9,
    }
}
fn validate(options: &NativeOptions) -> Result<(), NativeError> {
    if options.memory_limit == 0 {
        return Err(NativeError::InvalidOptions("memory limit must be non-zero"));
    }
    if options.workers == 0 {
        return Err(NativeError::InvalidOptions("worker count must be non-zero"));
    }
    Ok(())
}
struct LimitedWriter<W> {
    inner: W,
    remaining: usize,
}
impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other("CIX output exceeds caller limit"));
        }
        let written = self.inner.write(bytes)?;
        self.remaining -= written;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Encodes a native CIX container without CLI side effects.
pub fn encode<R: Read, W: Write>(
    src: R,
    dst: W,
    options: &NativeOptions,
) -> Result<(), NativeError> {
    validate(options)?;
    let _library = engine::limits::LibraryGuard::new();
    let _cancellation = options
        .cancellation
        .clone()
        .map(engine::limits::CancellationGuard::new);
    let _deadline = options.deadline.map(engine::limits::DeadlineGuard::new);
    let mut bounded = LimitedWriter {
        inner: dst,
        remaining: options.output_limit,
    };
    engine::encode(
        src,
        &mut bounded,
        engine::EncodeOptions {
            block: 65536,
            level: level(options.profile),
            forced: None,
            backend: "native",
            backend_set: false,
            format: "cix",
            input_fd: -1,
            flush_interval: None,
            memory: options.memory_limit,
            workers: options.workers,
            explain: false,
            verbose: false,
            strategy: "blocks",
        },
    )
    .map_err(codec_error)
}

/// Decodes any currently native-supported frame without CLI printing or exit.
pub fn decode<R: Read, W: Write>(
    src: R,
    dst: W,
    options: &NativeOptions,
) -> Result<(), NativeError> {
    validate(options)?;
    let _library = engine::limits::LibraryGuard::new();
    let _cancellation = options
        .cancellation
        .clone()
        .map(engine::limits::CancellationGuard::new);
    let _deadline = options.deadline.map(engine::limits::DeadlineGuard::new);
    let mut bounded = LimitedWriter {
        inner: dst,
        remaining: options.output_limit,
    };
    engine::decode(src, &mut bounded, false, false, -1, options.memory_limit).map_err(codec_error)
}

pub fn encode_buffer(input: &[u8], options: &NativeOptions) -> Result<Vec<u8>, NativeError> {
    let mut out = Vec::new();
    encode(input, &mut out, options)?;
    Ok(out)
}
pub fn decode_buffer(input: &[u8], options: &NativeOptions) -> Result<Vec<u8>, NativeError> {
    let mut out = Vec::new();
    decode(input, &mut out, options)?;
    Ok(out)
}
