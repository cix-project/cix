#!/usr/bin/env sh
set -eu

expected_revision=713eeca85cccadde39bf57f8b59ea663a085a347
: "${SQUASH_SOURCE_REVISION:?set to the provisioned QuixDB Squash source commit}"
: "${CIX_NATIVE_LIBRARY:?set to an absolute selected prebuilt libcix_native.so}"
: "${CIX_INCLUDE_DIR:?set to the matching installed/public CIX include directory}"
[ "$SQUASH_SOURCE_REVISION" = "$expected_revision" ] || {
  echo "Squash source revision must be $expected_revision" >&2; exit 1; }
case "$CIX_NATIVE_LIBRARY" in /*) ;; *) echo "CIX_NATIVE_LIBRARY must be absolute" >&2; exit 1;; esac
case "$CIX_INCLUDE_DIR" in /*) ;; *) echo "CIX_INCLUDE_DIR must be absolute" >&2; exit 1;; esac
[ -f "$CIX_NATIVE_LIBRARY" ] && [ -f "$CIX_INCLUDE_DIR/cix.h" ] && [ -f "$CIX_INCLUDE_DIR/cix_stream.h" ] || {
  echo "selected CIX library/cix.h/cix_stream.h are unavailable" >&2; exit 1; }

package_name=${SQUASH_PKG_CONFIG_NAME:-squash-0.8}
api_version=${SQUASH_API_VERSION:-0.8}
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
pkg-config --exists "$package_name" || {
  echo "Pinned QuixDB Squash SDK pkg-config package is unavailable: $package_name" >&2; exit 1; }
squash_version=$(pkg-config --modversion "$package_name")
case "$squash_version" in 0.8*) ;; *)
  echo "QuixDB Squash SDK must report 0.8.x, got $squash_version" >&2; exit 1;; esac

plugin_root=${SQUASH_PLUGIN_ROOT:-}
if [ -z "$plugin_root" ]; then
  plugin_root=$(pkg-config --variable=libdir "$package_name")/squash/$api_version/plugins
fi
plugin_dir=$plugin_root/cix
install -d "$plugin_dir"
install -m 0755 "$CIX_NATIVE_LIBRARY" "$plugin_dir/libcix_native.so"
install -m 0644 "$repo_root/packaging/squash/squash.ini" "$plugin_dir/squash.ini"
# pkg-config output is the selected SDK compiler/linker contract.
# shellcheck disable=SC2046
cc -std=c11 -fPIC -shared "$repo_root/packaging/squash/squash-cix.c" \
  -I"$CIX_INCLUDE_DIR" $(pkg-config --cflags "$package_name") \
  -L"$plugin_dir" -lcix_native $(pkg-config --libs "$package_name") \
  -Wl,-rpath,'$ORIGIN' -o "$plugin_dir/libsquash$api_version-plugin-cix.so"
