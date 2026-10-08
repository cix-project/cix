# PAQ v215 library bridge

`codecs/paq/v215` is CIX-owned glue around unchanged packaged PAQ v215
sources. It produces a hidden-symbol shared object with only
`cix_paq_v215_process` and `cix_paq_v215_bridge_version` exported. PAQ source
is configured through `CIX_PAQ_SOURCE_ROOT`; the bridge neither copies it into
the CIX tree nor searches a development or research checkout at runtime.

The source entry point is compiled with `main=cix_paq_v215_upstream_main` and
the bridge calls the unchanged `processCommandLine`. Safe floating-point flags
remain `-fno-fast-math -ffp-contract=off`; no `-march=native` flag is added.
Each future PAQ variant must be a separate shared object with hidden upstream
symbols. This prevents v215's bundled zlib and static globals from colliding
with v216, joint-discount, or store-state.

PAQ v215 intentionally catches `IntentionalException` inside
`processCommandLine` and can then return zero. It also retains static
`Models`/`ProgramChecker` state. Consequently the bridge accepts one call per
fresh worker process, requires the expected output not to exist before the
call, and reports a missing output as `UPSTREAM_FAILURE`. The CIX ABI itself never creates a child, changes environment variables, or
changes resource/signal state. However, the unchanged PAQ LSTM sources contain
`exit(1)` error paths; the bridge cannot make that process-local behavior safe
for embedding. Upstream output also remains on the worker stdout/stderr. It is
therefore not an SDK codec API.

The full engine must launch the same CIX executable in isolated worker mode,
create a private directory, apply its explicit worker environment and resource
limits, dynamically load one requested PAQ variant, and call it once. Native
SDK calls must reject this provider rather than silently invoking it.

## Build and qualification required from the scheduler

Build only under the shared four-core / 12 GB / 4 GB temporary policy:

```sh
CIX_PAQ_SOURCE_ROOT=/path/to/packaged/vendor/paq/v215 \
CIX_PAQ_BUILD_DIR=/path/to/disposable/build \
rust/cix-native/codecs/paq/v215/build-v215.sh
```

Root must add the resulting library to the CIX-owned loader/build closure; this
bridge deliberately does not edit `build.rs`, Cargo metadata, or root library
registration. Qualification must then show: exported-symbol allowlist; one
small archive matched byte-for-byte against the unchanged v215 executable;
cross-decode in both directions; a deliberate invalid request returning a
bridge status without terminating CIX; a second same-worker call returning
`ALREADY_USED`; and an SDK request rejected before loading PAQ. Run those as
focused isolated-worker checks, not as a corpus or performance campaign.

## Variant plan

| Variant | Source root supplied at build | Bridge status | Reason |
| --- | --- | --- | --- |
| v215 | `vendor/paq/v215` | CIX bridge source prepared | static model state; separate DSO |
| v216 | `vendor/paq/v216` | CIX bridge source prepared | static model state and LSTM `exit(1)` paths |
| joint-discount | `vendor/paq/joint-discount` | CIX bridge source prepared | static model state and LSTM `exit(1)` paths |
| store-state | `vendor/paq/store-state` | CIX bridge source prepared | static model state and LSTM `exit(1)` paths |

Each bridge has now been prepared after source inspection, with a distinct C
ABI symbol and library name. It compiles its own bundled dependencies into a
hidden DSO and still requires byte/cross-decode evidence. The worker launcher
must keep every call process-isolated because the unchanged sources retain
static state and the three listed LSTM paths may call `exit(1)`.
