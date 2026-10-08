//! Pure-Rust, bounded CIXG1 subset for WebAssembly and other restricted hosts.
//!
//! This crate writes ordinary `CIXG1` frames.  It deliberately supports only
//! raw, RLE, and route-2 composition/range substreams; it is not the native
//! portfolio selector and it never loads a library or starts a process.

mod cixg1;
mod limits;
mod wasm;

// Reuse the native codec source directly until it is extracted into a shared
// package.  Keeping a single source of arithmetic/rank semantics is required
// for byte-compatible substreams.
#[path = "../../cix-native/src/adaptive.rs"]
pub mod adaptive;
#[path = "../../cix-native/src/combinatorics.rs"]
pub mod combinatorics;
#[path = "../../cix-native/src/rank.rs"]
pub mod rank;

pub use cixg1::{
    decode, encode, DecodeConfig, EncodeConfig, Error, Progress, StreamDecoder, StreamEncoder,
    StreamState,
};
