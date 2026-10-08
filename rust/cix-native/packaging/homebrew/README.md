# Homebrew formula template

`Cix.rb.in` is a source-build template, not a published tap formula. Replace
all three `RELEASE_*` placeholders only after a signed source archive and its
release identity exist. The formula builds one native `cix` executable, creates
`uncix` and `cixcat` aliases, and installs the native SDK shared/static
libraries and metadata.

It makes no full-engine bridge, host-adapter, notarization, universal-binary,
or macOS qualification claim. Those need selected provider inputs and separate
Apple Silicon and Intel receipts before publication.
