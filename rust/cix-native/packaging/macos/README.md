# macOS native build

This is a source-build recipe, not a macOS binary release claim. Build and
test separately on Apple Silicon and Intel Macs before publishing either
architecture.

Install the Apple command-line tools, Rust 1.98 or later, and Homebrew
dependencies:

```sh
xcode-select --install
brew install rust bzip2 brotli xz zstd
make build
# SDK artifacts are built with the same command; stage them with
# make PREFIX=/usr/local install DESTDIR=/absolute/stage
```

If the linker cannot locate a Homebrew library, set the compiler and linker
search paths for the active Homebrew prefix before running Cargo. Capture the
exact environment and `otool -L` output in the platform test receipt.

The recipe builds one `cix` executable and the native SDK library; Unix aliases
are created by `make install`, not as independently built executables. No
universal binary, code signature, notarization, or runtime compatibility is
asserted by this recipe. Do not publish a macOS archive until it has passed the
native command-line, archive round-trip, malformed-input, and dynamic-library
checks.
