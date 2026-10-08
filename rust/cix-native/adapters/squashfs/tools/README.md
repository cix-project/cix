# Experimental SquashFS userspace overlay

This directory contains CIX-owned glue for **one pinned** SquashFS-tools source
revision: `db038ef265ed00bb17aaf695a72f9b0c3489eb20` (the locally inspected
upstream snapshot).  It is a GPL-2.0-or-later userspace experiment.  It does
not alter files in the upstream checkout: [cix-tools.mk](cix-tools.mk) compiles
`cix_compressor_table.c` instead of upstream `compressor.c` in a separate build
directory.

The replacement preserves the upstream compressor entries enabled by that
revision and adds `-comp cix`, with private experimental on-disk compressor ID
**65001**.  IDs 1 through 6 are untouched.  The ID has not been assigned by
the SquashFS project and must never be used for stock images, release media, or
an image intended for an unmodified reader.

`cix_wrapper.c` implements the actual `struct compressor` callbacks inspected
in upstream `compressor.h`.  Its fixed profile is [../profile/SPEC.md](../profile/SPEC.md):

* normal file data, including fragment data, uses the data cap (128 KiB);
* metadata, inode/directory tables, and xattr blocks use the 8 KiB metadata cap;
* a non-beneficial block returns zero to mksquashfs, which stores that block raw;
* malformed compressed blocks are rejected before output is accepted.

The wrapper rejects `-b` values above 131072.  It has no `-X` options and
persists no compressor-specific options; the profile version is carried in each
compressed block.  This intentionally keeps all decoding bounded and avoids
ambient process state.

## Qualification

From an empty build directory, the scheduled job must use a clean checkout at
the stated commit and invoke:

```sh
make -f /absolute/path/to/cix-tools.mk cix-tools UPSTREAM=/absolute/path/to/squashfs-tools/squashfs-tools
CIX_MKSQUASHFS=$PWD/cix-mksquashfs CIX_UNSQUASHFS=$PWD/cix-unsquashfs \
  CIX_SQUASHFS_WORKDIR=/absolute/new/retained-workdir \
  CIX_SQUASHFS_PROCESSORS=1 /absolute/path/to/qualify.sh
```

`qualify.sh` runs a deterministic Python fixture generator. The explicit work
directory is retained on success and failure and receives a JSON hash receipt.
It exercises compressible, deterministic incompressible, sparse, tiny-file/
metadata, fragment, and, where the filesystem supports Python `os.setxattr`,
required xattr paths. It reads the little-endian SquashFS superblock itself to
check magic and ID 65001, avoiding the pinned upstream `unsquashfs -s` path:
that path can read xattr IDs before it applies `bytes_used` and rejects valid
fixtures. It then extracts with the matching decoder, compares file hashes and
required xattrs, rejects ID 65002, and rejects a version-corrupted first
compressed inode metadata block.

The helper accepts one to four tool workers via `CIX_SQUASHFS_PROCESSORS`
(default one); profile state is per block. SquashFS does not checksum arbitrary
compressed blocks, so this targeted malformed-profile check is not a claim that
every arbitrary bit corruption must be detected. The qualification job must run
the profile differential harness and save the exact upstream commit, compiler,
and command outputs.

This prototype is only for the two produced userspace executables.  Stock
`mksquashfs`, stock `unsquashfs`, libsquashfs consumers, Squashfuse, and Linux
kernel readers do not support ID 65001.  Kernel/VM work needs a separate,
matching decoder and qualification; no mount claim follows from this overlay.
