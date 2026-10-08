# CIX native Java JNI binding

This is an optional JNI consumer of the installed native C API. It does not
build CIX, invoke the command-line program, use a runtime subprocess, search
`PATH`, download dependencies, or load a library implicitly. The application
must call `NativeCix.load` with absolute paths to the qualified `libcix_native`
and this CIX-owned `libcix_jni` shim. Paths are canonicalized; a later call
with different paths is rejected.

`NativeCix.Context` is `AutoCloseable` and wraps only the native complete-buffer
C API. Its sizing call obtains the complete codec result before its caller
buffer call, so it can perform codec work twice. Context input plus its JNI caller buffer is checked against configured limits,
but Java arrays and JNI copies are additional live memory
and no API makes an RSS hard-cap claim.

`Encoder` and `Decoder` are `AutoCloseable` wrappers for the real CIXG1
independent-block stream ABI. `process`, `flush`, and `finish` report native
consumed/produced counts and explicit stream state. The wrapper checks output
capacity against the remaining output limit, checks input plus output against
its configured memory limit, and resets its produced total only on `reset()`.
It does not represent full-engine, CIXM6, retained-history, specialist-routing,
PAQ, or JXL streaming.

`cancel()` is local and cooperative: it blocks later Java calls before JNI.
The public v1 C stream ABI has no cancellation operation, so it cannot stop a
native call already in progress. `reset()` clears that local cancellation flag.

## Build the CIX-owned shim

The host inspected for this source has OpenJDK 21 headers at
`/usr/lib/jvm/java-21-openjdk-amd64/include`; CMake uses `find_package(JNI)` so
a qualified build selects its own JDK headers.

```sh
cmake -S bindings/java -B bindings/java/build \
  -DCIX_NATIVE_LIBRARY=/absolute/path/to/libcix_native.so \
  -DCIX_NATIVE_INCLUDE_DIR=/absolute/path/to/installed/include
cmake --build bindings/java/build
```

## Plain `javac` contract

No Maven or external Java dependency is required. After building the shim,
compile and execute the focused consumer against explicitly selected artifacts:

```sh
mkdir -p bindings/java/classes
javac -d bindings/java/classes \
  bindings/java/src/main/java/dev/cix/binding/NativeCix.java \
  bindings/java/src/test/java/dev/cix/binding/Contract.java
CIX_NATIVE_LIBRARY=/absolute/path/to/libcix_native.so \
CIX_JNI_LIBRARY=/absolute/path/to/libcix_jni.so \
  java -cp bindings/java/classes dev.cix.binding.Contract
```

The contract exercises complete-buffer roundtrip and fragmented
encoder/flush/finish/decoder/finish streaming with reset. It must only be run
by the qualification scheduler against the selected native library.
