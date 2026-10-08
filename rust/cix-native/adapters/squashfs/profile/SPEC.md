# Experimental CIX SquashFS block profile, version 1

This is an experimental **block payload** profile.  It is not a SquashFS
compressor ID, does not modify an upstream filesystem image, and cannot be
mounted by stock SquashFS implementations.  A future, separately qualified
tool and kernel integration must give it an explicit private experimental ID.

Every compressed block is exactly:

```
u8 version = 1
u8 method                         # 0 raw, 1 RLE, 2 route-2/coder-4
u32le uncompressed_length
u32le payload_length
u8[payload_length] payload
```

The ten-byte header is paid overhead.  The decoder rejects a mismatched length
or any trailing byte. Source and destination ranges must not overlap. A data
block declares at most 131,072 bytes; a metadata
block declares at most 8,192.  Callers impose their own stored-block limits in
addition to these decode limits.

Method 0 payload is the uncompressed bytes.  It exists so a complete decoder
can read an explicit raw profile block, although the encoder returns
`NO_BENEFIT` for it: SquashFS should use its normal uncompressed-block path
when no profile representation is smaller.

Method 1 is the CIXG1 RLE payload: repeated `(byte, unsigned little-endian
base-128 count)` pairs.  Counts are nonzero, pairs consume the full payload,
and their sum must be `uncompressed_length`.

Method 2 carries precisely one CIXG1 route-2 substream:

```
u8 substream_count = 1
u8 coder = 4
u32le source_length              # equals uncompressed_length
u32le adaptive_payload_length
u8[adaptive_payload_length] adaptive payload
```

The adaptive payload is native CIX coder 4, order zero and eight bucket bits:
`[1, 0, 8, unsigned-LEB128 bit_length, MSB-first arithmetic bytes]`.  For each
symbol, the model uses `low = 2*prefix + symbol`,
`high = low + 2*count + 1`, and `total = 2*seen + 256`.  It is deliberately
block-local: no dictionary, prior block history, process state, or floating
point calculation influences the bytes. The arithmetic decoder follows the
native zero-padding rule after the declared bit length; the payload byte count
itself remains exact. This permits a valid arithmetic tail shorter than its
initialization width and does not make every payload-byte mutation detectable
at this layer.

The encoder considers RLE and coder 4 and returns only a representation shorter
than the source; a caller stores raw otherwise.  It uses only caller-provided
input/output buffers and fixed stack model state.  The API requires an encoder
output buffer at least as large as its source block; on success the result is
smaller.  Decoding validates limits before writing output.
