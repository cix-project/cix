# SGCC1 / SGCC2 reference fixtures

The scheduled generator freezes all five SGCC1 predictor modes and all twelve
SGCC2 context/coding combinations. Its 1,100-record valid SAO grid has mixed
negative/positive coordinate magnitudes and modulo-16 values, a three-symbol
correction palette, and an 1,100-symbol constant-coordinate group. That group
forces the 1,024/76 type-class frame boundary for both SGCC2 packed codings.

`manifest.json` records each Python transform's statistics, byte size and
SHA-256, plus the constructed-input branch-coverage facts. Native tests read
every generated fixture directly and fail when one is absent.
