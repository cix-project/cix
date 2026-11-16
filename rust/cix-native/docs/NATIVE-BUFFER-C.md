# Native complete-buffer C interface

This reference describes the source interface in [cix.h](../include/cix.h),
[c_api.rs](../src/c_api.rs) and [core.rs](../src/core.rs). It does not qualify a
particular built library, installed consumer, provider, platform or release.
Use the matching header and library from the selected SDK, not a header from a
different source edition.

## Entry points and lifetime

| Entry point | Contract |
| --- | --- |
| `cix_options_v1_default` | Fill a caller-owned, writable complete options object. |
| `cix_context_create` | Copy validated options into one newly owned opaque context. |
| `cix_context_destroy` | Destroy that live context exactly once; NULL is a no-op. |
| `cix_encode_buffer` | Encode one complete input into caller-owned output storage. |
| `cix_decode_buffer` | Decode one complete archive into caller-owned output storage. |
| `cix_context_last_error` | Copy the context's bounded diagnostic without replacing it. |

Initialize options with `cix_options_v1_default`, adjust the selected limits,
then check `cix_context_create` before using its returned handle. A valid writable
output-handle slot is set to NULL before options are validated. Creation failures
have no new context from which to retrieve a diagnostic. A successful context
owns an immutable options snapshot; changing the original options does not
reconfigure it. Destroy it only after every call using it has finished.

Defaults are ABI version 1, the current `sizeof(cix_options_v1)`, DEFAULT profile,
one worker, 128 MiB of codec memory and `usize::MAX` output bytes represented in
the C field. Set deliberate finite output and memory limits for your operation.
FAST, DEFAULT and BEST have profile values 1, 2 and 3. Version 1 requires at least
the complete v1 options size and ignores advertised trailing bytes. Invalid
version/profile/size, zero workers or memory, and limits not representable by
the target's `usize` are rejected. Advertising a size does not make unreadable
memory valid.

## Caller storage and the output count

The host must supply valid live handles and readable/writable storage for the
entire call, with the alignment required by typed output objects and counts.
The library's NULL/range/overlap checks do not establish pointer provenance,
allocation lifetime or the actual size of caller memory. Invalid handles,
double destruction and concurrent destruction are not recoverable validation
cases. Catching Rust unwinding does not make undefined behavior safe or contain
every allocation failure or process abort.

For encode/decode:

- NULL input is allowed only with zero input length. Empty input is still
  subject to the selected codec's actual encode/decode semantics.
- NULL output is allowed only with zero capacity. `needed` is always required.
- Input, output and `needed` storage must not overlap. This is not an in-place
  interface. A nonzero region must remain valid for its declared length.
- On OK, `needed` is the result length; only that prefix of output is written.
- On OUTPUT_TOO_SMALL, `needed` is the required length and output is untouched.
  The caller allocates a sufficiently large buffer and makes another complete
  operation; the library does not return ownership of an allocation.
- Initialize your count before calling and interpret it only for the status
  that defines it. Early invalid-context/count/overlap rejection can leave it
  unchanged; other errors do not supply a successful result-size estimate.

A zero-capacity output call is real codec work, not a cheap maximum-size query.
The current implementation first computes a complete result in a private Rust
`Vec`, then checks capacity and copies it. A probe can fail for codec or context
limits instead of returning OUTPUT_TOO_SMALL. Retrying recomputes the result;
the context does not retain the previous result for a later copy. Bound input
acquisition, output allocation and retries in the embedding application.

## Status and diagnostics

| Value | Status | Meaning at this boundary |
| --- | --- | --- |
| 0 | `CIX_STATUS_OK` | The requested operation completed. |
| 1 | `CIX_STATUS_INVALID_ARGUMENT` | A checked caller argument/range/overlap is invalid. |
| 2 | `CIX_STATUS_INVALID_OPTIONS` | Context/native options are rejected. |
| 3 | `CIX_STATUS_OUTPUT_TOO_SMALL` | The complete result or diagnostic needs more caller storage. |
| 4 | `CIX_STATUS_CODEC_ERROR` | The codec operation failed. |
| 5 | `CIX_STATUS_PANIC` | A caught unwind or poisoned operation/diagnostic lock prevented completion. |
| 6 | `CIX_STATUS_RESOURCE_LIMIT` | The core operation exceeded the context output limit. |

Do not equate every memory/admission error with status 6: the current mapping
specifically maps `NativeError::OutputLimit` to RESOURCE_LIMIT. Codec errors can
also describe rejected resource use. The status is authoritative; diagnostic
text is supplementary presentation, not a stable machine identifier.

After a failed operation with a still-live context, use `cix_context_last_error`
to query/copy its diagnostic. Its `needed` includes one appended NUL terminator;
an empty diagnostic therefore needs one byte. OUTPUT_TOO_SMALL leaves the
diagnostic output buffer unchanged. The retained message is a UTF-8 prefix of at
most 511 bytes; copying it does not replace it. Interior NUL bytes are not
stripped, so `strlen` need not describe every copied diagnostic byte. Successful
buffer operations clear the message. Early argument rejection can leave an
older message; callers must not assume a fresh message accompanies every error.

Encode/decode operations on one context serialize through its operation mutex;
distinct live contexts may be used concurrently. Diagnostic access has its own
mutex, not an operation/result association token. Another call can replace the
message before a later diagnostic read, so a host sharing one context must
coordinate error-to-call association itself. No call may race destruction.

## Resource and capability limits

`output_limit` bounds produced bytes, not every allocation in the host or codec.
Codec memory admission is not an aggregate process-RSS hard ceiling. The v1
options have no deadline or cancellation-handle field, and these whole-buffer
calls do not expose incremental process/flush/finish/reset callbacks. The host
owns scheduling, lifetime and any external time/memory containment it needs.

[cix_stream.h](../include/cix_stream.h) is a separate independent-block CIXG1
stream interface. [cix_formats.h](../include/cix_formats.h) and
[cix_dictionary.h](../include/cix_dictionary.h) have separate ordinary-format
and Zstd-dictionary contracts. Their presence does not turn this complete-buffer
ABI into a full-engine specialist or retained-history interface. See the
[SDK guide](../packaging/sdk/README.md) for package boundaries and selected
installed-consumer requirements, and the [Rust guide](../packaging/sdk/RUST.md)
for the distinct Rust source API.
