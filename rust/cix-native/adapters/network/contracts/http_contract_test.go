package contracts

import (
	"bytes"
	"compress/gzip"
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	cix "example.invalid/cix/native"
	network "example.invalid/cix/network"
	"example.invalid/cix/network/httpapi"
)

func TestControlledPeerHTTPContract(t *testing.T) {
	if os.Getenv("CIX_NETWORK_HOST") != "1" {
		t.Skip("scheduler-only native HTTP contract")
	}
	options, err := cix.DefaultOptions()
	if err != nil {
		t.Fatal(err)
	}
	options.OutputLimit, options.MemoryLimit = 4<<20, 768<<20
	handler, err := httpapi.NewHandler(httpapi.Config{Options: options, MaxInput: 1 << 20, MaxOutput: 4 << 20, AllowPrivatePeer: true})
	if err != nil {
		t.Fatal(err)
	}
	source := []byte("CIX HTTP controlled peer contract")
	if _, err := network.EncodeFormat(network.Zstd, source, 4<<20, 128<<20); err != nil {
		t.Fatalf("standard Zstd boundary: %v", err)
	}
	encode := httptest.NewRequest(http.MethodPost, "/v1/encode", bytes.NewReader(source))
	encode.Header.Set("Accept", httpapi.PrivateMediaType)
	encode.Header.Set("X-CIX-Peer-Version", "1")
	encode.Header.Set("Accept-Encoding", "gzip;q=0.2, zstd;q=0.9")
	encoded := httptest.NewRecorder()
	handler.ServeHTTP(encoded, encode)
	if encoded.Code != http.StatusOK || encoded.Header().Get("Content-Encoding") != "zstd" {
		t.Fatalf("encode status=%d encoding=%q body=%q", encoded.Code, encoded.Header().Get("Content-Encoding"), encoded.Body.String())
	}
	archive, err := network.DecodeFormat(network.Zstd, encoded.Body.Bytes(), 1<<20, 4<<20)
	if err != nil {
		t.Fatal(err)
	}
	decode := httptest.NewRequest(http.MethodPost, "/v1/decode", bytes.NewReader(archive))
	decode.Header.Set("Content-Type", httpapi.PrivateMediaType)
	decode.Header.Set("Accept", httpapi.PrivateMediaType)
	decode.Header.Set("X-CIX-Peer-Version", "1")
	restored := httptest.NewRecorder()
	handler.ServeHTTP(restored, decode)
	if restored.Code != http.StatusOK || !bytes.Equal(restored.Body.Bytes(), source) {
		t.Fatalf("decode status=%d body=%q", restored.Code, restored.Body.Bytes())
	}
	oversized := httptest.NewRequest(http.MethodPost, "/v1/encode", bytes.NewReader(nil))
	oversized.ContentLength = (1 << 20) + 1
	rejected := httptest.NewRecorder()
	handler.ServeHTTP(rejected, oversized)
	if rejected.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("oversize status=%d", rejected.Code)
	}
}

func TestHTTPStandardCodingMatrixAndRejections(t *testing.T) {
	if os.Getenv("CIX_NETWORK_HOST") != "1" {
		t.Skip("scheduler-only native HTTP contract")
	}
	options, err := cix.DefaultOptions()
	if err != nil {
		t.Fatal(err)
	}
	options.OutputLimit, options.MemoryLimit = 4<<20, 768<<20
	handler, err := httpapi.NewHandler(httpapi.Config{
		Options:          options,
		MaxInput:         1 << 20,
		MaxOutput:        4 << 20,
		AllowPrivatePeer: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	source := []byte("CIX HTTP standard coding matrix")
	for _, item := range []struct {
		name   string
		format network.Format
	}{
		{"gzip", network.Gzip},
		{"br", network.Brotli},
		{"zstd", network.Zstd},
	} {
		t.Run(item.name, func(t *testing.T) {
			requestBody := encodeRequestCoding(t, item.format, source)
			encode := controlledRequest(http.MethodPost, "/v1/encode", requestBody)
			encode.Header.Set("Content-Encoding", item.name)
			encode.Header.Set("Accept-Encoding", item.name)
			encoded := httptest.NewRecorder()
			handler.ServeHTTP(encoded, encode)
			if encoded.Code != http.StatusOK || encoded.Header().Get("Content-Encoding") != item.name {
				t.Fatalf("encode status=%d encoding=%q body=%q", encoded.Code, encoded.Header().Get("Content-Encoding"), encoded.Body.String())
			}
			archive := decodeResponseCoding(t, item.format, encoded.Body.Bytes())
			decode := controlledRequest(http.MethodPost, "/v1/decode", encoded.Body.Bytes())
			decode.Header.Set("Content-Type", httpapi.PrivateMediaType)
			decode.Header.Set("Content-Encoding", item.name)
			restored := httptest.NewRecorder()
			handler.ServeHTTP(restored, decode)
			if restored.Code != http.StatusOK || !bytes.Equal(restored.Body.Bytes(), source) {
				t.Fatalf("decode status=%d body=%q", restored.Code, restored.Body.Bytes())
			}
			if len(archive) == 0 {
				t.Fatal("standard-coded response decoded to an empty CIX archive")
			}
		})
	}
	for _, contentEncoding := range []string{"unknown", "gzip, br"} {
		request := controlledRequest(http.MethodPost, "/v1/encode", source)
		request.Header.Set("Content-Encoding", contentEncoding)
		response := httptest.NewRecorder()
		handler.ServeHTTP(response, request)
		if response.Code != http.StatusUnsupportedMediaType {
			t.Fatalf("content-encoding %q status=%d", contentEncoding, response.Code)
		}
	}
	truncated := controlledRequest(http.MethodPost, "/v1/encode", []byte("not a gzip member"))
	truncated.Header.Set("Content-Encoding", "gzip")
	truncatedResponse := httptest.NewRecorder()
	handler.ServeHTTP(truncatedResponse, truncated)
	if truncatedResponse.Code != http.StatusUnprocessableEntity {
		t.Fatalf("truncated coding status=%d", truncatedResponse.Code)
	}
	badNegotiation := controlledRequest(http.MethodPost, "/v1/encode", source)
	badNegotiation.Header.Set("Accept-Encoding", "gzip;q=NaN")
	badResponse := httptest.NewRecorder()
	handler.ServeHTTP(badResponse, badNegotiation)
	if badResponse.Code != http.StatusBadRequest {
		t.Fatalf("bad negotiation status=%d", badResponse.Code)
	}
	cancelled, cancel := context.WithCancel(context.Background())
	cancel()
	cancelledRequest := controlledRequest(http.MethodPost, "/v1/encode", source).WithContext(cancelled)
	cancelledResponse := httptest.NewRecorder()
	handler.ServeHTTP(cancelledResponse, cancelledRequest)
	if cancelledResponse.Code != http.StatusRequestTimeout {
		t.Fatalf("pre-entry cancellation status=%d", cancelledResponse.Code)
	}
}

func controlledRequest(method, path string, body []byte) *http.Request {
	request := httptest.NewRequest(method, path, bytes.NewReader(body))
	request.Header.Set("Accept", httpapi.PrivateMediaType)
	request.Header.Set("X-CIX-Peer-Version", "1")
	return request
}

func encodeRequestCoding(t *testing.T, format network.Format, source []byte) []byte {
	t.Helper()
	if format != network.Gzip {
		coded, err := network.EncodeFormat(format, source, 4<<20, 768<<20)
		if err != nil {
			t.Fatal(err)
		}
		return coded
	}
	var encoded bytes.Buffer
	writer := gzip.NewWriter(&encoded)
	if _, err := writer.Write(source); err != nil {
		t.Fatal(err)
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	return encoded.Bytes()
}

func decodeResponseCoding(t *testing.T, format network.Format, coded []byte) []byte {
	t.Helper()
	if format != network.Gzip {
		decoded, err := network.DecodeFormat(format, coded, 1<<20, 768<<20)
		if err != nil {
			t.Fatal(err)
		}
		return decoded
	}
	reader, err := gzip.NewReader(bytes.NewReader(coded))
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := io.ReadAll(reader)
	if closeErr := reader.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		t.Fatal(err)
	}
	return decoded
}
