# libbsc source provenance

This directory contains the minimal non-CUDA source closure of
[IlyaGrebnov/libbsc](https://github.com/IlyaGrebnov/libbsc), revision
`baffa62c70b6ebbecc9af14ce550e965ea247680` (libbsc 3.3.12), retrieved on
2026-10-02.

The upstream project is licensed under Apache License 2.0. `LICENSE` and
`AUTHORS` are retained here. `libbsc/bwt/libsais/LICENSE` is also retained for
the bundled SA-IS implementation.

The retained source closure is the one named by upstream `CMakeLists.txt` for
the CPU library, excluding CUDA-only `st.cu` and `libcubwt`. CIX deliberately
does not compile upstream OpenMP/CUDA support: CIX's bounded worker scheduler
controls parallelism and memory admission.

Upstream default encoder settings are BWT sorting, static QLFC coding, LZP
hash size 15, LZP minimum match length 72 and fast mode. Those defaults are
used only as an initial CIX candidate. They do not establish equivalence with
the historical Squash BSC plugin, whose exact libbsc revision and invocation
must be recorded before any comparison claim.
