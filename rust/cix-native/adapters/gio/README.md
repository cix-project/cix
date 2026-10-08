# CIX GIO GConverter

This installed-SDK adapter implements GIO's `GConverter` interface using the
native CIXG1 incremental API. It works with `GConverterInputStream` and
`GConverterOutputStream`; it does not launch processes or expose full-engine
specialists. GStreamer is a separate, unfinished adapter.

Configure CMake with explicit `CIX_INCLUDE_DIR` and `CIX_NATIVE_LIBRARY` paths
to one installed CIX SDK and with GIO's development package available through
pkg-config. Build and run `ctest --test-dir BUILD --output-on-failure`.
The contract covers empty/nonempty streams, midstream flush, one-byte output
backpressure, nonempty final input, reset, truncation and explicit cancellation.

Encoder flush and end-of-input follow the [GConverter contract](https://docs.gtk.org/gio/method.Converter.convert.html).
The decoder has no flush operation and rejects that request. Each handle uses
independent CIXG1 blocks with the native API's declared memory/output limits;
there is no cross-block history claim. Failed native calls require reset.

`cix_gconverter_cancel` is a thread-safe cancellation flag for the next converter
call. Keep the object alive while setting it. It does not interrupt an in-flight
native codec call, and `reset` clears it. Other converter operations and reset
must be serialized by the caller. GIO streams may buffer writes without
calling the converter, so cancelling a converter does not promise immediate
failure of a buffered host write. A `GCancellable` passed to a host I/O method
has GIO's own I/O cancellation semantics.

Qualification receipts are platform and SDK specific; source availability does
not establish Windows/macOS qualification or GStreamer support.
