# Native Zstd dictionary C reference

This caller reference describes [`cix_dictionary.h`](../include/cix_dictionary.h),
the [C boundary](../src/c_dictionary_api.rs) and
[dictionary implementation](../src/dictionaries.rs) at source commit
`baf581fed43125dcb9fb48078a64b83ac793c01e`. It is a source contract, not a new
installed-library, platform or release qualification. The API produces and
consumes ordinary Zstd frames; it is separate from complete-buffer `CIXB1`,
incremental `CIXG1` and other codecs' format APIs.

## Seven entry points

All names below are declared in the header. Status-returning calls use the
`cix_status` symbols from [`cix.h`](../include/cix.h).

| Function | Caller-visible purpose |
| --- | --- |
| `cix_zstd_dictionary_create_v1` | Copy supplied dictionary bytes into a new owned handle. |
| `cix_zstd_dictionary_train_v1` | Train a new owned dictionary from concatenated samples and their sizes. |
| `cix_zstd_dictionary_free` | Free one returned handle exactly once; `NULL` is a no-op. |
| `cix_zstd_dictionary_identity_v1` | Copy the handle's Zstd ID and 32-byte SHA-256 into caller storage. |
| `cix_zstd_dictionary_bytes_v1` | Copy the exact dictionary bytes into caller storage for persistence/import. |
| `cix_zstd_dictionary_encode_v1` | Encode one caller input into an ordinary Zstd frame. |
| `cix_zstd_dictionary_decode_v1` | Decode exactly one frame using the supplied handle and declared identity. |

## Ownership and storage

Create/train receive a writable, aligned result-pointer slot. After their initial
slot/range checks, they initialize it to `NULL`; use a returned handle only on
`CIX_STATUS_OK`, not a presumed result on every error path. Training sizes must
have `sample_count` readable `size_t` entries whose checked sum is exactly
`samples_len`. Sample data is one contiguous concatenation, not an array of
sample pointers. The backend also checks training capacity/sample/memory bounds;
passing size checks does not guarantee that Zstd training succeeds.

A successful handle owns its dictionary copy independently of the caller's
original sample/import buffers. Only this API's free function may release it.
Do not pass an arbitrary pointer, a freed handle or a handle from another ABI.
No CIX-owned allocation is returned for the caller to release with `free()`.

For the **entire call**, keep every supplied object and nonempty byte range live,
correctly sized and pairwise disjoint, with no conflicting concurrent access.
Result-pointer slots, training size arrays and `written` counts require their
native pointer/`size_t` alignment. A `NULL` byte pointer is allowed only with a
zero byte length/capacity; it does not make required object/count pointers optional.
Numeric range/overlap checks do not prove that an arbitrary address is mapped
or writable. Panic containment is not protection against invalid C pointers.

Operations on a handle are serialized by its mutex, but that mutex does not make
shared caller buffers or count/result storage safe. Some caller writes occur
outside the lock. Never free a handle concurrently with another operation on it;
coordinate its lifetime and the complete call, not just the native codec phase.

## Options and resource admission

Encode/decode require `cix_dictionary_options_v1` with:

- `abi_version = CIX_DICTIONARY_ABI_V1` and `struct_size` at least the current
  `sizeof(cix_dictionary_options_v1)`; larger structures do not add new behavior.
- `reserved = 0` and a `level` in the implementation's inclusive `-131072..22`
  range. There is no dictionary-options default initializer in this API.
- Nonzero `output_limit` and `memory_limit`, representable in the target's `size_t`.
  Decode still validates these options even though its codec path does not use
  the compression level.

`output_limit` caps the codec's temporary result; `output_capacity` is the separate
size of the caller's destination. Memory admission includes the live handle,
retained dictionary allocation capacity, caller input/destination, temporary
output and selected native-state estimates. Training also accounts for its input
blob, sizes/views and workspace. A trained dictionary may retain more allocation
capacity than its exported byte length.

These are per-operation admission/estimation rules, not a hard process-RSS limit,
global budget across handles, deadline or cancellation interface. A dictionary
import's budget must cover both source bytes and the retained copy. The
[existing lifecycle example](../examples/dictionary.c) supplies explicit options
and budgets; its constants are not a universal minimum or resource guarantee.

## Output counts and limits

Export, encode and decode receive caller output storage plus an aligned writable
`size_t *written`. On `CIX_STATUS_OK`, `written` is the produced byte count.
On `CIX_STATUS_OUTPUT_TOO_SMALL`, it is the required count and output bytes remain
unchanged. A `NULL` output with capacity zero can request this sizing result.

For encode/decode, sizing still performs the operation and allocates temporary
output. It succeeds as a sizing result only if codec/output/memory admission
also succeeds. A too-small `output_limit` is not a promise of a required-size
answer. Retrying with larger caller capacity also changes the admitted memory
cost, so a successful probe alone does not guarantee the retry fits its budget.
Read the status before interpreting any count; do not assume every refusal
provides a valid required length.

## Persistence and identity

Persist the exact exported bytes and the full `cix_dictionary_identity_v1` with
the data needed to select them later. Import creates a fresh independent handle;
an identity value cannot reconstruct the dictionary. Import success is not a
promise that arbitrary supplied bytes will work for every frame.

Decode requires the declared Zstd ID **and** SHA-256 to match the supplied handle.
This remains mandatory when the frame's own dictionary ID is zero. A nonzero
frame ID is additionally checked against the handle. Trailing or concatenated
frames are refused. Identity matching is not sender authentication, encryption,
an arbitrary-corruption guarantee or a distribution/rights decision.

## Status and qualification boundary

The implementation distinguishes invalid arguments/options, resource admission,
codec failures, insufficient caller output and caught panics using the symbolic
statuses in `cix.h`. Dictionary handles are not `cix_context` handles; there is no
dictionary diagnostic-string getter in this header. Do not pass a dictionary
handle to the complete-buffer context-error API.

The existing C example demonstrates train, identity, export, import, freeing the
first handle and decoding with the independent imported handle. Existing
[Rust contract source](../tests/dictionary_contract.rs) and C-boundary unit source
cover selected lifecycle/refusal/capacity assertions; this guide does not rerun
or extend their qualification. Source/header presence and documentation review
do not prove an installed C consumer, native linkage, target platform, all
malformed-input cases or full-engine/provider/package acceptance. Use the
[SDK guide](../packaging/sdk/README.md) for the separate selected-artifact workflow.
