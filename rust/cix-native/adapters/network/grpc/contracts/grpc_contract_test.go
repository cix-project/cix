package contracts

import (
	"bytes"
	"context"
	"net"
	"os"
	"testing"

	cix "example.invalid/cix/native"
	network "example.invalid/cix/network"
	pb "example.invalid/cix/network/grpc/gen/cix/network/v1"
	"example.invalid/cix/network/grpc/server"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/status"
	"google.golang.org/grpc/test/bufconn"
)

const (
	fixtureInputLimit  uint64 = 1 << 20
	fixtureOutputLimit uint64 = 4 << 20
	// Brotli's installed standard-format implementation conservatively admits
	// a 512 MiB workspace. This is a qualification budget, not a default.
	fixtureMemoryLimit uint64 = 768 << 20
)

type standardCoding struct {
	name   string
	enum   pb.StandardEncoding
	format network.Format
}

var standardCodings = []standardCoding{
	{"gzip", pb.StandardEncoding_STANDARD_ENCODING_GZIP, network.Gzip},
	{"br", pb.StandardEncoding_STANDARD_ENCODING_BROTLI, network.Brotli},
	{"zstd", pb.StandardEncoding_STANDARD_ENCODING_ZSTD, network.Zstd},
}

func TestUnaryContract(t *testing.T) {
	if os.Getenv("CIX_NETWORK_HOST") != "1" {
		t.Skip("scheduler-only native gRPC contract")
	}
	client, closeClient := newClient(t)
	defer closeClient()

	capabilities, err := client.GetCapabilities(context.Background(), &pb.CapabilitiesRequest{ProtocolVersion: 1})
	if err != nil || capabilities.GetMaxInputBytes() != fixtureInputLimit ||
		capabilities.GetMaxOutputBytes() != fixtureOutputLimit ||
		len(capabilities.GetSupportedStandardEncodings()) != len(standardCodings) {
		t.Fatalf("capabilities=%v err=%v", capabilities, err)
	}
	source := []byte("bounded CIX unary grpc contract")
	for _, requestCoding := range standardCodings {
		for _, responseCoding := range standardCodings {
			t.Run(requestCoding.name+"-to-"+responseCoding.name, func(t *testing.T) {
				testCodingPair(t, client, source, requestCoding, responseCoding)
			})
		}
	}

	testRejections(t, client, source)
}

func newClient(t *testing.T) (pb.CixTransformServiceClient, func()) {
	t.Helper()
	options, err := cix.DefaultOptions()
	if err != nil {
		t.Fatal(err)
	}
	options.OutputLimit, options.MemoryLimit = fixtureOutputLimit, fixtureMemoryLimit
	config := server.Config{Options: options, MaxInput: fixtureInputLimit, MaxOutput: fixtureOutputLimit}
	implementation, err := server.New(config)
	if err != nil {
		t.Fatal(err)
	}
	maxReceive, maxSend, err := server.TransportMessageLimits(config)
	if err != nil {
		t.Fatal(err)
	}
	listener := bufconn.Listen(1 << 20)
	grpcServer := grpc.NewServer(grpc.MaxRecvMsgSize(maxReceive), grpc.MaxSendMsgSize(maxSend))
	pb.RegisterCixTransformServiceServer(grpcServer, implementation)
	go func() { _ = grpcServer.Serve(listener) }()
	contextDialer := func(context.Context, string) (net.Conn, error) { return listener.Dial() }
	connection, err := grpc.DialContext(context.Background(), "bufnet", grpc.WithContextDialer(contextDialer), grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		t.Fatal(err)
	}
	return pb.NewCixTransformServiceClient(connection), func() {
		_ = connection.Close()
		grpcServer.Stop()
	}
}

func testCodingPair(t *testing.T, client pb.CixTransformServiceClient, source []byte, requestCoding, responseCoding standardCoding) {
	t.Helper()
	wireSource := standardEncode(t, requestCoding.format, source)
	encoded, err := client.Encode(context.Background(), &pb.TransformRequest{
		ProtocolVersion: 1, Payload: wireSource, InputEncoding: requestCoding.enum,
		ResponseEncoding: responseCoding.enum,
	})
	if err != nil || encoded.GetContentEncoding() != responseCoding.enum {
		t.Fatalf("encode response=%v err=%v", encoded, err)
	}
	archive := standardDecode(t, responseCoding.format, encoded.GetPayload(), fixtureOutputLimit)
	wireArchive := standardEncode(t, requestCoding.format, archive)
	decoded, err := client.Decode(context.Background(), &pb.TransformRequest{
		ProtocolVersion: 1, Payload: wireArchive, InputEncoding: requestCoding.enum,
		ResponseEncoding: responseCoding.enum,
	})
	if err != nil {
		t.Fatal(err)
	}
	restored := standardDecode(t, responseCoding.format, decoded.GetPayload(), fixtureOutputLimit)
	if !bytes.Equal(restored, source) {
		t.Fatalf("restored=%q", restored)
	}
}

func standardEncode(t *testing.T, format network.Format, input []byte) []byte {
	t.Helper()
	output, err := network.EncodeFormat(format, input, fixtureOutputLimit, fixtureMemoryLimit)
	if err != nil {
		t.Fatal(err)
	}
	return output
}

func standardDecode(t *testing.T, format network.Format, input []byte, outputLimit uint64) []byte {
	t.Helper()
	output, err := network.DecodeFormat(format, input, outputLimit, fixtureMemoryLimit)
	if err != nil {
		t.Fatal(err)
	}
	return output
}

func testRejections(t *testing.T, client pb.CixTransformServiceClient, source []byte) {
	t.Helper()
	_, err := client.GetCapabilities(context.Background(), &pb.CapabilitiesRequest{ProtocolVersion: 2})
	if status.Code(err) != codes.FailedPrecondition {
		t.Fatalf("version status=%v", status.Code(err))
	}
	_, err = client.Encode(context.Background(), &pb.TransformRequest{ProtocolVersion: 2, Payload: source})
	if status.Code(err) != codes.FailedPrecondition {
		t.Fatalf("transform version status=%v", status.Code(err))
	}
	_, err = client.Encode(context.Background(), &pb.TransformRequest{ProtocolVersion: 1, Payload: source, InputEncoding: pb.StandardEncoding(99)})
	if status.Code(err) != codes.InvalidArgument {
		t.Fatalf("unknown input encoding status=%v", status.Code(err))
	}
	_, err = client.Encode(context.Background(), &pb.TransformRequest{ProtocolVersion: 1, Payload: source, ResponseEncoding: pb.StandardEncoding(99)})
	if status.Code(err) != codes.InvalidArgument {
		t.Fatalf("unknown response encoding status=%v", status.Code(err))
	}
	_, err = client.Encode(context.Background(), &pb.TransformRequest{ProtocolVersion: 1, Payload: source, RequestedOutputLimit: fixtureOutputLimit + 1})
	if status.Code(err) != codes.InvalidArgument {
		t.Fatalf("over-server output cap status=%v", status.Code(err))
	}
	_, err = client.Encode(context.Background(), &pb.TransformRequest{ProtocolVersion: 1, Payload: source, RequestedOutputLimit: 1})
	if status.Code(err) != codes.ResourceExhausted {
		t.Fatalf("native output cap status=%v", status.Code(err))
	}
	_, err = client.Decode(context.Background(), &pb.TransformRequest{ProtocolVersion: 1, Payload: []byte("not a CIX archive")})
	if status.Code(err) != codes.DataLoss {
		t.Fatalf("malformed status=%v", status.Code(err))
	}
	cancelled, cancel := context.WithCancel(context.Background())
	cancel()
	_, err = client.Encode(cancelled, &pb.TransformRequest{ProtocolVersion: 1, Payload: source})
	if status.Code(err) != codes.Canceled {
		t.Fatalf("cancellation status=%v", status.Code(err))
	}
	_, err = client.Encode(context.Background(), &pb.TransformRequest{ProtocolVersion: 1, Payload: bytes.Repeat([]byte{'x'}, int(fixtureInputLimit+1))})
	if status.Code(err) != codes.ResourceExhausted {
		t.Fatalf("service input cap status=%v", status.Code(err))
	}
	_, err = client.Encode(context.Background(), &pb.TransformRequest{ProtocolVersion: 1, Payload: bytes.Repeat([]byte{'x'}, int(fixtureInputLimit+1025))})
	if status.Code(err) != codes.ResourceExhausted {
		t.Fatalf("transport input cap status=%v", status.Code(err))
	}
}
