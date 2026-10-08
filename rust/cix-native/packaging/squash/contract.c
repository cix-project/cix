// SPDX-License-Identifier: MIT
/*
 * Focused installed-host contract for the CIX QuixDB Squash plugin.
 *
 * Usage: contract /absolute/plugin-root
 *
 * plugin-root contains cix/squash.ini and
 * cix/libsquash0.8-plugin-cix.so.  This source intentionally exercises the
 * host's public codec, splice, and stream-emulation APIs; it does not call the
 * legacy cix_squash_splice symbol directly.
 */
#include <squash.h>

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CONTRACT_CAPACITY (UINT64_C(2) * 1024 * 1024)
#define IO_CHUNK 4096U

struct Bytes {
  const uint8_t *input;
  size_t input_size;
  size_t input_pos;
  uint8_t *output;
  size_t output_capacity;
  size_t output_size;
  size_t read_slice;
};

static int fail(const char *message) {
  fprintf(stderr, "squash CIX contract: %s\n", message);
  return 0;
}

static void fill_random(uint8_t *data, size_t size) {
  uint32_t value = UINT32_C(0x9e3779b9);
  size_t index;
  for (index = 0; index < size; ++index) {
    value = value * UINT32_C(1664525) + UINT32_C(1013904223);
    data[index] = (uint8_t) (value >> 24);
  }
}

static SquashStatus read_fragment(size_t *size, uint8_t data[], void *opaque) {
  struct Bytes *bytes = (struct Bytes *) opaque;
  size_t available;
  size_t amount;
  if (size == NULL || data == NULL || bytes == NULL) return SQUASH_BAD_PARAM;
  available = bytes->input_size - bytes->input_pos;
  if (available == 0) {
    *size = 0;
    return SQUASH_END_OF_STREAM;
  }
  amount = *size < available ? *size : available;
  if (bytes->read_slice != 0 && amount > bytes->read_slice) amount = bytes->read_slice;
  memcpy(data, bytes->input + bytes->input_pos, amount);
  bytes->input_pos += amount;
  *size = amount;
  return SQUASH_OK;
}

static SquashStatus write_all(size_t *size, const uint8_t data[], void *opaque) {
  struct Bytes *bytes = (struct Bytes *) opaque;
  size_t available;
  if (size == NULL || data == NULL || bytes == NULL) return SQUASH_BAD_PARAM;
  available = bytes->output_capacity - bytes->output_size;
  if (*size > available) {
    *size = available;
    return SQUASH_BUFFER_FULL;
  }
  if (*size != 0) memcpy(bytes->output + bytes->output_size, data, *size);
  bytes->output_size += *size;
  return SQUASH_OK;
}

static int buffer_roundtrip(SquashCodec *codec, const uint8_t *input, size_t input_size) {
  uint8_t *archive = NULL;
  uint8_t *restored = NULL;
  size_t archive_capacity = input_size * 2 + 16384;
  size_t archive_size = archive_capacity;
  /* Squash's public buffer API rejects a zero-capacity destination even when
   * decoding an empty asset. Supply its required one-byte working buffer. */
  size_t restored_size = input_size == 0 ? 1 : input_size;
  SquashStatus status;
  int ok = 0;

  archive = (uint8_t *) malloc(archive_capacity);
  restored = (uint8_t *) malloc(input_size == 0 ? 1 : input_size);
  if (archive == NULL || restored == NULL) goto done;
  status = squash_codec_compress(codec, &archive_size, archive, input_size, input, (void *) 0);
  if (status != SQUASH_OK || archive_size == 0 || archive_size > archive_capacity) {
    fprintf(stderr, "compress input=%zu status=%d size=%zu\n", input_size, status, archive_size);
    goto done;
  }
  status = squash_codec_decompress(codec, &restored_size, restored, archive_size, archive, (void *) 0);
  if (status != SQUASH_OK || restored_size != input_size ||
      (input_size != 0 && memcmp(restored, input, input_size) != 0)) {
    fprintf(stderr, "decompress input=%zu archive=%zu status=%d restored=%zu\n",
        input_size, archive_size, status, restored_size);
    goto done;
  }
  ok = 1;
done:
  free(archive);
  free(restored);
  return ok;
}

static int buffer_failures(SquashCodec *codec, const uint8_t *input, size_t input_size) {
  uint8_t archive[65536];
  uint8_t restored[32768];
  uint8_t one = 0;
  size_t archive_size = sizeof(archive);
  size_t restored_size = input_size;
  size_t too_small = 0;
  size_t one_byte = 1;
  SquashStatus status;

  /* The plugin must report a zero-capacity destination as insufficient, before
   * CIX receives a caller buffer. */
  status = squash_codec_compress(codec, &too_small, &one, input_size, input, (void *) 0);
  if (status != SQUASH_BUFFER_FULL) return 0;
  status = squash_codec_compress(codec, &one_byte, &one, input_size, input, (void *) 0);
  if (status != SQUASH_BUFFER_FULL) return 0;

  status = squash_codec_compress(codec, &archive_size, archive, input_size, input, (void *) 0);
  if (status != SQUASH_OK || archive_size + 1 > sizeof(archive) || input_size > sizeof(restored)) return 0;
  archive[archive_size / 2] ^= UINT8_C(0x80);
  status = squash_codec_decompress(codec, &restored_size, restored, archive_size, archive, (void *) 0);
  if (status == SQUASH_OK) return 0;
  archive[archive_size / 2] ^= UINT8_C(0x80);

  archive[archive_size] = UINT8_C(0xa5);
  restored_size = input_size;
  status = squash_codec_decompress(codec, &restored_size, restored, archive_size + 1, archive, (void *) 0);
  return status != SQUASH_OK;
}

static int splice_roundtrip(SquashCodec *codec, const uint8_t *input, size_t input_size) {
  uint8_t *archive = NULL;
  uint8_t *restored = NULL;
  struct Bytes state;
  SquashStatus status;
  int ok = 0;

  archive = (uint8_t *) malloc(CONTRACT_CAPACITY);
  restored = (uint8_t *) malloc(input_size == 0 ? 1 : input_size);
  if (archive == NULL || restored == NULL) goto done;
  state = (struct Bytes) { input, input_size, 0, archive, CONTRACT_CAPACITY, 0, 13 };
  status = squash_splice_custom(codec, SQUASH_STREAM_COMPRESS, write_all,
      read_fragment, &state, 0, (void *) 0);
  if (status != SQUASH_OK || state.input_pos != input_size || state.output_size == 0) goto done;

  state = (struct Bytes) { archive, state.output_size, 0, restored, input_size, 0, 11 };
  status = squash_splice_custom(codec, SQUASH_STREAM_DECOMPRESS, write_all,
      read_fragment, &state, 0, (void *) 0);
  if (status != SQUASH_OK || state.input_pos != state.input_size ||
      state.output_size != input_size ||
      (input_size != 0 && memcmp(restored, input, input_size) != 0)) goto done;
  ok = 1;
done:
  free(archive);
  free(restored);
  return ok;
}

static int stream_write(uint8_t *destination, size_t capacity, size_t *used,
    const uint8_t *data, size_t size) {
  if (*used > capacity || size > capacity - *used ||
      (size != 0 && data == NULL)) return 0;
  if (size != 0) memcpy(destination + *used, data, size);
  *used += size;
  return 1;
}

static int stream_once(SquashCodec *codec, SquashStreamType type,
    const uint8_t *input, size_t input_size, uint8_t *output,
    size_t output_capacity, size_t *output_size) {
  SquashStream *stream = squash_stream_new(codec, type, (void *) 0);
  size_t input_pos = 0;
  SquashStatus status = SQUASH_OK;
  if (stream == NULL) return 0;
  *output_size = 0;
  while (input_pos < input_size) {
    size_t offered = input_size - input_pos;
    if (offered > 17) offered = 17;
    stream->next_in = input + input_pos;
    stream->avail_in = offered;
    do {
      uint8_t buffer[IO_CHUNK];
      size_t made;
      stream->next_out = buffer;
      stream->avail_out = sizeof(buffer);
      status = squash_stream_process(stream);
      made = sizeof(buffer) - stream->avail_out;
      if (status < 0 || !stream_write(output, output_capacity, output_size, buffer, made)) {
        fprintf(stderr, "stream process type=%d status=%d remaining=%zu\n", type, status, stream->avail_in);
        squash_object_unref(stream);
        return 0;
      }
      if (status == SQUASH_END_OF_STREAM) {
        int complete = stream->avail_in == 0 && input_pos + offered == input_size &&
            stream->state == SQUASH_STREAM_STATE_FINISHED &&
            stream->total_in == input_size && stream->total_out == *output_size;
        squash_object_unref(stream);
        return complete;
      }
    } while (stream->avail_in != 0 || status == SQUASH_PROCESSING);
    input_pos += offered;
  }
  do {
    uint8_t buffer[IO_CHUNK];
    size_t made;
    stream->next_out = buffer;
    stream->avail_out = sizeof(buffer);
    status = squash_stream_finish(stream);
    made = sizeof(buffer) - stream->avail_out;
    if (status < 0 || !stream_write(output, output_capacity, output_size, buffer, made)) {
      fprintf(stderr, "stream finish type=%d status=%d\n", type, status);
      squash_object_unref(stream);
      return 0;
    }
  } while (status == SQUASH_PROCESSING);
  if (status != SQUASH_OK || stream->state != SQUASH_STREAM_STATE_FINISHED ||
      stream->total_in != input_size || stream->total_out != *output_size) {
    squash_object_unref(stream);
    return 0;
  }
  squash_object_unref(stream);
  return 1;
}

static int stream_roundtrip(SquashCodec *codec, const uint8_t *input, size_t input_size) {
  uint8_t *archive = (uint8_t *) malloc(CONTRACT_CAPACITY);
  uint8_t *restored = (uint8_t *) malloc(input_size == 0 ? 1 : input_size);
  size_t archive_size;
  size_t restored_size;
  int ok = archive != NULL && restored != NULL &&
      stream_once(codec, SQUASH_STREAM_COMPRESS, input, input_size, archive,
          CONTRACT_CAPACITY, &archive_size) &&
      stream_once(codec, SQUASH_STREAM_DECOMPRESS, archive, archive_size, restored,
          input_size, &restored_size) && restored_size == input_size &&
      (input_size == 0 || memcmp(restored, input, input_size) == 0);
  free(archive);
  free(restored);
  return ok;
}

int main(int argc, char **argv) {
  uint8_t empty = 0;
  uint8_t one[] = { UINT8_C(0x42) };
  uint8_t random[4096];
  uint8_t repeated[8192];
  SquashCodec *codec;
  if (argc != 2) {
    fprintf(stderr, "usage: %s /absolute/squash-plugin-root\n", argv[0]);
    return EXIT_FAILURE;
  }
  squash_set_default_search_path(argv[1]);
  codec = squash_get_codec("cix");
  if (codec == NULL) {
    fail("CIX plugin was not discovered");
    return EXIT_FAILURE;
  }
  fill_random(random, sizeof(random));
  memset(repeated, 'C', sizeof(repeated));
  if (!buffer_roundtrip(codec, &empty, 0) || !buffer_roundtrip(codec, one, sizeof(one)) ||
      !buffer_roundtrip(codec, random, sizeof(random)) ||
      !buffer_roundtrip(codec, repeated, sizeof(repeated))) {
    fail("buffer round trip");
    return EXIT_FAILURE;
  }
  if (!buffer_failures(codec, random, sizeof(random))) {
    fail("buffer insufficient/corrupt/trailing handling");
    return EXIT_FAILURE;
  }
  if (!splice_roundtrip(codec, repeated, sizeof(repeated))) {
    fail("host splice callback round trip");
    return EXIT_FAILURE;
  }
  if (!stream_roundtrip(codec, repeated, sizeof(repeated))) {
    fail("host stream emulation round trip");
    return EXIT_FAILURE;
  }
  return EXIT_SUCCESS;
}
