// Package server implements only generated unary RPC methods; it never registers a gRPC compression encoding.
package server

import (
	"context"
	"errors"
	"fmt"

	cix "example.invalid/cix/native"
	network "example.invalid/cix/network"
	pb "example.invalid/cix/network/grpc/gen/cix/network/v1"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

const protocolVersion = 1
const privateMediaType = "application/vnd.cix"
const protobufFramingAllowance uint64 = 1024

type Config struct {
	Options             cix.Options
	MaxInput, MaxOutput uint64
}
type Service struct {
	pb.UnimplementedCixTransformServiceServer
	config Config
}

func New(config Config) (*Service, error) {
	if config.MaxInput == 0 || config.MaxOutput == 0 ||
		config.Options.MemoryLimit < config.MaxInput ||
		config.MaxOutput > config.Options.MemoryLimit-config.MaxInput ||
		config.MaxInput > config.Options.MemoryLimit/3 ||
		config.MaxOutput > config.Options.MemoryLimit/3 {
		return nil, fmt.Errorf("invalid CIX gRPC limits")
	}
	// The public server cap must govern every subsequently created native context.
	config.Options.OutputLimit = config.MaxOutput
	return &Service{config: config}, nil
}

// TransportMessageLimits reserves a fixed protobuf/gRPC envelope allowance in
// addition to the application payload caps. Apply both values when creating a
// grpc.Server; server handlers run only after protobuf has allocated a message.
func TransportMessageLimits(config Config) (int, int, error) {
	if config.MaxInput == 0 || config.MaxOutput == 0 {
		return 0, 0, fmt.Errorf("invalid CIX gRPC transport limits")
	}
	maxInt := uint64(^uint(0) >> 1)
	if config.MaxInput > maxInt-protobufFramingAllowance ||
		config.MaxOutput > maxInt-protobufFramingAllowance {
		return 0, 0, fmt.Errorf("CIX gRPC transport limit exceeds Go int")
	}
	return int(config.MaxInput + protobufFramingAllowance),
		int(config.MaxOutput + protobufFramingAllowance), nil
}
func (s *Service) GetCapabilities(ctx context.Context, request *pb.CapabilitiesRequest) (*pb.CapabilitiesResponse, error) {
	if err := checkContextAndVersion(ctx, request.GetProtocolVersion()); err != nil {
		return nil, err
	}
	return &pb.CapabilitiesResponse{ProtocolVersion: protocolVersion, SupportedStandardEncodings: []pb.StandardEncoding{pb.StandardEncoding_STANDARD_ENCODING_GZIP, pb.StandardEncoding_STANDARD_ENCODING_BROTLI, pb.StandardEncoding_STANDARD_ENCODING_ZSTD}, MaxInputBytes: s.config.MaxInput, MaxOutputBytes: s.config.MaxOutput, PrivateCixEnvelope: true}, nil
}
func (s *Service) Encode(ctx context.Context, request *pb.TransformRequest) (*pb.TransformResponse, error) {
	return s.transform(ctx, request, true)
}
func (s *Service) Decode(ctx context.Context, request *pb.TransformRequest) (*pb.TransformResponse, error) {
	return s.transform(ctx, request, false)
}
func (s *Service) transform(ctx context.Context, request *pb.TransformRequest, encode bool) (*pb.TransformResponse, error) {
	if err := checkContextAndVersion(ctx, request.GetProtocolVersion()); err != nil {
		return nil, err
	}
	if uint64(len(request.GetPayload())) > s.config.MaxInput {
		return nil, status.Error(codes.ResourceExhausted, "payload exceeds input cap")
	}
	config := s.config
	if limit := request.GetRequestedOutputLimit(); limit != 0 {
		if limit > config.MaxOutput {
			return nil, status.Error(codes.InvalidArgument, "per-request output limit exceeds server cap")
		}
		config.MaxOutput = limit
		config.Options.OutputLimit = limit
	}
	input, compressedRequest, err := decodeStandard(request.GetInputEncoding(), request.GetPayload(), config)
	if err != nil {
		return nil, err
	}
	if err := ctx.Err(); err != nil {
		return nil, status.FromContextError(err).Err()
	}
	nativeOptions := contextOptions(config, compressedRequest)
	native, err := cix.NewContext(nativeOptions)
	if err != nil {
		return nil, status.Error(codes.Unavailable, "native context unavailable")
	}
	defer native.Close()
	if encode {
		input, err = native.Encode(input)
	} else {
		input, err = native.Decode(input)
	}
	if err != nil {
		return nil, nativeTransformError(err)
	}
	if uint64(len(input)) > config.MaxOutput {
		return nil, status.Error(codes.ResourceExhausted, "CIX result exceeds output cap")
	}
	output, err := encodeStandard(request.GetResponseEncoding(), input, config)
	if err != nil {
		return nil, err
	}
	if err := ctx.Err(); err != nil {
		return nil, status.FromContextError(err).Err()
	} // Native entry itself is synchronous and not preemptible.
	media := "application/octet-stream"
	if encode {
		media = privateMediaType
	}
	return &pb.TransformResponse{ProtocolVersion: protocolVersion, Payload: output, ContentEncoding: request.GetResponseEncoding(), EnvelopeMediaType: media}, nil
}

func nativeTransformError(err error) error {
	var nativeError *cix.Error
	if errors.As(err, &nativeError) && nativeError.Status == cix.StatusResourceLimit {
		return status.Error(codes.ResourceExhausted, "CIX result exceeds configured limits")
	}
	return status.Error(codes.DataLoss, "CIX payload rejected")
}

func contextOptions(config Config, compressedRequest bool) cix.Options {
	nativeOptions := config.Options
	// The generic C API owns a temporary result while the Go binding owns both
	// the retained request and destination buffer. Protobuf retains an encoded
	// request payload too, so reserve both it and the inflated input in that case.
	reserved := config.MaxInput + config.MaxOutput
	if compressedRequest {
		reserved += config.MaxInput
	}
	nativeOptions.MemoryLimit -= reserved
	return nativeOptions
}
func checkContextAndVersion(ctx context.Context, version uint32) error {
	if err := ctx.Err(); err != nil {
		return status.FromContextError(err).Err()
	}
	if version != protocolVersion {
		return status.Error(codes.FailedPrecondition, "unsupported CIX protocol version")
	}
	return nil
}
func format(encoding pb.StandardEncoding) (network.Format, bool) {
	switch encoding {
	case pb.StandardEncoding_STANDARD_ENCODING_GZIP:
		return network.Gzip, true
	case pb.StandardEncoding_STANDARD_ENCODING_BROTLI:
		return network.Brotli, true
	case pb.StandardEncoding_STANDARD_ENCODING_ZSTD:
		return network.Zstd, true
	}
	return 0, false
}
func decodeStandard(encoding pb.StandardEncoding, input []byte, config Config) ([]byte, bool, error) {
	if encoding == pb.StandardEncoding_STANDARD_ENCODING_IDENTITY {
		return input, false, nil
	}
	f, ok := format(encoding)
	if !ok {
		return nil, false, status.Error(codes.InvalidArgument, "unsupported input encoding")
	}
	output, err := network.DecodeFormat(f, input, config.MaxInput, config.Options.MemoryLimit)
	if err != nil {
		return nil, false, status.Error(codes.DataLoss, "encoded payload rejected")
	}
	return output, true, nil
}
func encodeStandard(encoding pb.StandardEncoding, input []byte, config Config) ([]byte, error) {
	if encoding == pb.StandardEncoding_STANDARD_ENCODING_IDENTITY {
		return input, nil
	}
	f, ok := format(encoding)
	if !ok {
		return nil, status.Error(codes.InvalidArgument, "unsupported response encoding")
	}
	output, err := network.EncodeFormat(f, input, config.MaxOutput, config.Options.MemoryLimit)
	if err != nil {
		return nil, status.Error(codes.Internal, "response encoding failed")
	}
	return output, nil
}
