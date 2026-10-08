#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# Run under the root lease; stage matching decompressed module inputs for init.
set -eu

release=${CIX_KERNEL_RELEASE:-6.8.0-142-generic}
module=${CIX_SQUASHFS_MODULE:?externally built cix_squashfs.ko}
stage=${CIX_MODULE_CLOSURE_STAGE:?new empty module closure directory}
test ! -e "$stage"
mkdir -p "$stage/modules"

# The exact virtio bus and block stacks are discovered, not guessed.  Keep
# first appearance order so dependencies load before their consumers.
{
  modprobe --show-depends --set-version "$release" virtio_pci
  modprobe --show-depends --set-version "$release" virtio_blk
  modprobe --show-depends --set-version "$release" fuse
} | awk '!seen[$0]++' >"$stage/dependencies.txt"
: >"$stage/modules.list"
index=0
while IFS= read -r line; do
  case "$line" in
    insmod\ *) path=${line#insmod } ;;
    *) continue ;;
  esac
  test -f "$path"
  name=$(printf '%03d.ko' "$index")
  case "$path" in
    *.ko) cp "$path" "$stage/modules/$name" ;;
    *.ko.xz) xz -dc "$path" >"$stage/modules/$name" ;;
    *.ko.zst) zstd -qdc "$path" >"$stage/modules/$name" ;;
    *.ko.gz) gzip -dc "$path" >"$stage/modules/$name" ;;
    *) exit 1 ;;
  esac
  printf '%s\n' "$name" >>"$stage/modules.list"
  index=$((index + 1))
done <"$stage/dependencies.txt"
cp "$module" "$stage/cix_squashfs.ko"
if test "$index" -gt 0; then
  sha256sum "$stage"/modules/* "$stage/cix_squashfs.ko" >"$stage/SHA256SUMS"
else
  sha256sum "$stage/cix_squashfs.ko" >"$stage/SHA256SUMS"
fi
