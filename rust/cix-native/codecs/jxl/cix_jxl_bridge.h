/* CIX-owned, bounded libjxl 0.12 bridge for gray and Z/H/W planar images. */
#ifndef CIX_JXL_BRIDGE_H
#define CIX_JXL_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#  if defined(CIX_JXL_BRIDGE_BUILDING)
#    define CIX_JXL_API __declspec(dllexport)
#  else
#    define CIX_JXL_API __declspec(dllimport)
#  endif
#else
#  define CIX_JXL_API __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

/**
 * @brief Status returned by every bridge operation.
 *
 * INVALID_ARGUMENT reports a caller-side contract violation; UNSUPPORTED
 * reports an input or requested layout that this bounded bridge does not
 * accept; OUTPUT_LIMIT reports an explicit caller-supplied output bound.
 * ENCODE_FAILURE and DECODE_FAILURE report codec processing failure. Allocation
 * and unexpected C++ exceptions have separate status values for callers that
 * retain their own diagnostics.
 */
enum cix_jxl_status {
    CIX_JXL_OK = 0,
    CIX_JXL_INVALID_ARGUMENT = 1,
    CIX_JXL_UNSUPPORTED = 2,
    CIX_JXL_OUTPUT_LIMIT = 3,
    CIX_JXL_ENCODE_FAILURE = 4,
    CIX_JXL_DECODE_FAILURE = 5,
    CIX_JXL_ALLOCATION_FAILURE = 6,
    CIX_JXL_EXCEPTION = 7
};

/** @brief Sample storage selected by bits_per_sample: UINT8 or little-endian UINT16. */
enum cix_jxl_sample_type {
    CIX_JXL_UINT8 = 1,
    CIX_JXL_UINT16 = 2
};

/**
 * @brief Caller-owned source for one grayscale still image.
 *
 * pixels holds width * height samples in row-major order. pixels_size must
 * equal that byte count exactly; UINT16 samples are little-endian. effort is
 * an encoder setting in the inclusive range 1..10.
 */
struct cix_jxl_gray_image {
    const uint8_t *pixels;
    size_t pixels_size;
    uint32_t width;
    uint32_t height;
    uint32_t bits_per_sample;
    uint32_t sample_type;
    uint32_t effort;
};

/**
 * @brief Bridge-owned byte result.
 *
 * A successful encoder or exact LZMA decoder allocates data. Release it with
 * cix_jxl_free_buffer; the function is null-safe and clears this structure.
 * Callers should initialize an output structure before use: early invalid-
 * argument returns are permitted to leave it unchanged.
 */
struct cix_jxl_buffer {
    uint8_t *data;
    size_t size;
};

/** @brief Bounded grayscale metadata obtained without installing pixel output. */
struct cix_jxl_gray_info {
    uint32_t width;
    uint32_t height;
    uint32_t bits_per_sample;
    uint32_t sample_type;
};

/**
 * @brief Caller-owned contiguous planar still-image source.
 *
 * The layout is depth row-major planes of equal width and height. Plane zero
 * is grayscale; following planes are JXL optional channels in increasing
 * channel-index order. pixels_size must equal depth * width * height * sample
 * bytes exactly. UINT16 samples are little-endian and effort is 1..10.
 */
struct cix_jxl_planar_image {
    const uint8_t *pixels;
    size_t pixels_size;
    uint32_t depth;
    uint32_t width;
    uint32_t height;
    uint32_t bits_per_sample;
    uint32_t sample_type;
    uint32_t effort;
};

struct cix_jxl_planar_info {
    uint32_t depth;
    uint32_t width;
    uint32_t height;
    uint32_t bits_per_sample;
    uint32_t sample_type;
};

/**
 * @brief Encode one lossless, non-animated grayscale still image.
 *
 * The bridge fixes lossless mode and frame distance to zero. max_output_bytes
 * is a hard cap on the returned JXL codestream. On success output is owned by
 * the bridge and must be released with cix_jxl_free_buffer.
 */
CIX_JXL_API int cix_jxl_encode_gray_2d(
    const struct cix_jxl_gray_image *image,
    size_t max_output_bytes,
    struct cix_jxl_buffer *output
);

/**
 * @brief Inspect one grayscale still without allocating or decoding pixels.
 *
 * expected_pixels comes from the enclosing CIX frame and is checked from JXL
 * basic information before the bridge supplies an image-output buffer. info is
 * cleared before processing and is populated only on CIX_JXL_OK.
 */
CIX_JXL_API int cix_jxl_probe_gray_2d(
    const uint8_t *input,
    size_t input_size,
    size_t expected_pixels,
    struct cix_jxl_gray_info *info
);

/**
 * @brief Decode a grayscale still into caller-owned storage.
 *
 * Geometry, sample width and the exact output_size are checked from basic
 * information before libjxl receives output. The output layout is row-major
 * UINT8 or little-endian UINT16 samples as selected by expected_sample_type.
 */
CIX_JXL_API int cix_jxl_decode_gray_2d(
    const uint8_t *input,
    size_t input_size,
    uint32_t expected_width,
    uint32_t expected_height,
    uint32_t expected_bits_per_sample,
    uint32_t expected_sample_type,
    uint8_t *output,
    size_t output_size
);

/**
 * @brief Encode bounded lossless gray-plus-optional-channel planar data.
 *
 * This preserves the cix_jxl_planar_image plane order. max_output_bytes is a
 * hard codestream cap; successful output is bridge-owned.
 */
CIX_JXL_API int cix_jxl_encode_planar(
    const struct cix_jxl_planar_image *image,
    size_t max_output_bytes,
    struct cix_jxl_buffer *output
);
/**
 * @brief Inspect planar still metadata without installing output planes.
 *
 * expected_depth and expected_pixels are CIX framing limits. The reported
 * plane order is the input order described by cix_jxl_planar_image.
 */
CIX_JXL_API int cix_jxl_probe_planar(
    const uint8_t *input,
    size_t input_size,
    uint32_t expected_depth,
    size_t expected_pixels,
    struct cix_jxl_planar_info *info
);
/**
 * @brief Decode planar data into exact, caller-owned contiguous planes.
 *
 * The bridge validates depth, dimensions, precision, optional-channel layout,
 * and output_size before it installs any decoder plane buffer.
 */
CIX_JXL_API int cix_jxl_decode_planar(
    const uint8_t *input,
    size_t input_size,
    uint32_t expected_depth,
    uint32_t expected_width,
    uint32_t expected_height,
    uint32_t expected_bits_per_sample,
    uint32_t expected_sample_type,
    uint8_t *output,
    size_t output_size
);

/**
 * @brief Encode an XZ stream with LZMA preset 9 and CRC64 integrity check.
 *
 * max_output_bytes bounds the complete container. Successful output follows
 * cix_jxl_buffer ownership rules.
 */
CIX_JXL_API int cix_lzma_compress_preset9(
    const uint8_t *input,
    size_t input_size,
    size_t max_output_bytes,
    struct cix_jxl_buffer *output
);
/**
 * @brief Decode one complete XZ stream to an exact caller-declared byte count.
 *
 * The decoder rejects trailing or unconsumed input and a produced length other
 * than expected_output_bytes. memory_bytes bounds input, output, and the
 * decoder's internal memory allowance together.
 */
CIX_JXL_API int cix_lzma_decompress_exact(
    const uint8_t *input,
    size_t input_size,
    size_t expected_output_bytes,
    size_t memory_bytes,
    struct cix_jxl_buffer *output
);

/** @brief Release bridge-owned output; accepts NULL and always clears the buffer. */
CIX_JXL_API void cix_jxl_free_buffer(struct cix_jxl_buffer *buffer);
/** @brief Return the libjxl encoder version encoded by libjxl itself. */
CIX_JXL_API uint32_t cix_jxl_library_version(void);
/** @brief Return this bridge's immutable implementation identifier. */
CIX_JXL_API const char *cix_jxl_bridge_version(void);

#ifdef __cplusplus
}
#endif
#endif
