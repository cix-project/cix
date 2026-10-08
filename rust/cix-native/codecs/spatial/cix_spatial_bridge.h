/* CIX-owned bounded JPEG-LS/JPEG 2000 grayscale bridge. */
#ifndef CIX_SPATIAL_BRIDGE_H
#define CIX_SPATIAL_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
# if defined(CIX_SPATIAL_BRIDGE_BUILDING)
#  define CIX_SPATIAL_API __declspec(dllexport)
# else
#  define CIX_SPATIAL_API __declspec(dllimport)
# endif
#else
# define CIX_SPATIAL_API __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

/**
 * @brief Status returned by the bounded spatial bridge.
 *
 * INVALID_ARGUMENT identifies caller-side shape, pointer, or limit errors;
 * UNSUPPORTED identifies a codec or stream/layout outside the single-plane
 * lossless contract. OUTPUT_LIMIT reports a caller-supplied bound. Codec
 * failures remain separated from allocation and unexpected exception status.
 */
enum cix_spatial_status { CIX_SPATIAL_OK=0, CIX_SPATIAL_INVALID_ARGUMENT=1,
  CIX_SPATIAL_UNSUPPORTED=2, CIX_SPATIAL_OUTPUT_LIMIT=3,
  CIX_SPATIAL_ENCODE_FAILURE=4, CIX_SPATIAL_DECODE_FAILURE=5,
  CIX_SPATIAL_ALLOCATION_FAILURE=6, CIX_SPATIAL_EXCEPTION=7 };
/** @brief Supported lossless grayscale codestream families; numeric values are wire selectors. */
enum cix_spatial_codec { CIX_SPATIAL_JPEG2000=2, CIX_SPATIAL_JPEGLS=3 };
/**
 * @brief Bridge-owned encoded byte result.
 *
 * Release successful allocations with cix_spatial_free_buffer. The free call
 * accepts NULL and clears the structure. The encoder initializes output after
 * validating its output pointer; early invalid-argument returns may leave it
 * unchanged. Decoding writes caller-owned storage directly.
 */
struct cix_spatial_buffer { uint8_t* data; size_t size; };
/** @brief Gray image dimensions and source precision obtained by a bounded probe. */
struct cix_spatial_info { uint32_t width, height, bits_per_sample; };

/**
 * @brief Encode one lossless grayscale plane from canonical little-endian u16 samples.
 *
 * pixels_size must be width * height * 2 exactly, including for source
 * precision at most 8 bits. max_output_bytes is a hard codestream cap. A
 * successful output allocation belongs to the bridge.
 */
CIX_SPATIAL_API int cix_spatial_encode_u16_gray(
  uint32_t codec, const uint8_t* pixels, size_t pixels_size,
  uint32_t width, uint32_t height, uint32_t bits_per_sample,
  size_t max_output_bytes, struct cix_spatial_buffer* output);
/**
 * @brief Validate a stream against an exact framed grayscale shape.
 *
 * No pixel buffer is installed. After pointer and precision validation, info
 * is cleared before processing and is populated only after codec, dimensions,
 * precision, single component, and lossless constraints pass.
 */
CIX_SPATIAL_API int cix_spatial_probe_gray(
  uint32_t codec, const uint8_t* input, size_t input_size,
  uint32_t expected_width, uint32_t expected_height,
  uint32_t expected_bits_per_sample, struct cix_spatial_info* info);
/**
 * @brief Recover bounded grayscale metadata when framing has no exact shape.
 *
 * No pixel buffer is installed. max_pixels bounds the sample count and
 * max_output_bytes bounds its canonical little-endian-u16 byte count; either
 * exceeded limit returns CIX_SPATIAL_OUTPUT_LIMIT.
 */
CIX_SPATIAL_API int cix_spatial_probe_any_gray(
  uint32_t codec, const uint8_t* input, size_t input_size,
  size_t max_pixels, size_t max_output_bytes, struct cix_spatial_info* info);
/**
 * @brief Decode an exact, lossless grayscale shape into caller-owned u16 LE storage.
 *
 * output_size must equal width * height * 2 exactly. For an at-most-8-bit
 * JPEG-LS stream the significant source byte is expanded into the low byte and
 * the high byte is zero. The bridge validates stream shape before decoding.
 */
CIX_SPATIAL_API int cix_spatial_decode_u16_gray(
  uint32_t codec, const uint8_t* input, size_t input_size,
  uint32_t expected_width, uint32_t expected_height,
  uint32_t expected_bits_per_sample, uint8_t* output, size_t output_size);
/** @brief Release bridge-owned output; accepts NULL and clears the buffer. */
CIX_SPATIAL_API void cix_spatial_free_buffer(struct cix_spatial_buffer* buffer);
/** @brief Return the linked CharLS version as major * 10000 + minor * 100 + patch. */
CIX_SPATIAL_API uint32_t cix_spatial_charls_version(void);
/** @brief Return the linked OpenJPEG version as major * 10000 + minor * 100 + build. */
CIX_SPATIAL_API uint32_t cix_spatial_openjpeg_version(void);
/** @brief Return this bridge's immutable implementation identifier. */
CIX_SPATIAL_API const char* cix_spatial_bridge_version(void);
#ifdef __cplusplus
}
#endif
#endif
