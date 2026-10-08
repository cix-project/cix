# CIX native SDK package skeleton

This is a source-side installation helper for the current **native-buffer-v1**
ABI. It installs `cix.h`, `cix_stream.h`, `cix_formats.h`, `cix_dictionary.h`, the C++17 RAII convenience header
`cix.hpp`, an already-built shared `libcix_native` library and optional matching
static archive, `find_package(CIX CONFIG)` metadata, and `pkg-config` metadata. It does not build Rust itself, create an SDK artifact, or
qualify an installed consumer.

## Capability boundary

The installed `CIX::native` target exposes process-free, complete-buffer
operations from `cix.h`: option defaults, opaque contexts, encode, decode and
context diagnostics. Calls on a context are serialized; different contexts may
be used concurrently. The complete-buffer ABI does not provide incremental process/flush/finish/reset
calls, callbacks, hard process-RSS limits, cancellation-handle transfer, or
full-engine/same-CIX worker selection. The package configuration identifies it
as `native-buffer-v1`.

`cix_stream.h` separately exposes the native independent-block `CIXG1` stream
API: opaque encoder/decoder handles plus process, flush, finish and reset. It
is not a full-engine route, retained-history decoder, specialist selector, or
streaming archive adapter. The installed C stream consumer in
`packaging/sdk/tests/c_stream_installed_consumer.c` exercises fragmented input
and output using only public C headers.

`cix_formats.h` provides complete-buffer ordinary gzip, zlib, raw DEFLATE,
bzip2, XZ, Zstd, Brotli, LZ4 frame and Snappy framed operations. These explicit
format APIs do not imply incremental standard-format streaming.

`cix_dictionary.h` provides owned Zstd dictionary handles, training/import,
SHA-256 and native dictionary identity, and caller-buffer encode/decode of
ordinary Zstd frames. Decode requires the exact declared dictionary identity.
Export trained bytes with `cix_zstd_dictionary_bytes_v1` and import them with
`cix_zstd_dictionary_create_v1` to persist a dictionary or share it with a later
decoder. Identity values alone cannot reconstruct dictionary contents.
Calls on a handle serialize; the caller must not free a handle concurrently.
The installed `examples/dictionary.c` exercises training, identity and exact
restoration. Dictionary memory admission includes retained allocation capacity,
the opaque handle, caller output, temporary output and estimated codec state;
it is not a hard process RSS ceiling. Other codecs do not gain dictionary
support through this Zstd-specific API.

A library found through `CIX::native` is not proof that PAQ/JXL bridges,
historical full-engine decoders, automatic specialist selection, platform
adapters or a full CIX release candidate are present. Full-engine provider
selection and dependency acceptance are separate release-management work.

## Prepare an install tree after a selected native build

Do not run this command until a selected library exists and its dependency
closure has been recorded. For a shared Linux build, for example:

```sh
cmake -S packaging/sdk -B sdk-build \
  -DCIX_NATIVE_LIBRARY="$PWD/target/release/libcix_native.so" \
  -DCIX_NATIVE_STATIC_LIBRARY="$PWD/target/release/libcix_native.a" \
  -DCMAKE_INSTALL_PREFIX="$PWD/sdk-prefix"
cmake --install sdk-build
```

The generated `cix-native.pc` derives `prefix` from `pcfiledir` and the
configured prefix-relative library directory, so moving a complete staged
prefix preserves its include and library paths, including multiarch libdirs.
The SDK CMake project enables C deliberately so GNUInstallDirs selects an
architecture-appropriate default library directory rather than guessing under
`LANGUAGES NONE`.
The installer rejects absolute include/library install directories because that
would make a relocatable package claim false.

On Unix the helper accepts `libcix_native.so`, `libcix_native.dylib`, or
`libcix_native.a`. On Windows it requires the selected `cix_native.dll` **and**
the matching `cix_native.lib` or `cix_native.dll.lib` import library. It stages
the DLL in the runtime `bin` directory and the import library in the `lib`
archive directory. The generated `CIX::native` imported target records both
`IMPORTED_LOCATION` and `IMPORTED_IMPLIB`; a partial DLL-only install is
rejected by `find_package`.

The following is a source-layout contract for a Windows toolchain that has
already built the two selected artifacts. It is not a Windows build, loader, or
consumer qualification, and it must not use empty placeholder files:

```sh
cmake -S packaging/sdk -B sdk-windows-layout \
  -DCMAKE_SYSTEM_NAME=Windows \
  -DCIX_NATIVE_LIBRARY=/absolute/cix_native.dll \
  -DCIX_NATIVE_IMPORT_LIBRARY=/absolute/cix_native.dll.lib \
  -DCMAKE_INSTALL_PREFIX=/absolute/sdk-prefix
cmake --install sdk-windows-layout
```

A focused Windows receipt must confirm the installed DLL/import-library pair,
CMake package metadata, and a fresh Windows consumer before a Windows SDK or
package claim can be made. No such receipt exists here. The bounded synthetic
layout-only recipe is [WINDOWS-LAYOUT-TEST.md](WINDOWS-LAYOUT-TEST.md).

When a Unix static archive is installed, its generated CMake package resolves
the system codecs directly linked by `src/external.rs`,
the platform thread/dynamic-loader facilities, and the C++ runtime explicitly
selected by `build.rs` (`c++` on Apple, `stdc++` on other Unix targets). These
are passed as compiler-resolved linker names, never as a build-host library
path. On Linux its static metadata also carries the math, dynamic-loader,
pthread, realtime and util links recorded in the target link receipt:
`-lstdc++ -lm -ldl -lpthread -lrt -lutil`. This is Linux-only; the CMake package
does not project those flags onto other platforms. The libzpaq/libbsc C++
objects are bundled by the Rust static archive. The actual target dependency
receipt remains required.

An installed CMake consumer uses:

```cmake
find_package(CIX 0.1 CONFIG REQUIRED)
add_executable(consumer main.cpp)
target_link_libraries(consumer PRIVATE CIX::native)
target_compile_features(consumer PRIVATE cxx_std_17)
```

`tests/cpp_installed_consumer.cpp` is the small installed C++ consumer source.
It should be built only against a staged prefix by the focused RC24 receipt.
The static-link set must be derived and checked against the selected target
build. A source export contains no build-host paths or historical job receipts,
and no such receipt is a qualification of a later source revision.
