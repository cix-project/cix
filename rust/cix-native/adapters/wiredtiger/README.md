# CIX WiredTiger compressor extension

This is a CIX-owned `WT_COMPRESSOR` extension for the public WiredTiger API.
It is separate from RocksDB and has no RocksDB source or registry dependency.

The pinned upstream is **WiredTiger 11.3.1**, release commit `05c5601`, from
[`wiredtiger/wiredtiger`](https://github.com/wiredtiger/wiredtiger/tree/11.3.1).
Root must provision the unchanged official archive
`https://github.com/wiredtiger/wiredtiger/archive/refs/tags/11.3.1.tar.gz`,
record its SHA-256 at provision time, and build/install a matching shared
library plus generated public `wiredtiger.h`. This adapter verifies generated
major/minor/patch macros and does not download or use a host fallback.

The extension registers the deliberately named experimental compressor
`cix-v1-experimental`. Each stored compressed block begins with a fixed 16-byte
CIX-owned frame: `CIXW`, frame version 1, three zero reserved bytes, then the
little-endian CIX archive payload length. The explicit length is necessary
because WiredTiger may zero-pad `decompress` input beyond the compressed result.
Unknown version/reserved bytes, impossible payload bounds, truncated payloads,
and a CIX restore that does not exactly fill the caller-provided output are
rejected.

`pre_size` returns the original block size. `compress` only stores a CIX frame
when the entire frame is strictly shorter than the source and fits the supplied
destination. Otherwise it returns success with `compression_failed=1`, which is
the WiredTiger incompressible-data contract. It does not emit a partial frame.
The installed-build policy `CIX_WIREDTIGER_MAX_BLOCK_BYTES` defaults to 64 MiB;
larger host blocks return `EFBIG` before CIX work. The bound cannot exceed the
native CIX 128 MiB input admission. Callback contexts use one CIX
worker. On encode, CIX is admitted with the source plus a 2 MiB native archive
slack so that a host-short destination reaches CIX's documented
`OUTPUT_TOO_SMALL` result; the actual write remains bounded by the host
destination. Decode admits paid CIX payload plus output and 64 MiB context
margin. These are not hard process-RSS limits.

## Installed-SDK build and contract

After root provisions the pinned SDKs, configure only this adapter directory:

```sh
cmake -S rust/cix-native/adapters/wiredtiger -B build/cix-wiredtiger \
  -DCIX_NATIVE_INCLUDE_DIR=/absolute/cix/include \
  -DCIX_NATIVE_LIBRARY=/absolute/cix/lib/libcix_native.so \
  -DWIREDTIGER_INCLUDE_DIR=/absolute/wiredtiger/include \
  -DWIREDTIGER_LIBRARY=/absolute/wiredtiger/lib/libwiredtiger.so \
  -DCIX_WIREDTIGER_BUILD_CONTRACT_TEST=ON
cmake --build build/cix-wiredtiger
ctest --test-dir build/cix-wiredtiger --output-on-failure
```

The contract source dynamically loads the extension into a real database,
stores both a compressible and deterministic incompressible raw value,
checkpoints, closes, reopens, reloads the extension, and verifies exact bytes.
It still requires final-source-identity qualification of malformed CIXW frames,
short destination behavior, block-size rejection, package rpaths, and the
database's own on-disk recovery paths.

## Current qualification scope

The adapter passed with WiredTiger 11.3.1 against a real database: checkpoint, close/reopen, exact reads, and compression read/write counters. Focused callback checks covered destination bounds, incompressible fallback and malformed frames. Installation into a final distribution still requires its own loader closure test.
