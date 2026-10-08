package httpapi

import (
	"net/http"
	"net/http/httptest"
	"testing"

	cix "example.invalid/cix/native"
)

func testConfig() Config {
	return Config{
		Options:   cix.Options{MemoryLimit: 100, OutputLimit: 99},
		MaxInput:  10,
		MaxOutput: 20,
	}
}

func TestContextOptionsUsesPublicOutputCap(t *testing.T) {
	config := testConfig()
	plain := contextOptions(config, false)
	if plain.OutputLimit != config.MaxOutput || plain.MemoryLimit != config.Options.MemoryLimit-config.MaxInput-config.MaxOutput {
		t.Fatalf("plain options = %#v", plain)
	}
	compressed := contextOptions(config, true)
	if compressed.OutputLimit != config.MaxOutput || compressed.MemoryLimit != config.Options.MemoryLimit-2*config.MaxInput-config.MaxOutput {
		t.Fatalf("compressed options = %#v", compressed)
	}
}

func TestConfigRejectsInsufficientSimultaneousBufferBudget(t *testing.T) {
	config := testConfig()
	config.MaxInput = 40
	config.MaxOutput = 30
	if err := config.valid(); err == nil {
		t.Fatal("accepted a budget below the request/body/output reservation")
	}
}

func TestConfigRejectsThreeLiveStandardFormatBuffers(t *testing.T) {
	config := testConfig()
	config.MaxInput = 10
	config.MaxOutput = 40
	if err := config.valid(); err == nil {
		t.Fatal("accepted an output cap that permits three simultaneous response buffers")
	}
}

func TestAcceptEncodingHonoursIdentityAndRejectsNonFiniteQuality(t *testing.T) {
	format, useEncoding, identityAllowed, err := acceptedEncoding("zstd;q=0.4, identity;q=0.9")
	if err != nil || useEncoding || !identityAllowed || format != 0 {
		t.Fatalf("identity preference result: format=%v use=%v identity=%v err=%v", format, useEncoding, identityAllowed, err)
	}
	format, useEncoding, identityAllowed, err = acceptedEncoding("zstd;q=0.9")
	if err != nil || !useEncoding || !identityAllowed || format == 0 {
		t.Fatalf("implicit identity result: format=%v use=%v identity=%v err=%v", format, useEncoding, identityAllowed, err)
	}
	for _, header := range []string{"zstd;q=NaN", "gzip;q=+Inf"} {
		if _, _, _, err := acceptedEncoding(header); err == nil {
			t.Fatalf("accepted non-finite quality %q", header)
		}
	}
	_, useEncoding, identityAllowed, err = acceptedEncoding("identity;q=0")
	if err != nil || useEncoding || identityAllowed {
		t.Fatalf("identity exclusion result: use=%v identity=%v err=%v", useEncoding, identityAllowed, err)
	}
}

func TestPrivateMediaRejectsZeroQuality(t *testing.T) {
	if isMediaType("application/vnd.cix;q=0", PrivateMediaType) {
		t.Fatal("zero-quality private media type was accepted")
	}
}

func TestZeroQualityPrivatePeerIsRejectedBeforeNativeEntry(t *testing.T) {
	handler, err := NewHandler(testConfig())
	if err != nil {
		t.Fatal(err)
	}
	request := httptest.NewRequest(http.MethodPost, "/v1/encode", nil)
	request.Header.Set(peerHeader, "1")
	request.Header.Set("Accept", PrivateMediaType+";q=0")
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusNotAcceptable {
		t.Fatalf("status=%d", response.Code)
	}
}
