#!/usr/bin/env python3
"""Deterministic, retained-artifact contract for experimental CIX tools."""
import errno
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess

MAGIC, CIX_ID, UNKNOWN_ID = 0x73717368, 65001, 65002
SUPERBLOCK, COMPRESSION_OFFSET, INODE_TABLE_START_OFFSET = 96, 20, 64
METADATA_UNCOMPRESSED = 0x8000

def require(name):
    value = os.environ.get(name)
    if not value: raise SystemExit(f"{name} is required")
    return value

def deterministic(n):
    output, counter = bytearray(), 0
    while len(output) < n:
        output.extend(hashlib.sha256(b"cix-squashfs-fixture-v1" + counter.to_bytes(8, "little")).digest()); counter += 1
    return bytes(output[:n])

def write_fixture(root):
    (root / "tiny").mkdir(parents=True)
    (root / "repeated").write_bytes(b"CIX SquashFS profile fixture\n" + b"\0" * 65536)
    (root / "incompressible").write_bytes(deterministic(32768))
    (root / "fragment").write_bytes((b"fragment-cix-v1\n" * 438)[:7000])
    with (root / "sparse").open("wb") as f: f.seek(131071); f.write(b"\0")
    for i in range(300): (root / "tiny" / str(i)).write_bytes(f"tiny-{i:03d}\n".encode())
    xattrs = {"repeated": {"user.cix.fixture": b"experimental"}, "tiny/0": {"user.cix.tiny": b"yes"}}
    if not (hasattr(os, "setxattr") and hasattr(os, "getxattr")): return {}
    try:
        for relative, attrs in xattrs.items():
            for name, value in attrs.items(): os.setxattr(root / relative, name, value)
    except OSError as exc:
        if exc.errno not in (errno.ENOTSUP, errno.EOPNOTSUPP): raise
        return {}
    return xattrs

def receipt_tree(root, xattrs):
    entries = {}
    for path in sorted(root.rglob("*")):
        if path.is_file():
            relative = path.relative_to(root).as_posix()
            entries[relative] = {"sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "size": path.stat().st_size}
    for relative, attrs in xattrs.items(): entries[relative]["xattrs"] = {name: os.getxattr(root / relative, name).hex() for name in attrs}
    return entries

def superblock(image):
    data = bytearray(image.read_bytes())
    if len(data) < SUPERBLOCK: raise AssertionError("truncated superblock")
    magic, = struct.unpack_from("<I", data, 0); compression, = struct.unpack_from("<H", data, COMPRESSION_OFFSET); inode_start, = struct.unpack_from("<Q", data, INODE_TABLE_START_OFFSET)
    if magic != MAGIC or compression != CIX_ID: raise AssertionError(f"unexpected superblock magic={magic:#x} compression={compression}")
    if inode_start + 2 > len(data): raise AssertionError("inode table starts outside image")
    return data, inode_start

def must_fail(tool, image, destination, processors):
    result = subprocess.run([tool, "-processors", processors, "-no-progress", "-d", str(destination), str(image)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode == 0: raise AssertionError(f"negative image unexpectedly extracted: {image}")

def main():
    mksquashfs, unsquashfs = require("CIX_MKSQUASHFS"), require("CIX_UNSQUASHFS")
    work, processors = Path(require("CIX_SQUASHFS_WORKDIR")).resolve(), os.environ.get("CIX_SQUASHFS_PROCESSORS", "1")
    if processors not in {"1", "2", "3", "4"}: raise SystemExit("CIX_SQUASHFS_PROCESSORS must be 1 through 4")
    if work.exists(): raise SystemExit(f"work directory already exists and is retained: {work}")
    root, out, image = work / "root", work / "out", work / "image.sqfs"; work.mkdir(parents=True)
    xattrs = write_fixture(root)
    expected = receipt_tree(root, xattrs)
    receipt_path = work / "receipt.json"
    receipt = {"fixture": "cix-squashfs-userspace-v1", "processors": int(processors), "xattrs_required": bool(xattrs), "entries": expected, "status": "fixture-created"}
    receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    subprocess.run([mksquashfs, str(root), str(image), "-noappend", "-comp", "cix", "-b", "131072", "-processors", processors, "-all-time", "0", "-mkfs-time", "0", "-no-progress"], check=True)
    receipt["image_sha256"] = hashlib.sha256(image.read_bytes()).hexdigest(); receipt["status"] = "image-created"; receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    original, inode_start = superblock(image)
    subprocess.run([unsquashfs, "-processors", processors, "-no-progress", "-d", str(out), str(image)], check=True)
    actual = receipt_tree(out, xattrs)
    if actual != expected: raise AssertionError("extracted data or required xattrs differ; retained manifest identifies expected state")
    unknown = bytearray(original); struct.pack_into("<H", unknown, COMPRESSION_OFFSET, UNKNOWN_ID); unknown_image = work / "unknown-method.sqfs"; unknown_image.write_bytes(unknown); must_fail(unsquashfs, unknown_image, work / "unknown-out", processors)
    malformed = bytearray(original); metadata_header, = struct.unpack_from("<H", malformed, inode_start)
    if metadata_header & METADATA_UNCOMPRESSED: raise AssertionError("fixture did not produce a compressed first inode metadata block")
    malformed[inode_start + 2] = 2; malformed_image = work / "malformed-inode-profile-version.sqfs"; malformed_image.write_bytes(malformed); must_fail(unsquashfs, malformed_image, work / "malformed-out", processors)
    receipt["status"] = "passed"; receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    print(f"CIX experimental SquashFS userspace fixture passed; retained receipt: {receipt_path}")

if __name__ == "__main__": main()
