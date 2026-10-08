# CIXB1 native backend envelope

CIXB1 stores one complete, bounded native-backend payload inside a CIX
archive. It is an optional whole-archive candidate for a named regular file;
it is not a CIXG streaming frame format and it is not a ZIP or gzip file.

## Wire layout

All multi-byte fields are unsigned little-endian integers. The fixed header
is 47 bytes, followed by exactly `payload_bytes` payload bytes:

| Offset | Size | Field |
| --- | ---: | --- |
| 0 | 5 | ASCII magic `CIXB1` |
| 5 | 1 | backend ID |
| 6 | 1 | effort profile: 1 FAST, 2 DEFAULT, 3 BEST/size |
| 7 | 4 | restored byte count |
| 11 | 4 | payload byte count |
| 15 | 32 | SHA-256 of the restored bytes |
| 47 | variable | backend payload |

Backend IDs are 1 gzip, 2 bzip2, 3 XZ, 4 Zstd, 5 Brotli, 6 pinned ZPAQ
7.15 level 5, and 7 vendored libbsc 3.3.12. The payload uses that backend's
native self-describing stream or block representation. The CIXB1 header
supplies the bounded output length and independent integrity digest.

The decoder rejects unknown backend/profile IDs, oversized fields, a header
whose declared payload length does not exactly consume the archive, backend
trailing or corrupt payloads, decoded-length mismatches, and SHA-256
mismatches. Backend 7 additionally rejects a zero restored length: libbsc
does not encode empty memory blocks, so empty CIX data must use another valid
CIX representation.

## Candidate set and limits

At this revision BEST considers 14 configuration-level CIXB1 descriptors,
not merely seven backend names:

- gzip fast and size;
- bzip2 size;
- XZ default-6, size-9e, and two 128 MiB-dictionary settings;
- Zstd fast, default-3, and size-22;
- Brotli fast-0 and generic quality-11/window-22;
- libbsc 3.3.12 BWT/static-QLFC/LZP(15,72)/fast-mode; and
- ZPAQ 7.15 level 5.

All configuration metadata, native payload bytes, this header and the SHA-256
digest are included when candidates are compared. No filename, corpus,
content-type or fixed source dictionary selects a backend. A candidate can be
omitted only with a reported resource, constraint or normal libbsc
not-compressible reason; CIX retains a valid native fallback.

The CIXB1 source and restored-size cap is 128 MiB. It accepts named regular
input only, buffers that bounded input once, and conflicts with `--stream`.
BEST and decoding default to a 6 GiB accounted working-memory budget. This is
an admission budget, not an aggregate-process RSS guarantee. The decoder uses
the same implicit policy so it can restore a PAQ archive emitted by BEST;
ZPAQ L5 and BSC may still be omitted or rejected under a smaller explicit
`--memory` setting.

## Cancellation and portability

ZPAQ's pinned interpreter observes cancellation during source reads and
interpreted instructions. bzip2, Zstd, Brotli, libbsc and XZ decoding use
coarse native calls with checks before and after the call. A libbsc encode or
decode can therefore delay SIGINT or a BEST deadline until its block returns,
up to the 128 MiB CIXB1 cap.

The current host's Rust build and native library linkage have been checked.
This document makes no build, performance or compatibility claim for another
host until that host is built and verified. CIXG framed streaming, including
automatic streaming selection, remains the path for pipes and unknown input
lengths. Automatic CIXM6 packaging remains contingent on its final
qualification.
