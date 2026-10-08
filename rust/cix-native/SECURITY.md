# Security policy

## Supported release line

Only released 0.1.x source and binary artifacts with a published checksum and
test receipt are supported. This release-preparation candidate is not a
published release.

## Reporting a vulnerability

Until a public repository and security contact are established, do not file
public issue reports containing exploit details. Contact the release owner
through the private channel used to obtain this candidate. The public release
must replace this paragraph with a monitored security contact and disclosure
policy before publication.

## Archive handling

Treat untrusted archives as untrusted input. Prefer bounded-memory operation,
run `uncix -t` when restoration is not needed, and preserve normal
operating-system process limits in automated services. CIX’s checks validate
its archive framing and integrity data; they do not make arbitrary input safe
to process without ordinary resource controls.
