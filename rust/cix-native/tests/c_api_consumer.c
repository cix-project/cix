// SPDX-License-Identifier: MIT
#include "cix.h"
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* Build/link this against the produced CIX shared library as an independent
 * C consumer smoke test.  Return zero only after exact byte restoration. */
int main(void) {
    const uint8_t source[] = "CIX C ABI consumer";
    cix_options_v1 options;
    struct options_v2 { cix_options_v1 v1; uint64_t reserved_future; } extended;
    cix_context *context = NULL;
    size_t archive_len = 0, restored_len = 0;
    if (cix_options_v1_default(&options) != CIX_STATUS_OK ||
        cix_context_create(&options, &context) != CIX_STATUS_OK) return 1;
    extended.v1 = options;
    extended.v1.struct_size = (uint32_t)sizeof(extended);
    extended.reserved_future = UINT64_C(0);
    cix_context *trailing_context = NULL;
    if (cix_context_create(&extended.v1, &trailing_context) != CIX_STATUS_OK) return 2;
    cix_context_destroy(trailing_context);
    cix_options_v1 limited_options = options;
    limited_options.output_limit = 1;
    cix_context *limited_context = NULL;
    if (cix_context_create(&limited_options, &limited_context) != CIX_STATUS_OK) return 2;
    size_t limited_needed = 123;
    if (cix_encode_buffer(limited_context, source, sizeof(source), NULL, 0, &limited_needed) != CIX_STATUS_RESOURCE_LIMIT || limited_needed != 0) return 2;
    cix_context_destroy(limited_context);
    if (cix_encode_buffer(context, source, sizeof(source), NULL, 0, &archive_len) != CIX_STATUS_OUTPUT_TOO_SMALL) return 2;
    uint8_t *archive = malloc(archive_len);
    if (!archive || cix_encode_buffer(context, source, sizeof(source), archive, archive_len, &archive_len) != CIX_STATUS_OK) return 3;
    if (cix_decode_buffer(context, archive, archive_len, NULL, 0, &restored_len) != CIX_STATUS_OUTPUT_TOO_SMALL) return 4;
    uint8_t *restored = malloc(restored_len);
    if (!restored || cix_decode_buffer(context, archive, archive_len, restored, restored_len, &restored_len) != CIX_STATUS_OK) return 5;
    int result = restored_len != sizeof(source);
    for (size_t i = 0; !result && i < sizeof(source); ++i) result = restored[i] != source[i];
    uint8_t untouched[4] = { 7, 7, 7, 7 };
    size_t required = 0;
    if (!result && cix_encode_buffer(context, source, sizeof(source), untouched, sizeof(untouched), &required) != CIX_STATUS_OUTPUT_TOO_SMALL) result = 1;
    for (size_t i = 0; !result && i < sizeof(untouched); ++i) result = untouched[i] != 7;
    if (!result && cix_encode_buffer(context, archive, archive_len, archive, archive_len, &required) != CIX_STATUS_INVALID_ARGUMENT) result = 1;
    if (!result && cix_encode_buffer(context, NULL, 1, NULL, 0, &required) != CIX_STATUS_INVALID_ARGUMENT) result = 1;
    char error[128]; size_t error_needed = 0;
    if (!result && cix_context_last_error(context, NULL, 0, &error_needed) != CIX_STATUS_OUTPUT_TOO_SMALL) result = 1;
    if (!result && (error_needed > sizeof(error) || cix_context_last_error(context, error, sizeof(error), &error_needed) != CIX_STATUS_OK || error[error_needed - 1] != '\0')) result = 1;
    free(restored); free(archive); cix_context_destroy(context);
    return result;
}
