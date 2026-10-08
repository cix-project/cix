# Experimental CIX Hadoop `CompressionCodec`

This is a sequential Hadoop Common adapter for the installed CIX Java JNI
binding. It wraps the public independent-block `CIXG1` stream only. It is not a
`SplittableCompressionCodec`, does not create a seek index, and does not expose
full-engine selection, retained-history streams, PAQ/JXL routing or random
access.

The Maven dependency is pinned to Hadoop Common 3.4.1. The CIX JNI source is
compiled from `bindings/java/src/main/java`; this adapter never launches the
CIX command-line program, discovers a library, downloads a native dependency,
or uses archive metadata to choose a library.

Before constructing a codec, set these `Configuration` properties to existing
absolute files:

- `cix.hadoop.native.library` — qualified `libcix_native`;
- `cix.hadoop.jni.library` — CIX-owned JNI shim built against that SDK.

Optional limits are `cix.hadoop.profile` (1 fast, 2 default, 3 best),
`cix.hadoop.workers`, `cix.hadoop.output.limit` and
`cix.hadoop.memory.limit`. They are copied into the native binding. The
adapter uses bounded chunks derived from the configured memory limit and maps
truncation, malformed stream state and no-progress backpressure to `IOException`.
`flush()` enters the native encoder and drains its bounded output; `resetState()`
requires a completed stream so it cannot silently discard unframed input.
Native calls remain synchronous: close/cancellation cannot interrupt a call
already in progress, and neither Hadoop nor this adapter receives a process-RSS
hard-cap guarantee.

`src/test/java/.../HadoopContract.java` is a scheduler-only contract source. It
requires a real Hadoop Common runtime, the explicitly configured native and JNI
libraries, and must run on the target JVM/OS. It checks sequential round-trip,
finish/close, reset and truncated-input rejection. It is not run by this source
change.

## Current qualification scope

The Java 17-targeted codec and stream contract passed against the actual Hadoop Common 3.4.1 API on JDK 21 with the installed JNI library. Sequential round trip, flush/finish, reset/end and truncation rejection passed. Splittable input and random access are separate capabilities and are not claimed here.
