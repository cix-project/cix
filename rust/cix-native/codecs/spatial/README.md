# CIX spatial-image bridge

This CIX-owned bridge exposes lossless single-component JPEG-LS (CharLS 2.4.2)
and reversible JPEG 2000 (OpenJPEG 2.5.0) for legacy CIXI1 spatial payloads.
It accepts only canonical little-endian u16 samples and validates paid width,
height, unsigned precision, one component, and no subsampling before CIX gives
it an output buffer. JPEG-LS uses NEAR=0 and non-interleaved grayscale; JPEG
2000 uses a reversible transform and one lossless layer. JPEG-LS accepts
precision 2–16; one-bit JPEG-LS is explicitly unsupported because CharLS has
no faithful one-bit frame representation for this canonical u16 contract.
Historical JPEG 2000 uses the JP2 container emitted by
`imagecodecs.jpeg2k_encode`; the bridge emits JP2 and accepts both JP2 and raw
J2K. For JP2 it validates declared boxes, isolates exactly one `jp2c` member,
and applies the codestream EOC check to that member rather than treating legal
container boxes as trailing bytes. Its bounded output stream supports OpenJPEG
seek/skip backpatching for JP2 box lengths while enforcing the same output cap.

Configure only against the receipt-backed SDK prefix:

```sh
cmake -S rust/cix-native/codecs/spatial -B <build> \
  -DCIX_SPATIAL_ROOT=/absolute/receipt-backed/spatial-sdk-prefix
```

`SpatialProvider` loads an explicitly supplied absolute package bridge path,
requires CharLS 2.4.2 and OpenJPEG 2.5.0, and does no host-library or Python
fallback. Decode admission charges retained input, sixteen times the CIX output
size for codec-side working state, and a 64 MiB bridge-working estimate before
allocating; encode uses the corresponding sixteen-times-input working estimate
plus both simultaneously live native/Rust output copies. These are conservative
CIX-side estimates, not a hard RSS limit for CharLS or OpenJPEG internals.

The bridges reject a valid member followed by trailing bytes before Rust
allocates its decode output. Their bounded structural scans account for the
entropy byte-stuffing rules: [CharLS 2.4.2 decoder strategy]
(https://raw.githubusercontent.com/team-charls/charls/2.4.2/src/decoder_strategy.h)
treats an `FF` followed by any byte below `80` as JPEG-LS entropy data, and
[OpenJPEG's MQ input logic](https://www.openjpeg.org/doxygen/mqc__inl_8h_source.html)
treats `FF 00` through `FF 8F` as JPEG 2000 packet data. JPEG 2000 tile parts
use declared `Psot` bounds; the scan also skips legal SOP and EPH packet
markers and all length-delimited header payloads before accepting the terminal
EOC.

`full_engine/spatial_frames.rs` retains the historical CIXI1 modes 2 (JPEG
2000) and 3 (JPEG-LS) envelope: `CIXI\x01`, representation byte, unsigned
LEB128 original/offset/region/metadata/payload lengths, big-endian CRC32,
metadata, and codec payload. CIXI1 deliberately has no shape fields. The
native `cix_spatial_probe_any_gray` ABI reads only bounded codestream headers,
requires one unsigned unsubsampled gray component and precision 1–16 (2–16
for JPEG-LS), and rejects a pixel or canonical-u16 output count above the paid
limits. The frame decoder then requires `width * height * 2 == region_bytes`
before requesting a decode buffer. The field order and representation behavior
are cross-referenced to
[`runtime/cix_runtime/legacy/spatial_image_codec.py`](../../../../runtime/cix_runtime/legacy/spatial_image_codec.py):
JPEG 2000 retains its significant precision; historical JPEG-LS receives a
uint16 array and therefore retains 16-bit frame precision.
The bridge converts any non-little-endian CharLS u16 I/O at its boundary, so
the CIX representation remains canonical little-endian u16 on every host.
`encode_spatial_frame` takes an explicit frame-output cap. Before every
metadata/native call it charges the caller-retained input bytes and the worst
simultaneously live CIX buffers. It caps the native payload first, then gives
metadata the remaining space after that *actual* payload, avoiding an arbitrary
potential-payload/metadata split. Decode subtracts retained frame and metadata
capacities before granting a native provider budget, then checks the final
reconstruction allocation separately.
