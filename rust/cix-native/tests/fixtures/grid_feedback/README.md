# Historical SGFB1 feedback fixtures

`make_fixtures.py` is the retained Python-reference generator for the six
valid policy combinations: correction mode `1`, `2`, and `3`, each with causal
feedback model `0` and `1`.  It writes raw `SGFB1` streams and a manifest with
their SHA-256 values and complete expected hex identities.

The generator and outputs are intentionally separate.  Run it only as the
scheduled focused fixture qualification, then add the generated raw files and
enable byte-identity tests for `full_engine::grid_context`.
