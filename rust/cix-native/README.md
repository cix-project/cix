# CIX

CIX (pronounced “six”) is a lossless command-line compressor for arbitrary
byte streams. It writes self-describing CIX archives and provides separate
commands to compress, restore, and stream restored bytes.

This repository is the source-only 0.1.0 release-preparation candidate. Its
publication remains blocked by the documented technical release gates,
including full-engine dependency closure and qualified platform/package evidence.

## Commands

| Command | Purpose |
| --- | --- |
| `cix` | Compress a file or standard input. |
| `uncix` | Restore a CIX archive, test it, or list it. |
| `cixcat` | Restore a CIX archive to standard output. |

```sh
cix report.dat
uncix report.dat.cix
cix --fast -c input.bin > input.cix
cixcat input.cix | tar -x
uncix -t input.cix
```

Input files are retained by default. Use `-f` to replace an existing output,
`-c` for standard output, `--fast` for lower compression latency, and
`--best` for a broader bounded search. Whole-file BEST can spend minutes even
on small inputs: eligible native whole-file work has a 15-minute allowance plus
up to 45 minutes based on input size. Direct external-library calls can add
time and are not preemptible, so this is not a total wall-time limit. Run
`cix --help`, `uncix --help`, or `cixcat --help` for the complete interface.

Archives are CIX streams, not ZIP, gzip, or tar files. CIX processes one file
per invocation; use tar or another archiver when preserving directory trees.

## Build from source

CIX requires Rust 1.98 or later, a C++17 compiler, and development libraries
for zlib, bzip2, liblzma, Zstandard, Brotli, LZ4, and Snappy. The locked dependency graph
is fetched from crates.io by Cargo unless your build environment supplies it
from an approved local mirror or vendor directory.

Debian or Ubuntu:

```sh
sudo apt install cargo rustc build-essential pkg-config zlib1g-dev libbz2-dev liblzma-dev libzstd-dev libbrotli-dev liblz4-dev libsnappy-dev
rustc --version   # must report 1.98 or later
make build
```

Fedora, RHEL-compatible systems, or derivatives:

```sh
sudo dnf install cargo rust gcc-c++ pkgconf-pkg-config zlib-devel bzip2-devel xz-devel libzstd-devel brotli-devel lz4-devel snappy-devel
rustc --version   # must report 1.98 or later
make build
```

Some distribution releases package an older Rust compiler. In that case, use
a compatible Rust 1.98-or-later toolchain before building; do not assume the
distribution package meets this requirement. The default build uses Cargo’s
release profile. The LTO profile is available when a distribution policy
permits a slower build:

```sh
make PROFILE=release-lto build
```

Install to the conventional local prefix:

```sh
make PREFIX=/usr/local install
# stage without changing the host
make PREFIX=/usr DESTDIR=/absolute/stage install
```

For a user installation without Make:

```sh
cargo install --path . --locked --root "$HOME/.local"
```

## Platform status

The release recipes cover source builds on Linux, macOS, and Windows. Linux
x86_64 is the only currently qualified host. macOS and Windows recipes are
provided for native qualification; no binaries for those systems are claimed
or supplied by this source candidate.

The normal CLI links system zlib, bzip2, liblzma, Zstandard, Brotli, LZ4, Snappy, and the
platform C++ runtime. Distribution packages should use their system libraries.
Do not present an arbitrary copied binary as a portable standalone build.

## Packaging

`packaging/debian/` and `packaging/rpm/` contain source-package recipes.
They install one `cix` executable with `uncix` and `cixcat` aliases, the three
manual pages, licence, and third-party notices. Their `-dev` packages also
install the process-free native SDK headers, shared/static libraries, CMake
metadata, pkg-config metadata, and a small embedding example. `packaging/macos/` and `packaging/windows/`
contain native source-build recipes; neither claims a qualified binary. The
full-engine runtime bridge closure is staged separately by `packaging/runtime/`;
it is not implied by the base package and needs its own selected-provider
receipt. Optional host adapters, including the Squash 0.8 plugin under
`packaging/squash/`, are source integrations with their declared host SDK and
separate qualification requirements.

A public source export contains only the files required to build, install, and
package CIX. It projects the maintained Debian recipe to the conventional
exported `debian/` directory for `dpkg-source`. Release publication and
full-engine dependency acceptance are separate release-management steps.

## Security and compatibility

Use `uncix -t` before consuming data from an untrusted archive when you
only need an integrity check. CIX validates its framing, length checks, and
checksums, but decompression can require the resource limits selected by the
archive mode. See [SECURITY.md](SECURITY.md) for the supported
reporting path.

Current format references cover [container compatibility](docs/formats/COMPATIBILITY.md),
[CIXB1](docs/formats/FORMAT-CIXB1.md), and
[CIXZ1](docs/formats/FORMAT-CIXZ1.md). The public release should only make
compatibility claims backed by the corresponding release test receipt.

## Licence and notices

CIX-authored user-space and library material is MIT-licensed. Third-party
material retains its own licence terms; see
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt) and
[THIRD_PARTY_NOTICES_RUST.md](THIRD_PARTY_NOTICES_RUST.md).

A complete full-engine distribution that includes the mandatory PAQ bridge
providers is conveyed under GPL-3.0-or-later and must retain the applicable
PAQ source, build, and notice materials. The separate experimental Linux
SquashFS kernel adapter is GPL-2.0-only; this user-space statement does not
change that module's licence. The full-engine source/build closure and its
provider notices remain unresolved release requirements, so this statement
makes no redistribution-readiness claim.
