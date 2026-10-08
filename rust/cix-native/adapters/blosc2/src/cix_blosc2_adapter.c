// SPDX-License-Identifier: MIT
#include "cix_blosc2_adapter.h"

#include <limits.h>
#include <stddef.h>
#include <stdint.h>

#include "cix.h"

#ifndef CIX_BLOSC2_EXPERIMENTAL_CODEC_ID
#error "CIX_BLOSC2_EXPERIMENTAL_CODEC_ID must be supplied by the installed-SDK build"
#endif
#ifndef CIX_BLOSC2_MAX_CHUNK_BYTES
#error "CIX_BLOSC2_MAX_CHUNK_BYTES must be supplied by the installed-SDK build"
#endif

#if CIX_BLOSC2_EXPERIMENTAL_CODEC_ID < 160 || CIX_BLOSC2_EXPERIMENTAL_CODEC_ID > 255
#error "CIX_BLOSC2_EXPERIMENTAL_CODEC_ID must be in Blosc2's user range 160..255"
#endif
#if CIX_BLOSC2_MAX_CHUNK_BYTES < 1 || CIX_BLOSC2_MAX_CHUNK_BYTES > 134217728
#error "CIX_BLOSC2_MAX_CHUNK_BYTES must not exceed CIX's 128 MiB native admission"
#endif

/* CIX's memory limit is an admission contract, not a process-RSS hard cap. */
#define CIX_BLOSC2_CONTEXT_HEADROOM (UINT64_C(64) * UINT64_C(1024) * UINT64_C(1024))
#define CIX_BLOSC2_ARCHIVE_SLACK (UINT64_C(2) * UINT64_C(1024) * UINT64_C(1024))

static char cix_blosc2_codec_name[] = "cix-native-v1";

static int cix_blosc2_profile(uint8_t clevel, uint32_t *profile) {
    if (profile == NULL || clevel > 9) {
        return BLOSC2_ERROR_CODEC_PARAM;
    }
    if (clevel <= 3) {
        *profile = CIX_PROFILE_FAST;
    } else if (clevel <= 6) {
        *profile = CIX_PROFILE_DEFAULT;
    } else {
        *profile = CIX_PROFILE_BEST;
    }
    return 0;
}

static int cix_blosc2_memory_limit(int32_t input_len, int32_t output_len,
                                   uint64_t *memory_limit) {
    uint64_t required;

    if (memory_limit == NULL || input_len < 0 || output_len < 0) {
        return BLOSC2_ERROR_INVALID_PARAM;
    }
    required = (uint64_t)(uint32_t)input_len + (uint64_t)(uint32_t)output_len;
    if (required > UINT64_MAX - CIX_BLOSC2_CONTEXT_HEADROOM) {
        return BLOSC2_ERROR_MEMORY_ALLOC;
    }
    *memory_limit = required + CIX_BLOSC2_CONTEXT_HEADROOM;
    return 0;
}

static int cix_blosc2_archive_limit(int32_t input_len, size_t *archive_limit) {
    uint64_t required;

    if (archive_limit == NULL || input_len < 0) {
        return BLOSC2_ERROR_INVALID_PARAM;
    }
    required = (uint64_t)(uint32_t)input_len;
    if (required > UINT64_MAX - CIX_BLOSC2_ARCHIVE_SLACK) {
        return BLOSC2_ERROR_MEMORY_ALLOC;
    }
    required += CIX_BLOSC2_ARCHIVE_SLACK;
    if (required > SIZE_MAX) {
        return BLOSC2_ERROR_MEMORY_ALLOC;
    }
    *archive_limit = (size_t)required;
    return 0;
}

static int cix_blosc2_options(int32_t input_len, int32_t output_len,
                               uint32_t profile, cix_options_v1 *options) {
    cix_status status;
    uint64_t memory_limit;
    int result;

    if (options == NULL || output_len < 0) {
        return BLOSC2_ERROR_INVALID_PARAM;
    }
    result = cix_blosc2_memory_limit(input_len, output_len, &memory_limit);
    if (result != 0) {
        return result;
    }
    status = cix_options_v1_default(options);
    if (status != CIX_STATUS_OK) {
        return BLOSC2_ERROR_FAILURE;
    }
    options->profile = profile;
    options->workers = 1;
    options->output_limit = (uint64_t)(uint32_t)output_len;
    options->memory_limit = memory_limit;
    return 0;
}

static int cix_blosc2_encode(const uint8_t *input, int32_t input_len,
                              uint8_t *output, int32_t output_len, uint8_t meta,
                              blosc2_cparams *cparams, const void *chunk) {
    cix_context *context = NULL;
    cix_options_v1 options;
    cix_status status;
    size_t written = 0;
    size_t archive_limit;
    uint32_t profile;
    int result;

    (void)chunk;
    if (meta != CIX_BLOSC2_ADAPTER_METADATA_V1 || cparams == NULL ||
            input_len < 0 || input_len > CIX_BLOSC2_MAX_CHUNK_BYTES || output_len < 0 ||
            (input_len != 0 && input == NULL) ||
            (output_len != 0 && output == NULL)) {
        return BLOSC2_ERROR_CODEC_PARAM;
    }
    result = cix_blosc2_profile(cparams->clevel, &profile);
    if (result != 0) {
        return result;
    }
    result = cix_blosc2_archive_limit(input_len, &archive_limit);
    if (result != 0) {
        return result;
    }
    /*
     * The native archive bound must exceed the host block buffer so CIX can
     * complete and report OUTPUT_TOO_SMALL. CIX still receives output_len as
     * the physical caller buffer capacity below and cannot write past it.
     */
    result = cix_blosc2_options(input_len, (int32_t)archive_limit, profile, &options);
    if (result != 0) {
        return result;
    }
    status = cix_context_create(&options, &context);
    if (status != CIX_STATUS_OK || context == NULL) {
        return BLOSC2_ERROR_MEMORY_ALLOC;
    }
    status = cix_encode_buffer(context, input, (size_t)input_len, output,
                               (size_t)output_len, &written);
    cix_context_destroy(context);

    if (status == CIX_STATUS_OK && written != 0 &&
            written < (size_t)input_len && written <= (size_t)INT32_MAX) {
        return (int)written;
    }
    if (status == CIX_STATUS_OUTPUT_TOO_SMALL || status == CIX_STATUS_OK) {
        /* Upstream's codec callback convention: zero asks Blosc2 to copy raw. */
        return 0;
    }
    return BLOSC2_ERROR_FAILURE;
}

static int cix_blosc2_decode(const uint8_t *input, int32_t input_len,
                              uint8_t *output, int32_t output_len, uint8_t meta,
                              blosc2_dparams *dparams, const void *chunk) {
    cix_context *context = NULL;
    cix_options_v1 options;
    cix_status status;
    size_t written = 0;
    int result;

    (void)dparams;
    (void)chunk;
    if (meta != CIX_BLOSC2_ADAPTER_METADATA_V1 || input_len < 0 ||
            input_len > CIX_BLOSC2_MAX_CHUNK_BYTES || output_len < 0 ||
            (input_len != 0 && input == NULL) || (output_len != 0 && output == NULL)) {
        return BLOSC2_ERROR_CODEC_PARAM;
    }
    if (output_len > CIX_BLOSC2_MAX_CHUNK_BYTES) {
        return BLOSC2_ERROR_INVALID_PARAM;
    }
    result = cix_blosc2_options(input_len, output_len, CIX_PROFILE_DEFAULT, &options);
    if (result != 0) {
        return result;
    }
    status = cix_context_create(&options, &context);
    if (status != CIX_STATUS_OK || context == NULL) {
        return BLOSC2_ERROR_MEMORY_ALLOC;
    }
    status = cix_decode_buffer(context, input, (size_t)input_len, output,
                               (size_t)output_len, &written);
    cix_context_destroy(context);
    if (status == CIX_STATUS_OK && written == (size_t)output_len) {
        return output_len;
    }
    if (status == CIX_STATUS_OUTPUT_TOO_SMALL) {
        return BLOSC2_ERROR_WRITE_BUFFER;
    }
    return BLOSC2_ERROR_DATA;
}

static blosc2_codec cix_blosc2_codec = {
    .compcode = CIX_BLOSC2_EXPERIMENTAL_CODEC_ID,
    .compname = cix_blosc2_codec_name,
    .complib = BLOSC_UDCODEC_LIB,
    .version = CIX_BLOSC2_ADAPTER_CODEC_VERSION,
    .encoder = cix_blosc2_encode,
    .decoder = cix_blosc2_decode,
};

int cix_blosc2_register(void) {
    return blosc2_register_codec(&cix_blosc2_codec);
}
