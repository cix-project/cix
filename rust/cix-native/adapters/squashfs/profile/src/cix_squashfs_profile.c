// SPDX-License-Identifier: MIT
#include "cix_squashfs_profile.h"

#include <limits.h>
#include <string.h>

#define VERSION 1u
#define METHOD_RAW 0u
#define METHOD_RLE 1u
#define METHOD_ADAPTIVE 2u
#define HEADER 10u
#define SUBSTREAM 10u
#ifndef CIX_SQUASHFS_PROFILE_DECODER_ONLY
#define MAX_BITS_RESERVE 5u
#endif
#define AC_MAX UINT64_C(0xffffffff)
#define AC_HALF UINT64_C(0x80000000)
#define AC_Q1 UINT64_C(0x40000000)
#define AC_Q3 UINT64_C(0xc0000000)

typedef struct {
    uint32_t counts[256];
    uint32_t tree[257];
    uint32_t seen;
} model;

/* Private probe: its value member defines model's required workspace alignment. */
typedef struct {
    char padding;
    model value;
} model_alignment_probe;

#define MODEL_ALIGNMENT offsetof(model_alignment_probe, value)

#ifndef CIX_SQUASHFS_PROFILE_DECODER_ONLY
typedef struct {
    uint8_t *p;
    size_t cap;
    size_t bytes;
    uint8_t cur;
    uint8_t used;
    uint32_t bits;
    int failed;
} bit_writer;
#endif

typedef struct {
    const uint8_t *p;
    size_t bytes;
    uint32_t bits;
    uint64_t pos;
} bit_reader;

#ifndef CIX_SQUASHFS_PROFILE_DECODER_ONLY
typedef struct {
    uint64_t low;
    uint64_t high;
    uint32_t pending;
    bit_writer writer;
} encoder;
#endif

typedef struct {
    uint64_t low;
    uint64_t high;
    uint64_t value;
    bit_reader reader;
} decoder;

static uint32_t le32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) |
        ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

#ifndef CIX_SQUASHFS_PROFILE_DECODER_ONLY
static void put32(uint8_t *p, uint32_t value) {
    p[0] = (uint8_t)value;
    p[1] = (uint8_t)(value >> 8);
    p[2] = (uint8_t)(value >> 16);
    p[3] = (uint8_t)(value >> 24);
}
#endif

size_t cix_squashfs_profile_max_input(enum cix_squashfs_profile_kind kind) {
    switch (kind) {
    case CIX_SQUASHFS_DATA:
        return 131072u;
    case CIX_SQUASHFS_METADATA:
        return 8192u;
    default:
        return 0;
    }
}

static int valid_io(const uint8_t *pointer, size_t length) {
    return length == 0 || pointer != NULL;
}

static int ranges_overlap(const uint8_t *left, size_t left_length,
                          const uint8_t *right, size_t right_length) {
    uintptr_t left_start;
    uintptr_t right_start;

    if (left_length == 0 || right_length == 0) {
        return 0;
    }
    left_start = (uintptr_t)left;
    right_start = (uintptr_t)right;
    if (left_start > UINTPTR_MAX - left_length ||
        right_start > UINTPTR_MAX - right_length) {
        return 1;
    }
    return left_start < right_start + right_length &&
        right_start < left_start + left_length;
}

static void model_add(model *state, unsigned symbol) {
    unsigned index;

    ++state->counts[symbol];
    ++state->seen;
    for (index = symbol + 1; index <= 256; index += index & (~index + 1)) {
        ++state->tree[index];
    }
}

static uint32_t prefix(const model *state, unsigned symbol) {
    uint32_t result = 0;

    while (symbol != 0) {
        result += state->tree[symbol];
        symbol &= symbol - 1;
    }
    return result;
}

static void interval(const model *state, unsigned symbol,
                     uint32_t *low, uint32_t *high, uint32_t *total) {
    *low = 2u * prefix(state, symbol) + symbol;
    *high = *low + 2u * state->counts[symbol] + 1u;
    *total = 2u * state->seen + 256u;
}

static int select_symbol(const model *state, uint32_t target, unsigned *symbol,
                         uint32_t *low, uint32_t *high, uint32_t *total) {
    unsigned first = 0;
    unsigned last = 255;

    while (first < last) {
        unsigned middle = first + (last - first + 1) / 2;
        if (2u * prefix(state, middle) + middle <= target) {
            first = middle;
        } else {
            last = middle - 1;
        }
    }
    interval(state, first, low, high, total);
    if (target < *low || target >= *high) {
        return 0;
    }
    *symbol = first;
    return 1;
}

#ifndef CIX_SQUASHFS_PROFILE_DECODER_ONLY
static void bw_bit(bit_writer *writer, unsigned bit) {
    if (writer->failed != 0) {
        return;
    }
    writer->cur = (uint8_t)((writer->cur << 1) | (bit & 1u));
    if (++writer->used == 8) {
        if (writer->bytes == writer->cap) {
            writer->failed = 1;
            return;
        }
        writer->p[writer->bytes++] = writer->cur;
        writer->used = 0;
        writer->cur = 0;
    }
    if (writer->bits == UINT32_MAX) {
        writer->failed = 1;
    } else {
        ++writer->bits;
    }
}

static void emit(encoder *state, unsigned bit) {
    unsigned index;

    bw_bit(&state->writer, bit);
    for (index = 0; index < state->pending; ++index) {
        bw_bit(&state->writer, !bit);
    }
    state->pending = 0;
}

static void enc_update(encoder *state, uint32_t low, uint32_t high, uint32_t total) {
    uint64_t width = state->high - state->low + 1;

    state->high = state->low + width * high / total - 1;
    state->low += width * low / total;
    for (;;) {
        if (state->high < AC_HALF) {
            emit(state, 0);
        } else if (state->low >= AC_HALF) {
            emit(state, 1);
            state->low -= AC_HALF;
            state->high -= AC_HALF;
        } else if (state->low >= AC_Q1 && state->high < AC_Q3) {
            ++state->pending;
            state->low -= AC_Q1;
            state->high -= AC_Q1;
        } else {
            break;
        }
        state->low <<= 1;
        state->high = (state->high << 1) + 1;
    }
}

static int enc_finish(encoder *state) {
    ++state->pending;
    emit(state, state->low < AC_Q1 ? 0 : 1);
    if (state->writer.used != 0) {
        if (state->writer.bytes == state->writer.cap) {
            state->writer.failed = 1;
        } else {
            state->writer.p[state->writer.bytes++] =
                (uint8_t)(state->writer.cur << (8 - state->writer.used));
        }
    }
    return state->writer.failed == 0;
}
#endif

static unsigned br_bit(bit_reader *reader) {
    unsigned bit = 0;

    if (reader->pos < reader->bits) {
        bit = (reader->p[reader->pos / 8] >> (7 - (reader->pos % 8))) & 1u;
    }
    ++reader->pos;
    return bit;
}

static void dec_init(decoder *state, const uint8_t *payload, size_t bytes, uint32_t bits) {
    unsigned index;

    state->low = 0;
    state->high = AC_MAX;
    state->value = 0;
    state->reader.p = payload;
    state->reader.bytes = bytes;
    state->reader.bits = bits;
    state->reader.pos = 0;
    for (index = 0; index < 32; ++index) {
        state->value = (state->value << 1) | br_bit(&state->reader);
    }
}

static uint32_t dec_target(const decoder *state, uint32_t total) {
    return (uint32_t)(((state->value - state->low + 1) * total - 1) /
                      (state->high - state->low + 1));
}

static void dec_update(decoder *state, uint32_t low, uint32_t high, uint32_t total) {
    uint64_t width = state->high - state->low + 1;

    state->high = state->low + width * high / total - 1;
    state->low += width * low / total;
    for (;;) {
        if (state->high < AC_HALF) {
            /* No offset. */
        } else if (state->low >= AC_HALF) {
            state->low -= AC_HALF;
            state->high -= AC_HALF;
            state->value -= AC_HALF;
        } else if (state->low >= AC_Q1 && state->high < AC_Q3) {
            state->low -= AC_Q1;
            state->high -= AC_Q1;
            state->value -= AC_Q1;
        } else {
            break;
        }
        state->low <<= 1;
        state->high = (state->high << 1) + 1;
        state->value = (state->value << 1) | br_bit(&state->reader);
    }
}

#ifndef CIX_SQUASHFS_PROFILE_DECODER_ONLY
static size_t varint(uint32_t value, uint8_t *output) {
    size_t written = 0;

    while (value >= 128) {
        output[written++] = (uint8_t)(value | 128u);
        value >>= 7;
    }
    output[written++] = (uint8_t)value;
    return written;
}
#endif

static int get_varint(const uint8_t *input, size_t length, size_t *offset,
                      uint32_t *output) {
    uint32_t value = 0;
    unsigned shift = 0;

    while (*offset < length && shift < 35) {
        uint8_t byte = input[(*offset)++];
        if (shift == 28 && (byte & 127u) > 15u) {
            return 0;
        }
        value |= (uint32_t)(byte & 127u) << shift;
        if ((byte & 128u) == 0) {
            *output = value;
            return 1;
        }
        shift += 7;
    }
    return 0;
}

#ifndef CIX_SQUASHFS_PROFILE_DECODER_ONLY
static size_t rle_size(const uint8_t *input, size_t length) {
    size_t offset = 0;
    size_t result = 0;

    while (offset < length) {
        size_t end = offset + 1;
        size_t count;
        size_t varint_bytes = 1;

        while (end < length && input[end] == input[offset]) {
            ++end;
        }
        count = end - offset;
        while (count >= 128) {
            ++varint_bytes;
            count >>= 7;
        }
        result += 1 + varint_bytes;
        offset = end;
    }
    return result;
}

static void rle_write(const uint8_t *input, size_t length, uint8_t *output) {
    size_t input_offset = 0;
    size_t output_offset = 0;

    while (input_offset < length) {
        size_t end = input_offset + 1;
        size_t count;

        while (end < length && input[end] == input[input_offset]) {
            ++end;
        }
        output[output_offset++] = input[input_offset];
        count = end - input_offset;
        while (count >= 128) {
            output[output_offset++] = (uint8_t)(count | 128u);
            count >>= 7;
        }
        output[output_offset++] = (uint8_t)count;
        input_offset = end;
    }
}

static int adaptive_encode(const uint8_t *input, size_t length, uint8_t *output,
                           size_t capacity, size_t *written) {
    model state;
    encoder arithmetic;
    size_t offset = 0;
    size_t bit_length_offset;
    size_t bit_start;
    size_t bit_bytes;
    size_t varint_bytes;
    size_t index;
    uint32_t low;
    uint32_t high;
    uint32_t total;

    memset(&state, 0, sizeof(state));
    if (capacity < SUBSTREAM + 3 + MAX_BITS_RESERVE) {
        return 0;
    }
    output[offset++] = 1;
    output[offset++] = 4;
    put32(output + offset, (uint32_t)length);
    offset += 4;
    offset += 4;
    output[offset++] = 1;
    output[offset++] = 0;
    output[offset++] = 8;
    bit_length_offset = offset;
    bit_start = offset + MAX_BITS_RESERVE;
    arithmetic.low = 0;
    arithmetic.high = AC_MAX;
    arithmetic.pending = 0;
    arithmetic.writer.p = output + bit_start;
    arithmetic.writer.cap = capacity - bit_start;
    arithmetic.writer.bytes = 0;
    arithmetic.writer.cur = 0;
    arithmetic.writer.used = 0;
    arithmetic.writer.bits = 0;
    arithmetic.writer.failed = 0;
    for (index = 0; index < length; ++index) {
        interval(&state, input[index], &low, &high, &total);
        enc_update(&arithmetic, low, high, total);
        model_add(&state, input[index]);
    }
    if (!enc_finish(&arithmetic)) {
        return 0;
    }
    bit_bytes = arithmetic.writer.bytes;
    varint_bytes = varint(arithmetic.writer.bits, output + bit_length_offset);
    memmove(output + bit_length_offset + varint_bytes, output + bit_start, bit_bytes);
    put32(output + 6, (uint32_t)(3 + varint_bytes + bit_bytes));
    *written = SUBSTREAM + 3 + varint_bytes + bit_bytes;
    return 1;
}

int cix_squashfs_profile_encode(enum cix_squashfs_profile_kind kind,
                                const uint8_t *input, size_t input_length,
                                uint8_t *output, size_t output_capacity,
                                size_t *written) {
    size_t limit = cix_squashfs_profile_max_input(kind);
    size_t best = 0;
    size_t rle;
    size_t adaptive = 0;

    if (written != NULL) {
        *written = 0;
    }
    if (written == NULL || limit == 0 || !valid_io(input, input_length) ||
        !valid_io(output, output_capacity) ||
        ranges_overlap(input, input_length, output, output_capacity)) {
        return CIX_SQUASHFS_INVALID_ARGUMENT;
    }
    if (input_length > limit) {
        return CIX_SQUASHFS_LIMIT;
    }
    if (output_capacity < input_length) {
        return CIX_SQUASHFS_OUTPUT_TOO_SMALL;
    }
    if (input_length <= HEADER) {
        return CIX_SQUASHFS_NO_BENEFIT;
    }
    rle = rle_size(input, input_length);
    if (rle + HEADER < input_length) {
        best = rle;
    }
    if (adaptive_encode(input, input_length, output + HEADER,
                        output_capacity - HEADER, &adaptive) &&
        adaptive + HEADER < input_length && (best == 0 || adaptive < best)) {
        best = adaptive;
    }
    if (best == 0) {
        return CIX_SQUASHFS_NO_BENEFIT;
    }
    output[0] = VERSION;
    output[1] = best == rle ? METHOD_RLE : METHOD_ADAPTIVE;
    put32(output + 2, (uint32_t)input_length);
    put32(output + 6, (uint32_t)best);
    if (best == rle) {
        rle_write(input, input_length, output + HEADER);
    }
    *written = HEADER + best;
    return CIX_SQUASHFS_OK;
}
#endif

static int decode_rle(const uint8_t *input, size_t offset, size_t end,
                      uint8_t *output, size_t plain_length) {
    size_t output_offset = 0;

    if (offset == end) {
        return plain_length == 0 ? CIX_SQUASHFS_OK : CIX_SQUASHFS_MALFORMED;
    }
    while (offset < end) {
        uint8_t value;
        uint32_t count;

        if (output_offset >= plain_length) {
            return CIX_SQUASHFS_MALFORMED;
        }
        value = input[offset++];
        if (!get_varint(input, end, &offset, &count) || count == 0 ||
            count > plain_length - output_offset) {
            return CIX_SQUASHFS_MALFORMED;
        }
        memset(output + output_offset, value, count);
        output_offset += count;
    }
    return output_offset == plain_length ? CIX_SQUASHFS_OK : CIX_SQUASHFS_MALFORMED;
}

static int decode_adaptive(const uint8_t *input, size_t offset, size_t end,
                           uint8_t *output, size_t plain_length,
                           model *state) {
    uint32_t substream_length;
    uint32_t adaptive_length;
    uint32_t bits;
    size_t bytes;
    size_t index;
    decoder arithmetic;

    if (end - offset < SUBSTREAM + 4 || input[offset++] != 1 || input[offset++] != 4) {
        return CIX_SQUASHFS_MALFORMED;
    }
    substream_length = le32(input + offset);
    offset += 4;
    adaptive_length = le32(input + offset);
    offset += 4;
    if (substream_length != plain_length || adaptive_length != end - offset ||
        adaptive_length < 4 || input[offset++] != 1 || input[offset++] != 0 ||
        input[offset++] != 8 || !get_varint(input, end, &offset, &bits)) {
        return CIX_SQUASHFS_MALFORMED;
    }
    bytes = (size_t)(bits / 8u) + (bits % 8u != 0);
    if (bytes != end - offset) {
        return CIX_SQUASHFS_MALFORMED;
    }
    memset(state, 0, sizeof(*state));
    dec_init(&arithmetic, input + offset, bytes, bits);
    for (index = 0; index < plain_length; ++index) {
        uint32_t low;
        uint32_t high;
        uint32_t total;
        uint32_t target = dec_target(&arithmetic, 2u * state->seen + 256u);
        unsigned symbol;

        if (!select_symbol(state, target, &symbol, &low, &high, &total)) {
            return CIX_SQUASHFS_MALFORMED;
        }
        dec_update(&arithmetic, low, high, total);
        model_add(state, symbol);
        output[index] = (uint8_t)symbol;
    }
    return CIX_SQUASHFS_OK;
}

size_t cix_squashfs_profile_decode_workspace_size(void) {
    return sizeof(model);
}

int cix_squashfs_profile_decode_with_workspace(
    enum cix_squashfs_profile_kind kind, const uint8_t *input,
    size_t input_length, uint8_t *output, size_t output_capacity,
    void *workspace, size_t workspace_length, size_t *written) {
    size_t limit = cix_squashfs_profile_max_input(kind);
    size_t end;
    uint32_t plain_length;
    uint32_t payload_length;
    int status;

    if (written != NULL) {
        *written = 0;
    }
    if (written == NULL || workspace == NULL ||
        workspace_length < sizeof(model) || limit == 0 || !valid_io(input, input_length) ||
        !valid_io(output, output_capacity) ||
        ranges_overlap(input, input_length, output, output_capacity)) {
        return CIX_SQUASHFS_INVALID_ARGUMENT;
    }
    if ((uintptr_t)workspace % MODEL_ALIGNMENT != 0 ||
        ranges_overlap((const uint8_t *)workspace, sizeof(model), input, input_length) ||
        ranges_overlap((const uint8_t *)workspace, sizeof(model), output, output_capacity)) {
        return CIX_SQUASHFS_INVALID_ARGUMENT;
    }
    if (input_length < HEADER || input[0] != VERSION) {
        return CIX_SQUASHFS_MALFORMED;
    }
    plain_length = le32(input + 2);
    payload_length = le32(input + 6);
    if (plain_length > limit) {
        return CIX_SQUASHFS_LIMIT;
    }
    if ((size_t)payload_length > input_length - HEADER) {
        return CIX_SQUASHFS_MALFORMED;
    }
    end = HEADER + (size_t)payload_length;
    if (end != input_length) {
        return CIX_SQUASHFS_MALFORMED;
    }
    if (output_capacity < plain_length) {
        return CIX_SQUASHFS_OUTPUT_TOO_SMALL;
    }
    switch (input[1]) {
    case METHOD_RAW:
        if (payload_length != plain_length) {
            return CIX_SQUASHFS_MALFORMED;
        }
        if (plain_length != 0) {
            memcpy(output, input + HEADER, plain_length);
        }
        status = CIX_SQUASHFS_OK;
        break;
    case METHOD_RLE:
        status = decode_rle(input, HEADER, end, output, plain_length);
        break;
    case METHOD_ADAPTIVE:
        status = decode_adaptive(input, HEADER, end, output, plain_length,
                                 (model *)workspace);
        break;
    default:
        return CIX_SQUASHFS_MALFORMED;
    }
    if (status != CIX_SQUASHFS_OK) {
        return status;
    }
    *written = plain_length;
    return CIX_SQUASHFS_OK;
}

#ifndef CIX_SQUASHFS_PROFILE_DECODER_ONLY
int cix_squashfs_profile_decode(enum cix_squashfs_profile_kind kind,
                                const uint8_t *input, size_t input_length,
                                uint8_t *output, size_t output_capacity,
                                size_t *written) {
    model workspace;

    return cix_squashfs_profile_decode_with_workspace(kind, input, input_length,
        output, output_capacity, &workspace, sizeof(workspace), written);
}
#endif
