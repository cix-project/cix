module example.invalid/cix/network/grpc

go 1.21

toolchain go1.22.2

require (
	example.invalid/cix/native v0.0.0
	example.invalid/cix/network v0.0.0
	google.golang.org/grpc v1.67.1
	google.golang.org/protobuf v1.34.2
)

require (
	golang.org/x/net v0.28.0 // indirect
	golang.org/x/sys v0.24.0 // indirect
	golang.org/x/text v0.17.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20240814211410-ddb44dafa142 // indirect
)

replace example.invalid/cix/network => ..

// A replace in ../go.mod is not transitive. Keep the native SDK identity
// explicit for this independently tested gRPC module.
replace example.invalid/cix/native => ../../../bindings/go
