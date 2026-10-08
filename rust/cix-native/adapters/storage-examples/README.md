# Local CIX storage example

`cix-backup` is an operational Linux/POSIX local-repository example. It has no cloud SDK, credentials, network endpoint, or shipping Python runtime. It uses the public installed `cix.h` buffer API separately for each chunk.

Configure against an explicit installed SDK only:

    cmake -S . -B build -DCIX_SDK_INCLUDE_DIR=/opt/cix/include -DCIX_SDK_LIBRARY=/opt/cix/lib/libcix_native.so
    cmake --build build

Exact usage:

    cix-backup pack INPUT NEW_REPO CHUNK_BYTES
    cix-backup restore REPO NEW_OUTPUT OUTPUT_CAP

`CHUNK_BYTES` is 1 through 1,048,576. `OUTPUT_CAP` is the maximum restored plaintext byte count and may be zero for an empty input. The driver reserves 268,435,456 bytes for each operation, subtracting caller buffers and manifest/row allowances before passing the remaining native codec budget; encode output is bounded by plaintext plus 2 MiB and decode output by the manifest row's plaintext size. The driver keeps one plaintext and one archive chunk at a time, hashes the input incrementally with OpenSSL EVP SHA-256, and never buffers the whole input.

Packing requires a nonexistent repository, writes chunks into a private staging directory, writes `manifest.cixb1` after all chunks, then writes its paid `manifest.cixb1.sha256` sidecar and atomically publishes the directory using Linux `renameat2(RENAME_NOREPLACE)`. The manifest records row count, total plaintext bytes, chunk size, whole-file SHA-256, and canonical index/plain bytes/archive bytes/plain SHA-256/archive SHA-256 rows. Chunk names are deterministic (`chunk-0000000000000000.cix`).

Restore validates the bounded sidecar before parsing the bounded 16 MiB manifest, rejects symlinks, missing objects, extra objects, malformed, truncated, trailing, or inconsistent manifests, and verifies every archive, plaintext, and whole-file digest. It refuses an existing output and publishes only the verified temporary result using a no-overwrite hard-link publish.

This is Linux-first because safe directory publication relies on `renameat2(RENAME_NOREPLACE)`. Other platforms need an equivalent no-replace directory publish primitive; do not substitute a deleting rename.

`test_fixture.py` is test tooling only. Run it with a built executable:

    python3 test_fixture.py ./build/cix-backup
