module example.invalid/cix/network/grpc

go 1.25.0

require (
	example.invalid/cix/native v0.0.0
	example.invalid/cix/network v0.0.0
	google.golang.org/grpc v1.83.2
	google.golang.org/protobuf v1.36.11
)

require (
	golang.org/x/net v0.58.0 // indirect
	golang.org/x/sys v0.47.0 // indirect
	golang.org/x/text v0.41.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20260526163538-3dc84a4a5aaa // indirect
)

replace example.invalid/cix/network => ..

// A replace in ../go.mod is not transitive. Keep the native SDK identity
// explicit for this independently tested gRPC module.
replace example.invalid/cix/native => ../../../bindings/go
