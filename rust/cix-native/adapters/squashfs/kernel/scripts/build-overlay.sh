#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# Build an out-of-tree, separately named reader from an unchanged pinned tree.
set -eu

src=${CIX_LINUX_SRC:?set CIX_LINUX_SRC to the official matching Linux source}
build=${CIX_KERNEL_BUILD:?set CIX_KERNEL_BUILD to its configured build tree}
overlay=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
receipt=${CIX_SQUASHFS_SOURCE_RECEIPT:-"$overlay/upstream-linux-6.8.0-142.142-squashfs-manifest.json"}
stage=${CIX_SQUASHFS_STAGE:?set CIX_SQUASHFS_STAGE outside the source tree}

test -f "$src/fs/squashfs/super.c"
test -f "$src/fs/squashfs/decompressor.c"
test -f "$build/include/config/kernel.release"
test "$(cat "$build/include/config/kernel.release")" = "${CIX_EXPECTED_KERNEL_RELEASE:-6.8.0-142-generic}"
python3 - "$src" "$receipt" <<'PY'
import hashlib, json, pathlib, sys
source, receipt = map(pathlib.Path, sys.argv[1:])
document = json.loads(receipt.read_text(encoding="utf-8"))
files = document.get("files")
if not isinstance(files, dict) or not files:
    raise SystemExit("invalid source manifest: files must be a non-empty object")
for name, expected in files.items():
    if not isinstance(name, str):
        raise SystemExit("invalid source manifest entry name")
    relative = pathlib.PurePosixPath(name)
    if (relative.is_absolute() or ".." in relative.parts or
            relative.parts[:2] != ("fs", "squashfs") or
            not isinstance(expected, str) or len(expected) != 64 or
            any(char not in "0123456789abcdef" for char in expected)):
        raise SystemExit(f"invalid source manifest entry: {name!r}")
    path = source.joinpath(*relative.parts)
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(f"source digest mismatch: {path}")
PY
test ! -e "$stage"

mkdir -p "$stage/upstream"
# Preserve every receipt-verified reader source, including deliberately unused
# stock codec tables; only CIX-owned wrappers select the linked reader TUs.
for f in "$src"/fs/squashfs/*.c "$src"/fs/squashfs/*.h; do
  test -f "$f"
  ln -s "$f" "$stage/upstream/$(basename "$f")"
done
mkdir -p "$stage/profile/src" "$stage/profile/include"
cp "$overlay"/../profile/src/cix_squashfs_profile.c "$stage/profile/src/"
cp "$overlay"/../profile/include/cix_squashfs_profile.h "$stage/profile/include/"
cp "$overlay"/cix_profile_portable.c "$overlay"/cix_squashfs_decompressor.c \
  "$overlay"/cix_squashfs_fs_register.c "$overlay"/Makefile "$stage/"
cp -R "$overlay/include" "$stage/"
for f in block cache dir export file fragment id inode namei super symlink page_actor \
  file_direct decompressor_single decompressor_multi decompressor_multi_percpu xattr xattr_id; do
  {
    cat <<EOF
/* Generated CIX-owned compilation bridge; upstream source stays unchanged. */
#include "include/cix_squashfs_prefix.h"
EOF
    if test "$f" = super; then
      printf '%s\n' '#undef pr_fmt'
    fi
    printf '%s\n' "#include \"upstream/$f.c\""
  } >"$stage/cix_upstream_$f.c"
done

cat >"$stage/README.generated" <<'EOF'
This directory has symlinks to an unmodified pinned upstream fs/squashfs tree.
The upstream reader files are compiled through CIX-owned prefix wrappers.  The
private compressor dispatch and filesystem-name registration are CIX-owned.
Do not substitute a stock squashfs.ko: it cannot dispatch compressor id 65001.
EOF
printf '%s\n' "staged unchanged reader inputs at $stage"
