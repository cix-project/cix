# Release procedure

1. Confirm the project licence and remove the release-preparation warning from
   `LICENSE` and `README.md`.
2. Build from a clean, tagged source revision with `cargo build --locked`.
3. Generate and review the dependency licence report and SBOM from that exact
   lockfile.
4. Complete the documented platform and package test matrix, retaining logs,
   checksums, dynamic dependency lists, and toolchain versions.
5. Create the source archive, Linux packages, and only those platform binaries
   that have a corresponding successful native test receipt.
6. Publish checksums, third-party notices, supported-platform scope, and a
   reproducible benchmark-method record tied to the artifact revision.
