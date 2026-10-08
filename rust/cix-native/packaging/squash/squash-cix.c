// SPDX-License-Identifier: MIT
/*
 * Optional QuixDB Squash plugin boundary.
 *
 * This is not SquashFS.  It uses the current process-free CIX buffer ABI for
 * bounded all-in-one calls and public cix_stream.h processing.  It retains
 * cix_squash_splice only as an unregistered historical compatibility symbol.
 */
#include <squash.h>
#include "cix.h"
#include "cix_stream.h"
#include <stdint.h>
#include <string.h>

/* Deployment bounds are independent of the caller's destination capacity.
 * CIX must complete within this admission budget before it can report the
 * required destination size, including a valid zero-byte decoded asset. */
#define CIX_SQUASH_MAX_INPUT (64U * 1024U * 1024U)
#define CIX_SQUASH_MAX_ARCHIVE (CIX_SQUASH_MAX_INPUT + 2U * 1024U * 1024U)

/* Retained compatibility ABI; it is deliberately not part of cix.h. */
extern int32_t cix_squash_splice(
    int32_t direction, int32_t level, SquashReadFunc read_cb,
    SquashWriteFunc write_cb, void* user_data);

static SquashOptionInfo cix_options[] = {
  { "level", SQUASH_OPTION_TYPE_RANGE_INT,
    .info.range_int = { .min = 1, .max = 9 },
    .default_value.int_value = 6 },
  { NULL, SQUASH_OPTION_TYPE_NONE, }
};

static uint32_t cix_profile_for_level(int level) {
  if (level <= 3) return CIX_PROFILE_FAST;
  if (level >= 8) return CIX_PROFILE_BEST;
  return CIX_PROFILE_DEFAULT;
}

static SquashStatus cix_status_to_squash(cix_status status) {
  if (status == CIX_STATUS_OK) return SQUASH_OK;
  if (status == CIX_STATUS_OUTPUT_TOO_SMALL) return SQUASH_BUFFER_FULL;
  return SQUASH_FAILED;
}

static SquashStatus cix_context_for_options(SquashCodec* codec,
                                            SquashOptions* options,
                                            size_t output_limit,
                                            cix_context** out) {
  cix_options_v1 cix_options;
  int level = squash_options_get_int(options, codec, "level");
  cix_status status;
  if (level < 1 || level > 9) return SQUASH_BAD_PARAM;
  status = cix_options_v1_default(&cix_options);
  if (status != CIX_STATUS_OK) return cix_status_to_squash(status);
  cix_options.profile = cix_profile_for_level(level);
  cix_options.workers = 1;
  cix_options.output_limit = (uint64_t) output_limit;
  cix_options.memory_limit = UINT64_C(512) * 1024 * 1024;
  status = cix_context_create(&cix_options, out);
  return cix_status_to_squash(status);
}

static SquashStatus cix_compress_buffer(SquashCodec* codec,
                                        size_t* compressed_size,
                                        uint8_t compressed[],
                                        size_t uncompressed_size,
                                        const uint8_t uncompressed[],
                                        SquashOptions* options) {
  cix_context* context = NULL;
  size_t needed = 0;
  SquashStatus result;
  if (uncompressed_size > CIX_SQUASH_MAX_INPUT) return SQUASH_BAD_VALUE;
  result = cix_context_for_options(codec, options,
      uncompressed_size + 2U * 1024U * 1024U, &context);
  if (result != SQUASH_OK) return result;
  result = cix_status_to_squash(cix_encode_buffer(context, uncompressed,
      uncompressed_size, compressed, *compressed_size, &needed));
  *compressed_size = needed;
  cix_context_destroy(context);
  return result;
}

static SquashStatus cix_decompress_buffer(SquashCodec* codec,
                                          size_t* decompressed_size,
                                          uint8_t decompressed[],
                                          size_t compressed_size,
                                          const uint8_t compressed[],
                                          SquashOptions* options) {
  cix_context* context = NULL;
  size_t needed = 0;
  SquashStatus result;
  if (compressed_size > CIX_SQUASH_MAX_ARCHIVE) return SQUASH_BAD_VALUE;
  result = cix_context_for_options(codec, options, CIX_SQUASH_MAX_INPUT, &context);
  if (result != SQUASH_OK) return result;
  result = cix_status_to_squash(cix_decode_buffer(context, compressed,
      compressed_size, decompressed, *decompressed_size, &needed));
  *decompressed_size = needed;
  cix_context_destroy(context);
  return result;
}

/* CIX has no independently documented finite all-input maximum for this ABI.
 * Returning SIZE_MAX is conservative; callers should prefer splice/stream.
 */
static size_t cix_max_compressed_size(SquashCodec* codec, size_t input_size) {
  (void) codec;
  (void) input_size;
  return SIZE_MAX;
}

typedef struct {
  SquashStream base;
  int encoder;
  union { cix_stream_encoder* encoder; cix_stream_decoder* decoder; } handle;
} CixSquashStream;

static SquashStatus cix_stream_options(SquashCodec* codec, SquashOptions* options,
                                       int encoder, cix_options_v1* out) {
  int level = squash_options_get_int(options, codec, "level");
  cix_status status;
  if (level < 1 || level > 9) return SQUASH_BAD_PARAM;
  status = cix_options_v1_default(out);
  if (status != CIX_STATUS_OK) return cix_status_to_squash(status);
  out->profile = cix_profile_for_level(level);
  out->workers = 1;
  out->memory_limit = UINT64_C(512) * 1024 * 1024;
  out->output_limit = encoder ? CIX_SQUASH_MAX_ARCHIVE : CIX_SQUASH_MAX_INPUT;
  return SQUASH_OK;
}

static void cix_stream_destroy(void* opaque) {
  CixSquashStream* stream = (CixSquashStream*) opaque;
  if (stream->encoder) cix_stream_encoder_destroy(stream->handle.encoder);
  else cix_stream_decoder_destroy(stream->handle.decoder);
  squash_stream_destroy(&stream->base);
  /* squash_object_unref frees the allocation after the destroy callback. */
}

static SquashStream* cix_create_stream(SquashCodec* codec,
                                       SquashStreamType stream_type,
                                       SquashOptions* options) {
  CixSquashStream* stream;
  cix_options_v1 native;
  SquashStatus result;
  cix_status status;
  if (stream_type != SQUASH_STREAM_COMPRESS && stream_type != SQUASH_STREAM_DECOMPRESS)
    return NULL;
  result = cix_stream_options(codec, options,
      stream_type == SQUASH_STREAM_COMPRESS, &native);
  if (result != SQUASH_OK) return NULL;
  stream = (CixSquashStream*) squash_malloc(sizeof(*stream));
  if (stream == NULL) return NULL;
  memset(stream, 0, sizeof(*stream));
  stream->encoder = stream_type == SQUASH_STREAM_COMPRESS;
  status = stream->encoder
      ? cix_stream_encoder_create(&native, &stream->handle.encoder)
      : cix_stream_decoder_create(&native, &stream->handle.decoder);
  if (status != CIX_STATUS_OK) { squash_free(stream); return NULL; }
  squash_stream_init(&stream->base, codec, stream_type, options, cix_stream_destroy);
  return &stream->base;
}

static SquashStatus cix_process_stream(SquashStream* base, SquashOperation operation) {
  CixSquashStream* stream = (CixSquashStream*) base;
  cix_stream_result_v1 result = {0};
  cix_status status;
  int finish_with_input = operation == SQUASH_OPERATION_FINISH && base->avail_in != 0;
  if (operation == SQUASH_OPERATION_FLUSH) return SQUASH_INVALID_OPERATION;
  if (operation == SQUASH_OPERATION_TERMINATE) return SQUASH_OK;
  if (operation != SQUASH_OPERATION_PROCESS && operation != SQUASH_OPERATION_FINISH)
    return SQUASH_INVALID_OPERATION;
  /* Squash can enter finish with caller input still offered.  Consume that
   * input through CIX first; a later finish call closes the CIX frame. */
  if (operation == SQUASH_OPERATION_PROCESS || base->avail_in != 0) {
    status = stream->encoder
        ? cix_stream_encoder_process(stream->handle.encoder, base->next_in, base->avail_in,
            base->next_out, base->avail_out, &result)
        : cix_stream_decoder_process(stream->handle.decoder, base->next_in, base->avail_in,
            base->next_out, base->avail_out, &result);
  } else {
    status = stream->encoder
        ? cix_stream_encoder_finish(stream->handle.encoder, base->next_out, base->avail_out, &result)
        : cix_stream_decoder_finish(stream->handle.decoder, base->next_out, base->avail_out, &result);
  }
  if (status != CIX_STATUS_OK || result.consumed > base->avail_in || result.produced > base->avail_out)
    return status == CIX_STATUS_OK ? SQUASH_FAILED : cix_status_to_squash(status);
  if (result.consumed != 0) base->next_in += result.consumed;
  base->avail_in -= result.consumed;
  if (result.produced != 0) base->next_out += result.produced;
  base->avail_out -= result.produced;
  if (result.state == CIX_STREAM_FINISHED)
    return operation == SQUASH_OPERATION_FINISH ? SQUASH_OK : SQUASH_END_OF_STREAM;
  if (operation == SQUASH_OPERATION_FINISH && !finish_with_input && base->avail_in == 0 &&
      result.state != CIX_STREAM_NEEDS_OUTPUT)
    return SQUASH_FAILED;
  if (result.consumed == 0 && result.produced == 0) {
    if (result.state == CIX_STREAM_NEEDS_OUTPUT) return SQUASH_PROCESSING;
    return result.state == CIX_STREAM_NEEDS_INPUT && operation == SQUASH_OPERATION_PROCESS
        ? SQUASH_OK : SQUASH_FAILED;
  }
  /* A FINISH call can arrive with input still pending.  The host will call us
   * again with no input to close the CIX frame after this progress. */
  if (finish_with_input) return SQUASH_PROCESSING;
  return result.state == CIX_STREAM_NEEDS_OUTPUT ? SQUASH_PROCESSING : SQUASH_OK;
}

SQUASH_PLUGIN_EXPORT
SquashStatus squash_plugin_init_codec(SquashCodec* codec, SquashCodecImpl* impl) {
  if (strcmp(squash_codec_get_name(codec), "cix") != 0)
    return squash_error(SQUASH_UNABLE_TO_LOAD);
  /* CIX has a native process/finish stream for both directions.  It has no
   * decoder flush, so CAN_FLUSH is intentionally absent. */
  impl->info = SQUASH_CODEC_INFO_NATIVE_STREAMING;
  impl->options = cix_options;
  impl->get_max_compressed_size = cix_max_compressed_size;
  impl->compress_buffer = cix_compress_buffer;
  impl->decompress_buffer = cix_decompress_buffer;
  impl->create_stream = cix_create_stream;
  impl->process_stream = cix_process_stream;
  /* Keep cix_squash_splice exported by CIX for legacy callers, but do not
   * register it: Squash 0.8's custom-splice path is upstream-defective. */
  impl->splice = NULL;
  return SQUASH_OK;
}
