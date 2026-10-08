# CIX

CIX is a native lossless command-line compressor and SDK. This source layout
contains two Rust crates:

- `rust/cix-native`: the native `cix` executable, its `uncix` and `cixcat`
  aliases, process-free SDK, package recipes, native bridges, and optional host
  adapters.
- `rust/cix-portable`: a bounded pure-Rust CIXG1 subset for portable/WASM
  consumers. It shares selected native core modules by workspace-relative path;
  it is not a substitute for the full native engine.

Build the native executable and SDK from the workspace root:

```sh
# A release export records this workspace lockfile after an offline resolution.
cargo generate-lockfile --offline
make PROFILE=release-lto build
make PREFIX=/usr DESTDIR=/absolute/stage install
```

The installed Unix layout contains one `cix` executable and `uncix`/`cixcat`
symlinks, manual pages, notices, headers, shared/static SDK libraries, CMake
metadata, and pkg-config metadata. `make portable` builds the portable crate
for the host; WASM targets require the corresponding Rust target to have been
installed separately.

The base package does not imply full-engine bridge providers or optional host
adapter compatibility. `rust/cix-native/packaging/runtime/` stages selected
full-engine providers only with an explicit qualified dependency closure.
`rust/cix-native/packaging/` contains Debian, RPM, macOS, Windows, and
Homebrew source-package recipes. See `rust/cix-native/README.md` for CLI,
resource, platform, security, and licence details.

This is a source candidate. It must not be published until dependency closure,
platform test, package, provenance, and other documented technical release
gates are accepted.


## Existing public repository governance

# CIX

This local checkout contains an allowlisted CIX source candidate alongside the
existing contribution workflow and security controls. It is not an accepted
release or benchmark result. Publication and activation remain subject to owner
approval and the documented release requirements. Existing remote routing and
safeguards remain unchanged.

- [Contribution workflow](CONTRIBUTING.md)
- [Security reporting](SECURITY.md)
- [Support](SUPPORT.md)
- [Community conduct](CODE_OF_CONDUCT.md)
- [Gitflow and releases](docs/gitflow.md)
- [Codex instructions](AGENTS.md)
- [Activation requirements](docs/activation.md)

LICENSE, THIRD_PARTY_NOTICES.txt and THIRD_PARTY_NOTICES_RUST.md record the
included source terms and notices. Their presence does not establish acceptance
of all distribution licensing, provenance or dependency-closure obligations;
unresolved release requirements remain in force.
