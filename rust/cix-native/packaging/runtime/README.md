# Prebuilt CIX runtime staging

This CMake project stages one already-selected **CIX executable**. It installs
that file as `bin/cix` and creates Unix `bin/uncix` and `bin/cixcat` symlinks;
the program selects its mode from `argv[0]`. It does not build Rust, compile a
bridge, locate a provider, search `PATH`, or fall back to a workstation copy.

`CIX_RUNTIME_CAPABILITY=native-only` stages only that executable. It must be
described as the native-only capability subset: it does not include PAQ, JXL,
or spatial full-engine providers.

`CIX_RUNTIME_CAPABILITY=full-engine` requires six **absolute, existing**
prebuilt bridge paths at configure time. They are installed using the exact
names consumed by `full_engine::installed` under `<prefix>/lib/cix` (the discovery layout is deliberately `lib`, not a multiarch library directory):

- `libcix_paq_v215_bridge.so`
- `libcix_paq_v216_bridge.so`
- `libcix_paq_joint_discount_bridge.so`
- `libcix_paq_store_state_bridge.so`
- `libcix_jxl_bridge.so`
- `libcix_spatial_bridge.so`

On macOS the corresponding installed names end in `.dylib`, matching native discovery.

For example, a controlled Linux staging operation supplies every selected
artifact explicitly:

```sh
cmake -S packaging/runtime -B runtime-build \
  -DCMAKE_INSTALL_PREFIX=/usr \
  -DCIX_RUNTIME_CAPABILITY=full-engine \
  -DCIX_RUNTIME_READELF=/abs/readelf \
  -DCIX_RUNTIME_CIX_EXECUTABLE=/abs/cix \
  -DCIX_RUNTIME_PAQ_V215_BRIDGE=/abs/libcix_paq_v215_bridge.so \
  -DCIX_RUNTIME_PAQ_V216_BRIDGE=/abs/libcix_paq_v216_bridge.so \
  -DCIX_RUNTIME_PAQ_JOINT_DISCOUNT_BRIDGE=/abs/libcix_paq_joint_discount_bridge.so \
  -DCIX_RUNTIME_PAQ_STORE_STATE_BRIDGE=/abs/libcix_paq_store_state_bridge.so \
  -DCIX_RUNTIME_JXL_BRIDGE=/abs/libcix_jxl_bridge.so \
  -DCIX_RUNTIME_SPATIAL_BRIDGE=/abs/libcix_spatial_bridge.so \
  -DCIX_RUNTIME_JXL_LIBRARY=/abs/libjxl.so.0.12.0 \
  -DCIX_RUNTIME_JXL_THREADS_LIBRARY=/abs/libjxl_threads.so.0.12.0 \
  -DCIX_RUNTIME_JXL_CMS_LIBRARY=/abs/libjxl_cms.so.0.12.0 \
  -DCIX_RUNTIME_BROTLIDEC_LIBRARY=/abs/libbrotlidec.so.1.2.0 \
  -DCIX_RUNTIME_BROTLIENC_LIBRARY=/abs/libbrotlienc.so.1.2.0 \
  -DCIX_RUNTIME_BROTLICOMMON_LIBRARY=/abs/libbrotlicommon.so.1.2.0 \
  -DCIX_RUNTIME_CHARLS_LIBRARY=/abs/libcharls.so.2.4.2 \
  -DCIX_RUNTIME_OPENJPEG_LIBRARY=/abs/libopenjp2.so.2.5.0
DESTDIR="$stage" cmake --install runtime-build
```

This stages the six bridge DSOs and the explicit Linux JXL/Brotli/CharLS/OpenJPEG
companions named above. It does **not** claim the remaining system/transitive
dependency closure (for example libc, libstdc++, liblzma, libm or libgcc), a
source build, portable bundle, package, ABI/provider loading, or a full release.

A focused runtime-install receipt must use a clean relocation prefix and verify:

1. exactly one physical `cix` executable; `uncix` and `cixcat` are aliases to it;
2. all six expected bridge names under `lib/cix` for full-engine staging, and no
   bridge claim for native-only staging;
3. discovery uses the relocated `bin/cix` and exact `lib/cix` paths without
   `PATH`/environment/workstation fallback;
4. each provider is loaded and exercised under its declared resource limits;
5. archive-only decode, malformed-path rejection, and absence of duplicate CIX
   executables are checked after relocation.

## Linux full-engine companion closure

The full-engine installer currently supports **ELF/Linux inputs only**. In
addition to the six bridges it requires explicit absolute paths for these
selected non-system companions, installed beside the bridges in `lib/cix`:

- `libjxl.so.0.12.0`, `libjxl_threads.so.0.12.0`, and `libjxl_cms.so.0.12.0`;
- `libbrotlidec.so.1.2.0`, `libbrotlienc.so.1.2.0`, and
  `libbrotlicommon.so.1.2.0`;
- `libcharls.so.2.4.2` and `libopenjp2.so.2.5.0`.

The installer creates only the corresponding required ELF SONAME links in the
same staged directory. It does not collect arbitrary transitive libraries or
search any host location. The caller must explicitly provide an absolute
`CIX_RUNTIME_READELF`. CMake invokes it read-only on each selected JXL/spatial
bridge and every JXL/Brotli companion, rejecting inputs without `$ORIGIN`
RUNPATH. It never uses `file(RPATH_CHECK)`, because that CMake command may
remove a mismatching input file.

The inspected installed JXL prefix (`jxl-prefix-v3`) has `$ORIGIN` on its JXL
bridge and each JXL/Brotli companion. This is an input-layout observation, not
a staged provider-load or archive qualification. macOS and Windows full-engine
companion staging remain open: their loader identities, bundle mechanics, and
target execution must be implemented and qualified separately.

For a scheduled non-release source-export/build/install exercise using the
same explicit inputs, retain the selected hashes and staging evidence. That
procedure does not accept a dependency closure or qualify a distributable
package.

## Windows native-only layout

On Windows, `native-only` staging installs `bin/cix.exe` and byte-for-byte
copies it to `bin/uncix.exe` and `bin/cixcat.exe`; symlinks are deliberately
not required. Full-engine Windows staging remains refused because no target
companion closure or provider-loading qualification exists. This is a
synthetic staging-layout contract, not Windows runtime, alias, provider, or
archive qualification.
