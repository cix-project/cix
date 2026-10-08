# CIXZ1: bounded mixed streaming container

`CIXZ1` is a versioned CIX stream used by the bounded BEST streaming path. It
exists because a persistent native backend has decoder state across blocks,
while CIXG route payloads are independently decoded. Existing `CIXG1` and
`CIXG2` remain unchanged.

## Header and frames

The header is `CIXZ1`, a little-endian `u32` maximum source block size (at
most 65,536), then a reserved zero `u32`. Every frame is the normal CIX frame
prefix: route `u8`, source length `u32`, payload length `u32`, SHA-256 of the
reconstructed source bytes, then payload bytes.

| Route | Source length | Payload | Meaning |
|---|---:|---|---|
| 0 | 1..block | exactly source length | raw bounded fallback |
| 11 | 1..block | at most source + 256 KiB | a flushed portion of one live zstd stream |
| 12 | 0 | exactly 3 bytes | zstd epilogue and explicit state-reset boundary; hash is 32 zero bytes |
| 13 | 1..block | at most 8×source + 8192 | complete nested `CIXG1` or `CIXG2` archive for that block |
| 255 | 0 | exactly 3 bytes with an active zstd segment, otherwise empty | final zstd epilogue, with SHA-256 of the entire reconstructed stream |

Route 13 is deliberately a complete nested archive, including its header,
frame and footer. Its bytes are part of the outer candidate cost. It may only
contain CIXG1/CIXG2, never CIXZ1, which prevents recursive containers.

## State and integrity

Route 11 creates a zstd decoder state when none exists and preserves it until
route 12 or the final frame closes it. Route 0 and route 13 require no active
zstd state; the encoder writes route 12 before either when necessary. The
decoder checks each reconstructed frame hash before emitting it and accepts
the whole stream only after route 255 verifies the final zstd epilogue and
whole-stream SHA-256. A consumer can receive valid earlier output before EOF,
but final whole-stream integrity is only confirmed at the footer.

All frame payload and nested archive limits are checked before allocation.
Encoder candidate selection materialises raw, current-state zstd and complete
nested CIXG candidates for each buffered region. It retains two independently
advanced zstd contexts in identical committed state; one materialises the
trial and the other confirms it when selected. Both contexts are charged
against `--memory`. A route change additionally charges the reset-frame
prefix and the exact 3-byte final empty Zstd block during selection. Checksums are disabled in the native Zstd stream because CIX carries frame and whole-stream SHA-256. Every data block is flushed; the encoder verifies every materialized epilogue is exactly 3 bytes, otherwise it fails. The epilogue is serialized and verified in route 12.
