#!/bin/sh
# Compile and run the C consumer strictly against a staged install prefix.
# Usage: c_api_installed_consumer.sh /absolute/stage-prefix /absolute/work-dir
set -eu

prefix=${1:?stage prefix required}
work_dir=${2:?work directory required}
header="$prefix/include/cix.h"
library_dir="$prefix/lib"
consumer="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/c_api_consumer.c"

test -f "$header"
test -f "$library_dir/libcix_native.so"
mkdir -p "$work_dir"

cc -std=c11 -Wall -Wextra -Werror -pedantic \
  -I "$prefix/include" "$consumer" -L "$library_dir" -lcix_native \
  -Wl,-rpath,"$library_dir" -o "$work_dir/cix-c-api-consumer"
"$work_dir/cix-c-api-consumer"
