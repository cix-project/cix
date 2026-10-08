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
