# Generation is deliberately explicit. Generated bindings are not handwritten or checked in.
PROTOC ?= protoc
PROTOC_VERSION := 27.3
PROTOC_GEN_GO_VERSION := v1.34.2
PROTOC_GEN_GO_GRPC_VERSION := 1.5.1

.PHONY: verify-tools generate contract
verify-tools:
	@$(PROTOC) --version | grep -Fx 'libprotoc $(PROTOC_VERSION)'
	@protoc-gen-go --version | grep -F '$(PROTOC_GEN_GO_VERSION)'
	@protoc-gen-go-grpc --version | grep -F '$(PROTOC_GEN_GO_GRPC_VERSION)'
generate: verify-tools
	@mkdir -p gen
	$(PROTOC) -I proto --go_out=gen --go_opt=paths=source_relative --go-grpc_out=gen --go-grpc_opt=paths=source_relative proto/cix/network/v1/cix_network.proto
contract: generate
	go test ./...
