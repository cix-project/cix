# Optional QuixDB Squash plugin

This is an **experimental Linux-only QuixDB Squash plugin**, separate from the
base CIX command-line package. It is not SquashFS, `mksquashfs`, `unsquashfs`,
libsquashfs, or squashfuse support.

The plugin presents three bounded interfaces from the public QuixDB Squash
plugin ABI:

- **buffer:** uses installed `cix.h` contexts and caller-owned buffers;
  levels 1–3 map to CIX FAST, 4–7 to DEFAULT, and 8–9 to BEST. CIX does not
  publish a finite all-input archive maximum for this boundary, so the plugin
  reports `SIZE_MAX` as its conservative maximum and callers should not infer
  a practical allocation guarantee from it;
- **stream:** uses a CIX-owned Squash `create_stream`/`process_stream` object
  over public `cix_stream.h` CIXG1 processing. It is native independent-block
  streaming, not full-engine CIX routing or retained history;
- **splice:** is deliberately unregistered. Squash 0.8's upstream custom-splice
  path calls the wrong callbacks in the affected `size == 0` branch. The
  exported `cix_squash_splice` remains a legacy CIX compatibility symbol, but
  this plugin directs host file/custom splice work through its native stream;
- **flush:** is not advertised: `SQUASH_CODEC_INFO_CAN_FLUSH` is absent because
  CIX exposes no decoder flush. Process and finish are supported for both
  directions; a host `FLUSH` operation returns `SQUASH_INVALID_OPERATION`.

## Pinned SDK provisioning

Provision the public upstream repository at exact commit
`713eeca85cccadde39bf57f8b59ea663a085a347` (the current `master` observed on
2026-10-05), then record the resulting archive hash, build flags, installed
headers, `squash-0.8` pkg-config metadata, and ABI/API version in the release
receipt. The upstream’s latest public release page documents 0.7.0; this RC28
work therefore requires a source-pinned 0.8 SDK build and must not claim a
published 0.8 release tag.

- Source: <https://github.com/quixdb/squash/tree/713eeca85cccadde39bf57f8b59ea663a085a347>
- Plugin guide: <https://raw.githubusercontent.com/quixdb/squash/713eeca85cccadde39bf57f8b59ea663a085a347/docs/plugin-guide.md>
- Codec ABI structure: <https://raw.githubusercontent.com/quixdb/squash/713eeca85cccadde39bf57f8b59ea663a085a347/squash/squash-codec.h>
- Interface behavior: <https://raw.githubusercontent.com/quixdb/squash/713eeca85cccadde39bf57f8b59ea663a085a347/docs/internals.md>

The build helper requires `SQUASH_SOURCE_REVISION` to repeat that commit and
an installed `squash-0.8` pkg-config package. It compiles only CIX-owned shim
source and links a caller-selected prebuilt `libcix_native.so`; it neither
builds CIX nor acquires Squash.

## Qualification required

A scheduled host receipt must verify plugin discovery by exact directory,
codec initialization, buffer round trips with exact capacity/error handling,
native stream and host splice compression/decompression, empty/single-byte/random
input, malformed and trailing archive rejection, profile mapping, and rejected
flush behavior. It
must separately establish thread safety and valid host callback behavior.
Neither this source nor a successful plugin load qualifies a SquashFS adapter.

## Pinned host-contract source and core-only build

`contract.c` is a source-only installed-host qualification executable. It takes
one absolute Squash plugin-root argument (the parent of `cix/`), obtains the
codec through `squash_get_codec("cix")`, and then exercises public buffer,
`SquashReadFunc`/`SquashWriteFunc` splice, and host stream-emulation paths. It
checks empty, one-byte, deterministic random and repetitive buffer cases;
zero-capacity output; corruption; trailing input; fragmented callbacks; and
terminal `SQUASH_STREAM_STATE_FINISHED`. It does not call a native CIX flush or
claim one exists.

A non-recursive upstream source archive can lack required git-submodule files,
including `squash/tinycthread/source/tinycthread.c`, `squash/hedley`,
`tests/munit`, and `utils/parg`. The upstream top-level CMake unconditionally
adds the core, tests and utilities, so that archive alone cannot configure a
core build. Do not paper over that gap by changing upstream source.

Provision instead a **recursive checkout or equivalent source receipt** for
commit `713eeca85cccadde39bf57f8b59ea663a085a347`, recording each submodule
revision and source hashes. The following deliberately disables every optional
codec plugin; it still builds Squash's required core/tests/utilities from that
already provisioned recursive tree and requires no optional codec downloads:

```sh
cmake -S /absolute/squash-recursive -B build/squash-core \
  -DCMAKE_BUILD_TYPE=Release \
  -DENABLE_BRIEFLZ=no -DENABLE_BROTLI=no -DENABLE_BSC=no -DENABLE_BZIP2=no \
  -DENABLE_COPY=no -DENABLE_CRUSH=no -DENABLE_CSC=no -DENABLE_DENSITY=no \
  -DENABLE_DOBOZ=no -DENABLE_FARI=no -DENABLE_FASTLZ=no -DENABLE_GIPFELI=no \
  -DENABLE_HEATSHRINK=no -DENABLE_LIBDEFLATE=no -DENABLE_LZ4=no -DENABLE_LZF=no \
  -DENABLE_LZFSE=no -DENABLE_LZG=no -DENABLE_LZHAM=no -DENABLE_LZJB=no \
  -DENABLE_LZMA=no -DENABLE_LZO=no -DENABLE_MINIZ=no -DENABLE_MS_COMPRESS=no \
  -DENABLE_NCOMPRESS=no -DENABLE_QUICKLZ=no -DENABLE_SNAPPY=no -DENABLE_WFLZ=no \
  -DENABLE_YALZ77=no -DENABLE_ZLIB=no -DENABLE_ZLIB_NG=no -DENABLE_ZLING=no \
  -DENABLE_ZPAQ=no -DENABLE_ZSTD=no
cmake --build build/squash-core --target squash0.8
```

After separately building and placing the CIX plugin and selected native CIX
library in one explicit plugin root, compile the consumer against that core
installation's header and `squash-0.8` library, then invoke it with the plugin
root. The scheduled receipt must also record the exact plugin discovery path
and loader/RPATH evidence; source compilation is not proof that the host loaded
the selected plugin.

For the consumer itself, use the installed core's declared include/library
paths; do not substitute a system Squash ABI or let the compiler discover a
plugin from `PATH`:

```sh
cc -std=c11 -Wall -Wextra -Werror packaging/squash/contract.c \
  $(pkg-config --cflags squash-0.8) $(pkg-config --libs squash-0.8) \
  -o build/cix-squash-contract
build/cix-squash-contract /absolute/plugin-root
```

This command is a prepared qualification step only. It has not been run in
this source revision.
