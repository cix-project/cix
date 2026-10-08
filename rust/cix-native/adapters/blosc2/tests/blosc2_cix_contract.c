// SPDX-License-Identifier: MIT
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <blosc2.h>

#include "cix_blosc2_adapter.h"

#ifndef CIX_BLOSC2_TEST_CODEC_ID
#error "The contract target must define CIX_BLOSC2_TEST_CODEC_ID"
#endif

int main(void) {
    enum { input_size = 1024 * 1024, compressed_capacity = input_size + BLOSC2_MAX_OVERHEAD };
    static uint8_t input[input_size];
    static uint8_t incompressible[input_size];
    static uint8_t compressed[compressed_capacity];
    static uint8_t restored[input_size];
    blosc2_cparams cparams = BLOSC2_CPARAMS_DEFAULTS;
    blosc2_dparams dparams = BLOSC2_DPARAMS_DEFAULTS;
    blosc2_context *encoder;
    blosc2_context *decoder;
    blosc2_context *invalid_encoder;
    int compressed_size;
    int restored_size;
    int registration;
    uint32_t state = UINT32_C(0x9e3779b9);
    size_t index;

    for (index = 0; index < sizeof(input); ++index) {
        input[index] = (uint8_t)("CIX Blosc2 adapter contract payload "[index % 35]);
        state = state * UINT32_C(1664525) + UINT32_C(1013904223);
        incompressible[index] = (uint8_t)(state >> 24);
    }
    blosc2_init();
    registration = cix_blosc2_register();
    if (registration != 0) {
        blosc2_destroy();
        return 1;
    }
    cparams.typesize = 1;
    cparams.clevel = 5;
    cparams.compcode = CIX_BLOSC2_TEST_CODEC_ID;
    cparams.compcode_meta = CIX_BLOSC2_ADAPTER_METADATA_V1;
    encoder = blosc2_create_cctx(cparams);
    decoder = blosc2_create_dctx(dparams);
    if (encoder == NULL || decoder == NULL) {
        blosc2_free_ctx(encoder);
        blosc2_free_ctx(decoder);
        blosc2_destroy();
        return 2;
    }
    compressed_size = blosc2_compress_ctx(encoder, input, sizeof(input), compressed,
                                          sizeof(compressed));
    if (compressed_size <= 0) {
        blosc2_free_ctx(encoder);
        blosc2_free_ctx(decoder);
        blosc2_destroy();
        return 3;
    }
    if (strcmp(blosc2_cbuffer_complib(compressed), "cix-native-v1") != 0) {
        blosc2_free_ctx(encoder);
        blosc2_free_ctx(decoder);
        blosc2_destroy();
        return 4;
    }
    restored_size = blosc2_decompress_ctx(decoder, compressed, compressed_size, restored,
                                          sizeof(restored));
    if (restored_size != input_size || memcmp(input, restored, sizeof(input)) != 0) {
        fprintf(stderr, "CIX Blosc2 adapter did not restore the original host buffer\n");
        blosc2_free_ctx(encoder);
        blosc2_free_ctx(decoder);
        blosc2_destroy();
        return 5;
    }
    compressed[compressed_size / 2] ^= 1;
    if (blosc2_decompress_ctx(decoder, compressed, compressed_size, restored,
                              sizeof(restored)) >= 0) {
        blosc2_free_ctx(encoder);
        blosc2_free_ctx(decoder);
        blosc2_destroy();
        return 6;
    }
    compressed_size = blosc2_compress_ctx(encoder, incompressible, sizeof(incompressible),
                                          compressed, sizeof(compressed));
    if (compressed_size <= 0) {
        blosc2_free_ctx(encoder);
        blosc2_free_ctx(decoder);
        blosc2_destroy();
        return 7;
    }
    restored_size = blosc2_decompress_ctx(decoder, compressed, compressed_size, restored,
                                          sizeof(restored));
    if (restored_size != input_size ||
            memcmp(incompressible, restored, sizeof(incompressible)) != 0) {
        blosc2_free_ctx(encoder);
        blosc2_free_ctx(decoder);
        blosc2_destroy();
        return 8;
    }
    cparams.compcode_meta = 0;
    invalid_encoder = blosc2_create_cctx(cparams);
    if (invalid_encoder == NULL ||
            blosc2_compress_ctx(invalid_encoder, input, sizeof(input), compressed,
                                sizeof(compressed)) >= 0) {
        blosc2_free_ctx(invalid_encoder);
        blosc2_free_ctx(encoder);
        blosc2_free_ctx(decoder);
        blosc2_destroy();
        return 9;
    }
    blosc2_free_ctx(invalid_encoder);
    blosc2_free_ctx(encoder);
    blosc2_free_ctx(decoder);
    blosc2_destroy();
    return 0;
}
