# Native independent-block stream C reference

This caller reference describes [`cix_stream.h`](../include/cix_stream.h), its
[C boundary](../src/c_stream_api.rs) and
[incremental implementation](../src/incremental.rs) at source commit
`c999dd3ba4d5fe957a7270f579f03fc44dd33378`. It is source documentation, not a
new installed-consumer, native execution, platform or release qualification.

The profile is caller-driven, process-free **independent-block CIXG1**. The
encoder uses 65,536-byte input blocks; flush can close a smaller partial block.
There is no persistent compression-model history between those blocks. The
decoder accepts the selected independent CIXG1 header with a nonzero block size
no larger than 65,536 and zero history flags. It is not a full-engine route,
retained-history decoder, ordinary gzip/Zstd streaming interface, or a promise
that every format accepted by another CIX decoder is accepted here.

## Eleven entry points

| Function | Purpose |
| --- | --- |
| `cix_stream_encoder_create` | Snapshot options and allocate one encoder. |
| `cix_stream_decoder_create` | Snapshot options and allocate one decoder. |
| `cix_stream_encoder_destroy` | Release one live encoder exactly once. |
| `cix_stream_decoder_destroy` | Release one live decoder exactly once. |
| `cix_stream_encoder_process` | Consume available source bytes and drain archive output. |
| `cix_stream_decoder_process` | Consume available archive bytes and drain restored output. |
| `cix_stream_encoder_flush` | Close a pending partial block without ending the archive. |
| `cix_stream_encoder_finish` | Close remaining input and emit/drain the terminator. |
| `cix_stream_decoder_finish` | Drain pending plaintext and require completed archive input. |
| `cix_stream_encoder_reset` | Discard the current encoder archive and error state. |
| `cix_stream_decoder_reset` | Discard decoder input, pending plaintext and error state. |

Use options initialized by `cix_options_v1_default` from [`cix.h`](../include/cix.h).
Creation requires a readable, aligned complete v1 options prefix with version and
size, a recognized profile, nonzero workers and a representable memory/output
limit. A null options pointer is an invalid-options result, not default selection.
The writable aligned output-pointer slot must not overlap options. After the
initial slot/range/overlap checks it is set to `NULL`; use a handle only after
`CIX_STATUS_OK`. The handle owns its option snapshot, not the caller's structure.

Destroy accepts `NULL`. Any other handle must be live, returned by the matching
create function, and destroyed once after all calls have ended. Never cast it to
a complete-buffer context or pass it to `cix_context_last_error`: this stream
family exposes statuses and progress, not that context's diagnostic getter.

## Caller storage and progress

For the **entire call**, keep the handle and every supplied range/result object
live, correctly sized and without conflicting access. Input, output and the
complete aligned `cix_stream_result_v1` must be disjoint. A null byte pointer is
allowed only for length/capacity zero; the result pointer is always required.
Checked ranges/alignment do not prove arbitrary addresses are readable/writable.
Panic containment cannot make invalid C pointers safe.

The handle mutex serializes stream operations, not conflicting access to shared
caller buffers or result storage. Buffer slices are formed before the mutex is
acquired. Coordinate full calls and destruction externally when sharing a handle.

Read the result only on `CIX_STATUS_OK`. Its `consumed` and `produced` are byte
counts **for this call**, not cumulative positions or required buffer sizes.
Advance input by exactly `consumed`; retain any unconsumed suffix. Save exactly
the first `produced` output bytes before reusing that destination.

| Progress state | Caller action |
| --- | --- |
| `CIX_STREAM_NEEDS_INPUT` | No pending output remains. Offer more input, or use the appropriate finish operation when source input is exhausted. |
| `CIX_STREAM_NEEDS_OUTPUT` | Pending bytes remain. Supply writable output space and continue with unconsumed input or the same flush/finish operation. Input consumption can be zero. |
| `CIX_STREAM_FINISHED` | The selected archive is complete and pending output is drained. Finish may still produce its final bytes in this call. Reset before another archive. |

These states are not OS readiness notifications. The caller owns I/O, buffering,
loop/deadline bounds and backpressure. A zero-capacity destination can prevent
progress; repeating it is not a sizing query. Do not busy-loop on unchanged
zero-consumption/zero-production results or assume every call consumes its input.
This ABI has no complete-buffer `OUTPUT_TOO_SMALL`/required-length probe protocol.

## Flush, completion and reset

Encoder process buffers up to one block and can emit a header or pending frame
before consuming new input. Flush drains existing output, closes buffered partial
input and leaves the archive open; continue it with output space until pending
bytes are drained. It does not finish the archive or retain a compression model
for the next independent block. Do not discard pending flush output.

Once encoder finish begins, continue **finish**, not process or flush, until
`FINISHED`. The terminator includes the whole-stream checksum. Empty archives
still need header and terminal bytes. Finish is not implicit in destroy/reset.

Decoder process checks the selected header, bounded frame lengths and block
decoding before presenting that block's plaintext. The terminal frame validates
the whole-stream checksum; trailing bytes are refused. After all input has been
offered and consumed, call decoder finish with output space until `FINISHED`.
If no valid terminal frame was seen after pending plaintext drains, finish refuses
truncation. Plaintext from earlier calls is not proof of whole-archive completion:
keep incomplete/restored data provisional until successful completion, and do not
silently accept a valid prefix of a truncated, corrupt or trailing-data archive.

Errors returned from the incremental operation latch a failed state, requiring
reset before reuse. Invalid C-boundary arguments rejected before entering that
operation do not themselves run it. On a non-OK result the progress object is
not newly populated; output/internal progress may already have changed, so an
error is not a rollback or a safe instruction to replay the same chunk. Abort
that archive and reset/destroy according to the caller's recovery policy.
Reset discards pending data, counters and checksums; it is not a resume/checkpoint
API or a change to the snapshotted options. A poisoned mutex is not repaired by
reset's normal locking path.

## Status and resource boundaries

The C boundary uses `CIX_STATUS_INVALID_ARGUMENT` for selected pointer/range/
overlap checks, `INVALID_OPTIONS` for option/admission errors, `RESOURCE_LIMIT`
for the output-limit error, `CODEC_ERROR` for other incremental/codec refusals,
and `PANIC` for a poisoned lock or contained unwind. These are statuses from
[`cix.h`](../include/cix.h), not progress-state values. Not every allocation or
memory-budget refusal maps to `RESOURCE_LIMIT`; do not infer a required size.

Current construction requires at least 197,727 bytes of memory allowance, but
that lower bound does not guarantee a later codec operation fits. Admission
charges retained input/frame/output capacities and selected codec work against
the per-handle budget. Caller-owned input/output storage is budgeted by the
caller, not included in those stream-owned buffer charges. Output limits apply
across the archive: encoded bytes for the encoder, restored bytes for the
decoder. Neither limit is a hard process-RSS or global multi-handle ceiling.
The v1 C options do not expose the Rust-only deadline/cancellation handles.

The [existing C contract source](../tests/c_stream_contract.rs) covers selected
fragmentation, reset, range/alias, corruption and compatibility cases. The
[installed C consumer source](../packaging/sdk/tests/c_stream_installed_consumer.c)
shows bounded fragmented loops and exact restoration. This documentation pass
reads those sources, but does not execute them or broaden an original receipt.
Arbitrary chunk schedules, malformed inputs, all routes/codec combinations,
installed ABI, concurrency, platforms and full SDK/release acceptance remain
separate qualifications.
