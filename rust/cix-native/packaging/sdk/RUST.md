# Rust source consumers

Rust applications use the `cix-native` source crate. The installed C/C++ SDK
provides headers, libraries, CMake and pkg-config metadata; it does not install
a Cargo crate or a Rust dependency lockfile.

Use a complete selected source checkout with Rust 1.98 or later, a C++17
compiler and the native codec development libraries listed in the
[native build instructions](../../README.md#build-from-source). A shared version
string does not establish that two source, header and library sets are
interchangeable. Keep the source revision, features, lockfile, target and native
link dependencies together when recording a consumer build.

## A separate Cargo application

Add the selected source crate to the application's `Cargo.toml`, replacing the
example path with its actual absolute location:

```toml
[dependencies]
cix-native = { path = "/absolute/cix/rust/cix-native", default-features = false }
```

The dependency compiles as part of the application. Its `build.rs` compiles the
bundled C++ bridges and links the selected target's codec libraries. A previously
built CLI or installed C SDK is not that Cargo build. The application's own
`Cargo.lock` and offline dependency/cache availability must cover its complete
dependency graph; the source checkout's lockfile alone does not supply them.
The default feature set is empty. Enable the optional service feature only for
an application that intentionally needs that separate runtime.

## Complete-buffer CIX operations

The existing [native-buffer example](../../examples/native_buffer.rs) is a
process-free Rust consumer of `core::encode_buffer` and `core::decode_buffer`.
Its complete source is reproduced below unchanged:

```rust
//! Minimal process-free Rust consumer of the native CIX buffer API.
use cix_native::core::{decode_buffer, encode_buffer, NativeOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = b"A small application-owned byte buffer.";
    let options = NativeOptions {
        output_limit: 1 << 20,
        ..NativeOptions::default()
    };
    let archive = encode_buffer(source, &options)?;
    let restored = decode_buffer(&archive, &options)?;
    assert_eq!(restored, source);
    println!(
        "Restored {} bytes from a {}-byte CIX archive.",
        restored.len(),
        archive.len()
    );
    Ok(())
}
```

From the repository root, the explicitly registered example can be built and run
with an appropriate selected toolchain and native dependencies:

```sh
cargo run --locked --manifest-path rust/cix-native/Cargo.toml --example native_buffer
```

This is a small in-memory example. It does not implement a bounded file reader
or transactional output-file handling. For application input, bound reads and
allocations before calling the complete-buffer APIs. The returned `Vec<u8>` is
owned by the caller; use a deliberate application policy when publishing it to a
file or another consumer.

[`NativeOptions`](../../src/core.rs) defaults to one worker, 128 MiB of codec
memory, no deadline or cancellation token, and `usize::MAX` output bytes. Set
finite output and memory limits for the selected operation. Memory admission is
not a hard process-RSS ceiling, and output length does not bound every allocation
made by the application or native dependencies. Cancellation and deadlines are
cooperative and cannot forcibly interrupt a blocking provider or native call.
The embedding host owns scheduling, external time/memory containment and handle
lifetime.

## Bounded standard streams

[`standard_stream`](../../src/standard_stream.rs) exposes `encode_stream` and
`decode_stream` over borrowed `Read` and `Write` objects. This implementation
accepts gzip, zlib and raw DEFLATE. Other `StandardFormat` values are refused by
these entry points; a complete-buffer format API does not imply a streaming
implementation for that format.

Provide `StreamOptions` with a level in `0..=9`, explicit input/output byte
limits below `u64::MAX`, and an optional borrowed cancellation flag. The returned
`StreamProgress` distinguishes bytes read, consumed, produced and written. An
EOF/excess-input probe can read and count one extra byte without passing it to
the codec. The two fixed 8 KiB scratch buffers do not cap native codec
allocations or total process memory.

Decode accepts one complete stream/member and requires physical EOF, rejecting
trailing or concatenated data. Successful return includes codec completion and
a successful sink flush. On any error, discard the entire output prefix,
including data written before a later checksum, truncation, trailing-data or
flush failure. For files, stage output under application ownership and publish
it only after success; a flushed prefix is provisional.

The optional eager entry points flush each successful read batch or produced
output chunk before continuing. They provide neither a timer nor an
interruptible idle read. Cancellation remains cooperative between calls.

## Discovering the interface boundary

Use the public modules and headers shipped with the selected source revision
to choose the intended API. This edition has no native runtime capability-query
function in `cix.h` or `core`; do not call an interface copied from a different
SDK edition merely because both packages report version 0.1.0. The CLI's
full-engine job-capability protocol is a separate process interface and is not
an in-process SDK discovery call.

The complete-buffer and standard-stream APIs above do not advertise automatic
full-engine specialist selection or all-format streaming. Target-native build,
consumer execution, package/export completeness and platform qualification each
need evidence for the actual selected source and dependencies. This guide
supplies source usage, not those outcomes.
