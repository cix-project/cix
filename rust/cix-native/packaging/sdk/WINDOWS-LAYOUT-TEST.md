# Windows SDK layout contract

This is a **CMake install-layout check only**. It checks that the SDK project
requires and stages a DLL/import-library pair on a Windows-targeting CMake
configuration. It does not compile CIX, validate PE metadata, load the DLL, or
qualify a Windows consumer, toolchain, ABI, package, or release.

The caller supplies an explicit Windows CMake toolchain file. Use non-empty
synthetic files only to exercise CMake's selected-input and destination rules;
they must never be presented as CIX runtime artifacts.

```sh
work=/absolute/new/cix-windows-sdk-layout
mkdir -p "$work/input" "$work/prefix"
printf 'layout fixture only\n' > "$work/input/cix_native.dll"
printf 'layout fixture only\n' > "$work/input/cix_native.dll.lib"

cmake -S packaging/sdk -B "$work/build" \
  -DCMAKE_TOOLCHAIN_FILE=/absolute/windows-layout-toolchain.cmake \
  -DCIX_NATIVE_LIBRARY="$work/input/cix_native.dll" \
  -DCIX_NATIVE_IMPORT_LIBRARY="$work/input/cix_native.dll.lib" \
  -DCMAKE_INSTALL_PREFIX="$work/prefix"
cmake --install "$work/build"

test -f "$work/prefix/bin/cix_native.dll"
test -f "$work/prefix/lib/cix_native.dll.lib"
test -f "$work/prefix/lib/cmake/CIX/CIXConfig.cmake"
```

The check must also configure a negative case with only
`CIX_NATIVE_LIBRARY`; that configuration must fail because the paired import
library is required. A later Windows qualification needs a real selected DLL
and import library, a fresh `find_package(CIX CONFIG)` consumer, and a runtime
load/round-trip receipt.
