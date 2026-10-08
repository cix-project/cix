// SPDX-License-Identifier: MIT
#include <cix_formats.h>
#include <string.h>
int main(void) {
    const unsigned char input[] = "installed C standard-format consumer";
    unsigned char encoded[4096], decoded[sizeof input];
    struct cix_format_options_v1 options = { CIX_FORMAT_ABI_V1, sizeof options,
        CIX_FORMAT_GZIP, 6, 4 * 1024 * 1024, 128 * 1024 * 1024 };
    size_t encoded_size = 0, decoded_size = 0;
    if (cix_format_encode_v1(&options, input, sizeof input, encoded,
        sizeof encoded, &encoded_size) != CIX_STATUS_OK) return 1;
    if (encoded_size < 2 || encoded[0] != 0x1f || encoded[1] != 0x8b) return 2;
    if (cix_format_decode_v1(&options, encoded, encoded_size, decoded,
        sizeof decoded, &decoded_size) != CIX_STATUS_OK) return 3;
    return decoded_size == sizeof input && memcmp(input, decoded, sizeof input) == 0 ? 0 : 4;
}
