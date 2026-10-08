# CIX GStreamer byte-stream adapter

This source-only `gstcixstream` plugin supplies a `cixstream` element over the
public CIXG1 stream SDK. Set `encoder=true` for byte-stream encoding or
`encoder=false` for decoding. It is a byte-stream element, not an audio/video
codec, does not launch the CIX CLI, and does not expose full-engine selection.

Each input `GstBuffer` is bounded by `max-input`; output is emitted in bounded
`max-output-buffer` buffers. The adapter reserves one mapped input and one
output buffer from `memory-limit`, then gives the remaining memory to the
native handle and requests one native worker. `EOS` drains until the SDK
reports `FINISHED`; truncated input, invalid progress, and decoder trailing
bytes are errors, as are later buffers after `FINISHED`.

`FLUSH_START` sets cooperative cancellation and is forwarded downstream before
the adapter waits on its stream mutex, so a blocked downstream push can return.
`FLUSH_STOP` serializes a native reset before accepting more buffers.
Cancellation cannot interrupt a native call already in progress. Failed native
calls leave the element failed until a flush-stop reset or a state transition
recreates its handle.

Configure with explicit `CIX_INCLUDE_DIR` and `CIX_NATIVE_LIBRARY`, plus
GStreamer 1.0 and app-library development packages visible to pkg-config.
The CMake contract loads the built module through a private `GST_PLUGIN_PATH`
and uses `appsrc ! cixstream ! appsink` for fragmented encode/decode, exact
round-trip, empty input, truncation, trailing data, and flush-reset. No host
qualification has run here; there is no claim of media caps negotiation,
process-RSS enforcement, or hard cancellation.
