#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# Build a disposable BusyBox initramfs; run only under root's scheduled lease.
set -eu

busybox=${CIX_BUSYBOX:-$(command -v busybox)}
receipt=${CIX_SQUASHFS_RECEIPT:?fixture receipt.json}
closure=${CIX_MODULE_CLOSURE:?prepared matching module closure directory}
fuse_guest=${CIX_FUSE_GUEST_STAGE:?prepared FUSE reader and ELF runtime directory}
output=${CIX_VM_INITRD_OUTPUT:?absolute destination initramfs path}
overlay=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
stage=${CIX_INITRAMFS_STAGE:?empty disposable staging directory}

test ! -e "$stage"
case "$output" in /*) ;; *) exit 1;; esac
! readelf -l "$busybox" | grep -q 'Requesting program interpreter'
test -f "$closure/modules.list"; test -f "$closure/cix_squashfs.ko"
test -x "$fuse_guest/fuse/squashfuse"; test -x "$fuse_guest/fuse/cix-xattr-check"
test -f "$fuse_guest/interpreter.txt"; test -d "$fuse_guest/rootfs"
mkdir -p "$stage/bin" "$stage/sbin" "$stage/proc" "$stage/sys" "$stage/dev" "$stage/mnt"
cp "$busybox" "$stage/bin/busybox"
for app in sh mount umount insmod poweroff mkdir cat find sha256sum sync sleep; do
  ln -s busybox "$stage/bin/$app"
done
cp "$closure/cix_squashfs.ko" "$stage/cix_squashfs.ko"
mkdir -p "$stage/modules"
for module in "$closure"/modules/*.ko; do
  test -e "$module" || continue
  cp "$module" "$stage/modules/"
done
cp "$closure/modules.list" "$stage/modules.list"
cp -a "$fuse_guest/rootfs"/. "$stage"/
mkdir -p "$stage/fuse"
cp "$fuse_guest/fuse/squashfuse" "$fuse_guest/fuse/cix-xattr-check" "$stage/fuse/"
cp "$overlay/initramfs/init" "$stage/init"
chmod 0755 "$stage/init"

python3 - "$receipt" "$stage/fixture.sha256" <<'PY'
import json, sys
receipt, output = sys.argv[1:]
entries = json.load(open(receipt, encoding="utf-8"))["entries"]
with open(output, "w", encoding="utf-8") as handle:
    for path, item in sorted(entries.items()):
        handle.write(f'{item["sha256"]}  {path}\n')
PY
(cd "$stage" && find . -print | cpio -o -H newc | gzip -9 >"$output")
