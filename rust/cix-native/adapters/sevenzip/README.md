# Experimental CIX 7-Zip SDK coder

This directory is a CIX-owned implementation of the official 7-Zip SDK
`ICompressCoder` contract. It turns an installed native CIX **independent
CIXG1 stream** into a coder that a separately integrated 7-Zip host can call.
It is deliberately not a ZIP method, a `.7z` format implementation, or a
stock 7-Zip plugin registration.

The method name is `CIX-EXPERIMENTAL-v1`. Its `0xC1580001` value is a private
development marker, not an allocated 7-Zip method ID. A deployment must record
that marker and version outside this adapter, ship the matching decoder, and
only then claim it can restore its own archives. Stock 7-Zip and ordinary ZIP
installations cannot decode these CIX payloads.

## Exact provision contract

Provision the unmodified official **7-Zip SDK 26.03** source distribution from
[7-Zip's SDK page](https://www.7-zip.org/sdk.html), released 2026-09-03. Keep a
private receipt containing the URL, retrieval date, archive filename and its
SHA-256. The selected extracted root must contain:

- `CPP/7zip/ICoder.h`
- `CPP/7zip/IStream.h`
- `CPP/Common/MyCom.h`

The adapter compiles only against those public SDK headers. Its interface source is
[`ICoder.h`](https://raw.githubusercontent.com/ip7z/7zip/26.03/CPP/7zip/ICoder.h)
and [`IStream.h`](https://raw.githubusercontent.com/ip7z/7zip/26.03/CPP/7zip/IStream.h)
at the same 26.03 tag. It does not bundle, modify, fetch, or search for the SDK. The release integrator must review the
licensing of the selected full SDK distribution and any host/plugin code; the
SDK page's LZMA-SDK licensing statement alone is not a license determination
for a whole 7-Zip host integration.

It also needs an explicitly selected installed CIX native library and its
matching `cix.h` and `cix_stream.h`. CMake rejects relative paths, absent
headers, any SDK version other than 26.03, and an unrecorded SDK archive hash.
For example, after the SDK and CIX installation have been independently
qualified:

```sh
cmake -S rust/cix-native/adapters/sevenzip -B build/cix-7zip \
  -DCIX_NATIVE_INCLUDE_DIR=/absolute/cix/include \
  -DCIX_NATIVE_LIBRARY=/absolute/cix/lib/libcix_native.so \
  -DCIX_7ZIP_SDK_ROOT=/absolute/7z-sdk-26.03 \
  -DCIX_7ZIP_SDK_ARCHIVE_SHA256=recorded-64-hex-sha256
cmake --build build/cix-7zip
```

The build produces a CIX-owned static coder library and an experimental shared
codec module. The module compiles the official 26.03 `CodecExports.cpp`
unchanged and supplies its documented `g_Codecs` registry with one CIX method,
so it exports the SDK module functions `GetNumberOfMethods`,
`GetMethodProperty`, `CreateDecoder`, `CreateEncoder`, and `GetModuleProp`.
It is a real SDK module factory, not merely a standalone class. `cmake --install`
places the module and the exact selected native CIX library together in an
explicit `Codecs/` directory. On ELF builds the module has an `$ORIGIN` install
runpath; on macOS it has `@loader_path`. Those settings are for this CIX-owned
layout, never a `PATH`, `LD_LIBRARY_PATH`, or host-global-library fallback.
The selected native library's own loader identity and dependencies still need
platform-specific inspection before a relocatable distribution can be claimed.
This repository does not install the result into a stock 7-Zip directory or
assert that any stock host will discover it.

## Stream, errors and limits

The coder uses the SDK's sequential interfaces exactly: `Read` and `Write` are
looped until their requested bytes are consumed, so valid partial calls are not
mistaken for end-of-stream. It reports cumulative bytes through
`ICompressProgressInfo::SetRatioInfo`, returning the host's result unchanged
(including `E_ABORT`). Input, output and CIX working-memory limits are required
in `cix_7zip_coder_options_v1`; CIX runs with those explicit profile, worker
and memory values. The adapter has fixed 64 KiB transfer buffers and does not
retain host-owned buffers.

Coder properties are exactly the five-byte `CIX7` version marker. They carry
no memory, output, worker or profile setting. Encoder `level` is accepted only
as a local profile choice; the decoder uses deployment-local limits compiled
into the module and archive metadata cannot relax them. Decode requires a
known `outSize`, rejects it before writing if it exceeds the configured output
limit, and requires exactly that many output bytes when the CIX frame finishes. Truncated or malformed frames return `S_FALSE`; invalid
options return `E_INVALIDARG`; CIX resource/output admission failures map to
`E_OUTOFMEMORY`; stream and progress failures keep their original HRESULT.
These are admission limits, not a portable process-RSS or CPU-time sandbox.
A host must impose operating-system limits where its threat model requires
those guarantees.

The only CIX route exposed here is the public `cix_stream.h` CIXG1 stream. It
does not expose whole-input BEST selection, specialist routes, retained
history, or arbitrary CIX archive frames. This keeps the wire contract bounded
but also means it is not an adapter for the full CIX command-line capability.

## Required qualification before distribution

No build or interoperability check has been run for this newly added source.
Configure with `-DCIX_7ZIP_BUILD_CONTRACT=ON` to build the prepared module-API
contract after exact SDK provisioning; it is not run automatically.
Before any distribution, the RC must compile it against the received 26.03 SDK
and selected native CIX identity, then cover: partial input/output streams,
empty input, progress cancellation, every configured bound, output-limit
rejection before host writes, truncation/corruption/trailing input rejection,
encoder/decoder fresh-process restoration, and relocation/loading of the host
integration. A separately provisioned Windows ABI test is required; this
CMake contract currently records only a portable C++ source arrangement and
makes no Windows, macOS, package-registry, or stock-7-Zip compatibility claim.

For Linux host qualification, upstream SDK 26.03's external-codec Console
target (`CPP/7zip/UI/Console/makefile.gcc`) and Format7zF target
(`CPP/7zip/Bundles/Format7zF/makefile.gcc`) provide a `7z` host and its sibling
`7z.so`. Put the installed module and selected native library in `Codecs/`
beside those two files. [`host-closure.sh`](host-closure.sh) prepares a strict
fresh-host test from six absolute paths: host, `7z.so`, module, native library,
empty work directory and an absolute SHA-256 tool. It verifies discovery,
creates a tiny `.7z` archive using `-m0=CIX-EXPERIMENTAL-v1`, exact-restores it
from a separate staged host, and requires a no-`Codecs/` decoder failure. It
scrubs loader override variables. It is a prepared qualification script and is
not run automatically. The standalone `7zz` target lacks
`Z7_EXTERNAL_CODECS`, so it cannot serve as either side of this module test.

## Current qualification scope

Linux x86-64 qualification passed with the official 7-Zip 26.03 external-codec host: create in a clean staged installation, restore in a fresh process, exact data comparison, and expected failure without the CIX codec. The adapter contract also passed property, partial-stream, cancellation, size and malformed-frame checks. Stock 7zz, Windows and macOS compatibility are not established by these checks.
