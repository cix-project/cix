// SPDX-License-Identifier: MIT
#include "cix_squashfs_profile.h"

#include <stdint.h>
#include <string.h>

static int equal(const uint8_t *left, const uint8_t *right, size_t length) {
    return length == 0 || memcmp(left, right, length) == 0;
}

static int roundtrip(enum cix_squashfs_profile_kind kind,
                     const uint8_t *input, size_t length) {
    uint8_t packed[131072];
    uint8_t output[131072];
    size_t packed_length = 0;
    size_t output_length = 0;
    int status = cix_squashfs_profile_encode(kind, input, length, packed,
                                             sizeof(packed), &packed_length);

    if (status == CIX_SQUASHFS_NO_BENEFIT) {
        return 1;
    }
    return status == CIX_SQUASHFS_OK &&
        cix_squashfs_profile_decode(kind, packed, packed_length, output,
                                    sizeof(output), &output_length) == CIX_SQUASHFS_OK &&
        output_length == length && equal(input, output, length);
}

/* Hand-maintained CIX/native agreement vectors. The adaptive byte is zero:
 * its native coder-4 payload is [1,0,8,10,0,0x40]. */
static const uint8_t raw_abc[] = {1, 0, 3, 0, 0, 0, 3, 0, 0, 0, 'a', 'b', 'c'};
static const uint8_t rle_aaa[] = {1, 1, 3, 0, 0, 0, 2, 0, 0, 0, 'A', 3};
static const uint8_t adaptive_zero[] = {
    1, 2, 1, 0, 0, 0, 16, 0, 0, 0, 1, 4, 1, 0, 0, 0, 6, 0, 0, 0,
    1, 0, 8, 10, 0, 0x40
};
/* Declares UINT32_MAX arithmetic bits but supplies no arithmetic bytes. */
static const uint8_t adaptive_huge_bits[] = {
    1, 2, 0, 0, 0, 0, 18, 0, 0, 0, 1, 4, 0, 0, 0, 0, 8, 0, 0, 0,
    1, 0, 8, 0xff, 0xff, 0xff, 0xff, 0x0f
};

int main(void) {
    uint8_t output[32];
    uint8_t repeated[4096];
    uint8_t overlapping[sizeof(raw_abc)];
    union {
        uint32_t aligned;
        uint8_t bytes[4096];
    } workspace;
    size_t written = 0;
    unsigned index;

    if (cix_squashfs_profile_decode(CIX_SQUASHFS_DATA, raw_abc, sizeof(raw_abc),
                                    output, sizeof(output), &written) != CIX_SQUASHFS_OK ||
        written != 3 || !equal(output, (const uint8_t *)"abc", 3)) {
        return 1;
    }
    if (cix_squashfs_profile_decode_workspace_size() > sizeof(workspace.bytes) ||
        cix_squashfs_profile_decode_with_workspace(CIX_SQUASHFS_DATA, raw_abc,
            sizeof(raw_abc), output, sizeof(output), workspace.bytes,
            sizeof(workspace.bytes), &written) != CIX_SQUASHFS_OK ||
        written != 3 || !equal(output, (const uint8_t *)"abc", 3)) {
        return 13;
    }
    if (cix_squashfs_profile_decode_with_workspace(CIX_SQUASHFS_DATA,
            adaptive_zero, sizeof(adaptive_zero), output, sizeof(output),
            workspace.bytes, sizeof(workspace.bytes), &written) != CIX_SQUASHFS_OK ||
        written != 1 || output[0] != 0) {
        return 14;
    }
    if (cix_squashfs_profile_decode_with_workspace(CIX_SQUASHFS_DATA,
            adaptive_zero, sizeof(adaptive_zero), output, sizeof(output), NULL,
            0, &written) != CIX_SQUASHFS_INVALID_ARGUMENT) {
        return 15;
    }
    if (cix_squashfs_profile_decode_with_workspace(CIX_SQUASHFS_DATA,
            adaptive_zero, sizeof(adaptive_zero), output, sizeof(output),
            workspace.bytes, cix_squashfs_profile_decode_workspace_size() - 1,
            &written) != CIX_SQUASHFS_INVALID_ARGUMENT) {
        return 16;
    }
    if (cix_squashfs_profile_decode_with_workspace(CIX_SQUASHFS_DATA,
            raw_abc, sizeof(raw_abc), output, sizeof(output), workspace.bytes + 1,
            sizeof(workspace.bytes) - 1, &written) != CIX_SQUASHFS_INVALID_ARGUMENT) {
        return 17;
    }
    if (cix_squashfs_profile_decode_with_workspace(CIX_SQUASHFS_DATA,
            raw_abc, sizeof(raw_abc), workspace.bytes, sizeof(workspace.bytes),
            workspace.bytes, sizeof(workspace.bytes), &written) != CIX_SQUASHFS_INVALID_ARGUMENT) {
        return 18;
    }
    if (cix_squashfs_profile_decode(CIX_SQUASHFS_DATA, rle_aaa, sizeof(rle_aaa),
                                    output, sizeof(output), &written) != CIX_SQUASHFS_OK ||
        written != 3 || !equal(output, (const uint8_t *)"AAA", 3)) {
        return 2;
    }
    if (cix_squashfs_profile_decode(CIX_SQUASHFS_DATA, adaptive_zero,
                                    sizeof(adaptive_zero), output, sizeof(output),
                                    &written) != CIX_SQUASHFS_OK ||
        written != 1 || output[0] != 0) {
        return 3;
    }
    memset(repeated, 'Z', sizeof(repeated));
    if (!roundtrip(CIX_SQUASHFS_DATA, repeated, sizeof(repeated))) {
        return 4;
    }
    for (index = 0; index < sizeof(repeated); ++index) {
        repeated[index] = (uint8_t)index;
    }
    if (!roundtrip(CIX_SQUASHFS_DATA, repeated, sizeof(repeated))) {
        return 5;
    }
    if (cix_squashfs_profile_decode(CIX_SQUASHFS_DATA, raw_abc, sizeof(raw_abc) - 1,
                                    output, sizeof(output), &written) != CIX_SQUASHFS_MALFORMED) {
        return 6;
    }
    if (cix_squashfs_profile_decode(CIX_SQUASHFS_DATA, raw_abc, sizeof(raw_abc),
                                    output, 2, &written) != CIX_SQUASHFS_OUTPUT_TOO_SMALL) {
        return 7;
    }
    if (cix_squashfs_profile_decode(CIX_SQUASHFS_DATA, adaptive_huge_bits,
                                    sizeof(adaptive_huge_bits), output, sizeof(output),
                                    &written) != CIX_SQUASHFS_MALFORMED) {
        return 8;
    }
    if (cix_squashfs_profile_encode((enum cix_squashfs_profile_kind)99, NULL, 0,
                                    NULL, 0, &written) != CIX_SQUASHFS_INVALID_ARGUMENT) {
        return 9;
    }
    if (cix_squashfs_profile_decode((enum cix_squashfs_profile_kind)99, NULL, 0,
                                    NULL, 0, &written) != CIX_SQUASHFS_INVALID_ARGUMENT) {
        return 10;
    }
    if (cix_squashfs_profile_encode(CIX_SQUASHFS_DATA, repeated, sizeof(repeated),
                                    repeated, sizeof(repeated),
                                    &written) != CIX_SQUASHFS_INVALID_ARGUMENT) {
        return 11;
    }
    memcpy(overlapping, raw_abc, sizeof(overlapping));
    if (cix_squashfs_profile_decode(CIX_SQUASHFS_DATA, overlapping, sizeof(overlapping),
                                    overlapping, sizeof(overlapping),
                                    &written) != CIX_SQUASHFS_INVALID_ARGUMENT) {
        return 12;
    }
    return 0;
}
