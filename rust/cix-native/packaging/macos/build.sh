#!/usr/bin/env sh
set -eu

if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust Cargo is required." >&2
  exit 1
fi

cargo build --locked --release --lib --bin cix
echo "Build complete. Native macOS qualification remains required."
