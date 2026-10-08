# Bounded region-discovery fixtures

`make_fixtures.py` constructs only tiny synthetic members and records the
partition from `runtime.cix_runtime.regions` in `manifest.json`. The Rust unit
test embeds the binary fixtures and asserts those frozen spans.

The detector finds top-level, byte-aligned candidate spans and partitions the
remaining gaps. It does not recursively discover members inside accepted
spans, resolve arbitrary overlap nesting, or choose per-region codecs. Those
are planner responsibilities and are intentionally outside this primitive.
