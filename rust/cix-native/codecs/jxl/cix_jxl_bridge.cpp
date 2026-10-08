#include "cix_jxl_bridge.h"

#include <jxl/decode.h>
#include <jxl/encode.h>
#include <jxl/thread_parallel_runner.h>
#include <lzma.h>

#include <algorithm>
#include <cstring>
#include <exception>
#include <limits>
#include <new>
#include <vector>

namespace {

class Encoder final {
 public:
  Encoder() : encoder_(JxlEncoderCreate(nullptr)), runner_(nullptr) {
    if (encoder_ != nullptr) {
      runner_ = JxlThreadParallelRunnerCreate(nullptr, 1);
    }
  }
  ~Encoder() {
    if (runner_ != nullptr) JxlThreadParallelRunnerDestroy(runner_);
    if (encoder_ != nullptr) JxlEncoderDestroy(encoder_);
  }
  JxlEncoder* get() const { return encoder_; }
  void* runner() const { return runner_; }

 private:
  JxlEncoder* encoder_;
  void* runner_;
};

class Decoder final {
 public:
  Decoder() : decoder_(JxlDecoderCreate(nullptr)), runner_(nullptr) {
    if (decoder_ != nullptr) {
      runner_ = JxlThreadParallelRunnerCreate(nullptr, 1);
    }
  }
  ~Decoder() {
    if (runner_ != nullptr) JxlThreadParallelRunnerDestroy(runner_);
    if (decoder_ != nullptr) JxlDecoderDestroy(decoder_);
  }
  JxlDecoder* get() const { return decoder_; }
  void* runner() const { return runner_; }

 private:
  JxlDecoder* decoder_;
  void* runner_;
};

bool checked_product(uint32_t width, uint32_t height, size_t sample_bytes,
                     size_t* result) {
  if (width == 0 || height == 0 || sample_bytes == 0) return false;
  const size_t w = static_cast<size_t>(width);
  const size_t h = static_cast<size_t>(height);
  if (w > std::numeric_limits<size_t>::max() / h) return false;
  const size_t pixels = w * h;
  if (pixels > std::numeric_limits<size_t>::max() / sample_bytes) return false;
  *result = pixels * sample_bytes;
  return true;
}

bool checked_planar_product(uint32_t depth, uint32_t width, uint32_t height,
                            size_t sample_bytes, size_t* plane_bytes,
                            size_t* result) {
  if (depth == 0 || !checked_product(width, height, sample_bytes, plane_bytes)) {
    return false;
  }
  if (static_cast<size_t>(depth) >
      std::numeric_limits<size_t>::max() / *plane_bytes) {
    return false;
  }
  *result = static_cast<size_t>(depth) * *plane_bytes;
  return true;
}

bool valid_sample(uint32_t bits, uint32_t sample_type, size_t* sample_bytes,
                  JxlDataType* data_type) {
  if (bits == 0 || bits > 16) return false;
  if (sample_type == CIX_JXL_UINT8 && bits <= 8) {
    *sample_bytes = 1;
    *data_type = JXL_TYPE_UINT8;
    return true;
  }
  if (sample_type == CIX_JXL_UINT16 && bits > 8) {
    *sample_bytes = 2;
    *data_type = JXL_TYPE_UINT16;
    return true;
  }
  return false;
}

JxlPixelFormat gray_format(JxlDataType type) {
  // CIX image regions are byte streams: keep UINT16 input/output explicitly
  // little-endian instead of making archive meaning depend on the host.
  const JxlEndianness endianness =
      type == JXL_TYPE_UINT16 ? JXL_LITTLE_ENDIAN : JXL_NATIVE_ENDIAN;
  return JxlPixelFormat{1, type, endianness, 0};
}

int encoder_status(const Encoder& encoder, JxlEncoderStatus status) {
  if (status != JXL_ENC_ERROR) return CIX_JXL_ENCODE_FAILURE;
  return JxlEncoderGetError(encoder.get()) == JXL_ENC_ERR_OOM
             ? CIX_JXL_ALLOCATION_FAILURE
             : CIX_JXL_ENCODE_FAILURE;
}

// Both public encoders use this one bounded ownership transfer.  Keep the
// output-limit admission before vector growth so the returned C buffer is
// unchanged on every failure path.
int drain_encoder(Encoder* encoder, size_t max_output_bytes,
                  cix_jxl_buffer* output) {
  std::vector<uint8_t> encoded;
  encoded.reserve(std::min(max_output_bytes, static_cast<size_t>(4096)));
  uint8_t scratch[4096];
  for (;;) {
    uint8_t* next = scratch;
    size_t available = sizeof(scratch);
    const JxlEncoderStatus status =
        JxlEncoderProcessOutput(encoder->get(), &next, &available);
    const size_t produced = sizeof(scratch) - available;
    if (produced > max_output_bytes - encoded.size()) {
      return CIX_JXL_OUTPUT_LIMIT;
    }
    encoded.insert(encoded.end(), scratch, next);
    if (status == JXL_ENC_SUCCESS) break;
    if (status != JXL_ENC_NEED_MORE_OUTPUT) return encoder_status(*encoder, status);
  }
  uint8_t* allocated = new uint8_t[encoded.size()];
  std::memcpy(allocated, encoded.data(), encoded.size());
  output->data = allocated;
  output->size = encoded.size();
  return CIX_JXL_OK;
}

bool supported_info(const JxlBasicInfo& info, uint32_t width, uint32_t height,
                    uint32_t bits) {
  return info.xsize == width && info.ysize == height &&
         info.bits_per_sample == bits && info.exponent_bits_per_sample == 0 &&
         info.num_color_channels == 1 && info.num_extra_channels == 0 &&
         info.alpha_bits == 0 && info.have_animation == JXL_FALSE &&
         info.have_preview == JXL_FALSE && info.orientation == JXL_ORIENT_IDENTITY;
}

bool supported_gray_base(const JxlBasicInfo& info) {
  return info.xsize != 0 && info.ysize != 0 && info.bits_per_sample != 0 &&
         info.bits_per_sample <= 16 && info.exponent_bits_per_sample == 0 &&
         info.num_color_channels == 1 && info.alpha_bits == 0 &&
         info.have_animation == JXL_FALSE &&
         info.have_preview == JXL_FALSE && info.orientation == JXL_ORIENT_IDENTITY;
}

bool supported_gray_still(const JxlBasicInfo& info) {
  return supported_gray_base(info) && info.num_extra_channels == 0;
}

bool supported_planar_still(const JxlBasicInfo& info, uint32_t depth) {
  return depth > 1 && supported_gray_base(info) &&
         info.num_extra_channels == depth - 1;
}

bool supported_optional_channels(Decoder* decoder, const JxlBasicInfo& info) {
  for (uint32_t index = 0; index < info.num_extra_channels; ++index) {
    JxlExtraChannelInfo extra;
    if (JxlDecoderGetExtraChannelInfo(decoder->get(), index, &extra) !=
            JXL_DEC_SUCCESS ||
        extra.type != JXL_CHANNEL_OPTIONAL ||
        extra.bits_per_sample != info.bits_per_sample ||
        extra.exponent_bits_per_sample != 0 || extra.dim_shift != 0) {
      return false;
    }
  }
  return true;
}

bool exact_pixels(const JxlBasicInfo& info, size_t expected_pixels) {
  size_t pixels = 0;
  return checked_product(info.xsize, info.ysize, 1, &pixels) &&
         pixels == expected_pixels;
}

int set_decoder_buffer(Decoder* decoder, const JxlPixelFormat& format,
                       uint32_t bits_per_sample, uint8_t* output,
                       size_t output_size) {
  size_t required = 0;
  if (JxlDecoderImageOutBufferSize(decoder->get(), &format, &required) !=
          JXL_DEC_SUCCESS ||
      required != output_size) {
    return CIX_JXL_UNSUPPORTED;
  }
  if (JxlDecoderSetImageOutBuffer(decoder->get(), &format, output,
                                  output_size) != JXL_DEC_SUCCESS) {
    return CIX_JXL_DECODE_FAILURE;
  }
  const JxlBitDepth bit_depth = {JXL_BIT_DEPTH_FROM_CODESTREAM,
                                 bits_per_sample, 0};
  return JxlDecoderSetImageOutBitDepth(decoder->get(), &bit_depth) ==
                 JXL_DEC_SUCCESS
             ? CIX_JXL_OK
             : CIX_JXL_DECODE_FAILURE;
}

int set_planar_decoder_buffers(Decoder* decoder, const JxlPixelFormat& format,
                               uint32_t bits_per_sample, uint32_t depth,
                               uint8_t* output, size_t plane_bytes,
                               size_t output_size) {
  if (set_decoder_buffer(decoder, format, bits_per_sample, output, plane_bytes) !=
      CIX_JXL_OK) {
    return CIX_JXL_DECODE_FAILURE;
  }
  for (uint32_t index = 0; index + 1 < depth; ++index) {
    size_t required = 0;
    if (JxlDecoderExtraChannelBufferSize(decoder->get(), &format, &required,
                                         index) != JXL_DEC_SUCCESS ||
        required != plane_bytes ||
        JxlDecoderSetExtraChannelBuffer(decoder->get(), &format,
                                        output + (static_cast<size_t>(index) + 1) * plane_bytes,
                                        plane_bytes, index) != JXL_DEC_SUCCESS) {
      return CIX_JXL_UNSUPPORTED;
    }
  }
  (void)output_size;
  return CIX_JXL_OK;
}

}  // namespace

extern "C" int cix_jxl_encode_gray_2d(const cix_jxl_gray_image* image,
                                       size_t max_output_bytes,
                                       cix_jxl_buffer* output) {
  if (output == nullptr) return CIX_JXL_INVALID_ARGUMENT;
  output->data = nullptr;
  output->size = 0;
  if (image == nullptr || image->pixels == nullptr || max_output_bytes == 0 ||
      image->effort < 1 || image->effort > 10) {
    return CIX_JXL_INVALID_ARGUMENT;
  }

  size_t sample_bytes = 0;
  JxlDataType data_type;
  size_t expected_input = 0;
  if (!valid_sample(image->bits_per_sample, image->sample_type, &sample_bytes,
                    &data_type) ||
      !checked_product(image->width, image->height, sample_bytes,
                       &expected_input) ||
      image->pixels_size != expected_input) {
    return CIX_JXL_INVALID_ARGUMENT;
  }

  try {
    Encoder encoder;
    if (encoder.get() == nullptr || encoder.runner() == nullptr) {
      return CIX_JXL_ALLOCATION_FAILURE;
    }
    if (JxlEncoderSetParallelRunner(encoder.get(), JxlThreadParallelRunner,
                                    encoder.runner()) != JXL_ENC_SUCCESS) {
      return CIX_JXL_ENCODE_FAILURE;
    }

    JxlBasicInfo info;
    JxlEncoderInitBasicInfo(&info);
    info.xsize = image->width;
    info.ysize = image->height;
    info.bits_per_sample = image->bits_per_sample;
    info.exponent_bits_per_sample = 0;
    info.num_color_channels = 1;
    info.num_extra_channels = 0;
    info.uses_original_profile = JXL_TRUE;
    if (JxlEncoderSetBasicInfo(encoder.get(), &info) != JXL_ENC_SUCCESS) {
      return CIX_JXL_ENCODE_FAILURE;
    }

    JxlColorEncoding color;
    JxlColorEncodingSetToSRGB(&color, JXL_TRUE);
    if (JxlEncoderSetColorEncoding(encoder.get(), &color) != JXL_ENC_SUCCESS) {
      return CIX_JXL_ENCODE_FAILURE;
    }

    JxlEncoderFrameSettings* settings =
        JxlEncoderFrameSettingsCreate(encoder.get(), nullptr);
    const JxlBitDepth bit_depth = {JXL_BIT_DEPTH_FROM_CODESTREAM,
                                   image->bits_per_sample, 0};
    if (settings == nullptr ||
        JxlEncoderSetFrameBitDepth(settings, &bit_depth) != JXL_ENC_SUCCESS ||
        JxlEncoderFrameSettingsSetOption(settings,
                                          JXL_ENC_FRAME_SETTING_EFFORT,
                                          image->effort) != JXL_ENC_SUCCESS ||
        JxlEncoderSetFrameLossless(settings, JXL_TRUE) != JXL_ENC_SUCCESS ||
        JxlEncoderSetFrameDistance(settings, 0.0f) != JXL_ENC_SUCCESS) {
      return CIX_JXL_ENCODE_FAILURE;
    }

    const JxlPixelFormat format = gray_format(data_type);
    if (JxlEncoderAddImageFrame(settings, &format, image->pixels,
                                image->pixels_size) != JXL_ENC_SUCCESS) {
      return CIX_JXL_ENCODE_FAILURE;
    }
    JxlEncoderCloseInput(encoder.get());
    return drain_encoder(&encoder, max_output_bytes, output);
  } catch (const std::bad_alloc&) {
    return CIX_JXL_ALLOCATION_FAILURE;
  } catch (...) {
    return CIX_JXL_EXCEPTION;
  }
}

extern "C" int cix_jxl_probe_gray_2d(
    const uint8_t* input, size_t input_size, size_t expected_pixels,
    cix_jxl_gray_info* info) {
  if (input == nullptr || input_size == 0 || expected_pixels == 0 ||
      info == nullptr) {
    return CIX_JXL_INVALID_ARGUMENT;
  }
  std::memset(info, 0, sizeof(*info));
  try {
    Decoder decoder;
    if (decoder.get() == nullptr || decoder.runner() == nullptr) {
      return CIX_JXL_ALLOCATION_FAILURE;
    }
    if (JxlDecoderSetParallelRunner(decoder.get(), JxlThreadParallelRunner,
                                    decoder.runner()) != JXL_DEC_SUCCESS ||
        JxlDecoderSubscribeEvents(decoder.get(), JXL_DEC_BASIC_INFO) !=
            JXL_DEC_SUCCESS ||
        JxlDecoderSetInput(decoder.get(), input, input_size) != JXL_DEC_SUCCESS) {
      return CIX_JXL_DECODE_FAILURE;
    }
    JxlDecoderCloseInput(decoder.get());
    for (;;) {
      const JxlDecoderStatus status = JxlDecoderProcessInput(decoder.get());
      if (status == JXL_DEC_BASIC_INFO) {
        JxlBasicInfo basic;
        if (JxlDecoderGetBasicInfo(decoder.get(), &basic) != JXL_DEC_SUCCESS ||
            !supported_gray_still(basic) || !exact_pixels(basic, expected_pixels)) {
          return CIX_JXL_UNSUPPORTED;
        }
        info->width = basic.xsize;
        info->height = basic.ysize;
        info->bits_per_sample = basic.bits_per_sample;
        info->sample_type = basic.bits_per_sample <= 8 ? CIX_JXL_UINT8
                                                        : CIX_JXL_UINT16;
        return CIX_JXL_OK;
      }
      if (status == JXL_DEC_ERROR || status == JXL_DEC_NEED_MORE_INPUT) {
        return CIX_JXL_DECODE_FAILURE;
      }
      return CIX_JXL_DECODE_FAILURE;
    }
  } catch (const std::bad_alloc&) {
    return CIX_JXL_ALLOCATION_FAILURE;
  } catch (...) {
    return CIX_JXL_EXCEPTION;
  }
}

extern "C" int cix_jxl_decode_gray_2d(
    const uint8_t* input, size_t input_size, uint32_t expected_width,
    uint32_t expected_height, uint32_t expected_bits_per_sample,
    uint32_t expected_sample_type, uint8_t* output, size_t output_size) {
  if (input == nullptr || input_size == 0 || output == nullptr) {
    return CIX_JXL_INVALID_ARGUMENT;
  }
  size_t sample_bytes = 0;
  JxlDataType data_type;
  size_t expected_output = 0;
  if (!valid_sample(expected_bits_per_sample, expected_sample_type,
                    &sample_bytes, &data_type) ||
      !checked_product(expected_width, expected_height, sample_bytes,
                       &expected_output) ||
      output_size != expected_output) {
    return CIX_JXL_INVALID_ARGUMENT;
  }

  try {
    Decoder decoder;
    if (decoder.get() == nullptr || decoder.runner() == nullptr) {
      return CIX_JXL_ALLOCATION_FAILURE;
    }
    if (JxlDecoderSetParallelRunner(decoder.get(), JxlThreadParallelRunner,
                                    decoder.runner()) != JXL_DEC_SUCCESS ||
        JxlDecoderSubscribeEvents(decoder.get(),
                                  JXL_DEC_BASIC_INFO | JXL_DEC_FULL_IMAGE) !=
            JXL_DEC_SUCCESS ||
        JxlDecoderSetInput(decoder.get(), input, input_size) != JXL_DEC_SUCCESS) {
      return CIX_JXL_DECODE_FAILURE;
    }
    JxlDecoderCloseInput(decoder.get());

    const JxlPixelFormat format = gray_format(data_type);
    bool basic_info_seen = false;
    bool image_buffer_set = false;
    bool full_image_seen = false;
    for (;;) {
      const JxlDecoderStatus status = JxlDecoderProcessInput(decoder.get());
      if (status == JXL_DEC_BASIC_INFO) {
        JxlBasicInfo info;
        if (JxlDecoderGetBasicInfo(decoder.get(), &info) != JXL_DEC_SUCCESS ||
            !supported_info(info, expected_width, expected_height,
                            expected_bits_per_sample)) {
          return CIX_JXL_UNSUPPORTED;
        }
        basic_info_seen = true;
        continue;
      }
      if (status == JXL_DEC_NEED_IMAGE_OUT_BUFFER) {
        if (!basic_info_seen || image_buffer_set) return CIX_JXL_UNSUPPORTED;
        const int result =
            set_decoder_buffer(&decoder, format, expected_bits_per_sample,
                               output, output_size);
        if (result != CIX_JXL_OK) return result;
        image_buffer_set = true;
        continue;
      }
      if (status == JXL_DEC_FULL_IMAGE) {
        if (!basic_info_seen || !image_buffer_set || full_image_seen) {
          return CIX_JXL_UNSUPPORTED;
        }
        full_image_seen = true;
        continue;
      }
      if (status == JXL_DEC_SUCCESS) {
        if (!basic_info_seen || !image_buffer_set || !full_image_seen ||
            JxlDecoderReleaseInput(decoder.get()) != 0) {
          return CIX_JXL_DECODE_FAILURE;
        }
        return CIX_JXL_OK;
      }
      if (status == JXL_DEC_ERROR) return CIX_JXL_DECODE_FAILURE;
      return CIX_JXL_DECODE_FAILURE;
    }
  } catch (const std::bad_alloc&) {
    return CIX_JXL_ALLOCATION_FAILURE;
  } catch (...) {
    return CIX_JXL_EXCEPTION;
  }
}

extern "C" int cix_jxl_encode_planar(const cix_jxl_planar_image* image,
                                       size_t max_output_bytes,
                                       cix_jxl_buffer* output) {
  if (output == nullptr) return CIX_JXL_INVALID_ARGUMENT;
  output->data = nullptr;
  output->size = 0;
  if (image == nullptr || image->pixels == nullptr || image->depth < 2 ||
      max_output_bytes == 0 || image->effort < 1 || image->effort > 10) {
    return CIX_JXL_INVALID_ARGUMENT;
  }
  size_t sample_bytes = 0, plane_bytes = 0, expected_input = 0;
  JxlDataType data_type;
  if (!valid_sample(image->bits_per_sample, image->sample_type, &sample_bytes,
                    &data_type) ||
      !checked_planar_product(image->depth, image->width, image->height,
                              sample_bytes, &plane_bytes, &expected_input) ||
      image->pixels_size != expected_input) {
    return CIX_JXL_INVALID_ARGUMENT;
  }
  try {
    Encoder encoder;
    if (encoder.get() == nullptr || encoder.runner() == nullptr) return CIX_JXL_ALLOCATION_FAILURE;
    if (JxlEncoderSetParallelRunner(encoder.get(), JxlThreadParallelRunner,
                                    encoder.runner()) != JXL_ENC_SUCCESS) return CIX_JXL_ENCODE_FAILURE;
    JxlBasicInfo info;
    JxlEncoderInitBasicInfo(&info);
    info.xsize = image->width;
    info.ysize = image->height;
    info.bits_per_sample = image->bits_per_sample;
    info.exponent_bits_per_sample = 0;
    info.num_color_channels = 1;
    info.num_extra_channels = image->depth - 1;
    info.uses_original_profile = JXL_TRUE;
    if (JxlEncoderSetBasicInfo(encoder.get(), &info) != JXL_ENC_SUCCESS) return CIX_JXL_ENCODE_FAILURE;
    for (uint32_t index = 0; index < info.num_extra_channels; ++index) {
      JxlExtraChannelInfo extra;
      JxlEncoderInitExtraChannelInfo(JXL_CHANNEL_OPTIONAL, &extra);
      extra.bits_per_sample = image->bits_per_sample;
      extra.exponent_bits_per_sample = 0;
      extra.dim_shift = 0;
      if (JxlEncoderSetExtraChannelInfo(encoder.get(), index, &extra) != JXL_ENC_SUCCESS) {
        return CIX_JXL_ENCODE_FAILURE;
      }
    }
    JxlColorEncoding color;
    JxlColorEncodingSetToSRGB(&color, JXL_TRUE);
    if (JxlEncoderSetColorEncoding(encoder.get(), &color) != JXL_ENC_SUCCESS) return CIX_JXL_ENCODE_FAILURE;
    JxlEncoderFrameSettings* settings = JxlEncoderFrameSettingsCreate(encoder.get(), nullptr);
    const JxlBitDepth bit_depth = {JXL_BIT_DEPTH_FROM_CODESTREAM, image->bits_per_sample, 0};
    if (settings == nullptr || JxlEncoderSetFrameBitDepth(settings, &bit_depth) != JXL_ENC_SUCCESS ||
        JxlEncoderFrameSettingsSetOption(settings, JXL_ENC_FRAME_SETTING_EFFORT, image->effort) != JXL_ENC_SUCCESS ||
        JxlEncoderSetFrameLossless(settings, JXL_TRUE) != JXL_ENC_SUCCESS ||
        JxlEncoderSetFrameDistance(settings, 0.0f) != JXL_ENC_SUCCESS) return CIX_JXL_ENCODE_FAILURE;
    const JxlPixelFormat format = gray_format(data_type);
    if (JxlEncoderAddImageFrame(settings, &format, image->pixels, plane_bytes) != JXL_ENC_SUCCESS) return CIX_JXL_ENCODE_FAILURE;
    for (uint32_t index = 0; index < info.num_extra_channels; ++index) {
      if (JxlEncoderSetExtraChannelBuffer(settings, &format,
              image->pixels + (static_cast<size_t>(index) + 1) * plane_bytes,
              plane_bytes, index) != JXL_ENC_SUCCESS) return CIX_JXL_ENCODE_FAILURE;
    }
    JxlEncoderCloseInput(encoder.get());
    return drain_encoder(&encoder, max_output_bytes, output);
  } catch (const std::bad_alloc&) { return CIX_JXL_ALLOCATION_FAILURE; }
    catch (...) { return CIX_JXL_EXCEPTION; }
}

extern "C" int cix_jxl_probe_planar(const uint8_t* input, size_t input_size,
                                      uint32_t expected_depth, size_t expected_pixels,
                                      cix_jxl_planar_info* info) {
  if (input == nullptr || input_size == 0 || expected_depth < 2 ||
      expected_pixels == 0 || info == nullptr) return CIX_JXL_INVALID_ARGUMENT;
  std::memset(info, 0, sizeof(*info));
  try {
    Decoder decoder;
    if (decoder.get() == nullptr || decoder.runner() == nullptr) return CIX_JXL_ALLOCATION_FAILURE;
    if (JxlDecoderSetParallelRunner(decoder.get(), JxlThreadParallelRunner, decoder.runner()) != JXL_DEC_SUCCESS ||
        JxlDecoderSubscribeEvents(decoder.get(), JXL_DEC_BASIC_INFO) != JXL_DEC_SUCCESS ||
        JxlDecoderSetInput(decoder.get(), input, input_size) != JXL_DEC_SUCCESS) return CIX_JXL_DECODE_FAILURE;
    JxlDecoderCloseInput(decoder.get());
    if (JxlDecoderProcessInput(decoder.get()) != JXL_DEC_BASIC_INFO) return CIX_JXL_DECODE_FAILURE;
    JxlBasicInfo basic;
    size_t plane_pixels = 0, total_pixels = 0;
    if (JxlDecoderGetBasicInfo(decoder.get(), &basic) != JXL_DEC_SUCCESS ||
        !supported_planar_still(basic, expected_depth) ||
        !checked_product(basic.xsize, basic.ysize, 1, &plane_pixels) ||
        static_cast<size_t>(expected_depth) > std::numeric_limits<size_t>::max() / plane_pixels ||
        (total_pixels = static_cast<size_t>(expected_depth) * plane_pixels) != expected_pixels ||
        !supported_optional_channels(&decoder, basic)) return CIX_JXL_UNSUPPORTED;
    info->depth = expected_depth;
    info->width = basic.xsize;
    info->height = basic.ysize;
    info->bits_per_sample = basic.bits_per_sample;
    info->sample_type = basic.bits_per_sample <= 8 ? CIX_JXL_UINT8 : CIX_JXL_UINT16;
    return CIX_JXL_OK;
  } catch (const std::bad_alloc&) { return CIX_JXL_ALLOCATION_FAILURE; }
    catch (...) { return CIX_JXL_EXCEPTION; }
}

extern "C" int cix_jxl_decode_planar(const uint8_t* input, size_t input_size,
                                       uint32_t expected_depth, uint32_t expected_width,
                                       uint32_t expected_height, uint32_t expected_bits_per_sample,
                                       uint32_t expected_sample_type, uint8_t* output,
                                       size_t output_size) {
  if (input == nullptr || input_size == 0 || output == nullptr || expected_depth < 2) return CIX_JXL_INVALID_ARGUMENT;
  size_t sample_bytes = 0, plane_bytes = 0, expected_output = 0;
  JxlDataType data_type;
  if (!valid_sample(expected_bits_per_sample, expected_sample_type, &sample_bytes, &data_type) ||
      !checked_planar_product(expected_depth, expected_width, expected_height, sample_bytes, &plane_bytes, &expected_output) ||
      output_size != expected_output) return CIX_JXL_INVALID_ARGUMENT;
  try {
    Decoder decoder;
    if (decoder.get() == nullptr || decoder.runner() == nullptr) return CIX_JXL_ALLOCATION_FAILURE;
    if (JxlDecoderSetParallelRunner(decoder.get(), JxlThreadParallelRunner, decoder.runner()) != JXL_DEC_SUCCESS ||
        JxlDecoderSubscribeEvents(decoder.get(), JXL_DEC_BASIC_INFO | JXL_DEC_FULL_IMAGE) != JXL_DEC_SUCCESS ||
        JxlDecoderSetInput(decoder.get(), input, input_size) != JXL_DEC_SUCCESS) return CIX_JXL_DECODE_FAILURE;
    JxlDecoderCloseInput(decoder.get());
    const JxlPixelFormat format = gray_format(data_type);
    bool basic_seen = false, buffers_set = false, full_seen = false;
    for (;;) {
      const JxlDecoderStatus status = JxlDecoderProcessInput(decoder.get());
      if (status == JXL_DEC_BASIC_INFO) {
        JxlBasicInfo info;
        if (JxlDecoderGetBasicInfo(decoder.get(), &info) != JXL_DEC_SUCCESS ||
            !supported_planar_still(info, expected_depth) ||
            info.xsize != expected_width || info.ysize != expected_height ||
            info.bits_per_sample != expected_bits_per_sample ||
            info.num_extra_channels != expected_depth - 1 ||
            !supported_optional_channels(&decoder, info)) return CIX_JXL_UNSUPPORTED;
        basic_seen = true;
      } else if (status == JXL_DEC_NEED_IMAGE_OUT_BUFFER) {
        if (!basic_seen || buffers_set) return CIX_JXL_UNSUPPORTED;
        const int result = set_planar_decoder_buffers(&decoder, format, expected_bits_per_sample,
                                                       expected_depth, output, plane_bytes, output_size);
        if (result != CIX_JXL_OK) return result;
        buffers_set = true;
      } else if (status == JXL_DEC_FULL_IMAGE) {
        if (!basic_seen || !buffers_set || full_seen) return CIX_JXL_UNSUPPORTED;
        full_seen = true;
      } else if (status == JXL_DEC_SUCCESS) {
        return basic_seen && buffers_set && full_seen && JxlDecoderReleaseInput(decoder.get()) == 0
                   ? CIX_JXL_OK : CIX_JXL_DECODE_FAILURE;
      } else return CIX_JXL_DECODE_FAILURE;
    }
  } catch (const std::bad_alloc&) { return CIX_JXL_ALLOCATION_FAILURE; }
    catch (...) { return CIX_JXL_EXCEPTION; }
}

extern "C" int cix_lzma_compress_preset9(const uint8_t* input,
                                           size_t input_size,
                                           size_t max_output_bytes,
                                           cix_jxl_buffer* output) {
  if (output == nullptr || input == nullptr || max_output_bytes == 0) {
    return CIX_JXL_INVALID_ARGUMENT;
  }
  output->data = nullptr;
  output->size = 0;
  const size_t bound = lzma_stream_buffer_bound(input_size);
  if (bound == 0 || bound > max_output_bytes) return CIX_JXL_OUTPUT_LIMIT;
  try {
    uint8_t* encoded = new uint8_t[bound];
    size_t produced = 0;
    const lzma_ret result = lzma_easy_buffer_encode(
        9, LZMA_CHECK_CRC64, nullptr, input, input_size, encoded, &produced,
        bound);
    if (result != LZMA_OK) {
      delete[] encoded;
      return result == LZMA_MEM_ERROR ? CIX_JXL_ALLOCATION_FAILURE
                                      : CIX_JXL_ENCODE_FAILURE;
    }
    output->data = encoded;
    output->size = produced;
    return CIX_JXL_OK;
  } catch (const std::bad_alloc&) {
    return CIX_JXL_ALLOCATION_FAILURE;
  } catch (...) {
    return CIX_JXL_EXCEPTION;
  }
}

extern "C" int cix_lzma_decompress_exact(const uint8_t* input,
                                           size_t input_size,
                                           size_t expected_output_bytes,
                                           size_t memory_bytes,
                                           cix_jxl_buffer* output) {
  if (output == nullptr || input == nullptr ||
      input_size > memory_bytes ||
      expected_output_bytes > memory_bytes - input_size) {
    return CIX_JXL_INVALID_ARGUMENT;
  }
  output->data = nullptr;
  output->size = 0;
  try {
    uint8_t* decoded = new uint8_t[expected_output_bytes];
    size_t input_at = 0;
    size_t output_at = 0;
    uint64_t internal_limit = memory_bytes - input_size - expected_output_bytes;
    const lzma_ret result = lzma_stream_buffer_decode(
        &internal_limit, 0, nullptr, input, &input_at, input_size, decoded,
        &output_at, expected_output_bytes);
    if (result != LZMA_OK || input_at != input_size ||
        output_at != expected_output_bytes) {
      delete[] decoded;
      return result == LZMA_MEM_ERROR ? CIX_JXL_ALLOCATION_FAILURE
                                      : CIX_JXL_DECODE_FAILURE;
    }
    output->data = decoded;
    output->size = output_at;
    return CIX_JXL_OK;
  } catch (const std::bad_alloc&) {
    return CIX_JXL_ALLOCATION_FAILURE;
  } catch (...) {
    return CIX_JXL_EXCEPTION;
  }
}

extern "C" void cix_jxl_free_buffer(cix_jxl_buffer* buffer) {
  if (buffer == nullptr) return;
  delete[] buffer->data;
  buffer->data = nullptr;
  buffer->size = 0;
}

extern "C" uint32_t cix_jxl_library_version(void) {
  return JxlEncoderVersion();
}

extern "C" const char* cix_jxl_bridge_version(void) {
  return "cix-jxl-gray2d-planar-bridge/2";
}
