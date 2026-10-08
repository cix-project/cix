// Package cix is an optional cgo binding for the installed CIX native C ABI.
package cix

/*
#cgo pkg-config: cix-native
#include <stdlib.h>
#include <cix.h>
#include <cix_stream.h>
*/
import "C"

import (
	"errors"
	"fmt"
	"sync"
	"unsafe"
)

// Profile controls the native complete-buffer compression profile.
type Profile uint32

const (
	Fast    Profile = C.CIX_PROFILE_FAST
	Default Profile = C.CIX_PROFILE_DEFAULT
	Best    Profile = C.CIX_PROFILE_BEST
)

// Status is the unmodified status code returned by the C ABI.
type Status uint32

const (
	StatusOK              Status = C.CIX_STATUS_OK
	StatusInvalidArgument Status = C.CIX_STATUS_INVALID_ARGUMENT
	StatusInvalidOptions  Status = C.CIX_STATUS_INVALID_OPTIONS
	StatusOutputTooSmall  Status = C.CIX_STATUS_OUTPUT_TOO_SMALL
	StatusCodecError      Status = C.CIX_STATUS_CODEC_ERROR
	StatusPanic           Status = C.CIX_STATUS_PANIC
	StatusResourceLimit   Status = C.CIX_STATUS_RESOURCE_LIMIT
)

// Error reports a non-success C ABI status and, when available, its diagnostic.
type Error struct {
	Status  Status
	Message string
}

func (e *Error) Error() string {
	if e.Message == "" {
		return fmt.Sprintf("cix native error: status %d", e.Status)
	}
	return fmt.Sprintf("cix native error: %s (status %d)", e.Message, e.Status)
}

var ErrClosed = errors.New("cix: handle is closed")
var ErrCancelled = errors.New("cix: operation cancelled before native call")

// DefaultWrapperAllocationLimit bounds one Go-owned output or diagnostic
// buffer when Options.WrapperAllocationLimit is left at zero.
const DefaultWrapperAllocationLimit uint64 = 64 << 20

// Options are copied into each native context or stream at creation time.
// OutputLimit and MemoryLimit are passed to the native ABI and must both be
// nonzero. WrapperAllocationLimit is a separate Go heap allocation budget;
// zero selects DefaultWrapperAllocationLimit.
type Options struct {
	Profile                Profile
	Workers                uint32
	OutputLimit            uint64
	MemoryLimit            uint64
	WrapperAllocationLimit uint64
}

func DefaultOptions() (Options, error) {
	var native C.cix_options_v1
	if status := C.cix_options_v1_default(&native); status != C.CIX_STATUS_OK {
		return Options{}, statusError(Status(status), "default options")
	}
	return optionsFromNative(native), nil
}

func optionsFromNative(native C.cix_options_v1) Options {
	return Options{Profile: Profile(native.profile), Workers: uint32(native.workers), OutputLimit: uint64(native.output_limit), MemoryLimit: uint64(native.memory_limit), WrapperAllocationLimit: DefaultWrapperAllocationLimit}
}

func (o Options) wrapperAllocationLimit() (uint64, error) {
	limit := o.WrapperAllocationLimit
	if limit == 0 {
		limit = DefaultWrapperAllocationLimit
	}
	if limit > uint64(maxInt()) {
		return 0, errors.New("cix: wrapper allocation limit exceeds Go addressable size")
	}
	return limit, nil
}

func (o Options) native() (C.cix_options_v1, error) {
	if o.Profile != Fast && o.Profile != Default && o.Profile != Best {
		return C.cix_options_v1{}, fmt.Errorf("cix: invalid profile %d", o.Profile)
	}
	if o.OutputLimit == 0 || o.MemoryLimit == 0 {
		return C.cix_options_v1{}, errors.New("cix: output and memory limits must be nonzero")
	}
	if o.OutputLimit > uint64(maxInt()) || o.MemoryLimit > uint64(maxInt()) {
		return C.cix_options_v1{}, errors.New("cix: limit exceeds Go addressable size")
	}
	if _, err := o.wrapperAllocationLimit(); err != nil {
		return C.cix_options_v1{}, err
	}
	return C.cix_options_v1{abi_version: C.CIX_ABI_VERSION_1, struct_size: C.uint32_t(C.sizeof_cix_options_v1), profile: C.uint32_t(o.Profile), workers: C.uint32_t(o.Workers), output_limit: C.uint64_t(o.OutputLimit), memory_limit: C.uint64_t(o.MemoryLimit)}, nil
}

func maxInt() int { return int(^uint(0) >> 1) }
func bytesPtr(data []byte) *C.uint8_t {
	if len(data) == 0 {
		return nil
	}
	return (*C.uint8_t)(unsafe.Pointer(&data[0]))
}

// Context owns one serialized complete-buffer native context. Do not copy it.
type Context struct {
	mu              sync.Mutex
	native          *C.cix_context
	options         Options
	allocationLimit uint64
	closed          bool
}

func NewContext(options Options) (*Context, error) {
	native, err := options.native()
	if err != nil {
		return nil, err
	}
	var context *C.cix_context
	if status := C.cix_context_create(&native, &context); status != C.CIX_STATUS_OK {
		return nil, statusError(Status(status), "create context")
	}
	if context == nil {
		return nil, errors.New("cix: native create returned nil context")
	}
	allocationLimit, _ := options.wrapperAllocationLimit()
	return &Context{native: context, options: options, allocationLimit: allocationLimit}, nil
}

// Close destroys the native context after serializing with all calls on it.
func (c *Context) Close() error {
	if c == nil {
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return nil
	}
	C.cix_context_destroy(c.native)
	c.native = nil
	c.closed = true
	return nil
}

func (c *Context) Encode(input []byte) ([]byte, error) { return c.buffer(input, true) }
func (c *Context) Decode(input []byte) ([]byte, error) { return c.buffer(input, false) }

func (c *Context) buffer(input []byte, encode bool) ([]byte, error) {
	if c == nil {
		return nil, ErrClosed
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed || c.native == nil {
		return nil, ErrClosed
	}
	var needed C.size_t
	var status C.cix_status
	if encode {
		status = C.cix_encode_buffer(c.native, bytesPtr(input), C.size_t(len(input)), nil, 0, &needed)
	} else {
		status = C.cix_decode_buffer(c.native, bytesPtr(input), C.size_t(len(input)), nil, 0, &needed)
	}
	if status != C.CIX_STATUS_OUTPUT_TOO_SMALL && status != C.CIX_STATUS_OK {
		return nil, c.statusError(Status(status), "buffer sizing")
	}
	if uint64(needed) > c.options.OutputLimit || uint64(needed) > uint64(maxInt()) {
		return nil, &Error{Status: StatusResourceLimit, Message: "required output exceeds configured Go output limit"}
	}
	if uint64(needed) > c.allocationLimit {
		return nil, &Error{Status: StatusResourceLimit, Message: "required output exceeds configured Go wrapper allocation limit"}
	}
	output := make([]byte, int(needed))
	var written C.size_t
	if encode {
		status = C.cix_encode_buffer(c.native, bytesPtr(input), C.size_t(len(input)), bytesPtr(output), C.size_t(len(output)), &written)
	} else {
		status = C.cix_decode_buffer(c.native, bytesPtr(input), C.size_t(len(input)), bytesPtr(output), C.size_t(len(output)), &written)
	}
	if status != C.CIX_STATUS_OK {
		return nil, c.statusError(Status(status), "buffer operation")
	}
	if written > C.size_t(len(output)) {
		return nil, &Error{Status: StatusCodecError, Message: "native wrote an invalid output count"}
	}
	return output[:int(written)], nil
}

func (c *Context) statusError(status Status, operation string) error {
	return statusError(status, operation+": "+c.lastErrorLocked())
}
func statusError(status Status, operation string) error {
	return &Error{Status: status, Message: operation}
}
func (c *Context) lastErrorLocked() string {
	var needed C.size_t
	if C.cix_context_last_error(c.native, nil, 0, &needed) != C.CIX_STATUS_OUTPUT_TOO_SMALL || needed == 0 || uint64(needed) > uint64(maxInt()) || uint64(needed) > c.allocationLimit {
		return ""
	}
	buffer := make([]byte, int(needed))
	if C.cix_context_last_error(c.native, (*C.char)(unsafe.Pointer(bytesPtr(buffer))), C.size_t(len(buffer)), &needed) != C.CIX_STATUS_OK || needed == 0 {
		return ""
	}
	return string(buffer[:int(needed)-1])
}

// StreamState is the actual native result state.
type StreamState uint32

const (
	NeedsInput  StreamState = C.CIX_STREAM_NEEDS_INPUT
	NeedsOutput StreamState = C.CIX_STREAM_NEEDS_OUTPUT
	Finished    StreamState = C.CIX_STREAM_FINISHED
)

// Progress contains the actual C ABI counts for one call.
type Progress struct {
	Consumed, Produced int
	State              StreamState
}

func progress(result C.cix_stream_result_v1, input, capacity int) (Progress, error) {
	if result.consumed > C.size_t(input) || result.produced > C.size_t(capacity) {
		return Progress{}, &Error{Status: StatusCodecError, Message: "native returned invalid stream counts"}
	}
	state := StreamState(result.state)
	if state != NeedsInput && state != NeedsOutput && state != Finished {
		return Progress{}, &Error{Status: StatusCodecError, Message: "native returned an unknown stream state"}
	}
	return Progress{Consumed: int(result.consumed), Produced: int(result.produced), State: state}, nil
}
func outputBuffer(capacity int, remaining, allocationLimit uint64) ([]byte, error) {
	if capacity < 0 || uint64(capacity) > remaining {
		return nil, &Error{Status: StatusResourceLimit, Message: "stream output capacity exceeds remaining output limit"}
	}
	if uint64(capacity) > allocationLimit {
		return nil, &Error{Status: StatusResourceLimit, Message: "stream output capacity exceeds configured Go wrapper allocation limit"}
	}
	return make([]byte, capacity), nil
}

// Encoder owns one independent native CIXG1 encoder handle. Do not copy it.
type Encoder struct {
	mu                               sync.Mutex
	native                           *C.cix_stream_encoder
	limit, allocationLimit, produced uint64
	closed, cancelled                bool
}

func NewEncoder(options Options) (*Encoder, error) {
	native, err := options.native()
	if err != nil {
		return nil, err
	}
	var stream *C.cix_stream_encoder
	if status := C.cix_stream_encoder_create(&native, &stream); status != C.CIX_STATUS_OK {
		return nil, statusError(Status(status), "create encoder")
	}
	if stream == nil {
		return nil, errors.New("cix: native create returned nil encoder")
	}
	allocationLimit, _ := options.wrapperAllocationLimit()
	return &Encoder{native: stream, limit: options.OutputLimit, allocationLimit: allocationLimit}, nil
}
func (e *Encoder) Close() error {
	if e == nil {
		return nil
	}
	e.mu.Lock()
	defer e.mu.Unlock()
	if !e.closed {
		C.cix_stream_encoder_destroy(e.native)
		e.native = nil
		e.closed = true
	}
	return nil
}
func (e *Encoder) Cancel() {
	if e != nil {
		e.mu.Lock()
		e.cancelled = true
		e.mu.Unlock()
	}
}
func (e *Encoder) Reset() error {
	if e == nil {
		return ErrClosed
	}
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.closed {
		return ErrClosed
	}
	if status := C.cix_stream_encoder_reset(e.native); status != C.CIX_STATUS_OK {
		return statusError(Status(status), "reset encoder")
	}
	e.produced = 0
	e.cancelled = false
	return nil
}
func (e *Encoder) Process(input []byte, outputCapacity int) ([]byte, Progress, error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.closed {
		return nil, Progress{}, ErrClosed
	}
	if e.cancelled {
		return nil, Progress{}, ErrCancelled
	}
	out, err := outputBuffer(outputCapacity, e.limit-e.produced, e.allocationLimit)
	if err != nil {
		return nil, Progress{}, err
	}
	var result C.cix_stream_result_v1
	status := C.cix_stream_encoder_process(e.native, bytesPtr(input), C.size_t(len(input)), bytesPtr(out), C.size_t(len(out)), &result)
	if status != C.CIX_STATUS_OK {
		return nil, Progress{}, statusError(Status(status), "encoder process")
	}
	p, err := progress(result, len(input), len(out))
	if err != nil {
		return nil, Progress{}, err
	}
	e.produced += uint64(p.Produced)
	return out[:p.Produced], p, nil
}
func (e *Encoder) Flush(outputCapacity int) ([]byte, Progress, error) {
	return e.finishLike(outputCapacity, false)
}
func (e *Encoder) Finish(outputCapacity int) ([]byte, Progress, error) {
	return e.finishLike(outputCapacity, true)
}
func (e *Encoder) finishLike(capacity int, finish bool) ([]byte, Progress, error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.closed {
		return nil, Progress{}, ErrClosed
	}
	if e.cancelled {
		return nil, Progress{}, ErrCancelled
	}
	out, err := outputBuffer(capacity, e.limit-e.produced, e.allocationLimit)
	if err != nil {
		return nil, Progress{}, err
	}
	var result C.cix_stream_result_v1
	var status C.cix_status
	if finish {
		status = C.cix_stream_encoder_finish(e.native, bytesPtr(out), C.size_t(len(out)), &result)
	} else {
		status = C.cix_stream_encoder_flush(e.native, bytesPtr(out), C.size_t(len(out)), &result)
	}
	if status != C.CIX_STATUS_OK {
		return nil, Progress{}, statusError(Status(status), "encoder flush/finish")
	}
	p, err := progress(result, 0, len(out))
	if err != nil {
		return nil, Progress{}, err
	}
	e.produced += uint64(p.Produced)
	return out[:p.Produced], p, nil
}

// Decoder owns one independent native CIXG1 decoder handle. Do not copy it.
type Decoder struct {
	mu                               sync.Mutex
	native                           *C.cix_stream_decoder
	limit, allocationLimit, produced uint64
	closed, cancelled                bool
}

func NewDecoder(options Options) (*Decoder, error) {
	native, err := options.native()
	if err != nil {
		return nil, err
	}
	var stream *C.cix_stream_decoder
	if status := C.cix_stream_decoder_create(&native, &stream); status != C.CIX_STATUS_OK {
		return nil, statusError(Status(status), "create decoder")
	}
	if stream == nil {
		return nil, errors.New("cix: native create returned nil decoder")
	}
	allocationLimit, _ := options.wrapperAllocationLimit()
	return &Decoder{native: stream, limit: options.OutputLimit, allocationLimit: allocationLimit}, nil
}
func (d *Decoder) Close() error {
	if d == nil {
		return nil
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	if !d.closed {
		C.cix_stream_decoder_destroy(d.native)
		d.native = nil
		d.closed = true
	}
	return nil
}
func (d *Decoder) Cancel() {
	if d != nil {
		d.mu.Lock()
		d.cancelled = true
		d.mu.Unlock()
	}
}
func (d *Decoder) Reset() error {
	if d == nil {
		return ErrClosed
	}
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.closed {
		return ErrClosed
	}
	if status := C.cix_stream_decoder_reset(d.native); status != C.CIX_STATUS_OK {
		return statusError(Status(status), "reset decoder")
	}
	d.produced = 0
	d.cancelled = false
	return nil
}
func (d *Decoder) Process(input []byte, outputCapacity int) ([]byte, Progress, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.closed {
		return nil, Progress{}, ErrClosed
	}
	if d.cancelled {
		return nil, Progress{}, ErrCancelled
	}
	out, err := outputBuffer(outputCapacity, d.limit-d.produced, d.allocationLimit)
	if err != nil {
		return nil, Progress{}, err
	}
	var result C.cix_stream_result_v1
	status := C.cix_stream_decoder_process(d.native, bytesPtr(input), C.size_t(len(input)), bytesPtr(out), C.size_t(len(out)), &result)
	if status != C.CIX_STATUS_OK {
		return nil, Progress{}, statusError(Status(status), "decoder process")
	}
	p, err := progress(result, len(input), len(out))
	if err != nil {
		return nil, Progress{}, err
	}
	d.produced += uint64(p.Produced)
	return out[:p.Produced], p, nil
}
func (d *Decoder) Finish(outputCapacity int) ([]byte, Progress, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.closed {
		return nil, Progress{}, ErrClosed
	}
	if d.cancelled {
		return nil, Progress{}, ErrCancelled
	}
	out, err := outputBuffer(outputCapacity, d.limit-d.produced, d.allocationLimit)
	if err != nil {
		return nil, Progress{}, err
	}
	var result C.cix_stream_result_v1
	status := C.cix_stream_decoder_finish(d.native, bytesPtr(out), C.size_t(len(out)), &result)
	if status != C.CIX_STATUS_OK {
		return nil, Progress{}, statusError(Status(status), "decoder finish")
	}
	p, err := progress(result, 0, len(out))
	if err != nil {
		return nil, Progress{}, err
	}
	d.produced += uint64(p.Produced)
	return out[:p.Produced], p, nil
}
