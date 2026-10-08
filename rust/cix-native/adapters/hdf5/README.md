# Experimental CIX HDF5 chunk filter

`H5Zcix` is a native HDF5 dynamic filter plugin over the installed public
`cix.h` buffer API. It processes every HDF5 chunk independently with the native
CIXG1 SDK profile. It does not embed Python, invoke the full engine, or expose
the legacy specialist routes.

This is an experimental adapter, not a registered HDF5 filter and not stock
h5py/netCDF compatibility. The deployment must choose and record its own ID:
HDF5 reserves `256..511` for temporary testing, reserves `512..32767` for
HDF Group-managed third-party identifiers, and permits internal/application
testing at `32768..65535`. Do not publish files outside a controlled deployment
with a temporary or private ID. A stable public identity requires an HDF Group
registration and a separately qualified released plugin.

## Build

The plugin consumes an already installed CIX SDK and installed HDF5 headers and
library. It neither builds nor downloads either dependency.

```sh
cmake -S rust/cix-native/adapters/hdf5 -B build/hdf5-cix \
  -DCIX_NATIVE_INCLUDE_DIR=/absolute/sdk/include \
  -DCIX_NATIVE_LIBRARY=/absolute/sdk/lib/libcix_native.so \
  -DCIX_HDF5_FILTER_ID=65001 \
  -DCIX_HDF5_MAX_CHUNK_MIB=64 \
  -DCIX_HDF5_MAX_MEMORY_MIB=512 \
  -DCIX_HDF5_MAX_WORKERS=4
cmake --build build/hdf5-cix
```

Set `HDF5_PLUGIN_PATH` to the directory containing `H5Zcix` before starting an
HDF5 consumer. HDF5 discovers it when an application sets or reads this filter
ID. The plugin is linked to the explicitly selected HDF5 and CIX libraries; use
headers, library ABI, and loader settings from the same deployed SDKs.

## Stored parameters and bounds

The dataset creation property list must call `H5Pset_filter` with exactly these
five unsigned `cd_values`:

| Index | Meaning |
| --- | --- |
| 0 | Adapter parameter ABI: `1` |
| 1 | CIX profile: `1` FAST, `2` DEFAULT, `3` BEST |
| 2 | CIX worker count, at least `1` |
| 3 | Complete output limit in MiB, at least `1` |
| 4 | Native CIX working-memory limit in MiB, at least `1` |

The values in indices 2–4 are archive metadata requests, never authority to
consume arbitrary host resources. The deployment owns three compile-time caps:
`CIX_HDF5_MAX_CHUNK_MIB` (default `64`, maximum `128` because of the native
CIX input admission), `CIX_HDF5_MAX_MEMORY_MIB` (default `512`), and
`CIX_HDF5_MAX_WORKERS` (default `4`). A file requesting a worker, output, or
memory value above its deployed cap is rejected. The input `nbytes` and HDF5
buffer capacity are also checked against the deployed chunk cap before any CIX
context or codec work begins.

All accepted limits are checked before allocation. The filter first asks CIX for the
exact output size with a zero-capacity call, rejects a size over the configured
output limit, allocates that exact size through `H5allocate_memory`, and then
performs the operation. It releases the incoming HDF5 buffer through
`H5free_memory`; no CIX-owned allocation crosses the plugin ABI.

Use `H5Z_FLAG_OPTIONAL` when setting the filter. If CIX’s encoded output is not
smaller than the input, the filter returns zero. HDF5 then stores that chunk
unfiltered and records the filter mask; it never writes an ambiguous raw CIX
payload. With a required filter the same chunk write fails, by design. Decode
failure, malformed frames, invalid parameters, a CIX resource limit, and an
allocation failure also return zero; those should fail reads rather than yield
partial data.

The examples show h5py’s low-level DCPL API and netCDF-C’s `nc_def_var_filter`
API. They are optional consumers, not dependencies of the plugin. netCDF-C
needs a build with the HDF5 backend and arbitrary-filter API (4.9 or newer).

## Focused installed-host qualification

Use an HDF5 C development package at least 1.10 (including `H5PLextern.h`,
`H5Z_class2_t`, `H5allocate_memory`, and dynamic-plugin loading), a qualified
installed CIX SDK, and CMake 3.21 or later. For the C contract test:

```sh
cmake -S rust/cix-native/adapters/hdf5 -B build/hdf5-cix-test \
  -DCIX_NATIVE_INCLUDE_DIR=/absolute/sdk/include \
  -DCIX_NATIVE_LIBRARY=/absolute/sdk/lib/libcix_native.so \
  -DCIX_HDF5_FILTER_ID=65001 -DCIX_HDF5_BUILD_CONTRACT_TEST=ON
cmake --build build/hdf5-cix-test
ctest --test-dir build/hdf5-cix-test --output-on-failure
```

The test requires plugin discovery through the CTest-provided
`HDF5_PLUGIN_PATH`. It closes and reopens the file, checks the raw chunk's
filter mask and stored size, performs exact decoded readback, proves optional
raw fallback for an incompressible chunk, and checks invalid parameters,
over-deployment requests, persisted decode-limit failure, and a corrupted CIX
chunk. Run the h5py example
against the same HDF5 ABI with `HDF5_PLUGIN_PATH` and
`CIX_HDF5_FILTER_ID` exported. Compile the netCDF example only with a pinned
netCDF-C installation that declares `nc_def_var_filter`, using the same chosen
ID; it closes/reopens and verifies both persisted filter metadata and values.
