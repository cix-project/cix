#!/usr/bin/env sh
# Prepared, not automatically executed, 7-Zip external-codec closure check.
set -eu

usage() {
  echo "usage: $0 ABS_7Z ABS_7Z_SO ABS_CIX_MODULE ABS_CIX_NATIVE ABS_WORKDIR ABS_SHA256SUM" >&2
  exit 64
}
[ "$#" -eq 6 ] || usage

host=$1
host_library=$2
module=$3
native=$4
work=$5
sha256sum_tool=$6
for path in "$host" "$host_library" "$module" "$native" "$sha256sum_tool"; do
  case "$path" in /*) ;; *) usage ;; esac
  [ -f "$path" ] || { echo "missing selected file: $path" >&2; exit 66; }
done
case "$work" in /*) ;; *) usage ;; esac
[ ! -e "$work" ] || { echo "work directory already exists: $work" >&2; exit 73; }

copy_host() {
  destination=$1
  with_codecs=$2
  install -d "$destination"
  install -m 0755 "$host" "$destination/7z"
  install -m 0755 "$host_library" "$destination/7z.so"
  if [ "$with_codecs" = yes ]; then
    install -d "$destination/Codecs"
    install -m 0755 "$module" "$destination/Codecs/$(basename "$module")"
    install -m 0755 "$native" "$destination/Codecs/$(basename "$native")"
  fi
}

hash_file() {
  digest=$($sha256sum_tool "$1")
  printf '%s\n' "${digest%% *}"
}

run_clean() {
  consumer_home=$1
  shift
  # Deliberately omit LD_LIBRARY_PATH, DYLD_LIBRARY_PATH and LD_PRELOAD.
  env -i PATH=/usr/bin:/bin HOME="$consumer_home" "$@"
}

install -d "$work"
encoder="$work/encoder"
decoder="$work/decoder"
no_codecs="$work/no-codecs"
input="$work/input.bin"
archive="$work/cix-external.7z"
restored="$work/restored"
copy_host "$encoder" yes
copy_host "$decoder" yes
copy_host "$no_codecs" no
install -d "$encoder/home" "$decoder/home" "$no_codecs/home" "$restored"
printf 'CIX 7-Zip external-codec closure fixture\n0123456789abcdef\n' > "$input"

run_clean "$encoder/home" "$encoder/7z" i > "$work/discovery.txt"
grep -F "CIX-EXPERIMENTAL-v1" "$work/discovery.txt" >/dev/null
run_clean "$encoder/home" "$encoder/7z" a -t7z -m0=CIX-EXPERIMENTAL-v1 "$archive" "$input"
run_clean "$decoder/home" "$decoder/7z" x -y -o"$restored" "$archive"

input_hash=$(hash_file "$input")
restored_hash=$(hash_file "$restored/$(basename "$input")")
[ "$input_hash" = "$restored_hash" ] || {
  echo "fresh external-codec restoration hash mismatch" >&2; exit 1; }

if run_clean "$no_codecs/home" "$no_codecs/7z" x -y -o"$work/no-codecs-output" "$archive"; then
  echo "negative control unexpectedly decoded without the CIX Codecs module" >&2
  exit 1
fi
printf '%s\n' "external-codec closure passed: $input_hash"
