# CIX experimental kernel SquashFS overlay

This directory owns the restricted profile decoder and the
build/VM boundary for an experimental filesystem named `cix_squashfs`.  Its
only compressor id is private experimental value `65001`; it never changes the
stock `squashfs` module or claims stock interoperability.

`cix_profile_portable.c` compiles the already qualified portable profile source
unchanged through a CIX-owned kernel type shim.  It decodes the profile-v1 raw,
RLE and adaptive forms with the fixed limits of 128 KiB data and 8 KiB metadata
and validates complete framing before returning success.  The CIX-owned
decompressor wrapper turns decoder failure into `-EIO`.

The installed Ubuntu 6.8.0-142 header package contains only the SquashFS
Kconfig and Makefile, not its reader implementation.  Supply the immutable
official Ubuntu 6.8.0-142.142 `fs/squashfs/` extraction plus the matching
configured build tree and `Module.symvers`.  The public
[`upstream-linux-6.8.0-142.142-squashfs-manifest.json`](upstream-linux-6.8.0-142.142-squashfs-manifest.json)
pins the Ubuntu package URL, package digest, and SHA-256 digests of all 34
required reader files.  Every reader translation unit shares private structures
and static helpers.

The staging script symlinks unchanged upstream reader files and generates
CIX-owned wrapper translation units.  `cix_squashfs_prefix.h` gives reader
global symbols a private namespace.  `cix_squashfs_fs_register.c` changes only
the cloned reader's VFS name to `cix_squashfs`.  The CIX-owned decompressor
table accepts only 65001, rejects every other compressor ID and rejects
compressor-option bytes.  No vendor source is patched.

## External-module build

Extract the package named in the manifest and set `CIX_LINUX_SRC` to the
directory containing its `fs/squashfs/` tree.  Set `CIX_KERNEL_BUILD` to the
configured build tree for the exact target kernel, with its `Module.symvers`
present.  Use a new, empty directory outside both source trees for the stage.

```sh
export CIX_LINUX_SRC=/absolute/path/to/extracted-linux-source
export CIX_KERNEL_BUILD=/absolute/path/to/matching-configured-kernel-build
export CIX_EXPECTED_KERNEL_RELEASE=6.8.0-142-generic
export CIX_SQUASHFS_STAGE=/absolute/path/to/new-cix-squashfs-stage
sh scripts/build-overlay.sh
make -C "$CIX_KERNEL_BUILD" M="$CIX_SQUASHFS_STAGE" \
  CONFIG_CIX_SQUASHFS=m modules
```

`build-overlay.sh` verifies the bundled public manifest by default.  A caller
may set `CIX_SQUASHFS_SOURCE_RECEIPT` to another manifest with the same
non-empty `files` mapping schema when deliberately building against a separately
pinned source tree.  It never accepts a file whose digest differs from that
manifest.

For a disposable VM qualification, use `prepare-vm-fixture.sh` to build a
new 305-file fixture (the retained 304 files plus `empty`) and its bounded
corruption variants.  Its supplied offsets name a verified profile block and
outer SquashFS length field; the profile-cap case proves the 8 KiB/128 KiB
declared-length gate before profile output, rather than claiming an OOM test.
Then use `prepare-module-closure.sh` for the installed matching kernel's
VirtIO and FUSE module dependency order and decompressed bytes.  The existing
FUSE reader runs inside the guest because the host runner cannot mount FUSE
under its no-new-privileges policy.  `prepare-fuse-guest.sh` stages that reader,
a static CIX two-xattr checker, and the reader's resolved ELF loader and DSOs.
`make-initramfs.sh` refuses dynamic BusyBox and packages that closure, runtime
and externally built module.
`qualify-vm.sh` runs TCG by default with two vCPUs and a 90-second timeout;
KVM is only used when explicitly selected and available.
The guest must load `cix_squashfs.ko`, mount the good image as
`-t cix_squashfs`, hash every fixture file against `receipt.json`, read an
inode-table-backed small file, fragment-backed file, sparse file and empty
file.  It then uses FUSE to hash that same fixture and check two xattrs before
rejecting malformed version, truncated payload and oversized declared length at
the profile cap.  The separate kernel and FUSE `CIX_VM_PASS` markers are
deliberate receipt gates; their absence is qualification failure.

## Qualified scope

The pinned `kernel-module-v3` and disposable `kernel-vm-v2` receipts qualify
this overlay only for Ubuntu `6.8.0-142-generic`, the verified Ubuntu
6.8.0-142.142 `fs/squashfs` extraction, and the retained private-ID fixture.
The VM used TCG with two virtual CPUs.  Its kernel and guest Squashfuse paths
each SHA-256 checked all 305 fixture files and together checked two required
xattrs per reader; malformed-version, truncated-payload, and declared-cap
images were rejected. Source, module and fixture identities and the complete
test console are retained in the development qualification records.

This does not qualify stock `squashfs`, stock tools or Squashfuse, any other
kernel or reader source version, release media, arbitrary image corruption, or
module-signing policy outside the disposable VM.  The prior v1 VM failure is
retained in the ledger: the pinned reader narrowed ID 65001 to signed short
`-535`; the CIX-owned v3 dispatcher restores its `u16` wire identity without
patching upstream source.
