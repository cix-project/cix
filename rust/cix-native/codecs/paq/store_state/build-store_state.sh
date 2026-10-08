#!/usr/bin/env sh
set -eu
: "${CIX_PAQ_SOURCE_ROOT:?set to the packaged vendor/paq/store_state source root}"
: "${CIX_PAQ_BUILD_DIR:?set to a disposable build directory}"
cmake -S "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)" -B "$CIX_PAQ_BUILD_DIR" \
  -DCIX_PAQ_SOURCE_ROOT="$CIX_PAQ_SOURCE_ROOT" \
  -DCMAKE_BUILD_TYPE=Release
cmake --build "$CIX_PAQ_BUILD_DIR" --parallel 4
