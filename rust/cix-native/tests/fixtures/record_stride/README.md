# Record/stride parity fixtures

These small, synthetic byte vectors test record/stride framing and restoration.
They are checked in with `manifest.json`, which records every input and raw-frame
SHA-256 digest. No corpus data, Python generator, research document, or private
checkout is needed to build or test the native crate.

The vectors are compatibility evidence for the retained wire behavior only; they
are not compression-performance evidence.
