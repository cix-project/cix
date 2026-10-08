#ifndef CIX_FORMATS_H
#define CIX_FORMATS_H
#include "cix.h"
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#define CIX_FORMAT_ABI_V1 1u
/** Wire format selected for a stateless standard-format operation. */
enum cix_standard_format {
    CIX_FORMAT_GZIP=1, CIX_FORMAT_ZLIB=2, CIX_FORMAT_DEFLATE=3,
    CIX_FORMAT_BZIP2=4, CIX_FORMAT_XZ=5, CIX_FORMAT_ZSTD=6,
    CIX_FORMAT_BROTLI=7, CIX_FORMAT_LZ4_FRAME=8, CIX_FORMAT_SNAPPY_FRAMED=9
};
/** Append-only options for a complete-buffer standard-format operation. */
struct cix_format_options_v1 {
    uint32_t abi_version, struct_size, format, level;
    uint64_t output_limit, memory_limit;
};
/** Stateless calls; independent calls may execute concurrently. This is a
 * complete-buffer API (128 MiB input maximum), not an incremental stream.
 * level is 0..9. All buffers/options/result storage must be disjoint; NULL is
 * allowed only for zero-length buffers. Caller retains ownership throughout.
 * memory_limit includes input, caller destination, codec output and estimated
 * native workspace. It is not an OS RSS limit. output_limit bounds the decoded
 * or encoded result. On OUTPUT_TOO_SMALL, written holds required capacity and
 * output is untouched. Other failures set written=0 after argument validation.
 * Status values are the CIX_STATUS_* values in cix.h. */
/** Encode one complete buffer as the selected standard wire format. */
int cix_format_encode_v1(const struct cix_format_options_v1*, const uint8_t*, size_t,
                         uint8_t*, size_t, size_t*);
/** Decode one complete selected standard wire-format buffer. */
int cix_format_decode_v1(const struct cix_format_options_v1*, const uint8_t*, size_t,
                         uint8_t*, size_t, size_t*);
#ifdef __cplusplus
}
#endif
#endif
