/* Minimal lifecycle example for the CIX Zstd dictionary ABI.
 * The package build installs cix_dictionary.h beside cix.h. */
#include "cix_dictionary.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

int main(void) {
    uint8_t samples[64][96];
    size_t sizes[64];
    uint8_t flat[sizeof samples];
    size_t offset = 0;
    cix_zstd_dictionary *dictionary = NULL;
    cix_zstd_dictionary *imported = NULL;
    uint8_t exported[1024];
    size_t exported_len = 0;
    cix_dictionary_identity_v1 identity;
    cix_dictionary_options_v1 options = {
        CIX_DICTIONARY_ABI_V1, sizeof(cix_dictionary_options_v1), 3, 0, 4096, 128u << 20};
    uint8_t encoded[4096];
    uint8_t decoded[4096];
    size_t encoded_len = 0;
    size_t decoded_len = 0;
    const uint8_t message[] = "CIX dictionary example";

    for (size_t row = 0; row < 64; ++row) {
        sizes[row] = sizeof samples[row];
        for (size_t column = 0; column < sizeof samples[row]; ++column) {
            samples[row][column] = (uint8_t)(row * 17u + (column % 13u));
        }
        memcpy(flat + offset, samples[row], sizes[row]);
        offset += sizes[row];
    }
    if (cix_zstd_dictionary_train_v1(flat, sizeof flat, sizes, 64, 1024,
                                     128u << 20, &dictionary) != CIX_STATUS_OK ||
        cix_zstd_dictionary_identity_v1(dictionary, &identity) != CIX_STATUS_OK ||
        cix_zstd_dictionary_bytes_v1(dictionary, exported, sizeof exported, &exported_len) != CIX_STATUS_OK ||
        cix_zstd_dictionary_create_v1(exported, exported_len, 128u << 20, &imported) != CIX_STATUS_OK ||
        cix_zstd_dictionary_encode_v1(dictionary, &options, message, sizeof message,
                                      encoded, sizeof encoded, &encoded_len) != CIX_STATUS_OK) {
        cix_zstd_dictionary_free(imported);
        cix_zstd_dictionary_free(dictionary);
        return 1;
    }
    /* The imported handle owns its copy; neither source needs to remain live. */
    memset(exported, 0, sizeof exported);
    cix_zstd_dictionary_free(dictionary);
    dictionary = NULL;
    if (cix_zstd_dictionary_decode_v1(imported, &options, &identity, encoded, encoded_len,
                                      decoded, sizeof decoded, &decoded_len) != CIX_STATUS_OK ||
        decoded_len != sizeof message || memcmp(decoded, message, sizeof message) != 0) {
        cix_zstd_dictionary_free(imported);
        return 1;
    }
    cix_zstd_dictionary_free(imported);
    return 0;
}
