# Optional CIX network adapters

This directory contains source for two **unary, bounded, controlled-peer**
adapters over the installed native SDK. It never launches the CIX CLI, selects a
native library from client data, or registers CIX as an HTTP `Content-Encoding`
or gRPC compression encoding.

## HTTP

`httpapi.NewHandler` exposes only `POST /v1/encode` and `POST /v1/decode`.
Requests and responses are complete buffers admitted against explicit input,
output and combined memory caps before the adapter allocates an output. The HTTP
configuration requires space for three request-side standard-format buffers
(wire body, native temporary and inflated CIX input) and three response-side
buffers (CIX source, native temporary and coded response). The native generic
CIX call reserves its retained request input and caller output buffer before
entering the native context. The native context receives the public
`MaxOutput` cap even when its supplied options were wider. Standard
`gzip`, `br`, and `zstd` request/response codings use `cix_formats.h` through a
small cgo bridge. `Accept-Encoding` q-values, explicit exclusions and wildcard
selection are handled before a response coding is chosen; stacked request
codings are rejected.

CIX itself is a private controlled-peer envelope, never an HTTP coding. Set
`AllowPrivatePeer`, `X-CIX-Peer-Version: 1`, and `Accept: application/vnd.cix`
to request `/v1/encode`; `/v1/decode` additionally requires
`Content-Type: application/vnd.cix`. `application/vnd.cix` is a provisional
private media type, not an IANA registration or an interoperability claim.

The handler checks cancellation before native entry and before response write.
The native C ABI has no cancellation operation, so an in-flight transform cannot
be preempted. It also makes no process-RSS guarantee. Applications own TLS,
authentication, listener lifecycle, rate limiting and audit policy; the host
contract uses `httptest` and does not bind a public listener.

## gRPC

`grpc/proto/.../cix_network.proto` declares unary capability, encode and decode
methods. The request declares version, payload, input encoding, response
encoding and optional output cap. The service implementation uses those fields
and the same bounded SDK calls, including the server's `MaxOutput` as the native
output cap and `MaxInput` as the inflated-request cap; it does **not** register a `grpc-encoding: cix`
or expose streaming RPCs. The gRPC transport must be created with both
`grpc.MaxRecvMsgSize(maxInput)` and `grpc.MaxSendMsgSize(maxOutput)` because a
service method cannot cap protobuf allocation that happened before dispatch.

Generated Go files are deliberately absent. Generate them only with libprotoc
27.3, `protoc-gen-go` v1.34.2 and `protoc-gen-go-grpc` v1.5.1, as pinned in
`grpc/Makefile` and `grpc/tools/go.mod`. `grpc/go.mod` pins grpc-go v1.67.1 and
protobuf v1.34.2. No server is started by this source change.

## Qualification prerequisites

A real host run needs an installed `cix-native.pc` SDK, a matching C compiler
and loader, Go 1.20+, pinned protobuf/grpc tools and the target gRPC runtime.
It must execute `contracts/qualify-http.sh` and `grpc/scripts/qualify-grpc.sh`
with explicit native limits and artifacts, then separately verify TLS/auth at
the deployment boundary. This is not a public service or registry release.
