# Experimental SquashFS profile

`profile/` is a small, allocation-free C implementation of the experimental
block format in [SPEC.md](SPEC.md).  It has no dependency on a Python runtime,
filesystem tools, or the native CIX shared library.  Its only public surface is
[`include/cix_squashfs_profile.h`](include/cix_squashfs_profile.h).

It is intentionally not advertised as a stock SquashFS codec.  Stock tools,
kernel readers, and Squashfuse have no decoder for this profile.  A future
integration must use an explicit private experimental compressor ID and reject
images carrying it unless the matching decoder was selected at build time.

The profile has fixed decode caps: 128 KiB for data and 8 KiB for metadata.
It supports raw, RLE, and one CIXG1-compatible route-2/coder-4 substream.  The
encoder requests raw storage with `CIX_SQUASHFS_NO_BENEFIT` whenever the paid
profile header makes compression unhelpful.

## Future tools boundary

Upstream `squashfs-tools` has a compiled-in compressor table; it has no public
runtime codec plugin ABI.  Do not alter a vendor checkout in place.  The later
tools stage must pin an upstream revision, compile all unmodified upstream
translation units except its compressor-table translation unit, and compile a
CIX-owned replacement table against that revision's `compressor.h`.  That table
must reproduce every enabled upstream entry and add one explicitly configured
private experimental entry which calls this profile.  The source list and
header ABI form a reviewed compatibility contract for that pin.

That CIX-owned table is tool glue only.  A matching kernel decoder and a
userspace decoder are separate deliverables; neither can be substituted by an
opaque `.cix` transport or a generic external-program filter.
