# Experimental Numcodecs and Zarr adapters

`cix_native.integrations` provides optional host adapters over the installed,
process-free native buffer ABI. They are experimental integration artifacts,
not a claim of stock Numcodecs/Zarr interoperability or a full-engine bridge.

The Numcodecs and Zarr v2 adapter uses the v2 codec ID
`cix-native-experimental-v1`. It accepts C-contiguous bytes-like chunks and
preserves their bytes exactly; it does not inspect dtype, shape, or array
metadata. Call `register_numcodecs()` directly, or `register_zarr_v2()` when a
Zarr v2 host will resolve the Numcodecs registry.

Zarr v3 uses the separate `CIXNativeBytesBytesCodec` and the v3 codec name
`cix-native-experimental`. Register it explicitly with `register_zarr_v3()`.
It is a bytes-to-bytes codec and requires the normal Zarr array-to-bytes
serializer earlier in the pipeline. It does not offer partial chunk operations.

Persisted configurations contain only the codec identity, profile, worker
count, and output/memory limits. They never contain a library path or another
dynamic-load instruction. Before resolving persisted Numcodecs or Zarr metadata,
the embedding process must explicitly call `configure_native_library(path)` or
pass the path to `register_zarr_v2(path)` / `register_zarr_v3(path)`. That path
is trusted process-local deployment state, not archive data and not an
automatic dependency fetch. A consumer must install the same compatible native
SDK before opening metadata that uses these experimental IDs. The adapter
passes `output_limit` and `memory_limit` to the SDK and rejects input larger
than its configured memory limit before copying it for encoding or decoding.
The Python binding copies buffers, and its native size probe can perform codec
work twice; neither fact yields a process-RSS hard cap.

The adapter imports without Numcodecs or Zarr installed. Registration fails
clearly until the requested host dependency exists. Focused qualification must
run against a staged native SDK and the actual installed Numcodecs/Zarr host;
the absence of those packages on a development machine is not evidence that
the adapter works in a host.
