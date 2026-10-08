# Experimental CIX RocksDB CompressionManager

This C++ adapter is pinned to official RocksDB **v10.10.1**, release commit
`4595a5e` (2026-02-02), using public headers
`include/rocksdb/{advanced_compression.h,compression_type.h,options.h,table.h}`.
The upstream API is explicitly experimental. Root must provision the unchanged
official release archive `https://github.com/facebook/rocksdb/archive/refs/tags/v10.10.1.tar.gz`, record its SHA-256 and full commit, and compile against its matching library; no host fallback is permitted.

`compression_type.h` reserves `kCustomCompression80..FE` for custom managers.
This adapter uses only `kCustomCompression80`, writes an outer `CIXR` v1 frame
with an exact little-endian uncompressed size, and uses compatibility name
`CIX_RocksDB_Experimental_v1`. A database containing such SSTs must reopen with
this exact adapter: stock RocksDB has no CIX decoder and must fail instead of
misinterpreting a custom block. Compatibility is never claimed for another
manager/name.

The callbacks use public complete-buffer `cix.h`, one CIX worker, 64 MiB block
cap by default (128 MiB maximum), and CIX memory/output admission. They never
run the CLI or a subprocess. A non-beneficial, oversized-output, or resource
refusal returns RocksDB's raw `kNoCompression` fallback without emitting bytes.
Malformed frame, size, or CIX decode failures return corruption. No dictionary,
full-engine selection, or hard process-RSS limit is claimed.

```sh
cmake -S rust/cix-native/adapters/rocksdb -B build/cix-rocksdb \
  -DCIX_NATIVE_INCLUDE_DIR=/absolute/cix/include -DCIX_NATIVE_LIBRARY=/absolute/cix/lib/libcix_native.so \
  -DROCKSDB_INCLUDE_DIR=/absolute/rocksdb/include -DROCKSDB_LIBRARY=/absolute/rocksdb/lib/librocksdb.so
cmake --build build/cix-rocksdb && ctest --test-dir build/cix-rocksdb --output-on-failure
```

Required final host qualification additionally opens a real DB, writes/reopens
custom and raw blocks, rejects corrupted CIXR data, proves cap behavior, and
proves opening custom SSTs without this manager fails. The checked-in contract
already performs the create/write/flush/close/reopen/read lifecycle against a
real RocksDB library. It asserts the persisted table `compression_name` records
`CIX_RocksDB_Experimental_v1;80;`, callback counters show CIX encode/decode and
raw fallback execution, and malformed CIXR metadata is rejected. The final
qualification still needs its own fresh source/library identities and an
explicit no-manager reopen-failure receipt.

## Current qualification scope

Linux x86-64 host qualification passed against RocksDB v10.10.1 commit 4595a5e95ae8525c42e172a054435782b3479c57 with the SDK default no-RTTI build. The contract creates, flushes, closes and reopens a real database, verifies custom and raw blocks, rejects invalid frame bounds, and proves a reader without this manager cannot restore the custom SST data. This remains an experimental adapter; final release source and other platforms need their own qualification.
