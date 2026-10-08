# Native compatibility vectors

These small synthetic vectors exercise fixed-field graph residuals and object
address relations. They contain no evaluation-dataset bytes. The graph vector
contains one canonical atom record plus literal unsupported records; pointer
vectors contain a constructed object with two relocated fields.

Six historical frames cover graph prediction modes 0–2 and representations 0–1.
Six pointer frames cover delta modes 0–5. Nine palette frames cover three
prediction modes and palette sizes 1, 2, and 64, including grouped tokens and
escaped values. Tests compare complete encoded bytes and independently restore
the checked-in frames. File names and SHA-256 values in the surrounding fixture
manifests are the public compatibility identity; no generator is required to
build or test CIX.
