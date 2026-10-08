#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# Disposable QEMU qualification runner.  It never touches the host kernel.
set -eu

kernel=${CIX_VM_KERNEL:?matching rebuilt kernel image}
initrd=${CIX_VM_INITRD:?initramfs containing cix_squashfs.ko and fixture}
image=${CIX_SQUASHFS_IMAGE:?private-ID-65001 image.sqfs}
bad=${CIX_SQUASHFS_BAD_IMAGE:?malformed private-ID image.sqfs}
truncated=${CIX_SQUASHFS_TRUNCATED_IMAGE:?truncated private-ID image.sqfs}
oversized=${CIX_SQUASHFS_OVERSIZED_IMAGE:?oversized private-ID image.sqfs}
receipt=${CIX_SQUASHFS_RECEIPT:?fixture receipt.json}
out=${CIX_VM_OUTPUT:?absolute output directory}
timeout_seconds=${CIX_VM_TIMEOUT_SECONDS:-90}
accel=${CIX_QEMU_ACCEL:-tcg}
case "$out" in /*) ;; *) exit 1;; esac
mkdir -p "$out"
test -f "$kernel"; test -f "$initrd"; test -f "$image"; test -f "$bad"; test -f "$truncated"; test -f "$oversized"; test -f "$receipt"

timeout "$timeout_seconds" qemu-system-x86_64 -nodefaults -no-reboot -display none -accel "$accel" -m 1024M -smp 2 \
  -kernel "$kernel" -initrd "$initrd" -append 'console=ttyS0 panic=-1' \
  -drive "file=$image,format=raw,if=virtio,readonly=on" \
  -drive "file=$bad,format=raw,if=virtio,readonly=on" \
  -drive "file=$truncated,format=raw,if=virtio,readonly=on" \
  -drive "file=$oversized,format=raw,if=virtio,readonly=on" \
  -serial "file:$out/console.log" -monitor none

grep -F 'CIX_VM_PASS mount-read-metadata-fragment-empty' "$out/console.log"
grep -F 'CIX_VM_PASS fuse-read-xattr' "$out/console.log"
grep -F 'CIX_VM_PASS reject-corrupt-oversize-truncated-profile-cap' "$out/console.log"
grep -F 'CIX_VM_REJECT malformed-version' "$out/console.log"
grep -F 'CIX_VM_REJECT truncated-profile-payload' "$out/console.log"
grep -F 'CIX_VM_REJECT declared-profile-cap' "$out/console.log"
! grep -F 'CIX_VM_FAIL' "$out/console.log"
grep -F 'cix_squashfs' "$out/console.log"
cp "$receipt" "$out/fixture-receipt.json"
