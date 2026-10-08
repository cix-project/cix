// SPDX-License-Identifier: MIT
#include "cix_7zip_coder.h"

#include "Common/MyCom.h"
extern "C" {
#include "cix.h"
#include "cix_stream.h"
}

#include <algorithm>
#include <new>
#include <string.h>

namespace {

const UInt32 kBufferSize = 64 * 1024;
const Byte kWireProperties[CIX_7ZIP_WIRE_PROPERTIES_SIZE_V1] = {
    'C', 'I', 'X', '7', CIX_7ZIP_METHOD_VERSION_V1};
#ifndef CIX_7ZIP_DEFAULT_INPUT_LIMIT
#define CIX_7ZIP_DEFAULT_INPUT_LIMIT (UINT64_C(64) * 1024 * 1024)
#endif
#ifndef CIX_7ZIP_DEFAULT_OUTPUT_LIMIT
#define CIX_7ZIP_DEFAULT_OUTPUT_LIMIT (UINT64_C(128) * 1024 * 1024)
#endif
#ifndef CIX_7ZIP_DEFAULT_MEMORY_LIMIT
#define CIX_7ZIP_DEFAULT_MEMORY_LIMIT (UINT64_C(512) * 1024 * 1024)
#endif
#ifndef CIX_7ZIP_DEFAULT_WORKERS
#define CIX_7ZIP_DEFAULT_WORKERS 1U
#endif
#ifndef CIX_7ZIP_DEFAULT_PROFILE
#define CIX_7ZIP_DEFAULT_PROFILE CIX_PROFILE_DEFAULT
#endif
struct Limits {
  UInt64 input_limit;
  UInt64 output_limit;
  UInt64 memory_limit;
  UInt32 profile;
  UInt32 workers;
};

enum class Direction { Encode, Decode };

bool IsValidProfile(UInt32 profile) {
  return profile == CIX_PROFILE_FAST || profile == CIX_PROFILE_DEFAULT ||
      profile == CIX_PROFILE_BEST;
}

HRESULT ParseLimits(const cix_7zip_coder_options_v1 *source, Limits *limits) {
  if (source == NULL || limits == NULL ||
      source->abi_version != CIX_7ZIP_CODER_ABI_V1 ||
      source->struct_size != sizeof(*source) || !IsValidProfile(source->profile) ||
      source->workers == 0 || source->input_limit == 0 ||
      source->output_limit == 0 || source->memory_limit == 0)
    return E_INVALIDARG;
  *limits = {source->input_limit, source->output_limit, source->memory_limit,
      source->profile, source->workers};
  return S_OK;
}

HRESULT StatusToHresult(cix_status status) {
  switch (status) {
    case CIX_STATUS_OK: return S_OK;
    case CIX_STATUS_INVALID_ARGUMENT:
    case CIX_STATUS_INVALID_OPTIONS: return E_INVALIDARG;
    case CIX_STATUS_OUTPUT_TOO_SMALL:
    case CIX_STATUS_RESOURCE_LIMIT: return E_OUTOFMEMORY;
    default: return S_FALSE;
  }
}

HRESULT WriteAll(ISequentialOutStream *stream, const Byte *data, UInt32 size,
    UInt64 *total, UInt64 limit) {
  while (size != 0) {
    if (*total >= limit) return E_OUTOFMEMORY;
    const UInt64 remaining = limit - *total;
    const UInt32 offered = static_cast<UInt32>(std::min<UInt64>(size, remaining));
    UInt32 written = 0;
    const HRESULT result = stream->Write(data, offered, &written);
    if (result != S_OK) return result;
    if (written == 0 || written > offered) return E_FAIL;
    *total += written;
    data += written;
    size -= written;
  }
  return S_OK;
}

HRESULT Report(ICompressProgressInfo *progress, UInt64 input, UInt64 output) {
  return progress == NULL ? S_OK : progress->SetRatioInfo(&input, &output);
}

class CixStreamOwner {
 public:
  CixStreamOwner(Direction direction, const Limits &limits) : direction_(direction), encoder_(NULL), decoder_(NULL) {
    cix_options_v1 options;
    if (cix_options_v1_default(&options) != CIX_STATUS_OK) return;
    options.profile = limits.profile;
    options.workers = limits.workers;
    options.output_limit = limits.output_limit;
    options.memory_limit = limits.memory_limit;
    status_ = direction_ == Direction::Encode
        ? cix_stream_encoder_create(&options, &encoder_)
        : cix_stream_decoder_create(&options, &decoder_);
  }
  ~CixStreamOwner() {
    if (encoder_ != NULL) cix_stream_encoder_destroy(encoder_);
    if (decoder_ != NULL) cix_stream_decoder_destroy(decoder_);
  }
  cix_status status() const { return status_; }
  cix_status Process(const Byte *input, size_t input_size, Byte *output,
      size_t output_size, cix_stream_result_v1 *result) {
    return direction_ == Direction::Encode
        ? cix_stream_encoder_process(encoder_, input, input_size, output, output_size, result)
        : cix_stream_decoder_process(decoder_, input, input_size, output, output_size, result);
  }
  cix_status Finish(Byte *output, size_t output_size, cix_stream_result_v1 *result) {
    return direction_ == Direction::Encode
        ? cix_stream_encoder_finish(encoder_, output, output_size, result)
        : cix_stream_decoder_finish(decoder_, output, output_size, result);
  }
 private:
  Direction direction_;
  cix_stream_encoder *encoder_;
  cix_stream_decoder *decoder_;
  cix_status status_ = CIX_STATUS_PANIC;
};

Z7_CLASS_IMP_COM_4(CCix7zipCoder, ICompressCoder, ICompressSetCoderProperties,
    ICompressWriteCoderProperties, ICompressSetDecoderProperties2)
 public:
  CCix7zipCoder(Direction direction, const Limits &limits)
      : direction_(direction), limits_(limits) {}

 private:
  Direction direction_;
  Limits limits_;
};

Z7_COM7F_IMF(CCix7zipCoder::SetCoderProperties(const PROPID *prop_ids,
    const PROPVARIANT *props, UInt32 num_props)) {
  if (direction_ != Direction::Encode || (num_props != 0 && (prop_ids == NULL || props == NULL)))
    return E_INVALIDARG;
  Limits proposed = limits_;
  for (UInt32 index = 0; index < num_props; ++index) {
    switch (prop_ids[index]) {
      case NCoderPropID::kLevel: {
        if (props[index].vt != VT_UI4 || props[index].ulVal > 9) return E_INVALIDARG;
        const UInt32 level = props[index].ulVal;
        proposed.profile = level <= 3 ? CIX_PROFILE_FAST :
            (level >= 8 ? CIX_PROFILE_BEST : CIX_PROFILE_DEFAULT);
        break;
      }
      case NCoderPropID::kReduceSize:
        // Official 7z passes an estimated largest stream size to every coder.
        // This is only an allocation hint; it cannot relax deployment limits.
        if (props[index].vt != VT_UI8) return E_INVALIDARG;
        break;
      case NCoderPropID::kNumThreads:
        if (props[index].vt != VT_UI4 || props[index].ulVal == 0) return E_INVALIDARG;
        proposed.workers = std::min(proposed.workers, props[index].ulVal);
        break;
      default:
        return E_INVALIDARG;
    }
  }
  limits_ = proposed;
  return S_OK;
}

Z7_COM7F_IMF(CCix7zipCoder::WriteCoderProperties(ISequentialOutStream *out_stream)) {
  UInt64 ignored_total = 0;
  if (direction_ != Direction::Encode || out_stream == NULL) return E_INVALIDARG;
  return WriteAll(out_stream, kWireProperties, CIX_7ZIP_WIRE_PROPERTIES_SIZE_V1,
      &ignored_total, CIX_7ZIP_WIRE_PROPERTIES_SIZE_V1);
}

Z7_COM7F_IMF(CCix7zipCoder::SetDecoderProperties2(const Byte *data, UInt32 size)) {
  if (direction_ != Direction::Decode || data == NULL ||
      size != CIX_7ZIP_WIRE_PROPERTIES_SIZE_V1 ||
      memcmp(data, kWireProperties, CIX_7ZIP_WIRE_PROPERTIES_SIZE_V1) != 0)
    return E_INVALIDARG;
  return S_OK;
}

Z7_COM7F_IMF(CCix7zipCoder::Code(ISequentialInStream *in_stream,
    ISequentialOutStream *out_stream, const UInt64 *in_size,
    const UInt64 *out_size, ICompressProgressInfo *progress)) {
  if (in_stream == NULL || out_stream == NULL) return E_INVALIDARG;
  if (in_size != NULL && *in_size > limits_.input_limit) return E_OUTOFMEMORY;
  /* CIX stream decoding is full-frame only.  Requiring an expected output size
   * gives the host and this adapter a pre-write bound. */
  if (direction_ == Direction::Decode && out_size == NULL) return E_INVALIDARG;
  if (direction_ == Direction::Decode && *out_size > limits_.output_limit) return E_OUTOFMEMORY;
  const UInt64 write_limit = direction_ == Direction::Decode ? *out_size : limits_.output_limit;

  CixStreamOwner stream(direction_, limits_);
  HRESULT result = StatusToHresult(stream.status());
  if (result != S_OK) return result;

  Byte input[kBufferSize];
  Byte output[kBufferSize];
  UInt64 input_total = 0;
  UInt64 output_total = 0;
  UInt32 available = 0;
  UInt32 offset = 0;
  bool at_end = false;

  while (!at_end || available != 0) {
    if (available == 0 && !at_end) {
      UInt32 received = 0;
      result = in_stream->Read(input, kBufferSize, &received);
      if (result != S_OK) return result;
      if (received > kBufferSize) return E_FAIL;
      if (received == 0) { at_end = true; continue; }
      if (input_total > limits_.input_limit ||
          received > limits_.input_limit - input_total) return E_OUTOFMEMORY;
      input_total += received;
      available = received;
      offset = 0;
    }

    cix_stream_result_v1 step = {};
    const UInt32 offered = available;
    const cix_status status = stream.Process(input + offset, offered,
        output, sizeof(output), &step);
    result = StatusToHresult(status);
    if (result != S_OK || step.consumed > offered || step.produced > sizeof(output))
      return result == S_OK ? E_FAIL : result;
    if (step.produced != 0) {
      result = WriteAll(out_stream, output, static_cast<UInt32>(step.produced),
          &output_total, write_limit);
      if (result != S_OK) return result;
    }
    if (step.consumed == 0 && step.produced == 0) return E_FAIL;
    offset += static_cast<UInt32>(step.consumed);
    available -= static_cast<UInt32>(step.consumed);
    result = Report(progress, input_total, output_total);
    if (result != S_OK) return result;
  }

  for (;;) {
    cix_stream_result_v1 step = {};
    result = StatusToHresult(stream.Finish(output, sizeof(output), &step));
    if (result != S_OK || step.produced > sizeof(output))
      return result == S_OK ? E_FAIL : result;
    if (step.produced != 0) {
      result = WriteAll(out_stream, output, static_cast<UInt32>(step.produced),
          &output_total, write_limit);
      if (result != S_OK) return result;
    }
    result = Report(progress, input_total, output_total);
    if (result != S_OK) return result;
    if (step.state == CIX_STREAM_FINISHED) break;
    if (step.produced == 0 || step.state != CIX_STREAM_NEEDS_OUTPUT) return S_FALSE;
  }
  if (direction_ == Direction::Decode && output_total != *out_size) return S_FALSE;
  if (in_size != NULL && input_total != *in_size) return S_FALSE;
  return S_OK;
}

ICompressCoder *CreateRaw(Direction direction, const cix_7zip_coder_options_v1 *options) {
  Limits limits = {};
  if (ParseLimits(options, &limits) != S_OK) return NULL;
  return new (std::nothrow) CCix7zipCoder(direction, limits);
}

HRESULT Create(Direction direction, const cix_7zip_coder_options_v1 *options,
    ICompressCoder **coder) {
  Limits checked = {};
  if (coder == NULL) return E_INVALIDARG;
  *coder = NULL;
  const HRESULT validation = ParseLimits(options, &checked);
  if (validation != S_OK) return validation;
  *coder = new (std::nothrow) CCix7zipCoder(direction, checked);
  if (*coder == NULL) return E_OUTOFMEMORY;
  (*coder)->AddRef();
  return S_OK;
}

const cix_7zip_coder_options_v1 DefaultOptions() {
  return {CIX_7ZIP_CODER_ABI_V1, sizeof(cix_7zip_coder_options_v1),
      CIX_7ZIP_DEFAULT_INPUT_LIMIT, CIX_7ZIP_DEFAULT_OUTPUT_LIMIT,
      CIX_7ZIP_DEFAULT_MEMORY_LIMIT, CIX_7ZIP_DEFAULT_PROFILE,
      CIX_7ZIP_DEFAULT_WORKERS};
}

}  // namespace

ICompressCoder *Cix7zipNewExperimentalDefaultEncoder(void) {
  const cix_7zip_coder_options_v1 options = DefaultOptions();
  return CreateRaw(Direction::Encode, &options);
}

ICompressCoder *Cix7zipNewExperimentalDefaultDecoder(void) {
  const cix_7zip_coder_options_v1 options = DefaultOptions();
  return CreateRaw(Direction::Decode, &options);
}

HRESULT Cix7zipCreateExperimentalDefaultEncoder(ICompressCoder **coder) {
  const cix_7zip_coder_options_v1 options = DefaultOptions();
  return Create(Direction::Encode, &options, coder);
}

HRESULT Cix7zipCreateExperimentalDefaultDecoder(ICompressCoder **coder) {
  const cix_7zip_coder_options_v1 options = DefaultOptions();
  return Create(Direction::Decode, &options, coder);
}

HRESULT Cix7zipCreateExperimentalEncoder(const cix_7zip_coder_options_v1 *options,
    ICompressCoder **coder) {
  return Create(Direction::Encode, options, coder);
}

HRESULT Cix7zipCreateExperimentalDecoder(const cix_7zip_coder_options_v1 *options,
    ICompressCoder **coder) {
  return Create(Direction::Decode, options, coder);
}
