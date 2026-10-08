# Historical SGBI1 interval fixtures

`make_fixtures.py` freezes the Python-reference `SGBI1` raw stream for each
encoder policy: fixed-width interval rank (`1`), mixed-radix rank (`2`), and
arithmetic interval coding (`3`).  It retains both a three-record case and a
300-record case, which crosses the historical 256-value integer-block boundary.
The manifest records full expected hex plus SHA-256 for every raw stream.

Run this generator only through the scheduled focused fixture job.  Its output
then supports byte-identity tests for `full_engine::interval_context`.
