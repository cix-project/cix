#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# Stage the already-built FUSE reader and its host-resolved ELF runtime for initrd.
set -eu

reader=${CIX_FUSE_READER:?existing squashfuse binary}
checker=${CIX_FUSE_XATTR_CHECKER:?leased static two-xattr checker}
stage=${CIX_FUSE_GUEST_STAGE:?new empty FUSE guest staging directory}

test ! -e "$stage"
test -x "$reader"; test -x "$checker"
! readelf -l "$checker" | grep -q 'Requesting program interpreter'
mkdir -p "$stage/rootfs" "$stage/fuse"
cp "$reader" "$stage/fuse/squashfuse"
cp "$checker" "$stage/fuse/cix-xattr-check"

# ldd reports the concrete loader and transitive DSOs used by this binary.
# Preserve absolute destination names beneath rootfs so the copied loader can
# resolve them inside the initramfs without touching host configuration.
ldd "$reader" | awk '
  /=> \/[^ ]+/ { print $3; next }
  /^[[:space:]]*\// { print $1 }
' | awk '!seen[$0]++' >"$stage/runtime-paths.txt"
test -s "$stage/runtime-paths.txt"
while IFS= read -r path; do
  test -f "$path"
  mkdir -p "$stage/rootfs$(dirname "$path")"
  cp -L "$path" "$stage/rootfs$path"
done <"$stage/runtime-paths.txt"

readelf -l "$reader" | awk '/Requesting program interpreter:/ {
  gsub(/\[|\]/, ""); print $4
}' >"$stage/interpreter.txt"
test -s "$stage/interpreter.txt"
while IFS= read -r path; do
  test -f "$path"
  mkdir -p "$stage/rootfs$(dirname "$path")"
  cp -L "$path" "$stage/rootfs$path"
done <"$stage/interpreter.txt"
(
  cd "$stage"
  sha256sum fuse/squashfuse fuse/cix-xattr-check runtime-paths.txt interpreter.txt
  find rootfs -type f -print0 | sort -z | xargs -0 sha256sum
) >"$stage/SHA256SUMS"
