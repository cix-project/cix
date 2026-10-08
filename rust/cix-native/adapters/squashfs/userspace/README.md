# Experimental SquashFS reader overlays

This is CIX-owned userspace glue for readers of the experimental SquashFS
method ID **65001**.  It decodes individual compressed SquashFS blocks through
the restricted profile; it neither reads nor writes an opaque CIX archive.
The common bridge maps data and fragment blocks to the 128 KiB profile cap and
inode, directory, metadata-table, and xattr blocks to the 8 KiB cap.

The overlays are pinned to the exact provisioned reader sources:

| Consumer | Source package pin | Origin | Required host dependency |
| --- | --- | --- | --- |
| `libsquashfs` | squashfs-tools-ng v1.2.0 `f2a3ad56e40c9711b23371238f9fa07dd24245f1` | AgentD/squashfs-tools-ng | generated `config.h`, C compiler |
| Squashfuse / `libsquashfuse` | Squashfuse 0.5.0 `3f4dd2928ab362f8b20eab2be864d8e622472df5` | vasi/squashfuse | generated `config.h`, C compiler, matched FUSE ABI |

A distribution package or source artifact must not be substituted for a newer
upstream checkout.

## Overlay contract

`cix_sqfs_userspace_decode` is the only new compressed-block decoder.  It
preserves upstream raw-block handling, accepts only method 65001, and returns
the reader's ordinary malformed-compressed-block error on any nonzero result.

For Squashfuse, `squashfuse/cix_squashfuse_decode.c` compiles the unchanged
`decompress.c` under renamed symbols and exports the normal dispatcher with
65001 added. `fs.c` therefore uses it for data, fragments and every metadata
reader, including xattrs. For libsquashfs, `libsquashfs/cix_sqfsng_compressor.c`
replaces `lib/sqfs/comp/compressor.c`; `cix_sqfsng_read_super.c` compiles the
unchanged `read_super.c` with its compressor-ID range widened only for this
translation unit. `overlay.mk` is appended to the generated out-of-tree
Autotools Makefile; it overrides only the pinned object targets and links the
CIX bridge/profile objects into the corresponding upstream library. The
deferred `squashfs-readers-v3` recipe records initial/final manifests for the
pinned source trees, runs `autogen.sh` only in disposable copies, and retains
final binaries while deleting only hash-receipted object caches.

No source file in either upstream project is edited in place. The build must
exclude the corresponding vendor object files, compile these replacement TUs,
and record the resulting build recipe. Neither overlay routes 65001 to a
standard decompressor fallback.

## Qualification plan

1. Build the validated `cix-mksquashfs` fixture from the sibling `tools/`
   contract with data, metadata, fragments, sparse files and xattrs.
2. Build each pinned reader plus its CIX overlay. Verify superblock ID 65001 is
   accepted only by that reader, then enumerate/read the fixture and compare
   data hashes and xattrs to its retained receipt.
3. Exercise the unknown-ID 65002 and malformed profile-version images made by
   the tools contract. Both readers must fail without emitting restored data.
4. For Squashfuse, perform a full public-library traversal and `sqfs_read_range`
   byte comparison for every fixture file, including extended regular and
   directory inode forms. Also perform direct `sqfs_xattr_lookup` checks because
   `squashfuse_extract` does not restore xattrs. Only in an authorized
   disposable FUSE environment, perform a mount. This does not
   qualify stock Squashfuse or the Linux kernel.

These overlays are experimental userspace reader support only. The v3/v4
records retain the stock demo extractor failure on extended inode forms; CIX
does not claim that demo was repaired. The v5 CIX-owned public-library contract
did restore all 304 fixture files and two xattrs. A disposable FUSE mount still
requires a separate v6 run with `libfuse3-dev` available. Stock `libsquashfs`,
stock Squashfuse, and all kernel readers remain unable to decode ID 65001 until
independently built with the matching source overlay.
