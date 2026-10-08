// Package httpapi provides a controlled-peer, unary HTTP boundary over the installed CIX SDK.
package httpapi

import (
	"errors"
	"fmt"
	"io"
	"math"
	"mime"
	"net/http"
	"sort"
	"strconv"
	"strings"

	cix "example.invalid/cix/native"
	"example.invalid/cix/network"
)

const PrivateMediaType = "application/vnd.cix"
const peerHeader = "X-CIX-Peer-Version"

type Config struct {
	Options          cix.Options
	MaxInput         uint64
	MaxOutput        uint64
	AllowPrivatePeer bool
}

func (c Config) valid() error {
	if c.MaxInput == 0 || c.MaxOutput == 0 ||
		c.MaxInput > (uint64(1)<<63)-1 ||
		c.Options.MemoryLimit < c.MaxInput ||
		c.MaxOutput > c.Options.MemoryLimit-c.MaxInput ||
		c.MaxInput > c.Options.MemoryLimit/3 ||
		c.MaxOutput > c.Options.MemoryLimit/3 {
		return errors.New("invalid CIX HTTP limits")
	}
	return nil
}
func NewHandler(config Config) (http.Handler, error) {
	if err := config.valid(); err != nil {
		return nil, err
	}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			w.Header().Set("Allow", http.MethodPost)
			http.Error(w, "POST required", http.StatusMethodNotAllowed)
			return
		}
		switch r.URL.Path {
		case "/v1/encode":
			transform(w, r, config, true)
		case "/v1/decode":
			transform(w, r, config, false)
		default:
			http.NotFound(w, r)
		}
	}), nil
}
func transform(w http.ResponseWriter, r *http.Request, config Config, encode bool) {
	if r.Context().Err() != nil {
		http.Error(w, "request cancelled before native entry", http.StatusRequestTimeout)
		return
	}
	if r.ContentLength > int64(config.MaxInput) {
		http.Error(w, "request too large", http.StatusRequestEntityTooLarge)
		return
	}
	input, err := readBounded(w, r, config.MaxInput)
	if err != nil {
		http.Error(w, err.Error(), statusFor(err))
		return
	}
	compressedRequest := false
	if f, ok, err := requestEncoding(r.Header.Get("Content-Encoding")); err != nil {
		http.Error(w, err.Error(), http.StatusUnsupportedMediaType)
		return
	} else if ok {
		input, err = network.DecodeFormat(f, input, config.MaxInput, config.Options.MemoryLimit)
		if err != nil {
			http.Error(w, "encoded request rejected", http.StatusUnprocessableEntity)
			return
		}
		compressedRequest = true
	}
	if r.Context().Err() != nil {
		http.Error(w, "request cancelled before native entry", http.StatusRequestTimeout)
		return
	}
	if encode && !controlledPeer(r, config) {
		http.Error(w, "private CIX peer negotiation required", http.StatusNotAcceptable)
		return
	}
	if !encode && (!controlledPeer(r, config) || !isMediaType(r.Header.Get("Content-Type"), PrivateMediaType)) {
		http.Error(w, "private CIX peer negotiation and media type required", http.StatusNotAcceptable)
		return
	}
	nativeOptions := contextOptions(config, compressedRequest)
	context, err := cix.NewContext(nativeOptions)
	if err != nil {
		http.Error(w, "native context unavailable", http.StatusServiceUnavailable)
		return
	}
	defer context.Close()
	if encode {
		input, err = context.Encode(input)
	} else {
		input, err = context.Decode(input)
	}
	if err != nil {
		http.Error(w, "CIX transform rejected", http.StatusUnprocessableEntity)
		return
	}
	if uint64(len(input)) > config.MaxOutput {
		http.Error(w, "result too large", http.StatusRequestEntityTooLarge)
		return
	}
	if f, ok, identityAllowed, parseErr := acceptedEncoding(r.Header.Get("Accept-Encoding")); parseErr != nil {
		http.Error(w, "invalid Accept-Encoding", http.StatusBadRequest)
		return
	} else if ok {
		input, err = network.EncodeFormat(f, input, config.MaxOutput, config.Options.MemoryLimit)
		if err != nil {
			http.Error(w, "response encoding rejected", http.StatusInternalServerError)
			return
		}
		w.Header().Set("Content-Encoding", encodingName(f))
	} else if !identityAllowed {
		http.Error(w, "no acceptable response encoding", http.StatusNotAcceptable)
		return
	}
	if r.Context().Err() != nil {
		return
	} // The native call was synchronous; no hard cancellation claim.
	if encode {
		w.Header().Set("Content-Type", PrivateMediaType)
		w.Header().Set(peerHeader, "1")
	} else {
		w.Header().Set("Content-Type", "application/octet-stream")
	}
	w.Header().Set("Content-Length", strconv.Itoa(len(input)))
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write(input)
}

func contextOptions(config Config, compressedRequest bool) cix.Options {
	nativeOptions := config.Options
	// The adapter's public cap, rather than a wider caller option, governs native output.
	nativeOptions.OutputLimit = config.MaxOutput
	// cix_context_buffer creates an owned native result before the Go binding copies
	// it into its separately allocated caller buffer. Reserve that public output and
	// the request input. An encoded request still retains its wire body in addition
	// to the inflated input while the native call runs.
	reserved := config.MaxInput + config.MaxOutput
	if compressedRequest {
		reserved += config.MaxInput
	}
	nativeOptions.MemoryLimit -= reserved
	return nativeOptions
}
func controlledPeer(r *http.Request, config Config) bool {
	return config.AllowPrivatePeer && r.Header.Get(peerHeader) == "1" && accepts(r.Header.Get("Accept"), PrivateMediaType)
}
func readBounded(w http.ResponseWriter, r *http.Request, limit uint64) ([]byte, error) {
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, int64(limit)))
	if err != nil {
		return nil, fmt.Errorf("request too large or unreadable")
	}
	return body, nil
}
func statusFor(err error) int {
	if strings.Contains(err.Error(), "large") {
		return http.StatusRequestEntityTooLarge
	}
	return http.StatusBadRequest
}
func requestEncoding(value string) (network.Format, bool, error) {
	value = strings.TrimSpace(strings.ToLower(value))
	if value == "" || value == "identity" {
		return 0, false, nil
	}
	if strings.Contains(value, ",") {
		return 0, false, errors.New("stacked Content-Encoding is unsupported")
	}
	f, ok := formatFor(value)
	if !ok {
		return 0, false, errors.New("unsupported Content-Encoding")
	}
	return f, true, nil
}
func encodingName(f network.Format) string {
	if f == network.Gzip {
		return "gzip"
	}
	if f == network.Brotli {
		return "br"
	}
	return "zstd"
}
func formatFor(value string) (network.Format, bool) {
	switch value {
	case "gzip":
		return network.Gzip, true
	case "br":
		return network.Brotli, true
	case "zstd":
		return network.Zstd, true
	}
	return 0, false
}

type weighted struct {
	name string
	q    float64
	rank int
}

func acceptedEncoding(header string) (network.Format, bool, bool, error) {
	if strings.TrimSpace(header) == "" {
		return 0, false, true, nil
	}
	qualities := map[string]float64{}
	wildcard := -1.0
	for _, item := range strings.Split(header, ",") {
		parts := strings.Split(item, ";")
		name := strings.TrimSpace(strings.ToLower(parts[0]))
		q := 1.0
		for _, p := range parts[1:] {
			k, v, ok := strings.Cut(strings.TrimSpace(p), "=")
			if ok && strings.EqualFold(k, "q") {
				parsed, e := strconv.ParseFloat(v, 64)
				if e != nil || math.IsNaN(parsed) || math.IsInf(parsed, 0) || parsed < 0 || parsed > 1 {
					return 0, false, false, errors.New("invalid q-value")
				}
				q = parsed
			}
		}
		if name == "*" {
			wildcard = q
		} else {
			qualities[name] = q
		}
	}
	names := []string{"zstd", "br", "gzip"}
	choices := make([]weighted, 0, 3)
	for rank, name := range names {
		q, found := qualities[name]
		if !found {
			q = wildcard
		}
		if q > 0 {
			choices = append(choices, weighted{name, q, rank})
		}
	}
	sort.Slice(choices, func(i, j int) bool {
		if choices[i].q == choices[j].q {
			return choices[i].rank < choices[j].rank
		}
		return choices[i].q > choices[j].q
	})
	identity, identitySpecified := qualities["identity"]
	identityAllowed := !identitySpecified || identity > 0
	if !identitySpecified && wildcard == 0 {
		identityAllowed = false
	}
	if len(choices) == 0 {
		return 0, false, identityAllowed, nil
	}
	if identitySpecified && identity > choices[0].q {
		return 0, false, true, nil
	}
	f, _ := formatFor(choices[0].name)
	return f, true, identityAllowed, nil
}
func isMediaType(value, expected string) bool {
	media, parameters, err := mime.ParseMediaType(value)
	if err != nil || !strings.EqualFold(media, expected) {
		return false
	}
	if raw, ok := parameters["q"]; ok {
		q, err := strconv.ParseFloat(raw, 64)
		return err == nil && !math.IsNaN(q) && !math.IsInf(q, 0) && q > 0 && q <= 1
	}
	return true
}
func accepts(header, expected string) bool {
	for _, item := range strings.Split(header, ",") {
		if isMediaType(strings.TrimSpace(item), expected) {
			return true
		}
	}
	return false
}
