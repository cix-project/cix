# CIX portable WebAssembly adapter

`rust/cix-portable` is a deliberately limited, pure-Rust producer and consumer
of ordinary `CIXG1` archives. It does not invent a browser-only container:
its encoder uses only CIXG1 raw (route 0), RLE (route 1), and route-2 standard
byte substreams with composition (coder 2), adaptive range (coder 4), or
count-range (coder 5). Every emitted block has the normal CIXG1 SHA-256 frame
digest and the stream ends with the normal whole-stream digest.

The portable profile evaluates those fixed, bounded candidates for each input
block and retains the strictly smallest payload, keeping earlier candidate order
on a tie. It is not CIX's native portfolio selector and makes no compression-ratio
or process-RSS promise. Native CIX can decode every archive this adapter writes.
The portable decoder intentionally accepts only the listed routes/coders,
CIXG1 zero-history archives, 64 KiB-or-smaller blocks, and bounded input/output.
It must reject an otherwise valid native archive outside that subset.

The core uses source references to the native `rank.rs`, `combinatorics.rs`, and
`adaptive.rs`, so arithmetic and rank coding semantics have one source while the
native crate remains unchanged. Before distributing this adapter, move those
modules into a reviewed shared crate and add it to the shipping allowlist and
third-party/dependency notices; the source reference is an implementation bridge,
not an installation contract.

## Raw ABI

The `cix-portable` `cdylib` exposes `cix_portable_alloc/free`, one-shot
`cix_portable_encode/decode`, and stateful encoder/decoder constructors and
`*_process` calls. Callers provide linear-memory byte buffers plus two `u32`
out-parameters for bytes consumed and produced. A process return of `0`, `1`, or
`2` means `NeedsInput`, `NeedsOutput`, or `Finished`; negative values are errors.
The encoder requires `finish=1` only after all input has been consumed. No
filesystem, process, dynamic-library, or native codec API is involved.

These symbols are exported only on `wasm32`.  The raw functions contain panic
boundaries and reject null nonempty, overflowing, misaligned, or overlapping
input/output/result ranges before borrowing them.  Their unsafe contract still
requires live owned allocations and a live exclusive stream handle; raw-pointer
provenance cannot be established from an address. `alloc/free` use a boxed slice
whose exact length is part of the `free` call.

`cix-portable.mjs` is a small Node/browser wrapper around those exports.
`portable_streaming.mjs` and `differential.py` are source-only qualification
helpers. They are not run by package installation.

## Browser stream contract

`encodeStream` and `decodeStream` accept a `ReadableStream<Uint8Array>` and
return a new readable stream. They retain one supplied source chunk, one
caller-selected output chunk, and the native bounded codec state; each pull
reports the codec's actual consumed/produced progress. Cancelling the returned
stream cancels the supplied reader and releases the WASM handle. Decoding has
no synthetic end marker: EOF before the CIXG1 terminal frame is a truncation
error.

`browser_streaming.html` fetches the local module/WASM rather than using a
`file:` URL. It emits one structured `PASS` or `FAIL` JSON object after testing
fragmented nonempty and empty streams, malformed/truncated/tailed frames,
input/output caps, and cancellation. `run_browser_streaming.py --wasm ABS
--chromium ABS` stages only those local files under a temporary loopback HTTP
server and prints that JSON. It is a prepared scheduler command; it neither
builds nor downloads an artifact.

## Current qualification scope

The portable native subset passed 16 native tests, WebAssembly compilation, Node stream checks, and both-direction native cross-decode. The browser stream wrapper passed with Chrome 151 on Linux, including fragmented and empty input, malformed/truncated/trailing data, limits and cancellation. These are focused fixture results for the documented CIXG1 subset, not full-engine browser support.
