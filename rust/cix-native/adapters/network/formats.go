// Package network exposes bounded, optional network adapters for the installed CIX SDK.
package network

/*
#cgo pkg-config: cix-native
#include <stdlib.h>
#include <cix_formats.h>
*/
import "C"

import (
	"fmt"
	"unsafe"
)

func maxInt() uint64 { return uint64(^uint(0) >> 1) }

type Format uint32

const (
	Gzip   Format = 1
	Zstd   Format = 6
	Brotli Format = 7
)

func formatCall(decode bool, format Format, input []byte, outputLimit, memoryLimit uint64) ([]byte, error) {
	if outputLimit == 0 || memoryLimit == 0 || uint64(len(input)) > memoryLimit {
		return nil, fmt.Errorf("CIX network format input exceeds configured limits")
	}
	options := C.struct_cix_format_options_v1{abi_version: C.CIX_FORMAT_ABI_V1, struct_size: C.uint32_t(C.sizeof_struct_cix_format_options_v1), format: C.uint32_t(format), level: 6, output_limit: C.uint64_t(outputLimit), memory_limit: C.uint64_t(memoryLimit)}
	var inputPtr *C.uint8_t
	if len(input) != 0 {
		inputPtr = (*C.uint8_t)(unsafe.Pointer(&input[0]))
	}
	var needed C.size_t
	var status C.int
	if decode {
		status = C.cix_format_decode_v1(&options, inputPtr, C.size_t(len(input)), nil, 0, &needed)
	} else {
		status = C.cix_format_encode_v1(&options, inputPtr, C.size_t(len(input)), nil, 0, &needed)
	}
	if status == C.CIX_STATUS_OK {
		return []byte{}, nil
	}
	if status != C.CIX_STATUS_OUTPUT_TOO_SMALL {
		return nil, fmt.Errorf("CIX standard-format sizing failed: status %d", status)
	}
	if uint64(needed) > outputLimit || uint64(needed) > maxInt() || uint64(needed) > memoryLimit-uint64(len(input)) {
		return nil, fmt.Errorf("CIX standard-format result exceeds configured limits")
	}
	output := make([]byte, int(needed))
	var outputPtr *C.uint8_t
	if len(output) != 0 {
		outputPtr = (*C.uint8_t)(unsafe.Pointer(&output[0]))
	}
	var written C.size_t
	if decode {
		status = C.cix_format_decode_v1(&options, inputPtr, C.size_t(len(input)), outputPtr, needed, &written)
	} else {
		status = C.cix_format_encode_v1(&options, inputPtr, C.size_t(len(input)), outputPtr, needed, &written)
	}
	if status != C.CIX_STATUS_OK || written > needed {
		return nil, fmt.Errorf("CIX standard-format operation failed: status %d", status)
	}
	return output[:int(written)], nil
}
func DecodeFormat(format Format, input []byte, outputLimit, memoryLimit uint64) ([]byte, error) {
	return formatCall(true, format, input, outputLimit, memoryLimit)
}
func EncodeFormat(format Format, input []byte, outputLimit, memoryLimit uint64) ([]byte, error) {
	return formatCall(false, format, input, outputLimit, memoryLimit)
}
