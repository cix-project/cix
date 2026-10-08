# Full-engine worker protocol

The Rust `full_engine::job_protocol` module supplies bounded control-record
readers/writers and caller-provided `Read`/`Write` execution interfaces. It is
separate from the process-free native C buffer/stream SDK.

One isolated job uses the installed CIX executable:

```text
cix --cix-job-v1 --input ABS --output ABS --private-library-dir ABS --temporary-root ABS
```

The argument grammar remains version 1. The current binary control wire is
version 2. Input and archive bytes use the explicit files; stdin/stdout carry
control records. An output path must not exist. The worker stages output in its
private root and publishes only a completed result without overwriting another
file. It cleans its own staging file after a rejected operation.

Use the public Rust record helpers rather than serializing Rust enum memory.
Version 2 supports whole-input selection, independent-window selection and
archive decode. Version 1 control records remain readable, including its
original operation numbers, capability flags and result layout. An older
client must negotiate support before expecting version 2 replies; reading old
requests does not make a version 1-only client understand new replies.

An independent-window request supplies `window_bytes`, a profile and explicit
input, archive, output, memory, intermediate, temporary, worker and deadline
limits. Its CIXW1 result reports total input/archive bytes and independent
window count. Each window performs local selection and starts fresh codec
state. Eligible local models remain available; cross-window history, regions
and whole-file competition are not preserved. Empty input still produces a
complete carrier. Decode checks the terminal and cumulative output limit.

Progress records may precede a terminal result or error. Consumers must read
through those records, enforce the declared maximum control-record length, and
treat malformed versions, flags or lengths as protocol errors. Cancel messages
and native deadlines are cooperative. A parent requiring hard termination of a
non-preemptible native codec must terminate its isolated worker process.

The worker does not search `PATH` for codec programs and does not require
Python. Provider libraries and private temporary directories retain their
normal ownership and admission checks.
