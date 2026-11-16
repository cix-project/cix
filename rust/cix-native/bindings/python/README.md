# CIX native Python binding

This optional package is a **stdlib `ctypes` consumer** of an already installed,
qualified `libcix_native` library. It never compiles Rust, searches `PATH`,
downloads anything, or participates in the CIX command-line executable.

```python
from cix_native import NativeLibrary

native = NativeLibrary("/absolute/path/to/libcix_native.so")
with native.context(profile="default", workers=1) as cix:
    archive = cix.encode(b"data")
    assert cix.decode(archive) == b"data"
```

`NativeLibrary` rejects relative paths and missing files. The path is the full
trust decision of the calling application; this binding does not discover a
library through environment variables or loader search paths.

## API contract

`NativeContext` wraps the process-free, complete-buffer `cix.h` native ABI.
`encode` and `decode` make a size query followed by one caller-buffer call. The
native query computes the complete codec output, so this convenience path can
perform codec work twice; use the stream API where bounded incremental work is
needed. Calls raise `NativeError` for every nonzero C status. Contexts and dictionaries are
context managers; `close()` is idempotent. The supplied output and memory caps
are passed to the native ABI; native memory admission is not a hard process RSS
limit. The binding makes its own Python/ctypes input and output copies; its
pre-allocation checks bound those copies against the supplied limits, but they
are additional resident memory rather than part of a native RSS guarantee.

`NativeLibrary.stream_encoder()` and `.stream_decoder()` wrap the actual
`cix_stream.h` CIXG1 independent-block stream API. `process`, `flush`, and
`finish` return `(StreamProgress, bytes)` with the native consumed, produced,
and `needs_input` / `needs_output` / `finished` state. The wrapper bounds every
output buffer by the remaining configured output limit, checks input plus output
capacity before allocation, and resets its produced-total only on `reset()`.
They deliberately do not hide backpressure or claim retained-history/full-engine streaming.
`cancel()` is a Python-side cooperative fence that prevents subsequent calls;
the v1 C ABI has no cancellation entry point and cannot interrupt a native call
already in progress. `reset()` resets the native stream and clears that fence.

`format_encode` and `format_decode` expose the explicit complete-buffer
standard-format IDs in `cix_formats.h`. They do not add streaming format APIs.

`ZstdDictionary` owns a `cix_dictionary.h` handle. Train or import it, retain
its `identity`, call `export_bytes()` to persist its actual contents, and
import those bytes for a later decoder. An identity alone cannot reconstruct a
dictionary. Dictionary decode requires the exact identity, including for Zstd
frames whose in-frame dictionary ID is zero.

## Capability boundary

This package exposes only the installed native C APIs: native buffer contexts,
independent CIXG1 streams, explicit standard formats, and Zstd dictionaries.
It does **not** expose full-engine route selection, historical decoder coverage,
PAQ/JXL bridge selection, automatic specialist selection, a Python dependency
inside CIX, process cancellation, or an RSS hard cap. Availability of this
package is not evidence that any of those release capabilities is present.

## Qualified-library tests

The focused tests intentionally run only when a scheduler supplies the exact
qualified shared-library path:

```sh
PYTHONPATH=src CIX_PYTHON_NATIVE_LIBRARY=/absolute/path/to/libcix_native.so \
  python -m unittest discover -s tests -v
```

Without that variable the native integration tests skip; this package does not
fall back to an arbitrary host library.

## Experimental Numcodecs and Zarr adapters

The optional `numcodecs` extra selects Numcodecs 0.16.3; the `zarr` extra
selects Numcodecs 0.16.3 and Zarr 3.1.3. These hosts require Python >=3.11.
The base ctypes binding retains Python >=3.10 and no required host dependency.
The accepted consumer environment used Python 3.12.3 and NumPy 2.5.3;
that NumPy version requires Python >=3.12. Other interpreter/dependency
combinations are not qualified by that evidence. Extras name host versions;
they do not provide a complete hash lock, native library, or qualified wheel.

| Host | On-disk format | Adapter | Retained qualification |
| -- | -- | -- | -- |
| Numcodecs 0.16.3 | Numcodecs config | `CIXNativeCodec` | Limited registration/config and byte restoration evidence |
| Zarr 3.1.3 | 2 | `CIXNativeCodec` / `CIXNativeV2Codec` | Accepted CIX-250 focused disk case |
| Zarr 3.1.3 | 3 | `CIXNativeBytesBytesCodec` | Accepted CIX-250 focused disk case |
| Zarr 2.18.7 / Numcodecs 0.15.2 | 2 | `CIXNativeV2Codec` | Limited positive installed-binding disk/fresh-process proof |

The separate Zarr host-major 2 tuple used Linux, Python 3.12.14 and NumPy
2.5.5 with the installed pure-Python binding and the same current-source native
library. Disk write and a separate-process read-only reopen passed all 25
shuffled chunk reads, full restoration and persisted options. Zarr 2.18.7
requires Numcodecs <0.16; this qualification does not change the optional
extras selected above. Three separately accepted negative cases on this tuple
also rejected missing registration with the selected-codec UnknownCodecError,
raised NativeError status 4 for a copied chunk truncated to one byte after an
unchanged neighbor decoded, and rejected an injected library_path metadata key
with ValueError before trusted library configuration or loading. The missing-
registration process did not import the binding or load its native library.

Provision the selected host dependencies and an explicitly trusted native
library before using the adapters. No adapter downloads or discovers that
library. Registration is explicit and process-local; it is not an import side
effect or automatic entry-point lookup. Register each required format in every
application process before loading its metadata:

```python
from cix_native.integrations.numcodecs import CIXNativeCodec, register_numcodecs
from cix_native.integrations.zarr_v2 import register_zarr_v2
from cix_native.integrations.zarr_v3 import CIXNativeBytesBytesCodec, register_zarr_v3

library_path = "/absolute/path/to/libcix_native.so"  # application trust decision
register_numcodecs(library_path)
register_zarr_v2(library_path)
register_zarr_v3(library_path)
options = dict(profile="fast", workers=1, output_limit=1 << 20, memory_limit=8 << 20)
v2_codec = CIXNativeCodec(library_path, **options)
v3_codec = CIXNativeBytesBytesCodec(library_path, **options)
```

Use `v2_codec` as `compressor` with Zarr 3.1.3 `zarr.create(...,
zarr_format=2)`. For format 3, use `zarr.create_array(..., zarr_format=3,
serializer=zarr.codecs.BytesCodec(), compressors=[v3_codec])`. The v3 adapter
is a distinct bytes-to-bytes codec after serialization, not a dtype/shape
serializer. The adapters accept contiguous chunk bytes and do not implement
partial chunk reads or writes.

Numcodecs format metadata uses `id="cix-native-experimental-v1"`, whereas
v3 uses `name="cix-native-experimental"` with a `configuration` object.
Both persist profile, workers, output_limit and memory_limit. Neither persists
a native library path. Configure the trusted library before reconstructing
`numcodecs.get_codec(config)` or loading registered Zarr codec metadata.
An injected `library_path` key is rejected rather than trusted.

Close the `LocalStore` after use, then reopen it read-only with
`zarr.open_array(store=store, mode="r", zarr_format=2)` or format 3 as
appropriate. Keep the process registration/trusted library configured.
CIX-250 accepted disk write, same-process close/reopen, seeded reads of all
25 chunks, exact full restoration, all four persisted options, missing codec,
injected library path, and truncated/malformed chunk failure for the two
Zarr 3.1.3 formats. Missing registration fails with Numcodecs UnknownCodecError
(format 2) or Zarr UnknownCodecError wrapping the selected-codec KeyError
(format 3); malformed native archives raise NativeError.
Missing host dependencies require explicit provisioning, not a fallback codec.

This is experimental host integration. On Linux, Python 3.12.14, NumPy 2.5.5, Numcodecs 0.16.3 and Zarr 3.1.3
passed disk write and separate-process read-only reopening for formats 2 and 3,
including all 25 chunks in a seeded random order, exact full restoration and
the four persisted options. That qualification used the source-loaded binding
and a native library built from the same current product source. Missing
registration rejected each selected codec; truncating one selected disk chunk
to one byte raised NativeError while an unmodified chunk still decoded.
The selected one-byte truncation cases do not establish arbitrary-corruption
detection. The installed pure-Python binding also has bounded build, offline
install, fresh-consumer and removal evidence on the selected Linux tuples; no
native library is bundled. Other Zarr host-major 2 combinations, broader
wheel/sdist/platform qualification and full CIX-31 acceptance remain open.
The accepted historical c950 native component is not a current-main-built
library. The original CIX-250 external runner exited 1 because of its IntEnum
result-carrier defect; its two focused pytest cases were independently
accepted (2 passed in 0.47s). CIX-257 subsequently accepted the optional collection/fixture boundaries
and merged them through PR10 at e0d5c161. It also retained the source-only
corrected runner successor 620d309953c29942783034726a8d3a41a2de4c549d517a3e662911e31bf0b7a7;
the original 0398 runner remains retired historical evidence. That successor
has no new native execution authority. No consumer replay or postmerge suite
was performed by CIX-257. These instructions and optional metadata add no new
execution, artifact, platform, full-engine, benchmark or publication qualification.
