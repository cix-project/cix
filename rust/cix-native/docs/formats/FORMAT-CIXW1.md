# CIXW1 independent window carrier, version 1

CIXW1 permits bounded, read-once selection of complete CIX archives within
independent input windows. It is a transport for existing models, not a new
predictive mixer. The CLI uses it for eligible stdin, nonregular inputs and
files outside the whole-input selector's admission limits. Explicit retained
history, idle-time flush, legacy stream or unsupported expert controls keep
their existing encoder path.

All integers below are unsigned little-endian. Hashes are 32 raw SHA-256 bytes.

| Section | Fields | Bytes |
| --- | --- | ---: |
| Header | ASCII `CIXW1`, version `1`, maximum decoded window size (`u32`) | 10 |
| Window | tag `1`, sequence (`u64`), decoded length (`u32`), archive length (`u32`), decoded-window SHA-256, complete nested archive | 49 + archive length |
| Terminal | tag `2`, window count (`u64`), total decoded length (`u64`), SHA-256 of concatenated original bytes | 49 |

Sequence numbers start at zero and increase by one. Window lengths are positive
and at most the header's maximum; an empty stream contains only header and
terminal. Nested CIXW1 is prohibited. A nested archive must be complete and
independently decodable by the installed native/full-engine runtime. The
terminal is mandatory and no bytes may follow it.

The decoder admits encoded-window allocation, remaining model memory and
cumulative decoded output before decoding a window. It checks nested archive
integrity, decoded length, per-window hash and the final whole-stream hash.
Incremental stdout may contain a decoded prefix before a later integrity error;
ordinary file output retains the CLI's temporary-file/atomic-commit policy.
Cancellation and deadlines are cooperative at I/O/model boundaries.

The encoder retains one bounded input window and its selected output, plus
declared selector/model state. Current CLI windows are at most 16 MiB and shrink
to fit the configured memory budget. The reader waits for a complete window or
EOF, emits and flushes a completed record, then reads the next window. It does
not spool the entire asset and is not an idle-time flush protocol. Diagnostics
are bounded independently of stream length.

Every window charges both its nested archive framing and these carrier fields.
There is no history across windows and no comparison with a complete whole-file
archive. Whole-file PAQ context, schemas or relationships crossing a window
boundary are unavailable; models can still operate on eligible window-local
content. This behavior is separate from the retained-history CIXG stream and
from whole-input portfolio selection. No compression or throughput superiority
is implied by this format.
