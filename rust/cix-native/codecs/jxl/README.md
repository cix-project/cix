# CIX libjxl bridge

This is CIX-owned glue around a separately provisioned, unchanged **libjxl
0.12.0** build. It neither imports Python nor exposes libjxl structures to the
Rust ABI. `CIX_JXL_ROOT` is mandatory at configure time; CMake deliberately
refuses host-library discovery. The package recipe must ship the matching
libjxl runtime beside `cix_jxl_bridge` and verify `cix_jxl_library_version() ==
12000` before enabling this route.

The bridge supports one gray 2-D pixel plane and a bounded Z/H/W planar form.
Planar plane zero is the gray image; planes 1 through Z−1 are JXL optional
extra channels in increasing channel-index order. This is the frozen
`imagecodecs` `planar=True, photometric="GRAY"` mapping, and all samples use a
single unsigned depth and explicit little-endian transport. UINT8 is accepted
only for 1–8 bit samples and UINT16 only for 9–16 bit samples. It passes one
worker to `JxlThreadParallelRunner`, sets `JXL_BIT_DEPTH_FROM_CODESTREAM`,
`JxlEncoderSetFrameLossless(..., JXL_TRUE)`, and a frame distance of `0.0`.
Decode validates the advertised dimensions, unsigned sample depth, gray channel
count, absence of extra channels, preview and animation *before* installing its
caller-owned output buffer. It rejects unconsumed input after its single-image decode.
`cix_jxl_probe_gray_2d` supplies the same gray-still validation without an
output buffer and requires a CIX-paid exact pixel count; it is the only allowed
shape recovery path for the legacy CIXI1 frame.

`cix_jxl_probe_planar` requires the CIX-paid depth and total Z×H×W pixel count
before returning geometry. It rejects animation, preview, non-gray data,
subsampling, non-optional extras, mixed extra-channel depth, and dimensional
mismatches before any output buffer is supplied. `cix_jxl_decode_planar` then
installs one bounded buffer per plane. This permits retained CIXV1 mode 3 and
CIXI2 planar bands without flattening or changing their shape.

The native volume adapter retains the historical CIXI1 effort-7 entry point
and also exposes an explicit effort selector for the frozen effort-10 route.
Its shared mode-3 identity helper encodes each current input once at effort 10,
then compares complete CIXV1 mode-3 and CIXI2 zero-lifting envelopes around the
same metadata and payload. It selects CIXI2 only when its complete frame is
strictly smaller and reports both logical frame sizes plus the payload SHA-256.

Required qualification after provisioning:

1. Compile against the frozen official v0.12.0 headers and shared libraries;
   verify the runtime version exactly equals `12000`.
2. Round-trip gray 2-D 8-bit and each 9–16-bit depth through this bridge,
   including a value near each depth's maximum; compare every original byte.
3. Cross-decode CIX-produced JXL with the pinned `djxl`, and decode
   imagecodecs-produced 2-D and generated planar fixtures with this bridge. Record
   semantic byte equality, not a claimed identical codestream.
4. Run malformed/truncated input and geometry/type mismatch cases under the
   CIX output and memory policy; prove no output allocation occurs before
   basic-info bounds are checked.
5. Run bounded generated Z/H/W 8- and 16-bit fixtures at depths 2, 3, and 5,
   including CIXV1 mode 3 and CIXI2 restoration checks. Record the fixture
   generator version and digests in the release receipt.
