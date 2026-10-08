# gRPC generation and host-contract prerequisites

This adapter is source only until these exact, independently provisioned tools
are present. Generated bindings and module checksums are intentionally not
invented or checked in without that toolchain.

| Requirement | Exact version | Publisher / authoritative URL | Purpose |
| --- | --- | --- | --- |
| Go | 1.25.0 or newer | [Go downloads](https://go.dev/dl/) | Builds the adapter and plugins. |
| Protocol Buffers compiler | 27.3 | [Protocol Buffers v27.3 release](https://github.com/protocolbuffers/protobuf/releases/tag/v27.3) | `protoc`; `make verify-tools` requires the reported `libprotoc 27.3`. |
| Go protobuf plugin | `google.golang.org/protobuf/cmd/protoc-gen-go` v1.34.2 | [official Go module record](https://pkg.go.dev/google.golang.org/protobuf/cmd/protoc-gen-go@v1.34.2) | Generates message bindings. |
| Go gRPC plugin | `google.golang.org/grpc/cmd/protoc-gen-go-grpc` v1.5.1 | [official Go module record](https://pkg.go.dev/google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.5.1) | Generates unary service bindings. |
| gRPC Go runtime | `google.golang.org/grpc` v1.83.2 | [grpc-go v1.83.2](https://github.com/grpc/grpc-go/releases/tag/v1.83.2) | Transport and bufconn host contract. |
| Go protobuf runtime | `google.golang.org/protobuf` v1.36.11 | [protobuf-go v1.36.11](https://github.com/protocolbuffers/protobuf-go/releases/tag/v1.36.11) | Generated message runtime. |
| Installed CIX SDK | matching staged `cix-native.pc` | local staged distribution artifact | Required for cgo and the real native unary contract. |

`grpc/go.mod` and `grpc/tools/go.mod` pin the module versions above. A host
with the approved Go module proxy/cache must fetch their transitive modules and
produce `grpc/go.sum` and `grpc/tools/go.sum`; review and retain those files as
the resolved checksum evidence. Their hashes must come from the Go module
download, never be copied into this repository manually. The generated bindings
also need review against the declared `go_package` before a host contract is
run.

After provisioning, put both plugin binaries on `PATH`, run `make generate`
from this directory, then set `PKG_CONFIG_PATH` to the matching staged SDK and
run `grpc/scripts/qualify-grpc.sh` with `CIX_NETWORK_HOST=1`. The host must use
`server.TransportMessageLimits` to set both `grpc.MaxRecvMsgSize` and
`grpc.MaxSendMsgSize`; those values include a fixed protobuf/gRPC framing
allowance beyond the application payload caps. Server-side checks occur after
protobuf has parsed a message. The qualification
is unary and process-local (`bufconn`); it does not claim TLS, authentication,
public listener operation, streaming, process RSS limits, or hard cancellation
of an in-flight native transform.
