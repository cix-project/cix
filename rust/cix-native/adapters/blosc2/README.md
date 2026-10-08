# CIX Blosc2 adapter

This is a CIX-owned adapter for the public C-Blosc2 codec-plugin API.  It is
not a Squash adapter and does not use a Squash codec ID or registry.

It is pinned to upstream **C-Blosc2 3.3.2**, source tag commit `d1905e9`, from
[`Blosc/c-blosc2`](https://github.com/Blosc/c-blosc2/tree/v3.3.2). Root must
provision the unchanged source archive
`https://github.com/Blosc/c-blosc2/archive/refs/tags/v3.3.2.tar.gz`, record its
SHA-256 at provisioning time, and install its header and matching shared
library together. The build checks the installed `BLOSC2_VERSION_STRING`; it
does not download dependencies or fall back to a host library.

The deployer chooses `CIX_BLOSC2_EXPERIMENTAL_CODEC_ID` in the upstream
user-codec range 160–255. CIX does not claim or fabricate a global registry
allocation. An application calls `cix_blosc2_register()` once at its chosen
initialization point, then selects that ID and
`CIX_BLOSC2_ADAPTER_METADATA_V1` through Blosc2 `cparams.compcode` and
`cparams.compcode_meta`.

Each plugin callback uses the caller-owned Blosc2 block buffers directly and
creates one CIX context with one worker. CIX output is capped at the host's
provided output block. If CIX cannot fit its complete CIX archive in that
block, the encoder returns **zero**, which is the documented Blosc2 plugin
signal for an incompressible block; Blosc2 then stores that block as raw data.
Negative values report malformed parameters or CIX failure. No output is
allocated by the adapter. Decoder success requires CIX to restore exactly the
host-requested uncompressed block length.

`clevel` maps 0–3 to CIX FAST, 4–6 to DEFAULT, and 7–9 to BEST. This is a
declared adapter policy, not a Blosc2 compression-level equivalence claim.
`compcode_meta=1` is the versioned adapter payload marker; other values are
rejected by both callbacks. During encode CIX receives a native archive bound
of input plus 2 MiB, allowing its C ABI to report a host-short destination as
`OUTPUT_TOO_SMALL`; its physical write remains bounded by the Blosc2 buffer.
Its admission includes the input block, archive bound, and a documented 64 MiB
context margin. It is not a hard process-RSS cap.
`CIX_BLOSC2_MAX_CHUNK_BYTES` is an installed-build policy,
defaulting to 64 MiB, which rejects oversized host blocks before CIX work. It
cannot exceed CIX's native 128 MiB input admission.

## Installed-SDK build and qualification

Root should configure this directory only after provisioning a matching
installed CIX shared library and pinned Blosc2 prefix:

```sh
cmake -S rust/cix-native/adapters/blosc2 -B build/cix-blosc2 \
  -DCIX_NATIVE_INCLUDE_DIR=/absolute/cix/include \
  -DCIX_NATIVE_LIBRARY=/absolute/cix/lib/libcix_native.so \
  -DBLOSC2_INCLUDE_DIR=/absolute/blosc2/include \
  -DBLOSC2_LIBRARY=/absolute/blosc2/lib/libblosc2.so \
  -DCIX_BLOSC2_EXPERIMENTAL_CODEC_ID=160 \
  -DCIX_BLOSC2_BUILD_CONTRACT_TEST=ON
cmake --build build/cix-blosc2
ctest --test-dir build/cix-blosc2 --output-on-failure
```

The included contract uses the actual host registration/context API and checks
a registered CIX codec block round trip. Qualification still needs malformed
metadata, short-output/raw-fallback, truncation, independently produced Blosc2
stream, and package-rpath cases under the final native-source identity.

## Current qualification scope

The installed-SDK contract passed with Blosc2 3.3.2 for registered experimental codec 240, including compressible and incompressible/raw fallback behavior and invalid metadata. This is local experimental registration, not an upstream allocated identifier.
