#!/bin/sh
# Focused RC27 host qualification; never runs automatically.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
tar_bin=${TAR:-tar}
work=$(mktemp -d "${TMPDIR:-/tmp}/cix-tar-qualification.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
: "${CIX_TAR_CIX:?set CIX_TAR_CIX to the absolute installed cix binary}"
export CIX_TAR_CIX
receipt=${CIX_TAR_RECEIPT_DIR:-"$PWD/cix-tar-qualification-receipt-$$"}
case "$receipt" in
  /*) ;;
  *) receipt="$PWD/$receipt" ;;
esac
if ! mkdir "$receipt"; then
  printf '%s\n' "receipt directory must be new: $receipt" >&2
  exit 73
fi
filter="$here/cix-tar-filter.sh"
shell_quote() { printf "'%s'" "$(printf %s "$1" | sed "s/'/'\\\\''/g")"; }
filter_command=$(shell_quote "$filter")
manifest="$receipt/manifest.txt"
commands="$receipt/commands.txt"
: > "$commands"
printf '%s\n' "tar -C INPUT -I $filter_command -cf archive.tar.cix ." >> "$commands"
printf '%s\n' "tar -C RESTORED -I $filter_command -xf archive.tar.cix" >> "$commands"

mkdir -p "$work/input/nested" "$work/restored"
printf 'first member\n' > "$work/input/first.txt"
printf 'second member\n' > "$work/input/nested/second name.txt"
printf '%s\n' "GNU tar: $($tar_bin --version | head -n 1)"
printf '%s\n' "CIX: $CIX_TAR_CIX"
{
  printf '%s\n' "GNU tar: $($tar_bin --version | head -n 1)"
  printf '%s\n' "CIX binary: $CIX_TAR_CIX"
  printf '%s\n' "CIX sha256: $(sha256sum "$CIX_TAR_CIX" | awk '{print $1}')"
  printf '%s\n' "wrapper sha256: $(sha256sum "$filter" | awk '{print $1}')"
  printf '%s\n' "memory: ${CIX_TAR_MEMORY:-512MiB}"
} > "$manifest"
"$tar_bin" -C "$work/input" -I "$filter_command" -cf "$work/archive.tar.cix" .
"$tar_bin" -C "$work/restored" -I "$filter_command" -xf "$work/archive.tar.cix"
cmp "$work/input/first.txt" "$work/restored/first.txt"
cmp "$work/input/nested/second name.txt" "$work/restored/nested/second name.txt"

"$tar_bin" -C "$work/input" -I "$filter_command" -cf "$work/empty.tar.cix" --files-from /dev/null
"$tar_bin" -I "$filter_command" -tf "$work/empty.tar.cix" > "$work/empty.list"
test ! -s "$work/empty.list"
# Keep each pipeline stage separate so POSIX sh propagates every producer error.
"$tar_bin" -C "$work/input" -cf "$work/pipeline.tar" .
"$filter" < "$work/pipeline.tar" > "$work/pipeline.tar.cix"
"$filter" -d < "$work/pipeline.tar.cix" > "$work/pipeline.restored.tar"
mkdir "$work/pipeline-restored"
"$tar_bin" -C "$work/pipeline-restored" -xf "$work/pipeline.restored.tar"
cmp "$work/input/first.txt" "$work/pipeline-restored/first.txt"
cmp "$work/input/nested/second name.txt" "$work/pipeline-restored/nested/second name.txt"

dd if="$work/archive.tar.cix" of="$work/truncated.tar.cix" bs=1 count=16 status=none
if "$tar_bin" -I "$filter_command" -tf "$work/truncated.tar.cix" \
    >"$work/truncated.stdout" 2>"$work/truncated.stderr"; then
  printf '%s\n' 'expected truncated CIX archive to fail' >&2
  exit 1
fi
head -c 4096 "$work/truncated.stderr" >&2 || true
for artifact in archive.tar.cix empty.tar.cix pipeline.tar.cix; do
  size=$(wc -c < "$work/$artifact")
  test "$size" -le 16777216
  cp "$work/$artifact" "$receipt/$artifact"
  printf '%s bytes sha256 %s\n' "$artifact" "$size" \
    "$(sha256sum "$work/$artifact" | awk '{print $1}')" >> "$manifest"
done
head -c 4096 "$work/truncated.stderr" > "$receipt/truncated.stderr" || true
printf '%s\n' 'tar program-filter qualification passed; libarchive consumer remains separate' \
  | tee -a "$manifest"
