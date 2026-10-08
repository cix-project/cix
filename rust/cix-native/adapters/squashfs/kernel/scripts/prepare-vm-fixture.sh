#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# Build the disposable 305-file CIX image and precise corruption variants.
set -eu

mksquashfs=${CIX_MKSQUASHFS:?matching cix-mksquashfs}
source=${CIX_FIXTURE_SOURCE:?existing userspace fixture directory}
out=${CIX_VM_FIXTURE_OUTPUT:?new output directory}
offset=${CIX_PROFILE_OFFSET:-auto}
outer_offset=${CIX_OUTER_LENGTH_OFFSET:-auto}
outer_bytes=${CIX_OUTER_LENGTH_BYTES:-2}
limit=${CIX_PROFILE_PLAIN_LIMIT:-8192}
test ! -e "$out"
mkdir -p "$out/root"
cp -a "$source"/. "$out/root/"
: >"$out/root/empty"
"$mksquashfs" "$out/root" "$out/image.sqfs" -noappend -comp cix -b 131072 -processors 4

python3 - "$out/image.sqfs" "$out/malformed-version.sqfs" \
  "$out/truncated.sqfs" "$out/oversized.sqfs" "$offset" "$outer_offset" \
  "$outer_bytes" "$limit" <<'PY'
import shutil, sys
image, version, truncated, oversized, offset, outer, width, limit = sys.argv[1:]
raw = bytearray(open(image, 'rb').read())
if offset == outer == 'auto':
    if len(raw) < 96 or raw[:4] != b'hsqs':
        raise SystemExit('invalid generated SquashFS superblock')
    outer = int.from_bytes(raw[64:72], 'little')
    offset = outer + 2
offset, outer, width, limit = map(int, (offset, outer, width, limit))
if offset < 0 or outer < 0 or width not in (2, 3) or offset + 10 > len(raw):
    raise SystemExit('invalid bounded mutation offsets')
payload = int.from_bytes(raw[offset + 6:offset + 10], 'little')
outer_value = int.from_bytes(raw[outer:outer + width], 'little')
compressed_bit = 1 << (15 if width == 2 else 24)
if payload == 0 or outer_value & compressed_bit or outer_value < 10 + payload:
    raise SystemExit('offset does not identify a compressed CIX profile block')
for path, mutate in ((version, 'version'), (truncated, 'truncated'), (oversized, 'oversized')):
    data = bytearray(raw)
    if mutate == 'version':
        data[offset] = 2
    elif mutate == 'truncated':
        # Keep declared profile payload but shorten the enclosing block by one.
        data[outer:outer + width] = (outer_value - 1).to_bytes(width, 'little')
    else:
        data[offset + 2:offset + 6] = (limit + 1).to_bytes(4, 'little')
    open(path, 'wb').write(data)
PY
python3 - "$out/root" "$out/receipt.json" "$out/fixture.sha256" <<'PY'
import hashlib, json, pathlib, sys
root, receipt, manifest = map(pathlib.Path, sys.argv[1:])
entries = {}
for path in sorted(p for p in root.rglob('*') if p.is_file()):
    rel = path.relative_to(root).as_posix()
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    entries[rel] = {'sha256': digest, 'size': path.stat().st_size}
pathlib.Path(receipt).write_text(json.dumps({'entries': entries}, indent=2) + '\n')
pathlib.Path(manifest).write_text(''.join(f"{v['sha256']}  {k}\n" for k, v in entries.items()))
PY
