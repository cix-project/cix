// SPDX-License-Identifier: MIT
#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <wiredtiger.h>

#include "cix.h"

#ifndef CIX_WIREDTIGER_MAX_BLOCK_BYTES
#error "CIX_WIREDTIGER_MAX_BLOCK_BYTES must be supplied by the installed-SDK build"
#endif

#define CIX_WT_NAME "cix-v1-experimental"
#define CIX_WT_FRAME_SIZE 16U
#define CIX_WT_FRAME_VERSION 1U
#define CIX_WT_ARCHIVE_SLACK (UINT64_C(2) * UINT64_C(1024) * UINT64_C(1024))
#define CIX_WT_CONTEXT_HEADROOM (UINT64_C(64) * UINT64_C(1024) * UINT64_C(1024))

typedef struct {
    WT_COMPRESSOR compressor;
} CIX_WT_COMPRESSOR;

static const uint8_t cix_wt_magic[4] = {'C', 'I', 'X', 'W'};

static void cix_wt_put_u64le(uint8_t *out, uint64_t value) {
    size_t index;

    for (index = 0; index != 8; ++index) {
        out[index] = (uint8_t)(value >> (index * 8));
    }
}

static uint64_t cix_wt_get_u64le(const uint8_t *input) {
    size_t index;
    uint64_t value = 0;

    for (index = 0; index != 8; ++index) {
        value |= (uint64_t)input[index] << (index * 8);
    }
    return value;
}

static int cix_wt_memory_limit(size_t input_len, size_t output_len, uint64_t *limit) {
    uint64_t required;

    if (limit == NULL) {
        return EOVERFLOW;
    }
    required = (uint64_t)input_len;
    if (required > UINT64_MAX - (uint64_t)output_len) {
        return EOVERFLOW;
    }
    required += (uint64_t)output_len;
    if (required > UINT64_MAX - CIX_WT_CONTEXT_HEADROOM) {
        return EOVERFLOW;
    }
    *limit = required + CIX_WT_CONTEXT_HEADROOM;
    return 0;
}

static int cix_wt_archive_limit(size_t source_len, size_t *archive_limit) {
    uint64_t limit;

    if (archive_limit == NULL) {
        return EINVAL;
    }
    limit = (uint64_t)source_len;
    if (limit > UINT64_MAX - CIX_WT_ARCHIVE_SLACK) {
        return EOVERFLOW;
    }
    limit += CIX_WT_ARCHIVE_SLACK;
    if (limit > SIZE_MAX) {
        return EOVERFLOW;
    }
    *archive_limit = (size_t)limit;
    return 0;
}

static int cix_wt_context(size_t input_len, size_t output_len, uint32_t profile,
                          cix_context **out_context) {
    cix_options_v1 options;
    cix_status status;
    uint64_t memory_limit;
    int ret;

    if (out_context == NULL) {
        return EOVERFLOW;
    }
    *out_context = NULL;
    ret = cix_wt_memory_limit(input_len, output_len, &memory_limit);
    if (ret != 0) {
        return ret;
    }
    status = cix_options_v1_default(&options);
    if (status != CIX_STATUS_OK) {
        return EIO;
    }
    options.profile = profile;
    options.workers = 1;
    options.output_limit = (uint64_t)output_len;
    options.memory_limit = memory_limit;
    status = cix_context_create(&options, out_context);
    if (status == CIX_STATUS_RESOURCE_LIMIT) {
        return ENOMEM;
    }
    return status == CIX_STATUS_OK && *out_context != NULL ? 0 : EIO;
}

static int cix_wt_pre_size(WT_COMPRESSOR *compressor, WT_SESSION *session,
                           uint8_t *src, size_t src_len, size_t *result_lenp) {
    (void)compressor;
    (void)session;
    (void)src;
    if (result_lenp == NULL || src_len > CIX_WIREDTIGER_MAX_BLOCK_BYTES) {
        return EFBIG;
    }
    /* WiredTiger will give compress at least src_len; larger output is discarded. */
    *result_lenp = src_len;
    return 0;
}

static int cix_wt_compress(WT_COMPRESSOR *compressor, WT_SESSION *session,
                           uint8_t *src, size_t src_len, uint8_t *dst, size_t dst_len,
                           size_t *result_lenp, int *compression_failed) {
    cix_context *context = NULL;
    cix_status status;
    size_t payload_capacity;
    size_t payload_len = 0;
    size_t archive_limit;
    size_t total_len;
    int ret;

    (void)compressor;
    (void)session;
    if (result_lenp == NULL || compression_failed == NULL ||
            (src_len != 0 && src == NULL) || (dst_len != 0 && dst == NULL)) {
        return EINVAL;
    }
    *result_lenp = 0;
    *compression_failed = 1;
    if (src_len > CIX_WIREDTIGER_MAX_BLOCK_BYTES) {
        return EFBIG;
    }
    if (src_len == 0 || dst_len <= CIX_WT_FRAME_SIZE) {
        return 0;
    }
    payload_capacity = dst_len - CIX_WT_FRAME_SIZE;
    ret = cix_wt_archive_limit(src_len, &archive_limit);
    if (ret != 0) {
        return ret;
    }
    /*
     * CIX must be allowed to finish its bounded native archive before the C
     * ABI can classify a short WT destination as OUTPUT_TOO_SMALL. The caller
     * buffer remains payload_capacity, so CIX never writes past it.
     */
    ret = cix_wt_context(src_len, archive_limit, CIX_PROFILE_DEFAULT, &context);
    if (ret != 0) {
        return ret;
    }
    status = cix_encode_buffer(context, src, src_len, dst + CIX_WT_FRAME_SIZE,
                               payload_capacity, &payload_len);
    cix_context_destroy(context);
    if (status == CIX_STATUS_OUTPUT_TOO_SMALL) {
        return 0;
    }
    if (status != CIX_STATUS_OK || payload_len > payload_capacity ||
            payload_len > SIZE_MAX - CIX_WT_FRAME_SIZE) {
        return EIO;
    }
    total_len = CIX_WT_FRAME_SIZE + payload_len;
    if (total_len >= src_len) {
        return 0;
    }
    memcpy(dst, cix_wt_magic, sizeof(cix_wt_magic));
    dst[4] = CIX_WT_FRAME_VERSION;
    dst[5] = 0;
    dst[6] = 0;
    dst[7] = 0;
    cix_wt_put_u64le(dst + 8, (uint64_t)payload_len);
    *result_lenp = total_len;
    *compression_failed = 0;
    return 0;
}

static int cix_wt_decompress(WT_COMPRESSOR *compressor, WT_SESSION *session,
                             uint8_t *src, size_t src_len, uint8_t *dst, size_t dst_len,
                             size_t *result_lenp) {
    cix_context *context = NULL;
    cix_status status;
    uint64_t encoded_payload_len;
    size_t payload_len;
    size_t restored_len = 0;
    int ret;

    (void)compressor;
    (void)session;
    if (result_lenp == NULL || (src_len != 0 && src == NULL) ||
            (dst_len != 0 && dst == NULL)) {
        return EINVAL;
    }
    *result_lenp = 0;
    if (src_len < CIX_WT_FRAME_SIZE || src_len > CIX_WIREDTIGER_MAX_BLOCK_BYTES ||
            memcmp(src, cix_wt_magic, sizeof(cix_wt_magic)) != 0 ||
            src[4] != CIX_WT_FRAME_VERSION || src[5] != 0 || src[6] != 0 || src[7] != 0) {
        return EINVAL;
    }
    encoded_payload_len = cix_wt_get_u64le(src + 8);
    if (encoded_payload_len > SIZE_MAX) {
        return EOVERFLOW;
    }
    payload_len = (size_t)encoded_payload_len;
    /* WT may zero-pad its source buffer; only the paid CIX payload is decoded. */
    if (payload_len == 0 || payload_len > src_len - CIX_WT_FRAME_SIZE ||
            dst_len > CIX_WIREDTIGER_MAX_BLOCK_BYTES) {
        return EINVAL;
    }
    ret = cix_wt_context(payload_len, dst_len, CIX_PROFILE_DEFAULT, &context);
    if (ret != 0) {
        return ret;
    }
    status = cix_decode_buffer(context, src + CIX_WT_FRAME_SIZE, payload_len,
                               dst, dst_len, &restored_len);
    cix_context_destroy(context);
    if (status == CIX_STATUS_OUTPUT_TOO_SMALL) {
        return ENOBUFS;
    }
    if (status != CIX_STATUS_OK || restored_len != dst_len) {
        return EINVAL;
    }
    *result_lenp = restored_len;
    return 0;
}

static int cix_wt_terminate(WT_COMPRESSOR *compressor, WT_SESSION *session) {
    (void)session;
    free(compressor);
    return 0;
}

static void cix_wt_initialize(CIX_WT_COMPRESSOR *compressor) {
    compressor->compressor.compress = cix_wt_compress;
    compressor->compressor.decompress = cix_wt_decompress;
    compressor->compressor.pre_size = cix_wt_pre_size;
    compressor->compressor.terminate = cix_wt_terminate;
}

#ifdef CIX_WT_CONTRACT_TESTING
/* Deliberately absent from the shipping extension build. */
WT_ATTRIBUTE_LIBRARY_VISIBLE WT_COMPRESSOR *cix_wt_contract_compressor(void) {
    static CIX_WT_COMPRESSOR compressor;
    static int initialized;

    if (initialized == 0) {
        cix_wt_initialize(&compressor);
        initialized = 1;
    }
    return &compressor.compressor;
}
#endif

WT_ATTRIBUTE_LIBRARY_VISIBLE int wiredtiger_extension_init(WT_CONNECTION *connection,
                                                            WT_CONFIG_ARG *config) {
    CIX_WT_COMPRESSOR *compressor;
    int ret;

    (void)config;
    if (connection == NULL) {
        return EINVAL;
    }
    compressor = calloc(1, sizeof(*compressor));
    if (compressor == NULL) {
        return ENOMEM;
    }
    cix_wt_initialize(compressor);
    ret = connection->add_compressor(connection, CIX_WT_NAME, &compressor->compressor, NULL);
    if (ret != 0) {
        free(compressor);
    }
    return ret;
}

WT_ATTRIBUTE_LIBRARY_VISIBLE int wiredtiger_extension_terminate(WT_CONNECTION *connection) {
    (void)connection;
    /* WT_COMPRESSOR::terminate owns the registered per-connection allocation. */
    return 0;
}
