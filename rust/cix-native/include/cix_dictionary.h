#ifndef CIX_DICTIONARY_H
#define CIX_DICTIONARY_H

#include "cix.h"

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define CIX_DICTIONARY_ABI_V1 1u

typedef struct cix_zstd_dictionary cix_zstd_dictionary;

/** Append-only options structure. level is a standard Zstd compression level;
 * reserved must be zero. output_limit and memory_limit are nonzero.
 * memory_limit covers the live opaque handle, temporary native state, and
 * output while the caller's destination buffer is live. */
typedef struct cix_dictionary_options_v1 {
    uint32_t abi_version;
    uint32_t struct_size;
    int32_t level;
    uint32_t reserved;
    uint64_t output_limit;
    uint64_t memory_limit;
} cix_dictionary_options_v1;

/** The SHA-256 identifies the complete dictionary. It is required at decode,
 * including when an ordinary Zstd frame has dictionary ID 0. */
typedef struct cix_dictionary_identity_v1 {
    uint32_t zstd_id;
    uint8_t sha256[32];
} cix_dictionary_identity_v1;

/** Import a dictionary into an independent owned handle. memory_limit must
 * cover the source bytes and the retained copy. */
cix_status cix_zstd_dictionary_create_v1(const uint8_t *bytes, size_t bytes_len,
                                         uint64_t memory_limit,
                                         cix_zstd_dictionary **out_handle);

/** Train an owned dictionary from exactly concatenated samples. sample_sizes
 * must contain sample_count entries whose checked sum is samples_len.
 * dictionary_capacity and memory_limit bound the native training operation. */
cix_status cix_zstd_dictionary_train_v1(const uint8_t *samples, size_t samples_len,
                                        const size_t *sample_sizes, size_t sample_count,
                                        size_t dictionary_capacity, uint64_t memory_limit,
                                        cix_zstd_dictionary **out_handle);

/** A NULL handle is accepted. Do not free a handle concurrently with any other
 * operation on it, and free each non-NULL returned handle once. */
void cix_zstd_dictionary_free(cix_zstd_dictionary *handle);

/** Return the stable Zstd ID and SHA-256 identity for an owned dictionary. */
cix_status cix_zstd_dictionary_identity_v1(const cix_zstd_dictionary *handle,
                                            cix_dictionary_identity_v1 *out_identity);

/** Export exact dictionary bytes for later import in another process. On
 * CIX_STATUS_OUTPUT_TOO_SMALL, *written is the required count and output is
 * unchanged. */
cix_status cix_zstd_dictionary_bytes_v1(const cix_zstd_dictionary *handle,
                                        uint8_t *output, size_t output_capacity,
                                        size_t *written);

/** Encode/decode one ordinary Zstd frame at options->level. On CIX_STATUS_OUTPUT_TOO_SMALL,
 * *written receives the required capacity and output is unchanged. Decode
 * rejects trailing or concatenated frames and requires declared_identity to
 * match the supplied handle exactly. */
cix_status cix_zstd_dictionary_encode_v1(const cix_zstd_dictionary *handle,
                                         const cix_dictionary_options_v1 *options,
                                         const uint8_t *input, size_t input_len,
                                         uint8_t *output, size_t output_capacity,
                                         size_t *written);
cix_status cix_zstd_dictionary_decode_v1(const cix_zstd_dictionary *handle,
                                         const cix_dictionary_options_v1 *options,
                                         const cix_dictionary_identity_v1 *declared_identity,
                                         const uint8_t *input, size_t input_len,
                                         uint8_t *output, size_t output_capacity,
                                         size_t *written);

/** Caller contract: every non-empty input/output range is live and correctly
 * sized; all argument objects and byte ranges are pairwise disjoint; and a
 * handle is one returned by this API. NULL is allowed only for zero-length byte
 * ranges or cix_zstd_dictionary_free. No CIX allocation crosses this ABI. */

#ifdef __cplusplus
}
#endif

#endif
