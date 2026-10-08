// SPDX-License-Identifier: MIT
#include <cix_stream.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* This is deliberately an installed-SDK consumer: it uses only public C
 * headers and the CIX::native CMake target.  It feeds both directions in
 * small pieces and proves exact byte restoration. */

static int append(uint8_t **data, size_t *length, size_t *capacity,
                  const uint8_t *part, size_t part_length) {
    if (part_length == 0) return 1;
    if (part_length > SIZE_MAX - *length) return 0;
    size_t needed = *length + part_length;
    if (needed > *capacity) {
        size_t next = *capacity ? *capacity : 128;
        while (next < needed) {
            if (next > SIZE_MAX / 2) { next = needed; break; }
            next *= 2;
        }
        uint8_t *grown = (uint8_t *)realloc(*data, next);
        if (!grown) return 0;
        *data = grown;
        *capacity = next;
    }
    memcpy(*data + *length, part, part_length);
    *length = needed;
    return 1;
}

static int encode_fragmented(const uint8_t *source, size_t source_length,
                             uint8_t **archive, size_t *archive_length) {
    cix_options_v1 options;
    cix_stream_encoder *stream = NULL;
    size_t offset = 0, capacity = 0;
    uint8_t *written = NULL;
    int result = 0;
    *archive = NULL;
    *archive_length = 0;

    if (cix_options_v1_default(&options) != CIX_STATUS_OK ||
        cix_stream_encoder_create(&options, &stream) != CIX_STATUS_OK) goto done;
    for (size_t steps = 0; offset < source_length && steps < 100000; ++steps) {
        uint8_t output[1024];
        cix_stream_result_v1 progress;
        size_t offered = source_length - offset;
        if (offered > 13) offered = 13;
        if (cix_stream_encoder_process(stream, source + offset, offered,
                output, sizeof(output), &progress) != CIX_STATUS_OK ||
            progress.consumed > offered || progress.produced > sizeof(output) ||
            !append(&written, archive_length, &capacity, output, progress.produced) ||
            (progress.consumed == 0 && progress.produced == 0)) goto done;
        offset += progress.consumed;
    }
    if (offset != source_length) goto done;
    for (size_t steps = 0; steps < 100000; ++steps) {
        uint8_t output[1024];
        cix_stream_result_v1 progress;
        if (cix_stream_encoder_finish(stream, output, sizeof(output), &progress) != CIX_STATUS_OK ||
            progress.produced > sizeof(output) ||
            !append(&written, archive_length, &capacity, output, progress.produced)) goto done;
        if (progress.state == CIX_STREAM_FINISHED) { result = 1; break; }
        if (progress.produced == 0) goto done;
    }
done:
    cix_stream_encoder_destroy(stream);
    if (!result) { free(written); return 0; }
    *archive = written;
    return 1;
}

static int decode_fragmented(const uint8_t *archive, size_t archive_length,
                             uint8_t **restored, size_t *restored_length) {
    cix_options_v1 options;
    cix_stream_decoder *stream = NULL;
    size_t offset = 0, capacity = 0;
    uint8_t *written = NULL;
    int result = 0;
    *restored = NULL;
    *restored_length = 0;

    if (cix_options_v1_default(&options) != CIX_STATUS_OK ||
        cix_stream_decoder_create(&options, &stream) != CIX_STATUS_OK) goto done;
    for (size_t steps = 0; offset < archive_length && steps < 100000; ++steps) {
        uint8_t output[1024];
        cix_stream_result_v1 progress;
        size_t offered = archive_length - offset;
        if (offered > 11) offered = 11;
        if (cix_stream_decoder_process(stream, archive + offset, offered,
                output, sizeof(output), &progress) != CIX_STATUS_OK ||
            progress.consumed > offered || progress.produced > sizeof(output) ||
            !append(&written, restored_length, &capacity, output, progress.produced) ||
            (progress.consumed == 0 && progress.produced == 0)) goto done;
        offset += progress.consumed;
    }
    if (offset != archive_length) goto done;
    for (size_t steps = 0; steps < 100000; ++steps) {
        uint8_t output[1024];
        cix_stream_result_v1 progress;
        if (cix_stream_decoder_finish(stream, output, sizeof(output), &progress) != CIX_STATUS_OK ||
            progress.produced > sizeof(output) ||
            !append(&written, restored_length, &capacity, output, progress.produced)) goto done;
        if (progress.state == CIX_STREAM_FINISHED) { result = 1; break; }
        if (progress.produced == 0) goto done;
    }
done:
    cix_stream_decoder_destroy(stream);
    if (!result) { free(written); return 0; }
    *restored = written;
    return 1;
}

int main(void) {
    uint8_t source[4099];
    uint8_t *archive = NULL;
    uint8_t *restored = NULL;
    size_t archive_length = 0, restored_length = 0;
    for (size_t index = 0; index < sizeof(source); ++index)
        source[index] = (uint8_t)((index * 31u) ^ (index >> 3));

    int ok = encode_fragmented(source, sizeof(source), &archive, &archive_length) &&
             decode_fragmented(archive, archive_length, &restored, &restored_length) &&
             restored_length == sizeof(source) &&
             memcmp(restored, source, sizeof(source)) == 0;
    free(restored);
    free(archive);
    return ok ? 0 : 1;
}
