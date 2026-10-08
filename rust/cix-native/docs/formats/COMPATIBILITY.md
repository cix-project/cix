# Container compatibility

CIX identifies generic and streaming archives by a five-byte ASCII magic.
The native command-line decoder recognizes the following containers:

| Magic | Role | Encoder status in 0.1 |
| --- | --- | --- |
| `CIXG1` | generic framed CIX stream | supported |
| `CIXG2` | versioned generic framed CIX stream | supported |
| `CIXM5` | legacy CIXM stream | decode compatibility |
| `CIXM6` | legacy CIXM stream revision | supported |
| `CIXF1` | validated filename wrapper around a CIXM stream | supported |
| `CIXB1` | bounded native-backend envelope | supported; see [FORMAT-CIXB1.md](FORMAT-CIXB1.md) |
| `CIXZ1` | bounded mixed streaming container | supported; see [FORMAT-CIXZ1.md](FORMAT-CIXZ1.md) |
| `CIXW1` | independent full-engine windows with terminal integrity | supported; see [FORMAT-CIXW1.md](FORMAT-CIXW1.md) |

The generic containers use independently validated frames and terminal
integrity data. CIXM6 retains its historical persistent-state and history
semantics; CIXF1 adds validated UTF-8 filename metadata to a CIXM stream.

The full engine also dispatches its versioned binary frame catalogue, including
specialist and heterogeneous-region frames. Those frames do not all use an
ASCII magic. The full-engine capability response describes that separate
surface; the process-free incremental SDK and portable WASM subset expose
their own, narrower capability lists. A container recognized by the command
line is not necessarily supported by every embedding adapter.

The magic alone does not establish that an older encoder’s full option set is
available in this release. Decode compatibility is maintained only for
container variants accepted by the 0.1 decoder. Unknown container revisions,
unknown route identifiers, trailing bytes, malformed lengths, and failed
integrity checks are rejected.

Format documents are protocol references, not performance claims. Consumers
should test archives with `uncix -t` when restoration is not required and use
the same release line for any newly created archive that must be decoded in a
controlled environment.
