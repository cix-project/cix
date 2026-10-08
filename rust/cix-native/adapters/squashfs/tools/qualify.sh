#!/bin/sh
# Thin shell front for the deterministic Python fixture contract.
set -eu
: "${CIX_MKSQUASHFS:?path to cix-mksquashfs required}"
: "${CIX_UNSQUASHFS:?path to cix-unsquashfs required}"
: "${CIX_SQUASHFS_WORKDIR:?explicit retained work directory required}"
exec python3 "$(dirname "$0")/qualify.py"
