// SPDX-License-Identifier: MIT
/* Source-only qualification executable for cix_7zip_experimental_module. */
#include "cix_7zip_coder.h"
#include "Common/MyCom.h"
#include "Windows/PropVariant.h"

#include <algorithm>
#include <cstring>
#include <vector>

STDAPI GetNumberOfMethods(UInt32 *num_methods);
STDAPI GetMethodProperty(UInt32 method_index, PROPID prop_id, PROPVARIANT *value);
STDAPI CreateDecoder(UInt32 method_index, const GUID *iid, void **out_object);
STDAPI CreateEncoder(UInt32 method_index, const GUID *iid, void **out_object);

namespace {

Z7_CLASS_IMP_COM_1(CMemoryIn, ISequentialInStream)
 public:
  CMemoryIn(const std::vector<Byte> &bytes, UInt32 chunk)
      : bytes_(bytes), chunk_(chunk), position_(0) {}
 private:
  const std::vector<Byte> &bytes_;
  UInt32 chunk_;
  size_t position_;
};

Z7_COM7F_IMF(CMemoryIn::Read(void *data, UInt32 size, UInt32 *processed)) {
  if (processed == NULL || (size != 0 && data == NULL)) return E_INVALIDARG;
  const UInt32 available = static_cast<UInt32>(bytes_.size() - position_);
  const UInt32 amount = std::min(size, std::min(chunk_, available));
  if (amount != 0) std::memcpy(data, &bytes_[position_], amount);
  position_ += amount;
  *processed = amount;
  return S_OK;
}

Z7_CLASS_IMP_COM_1(CMemoryOut, ISequentialOutStream)
 public:
  CMemoryOut(UInt32 chunk, UInt64 limit) : chunk_(chunk), limit_(limit) {}
  const std::vector<Byte> &bytes() const { return bytes_; }
 private:
  UInt32 chunk_;
  UInt64 limit_;
  std::vector<Byte> bytes_;
};

Z7_COM7F_IMF(CMemoryOut::Write(const void *data, UInt32 size, UInt32 *processed)) {
  if (processed == NULL || (size != 0 && data == NULL)) return E_INVALIDARG;
  const UInt64 remaining = limit_ - bytes_.size();
  const UInt32 amount = static_cast<UInt32>(std::min<UInt64>(
      std::min(size, chunk_), remaining));
  if (amount == 0 && size != 0) return E_OUTOFMEMORY;
  const Byte *input = static_cast<const Byte *>(data);
  bytes_.insert(bytes_.end(), input, input + amount);
  *processed = amount;
  return S_OK;
}

Z7_CLASS_IMP_COM_1(CProgress, ICompressProgressInfo)
 public:
  UInt32 calls_ = 0;
  UInt64 input_ = 0;
  UInt64 output_ = 0;
};

Z7_COM7F_IMF(CProgress::SetRatioInfo(const UInt64 *input, const UInt64 *output)) {
  if (input == NULL || output == NULL) return E_INVALIDARG;
  calls_++;
  input_ = *input;
  output_ = *output;
  return S_OK;
}

Z7_CLASS_IMP_COM_1(CAbortProgress, ICompressProgressInfo)
 public:
  UInt32 calls_ = 0;
};
Z7_COM7F_IMF(CAbortProgress::SetRatioInfo(const UInt64 *, const UInt64 *)) {
  ++calls_;
  return E_ABORT;
}

bool ReadProperties(ICompressCoder *encoder, std::vector<Byte> *properties) {
  ICompressWriteCoderProperties *writer = NULL;
  if (encoder->QueryInterface(IID_ICompressWriteCoderProperties,
      reinterpret_cast<void **>(&writer)) != S_OK) return false;
  CMemoryOut output(2, CIX_7ZIP_WIRE_PROPERTIES_SIZE_V1);
  const HRESULT result = writer->WriteCoderProperties(&output);
  writer->Release();
  if (result != S_OK || output.bytes().size() != CIX_7ZIP_WIRE_PROPERTIES_SIZE_V1) return false;
  *properties = output.bytes();
  return true;
}

bool SetDecoderProperties(ICompressCoder *decoder, const std::vector<Byte> &properties) {
  ICompressSetDecoderProperties2 *setter = NULL;
  if (decoder->QueryInterface(IID_ICompressSetDecoderProperties2,
      reinterpret_cast<void **>(&setter)) != S_OK) return false;
  const HRESULT result = setter->SetDecoderProperties2(
      properties.data(), static_cast<UInt32>(properties.size()));
  setter->Release();
  return result == S_OK;
}

HRESULT SetEncoderProperties(ICompressCoder *encoder, const PROPID *ids,
    const PROPVARIANT *values, UInt32 count) {
  ICompressSetCoderProperties *setter = NULL;
  if (encoder->QueryInterface(IID_ICompressSetCoderProperties,
      reinterpret_cast<void **>(&setter)) != S_OK) return E_NOINTERFACE;
  const HRESULT result = setter->SetCoderProperties(ids, values, count);
  setter->Release();
  return result;
}

HRESULT CodeResult(ICompressCoder *coder, const std::vector<Byte> &input,
    UInt64 output_limit, const UInt64 *expected_output, ICompressProgressInfo *progress,
    std::vector<Byte> *output) {
  CMemoryIn source(input, 13); CMemoryOut destination(11, output_limit);
  const UInt64 input_size = input.size();
  const HRESULT result = coder->Code(&source, &destination, &input_size, expected_output, progress);
  *output = destination.bytes(); return result;
}

bool RunCodec(ICompressCoder *coder, const std::vector<Byte> &input,
    UInt64 output_limit, const UInt64 *expected_output,
    std::vector<Byte> *output) {
  CProgress progress;
  const HRESULT result = CodeResult(coder, input, output_limit, expected_output, &progress, output);
  if (result != S_OK ||
      (expected_output != NULL && output->size() != *expected_output))
    return false;
  return true;
}

bool CheckMethodRegistration() {
  UInt32 count = 0;
  PROPVARIANT value = {};
  if (GetNumberOfMethods(&count) != S_OK || count != 1) return false;
  if (GetMethodProperty(0, NMethodPropID::kID, &value) != S_OK ||
      value.vt != VT_UI8 || value.uhVal.QuadPart != CIX_7ZIP_EXPERIMENTAL_METHOD_ID_V1)
    return false;
  return true;
}

}  // namespace

int main() {
  std::vector<Byte> source(4099);
  for (size_t index = 0; index < source.size(); ++index)
    source[index] = static_cast<Byte>((index * 31U) ^ (index >> 3));
  if (!CheckMethodRegistration()) return 1;

  void *encoder_object = NULL;
  void *decoder_object = NULL;
  if (CreateEncoder(0, &IID_ICompressCoder, &encoder_object) != S_OK ||
      CreateDecoder(0, &IID_ICompressCoder, &decoder_object) != S_OK ||
      encoder_object == NULL || decoder_object == NULL) return 2;
  ICompressCoder *encoder = static_cast<ICompressCoder *>(encoder_object);
  ICompressCoder *decoder = static_cast<ICompressCoder *>(decoder_object);
  PROPID ids[] = {NCoderPropID::kLevel, NCoderPropID::kReduceSize, NCoderPropID::kNumThreads};
  PROPVARIANT props[3] = {}; props[0].vt = VT_UI4; props[0].ulVal = 9;
  props[1].vt = VT_UI8; props[1].uhVal.QuadPart = source.size();
  props[2].vt = VT_UI4; props[2].ulVal = 1;
  PROPID unknown = static_cast<PROPID>(0x7fffffff); PROPVARIANT invalid = {};
  invalid.vt = VT_UI4; invalid.ulVal = 1;
  std::vector<Byte> properties;
  std::vector<Byte> archive;
  std::vector<Byte> restored;
  const UInt64 restored_size = source.size();
  const bool ok = SetEncoderProperties(encoder, ids, props, 3) == S_OK &&
      SetEncoderProperties(encoder, &unknown, &invalid, 1) == E_INVALIDARG &&
      (props[1].vt = VT_UI4, SetEncoderProperties(encoder, &ids[1], &props[1], 1) == E_INVALIDARG) &&
      (props[1].vt = VT_UI8, ReadProperties(encoder, &properties)) &&
      SetDecoderProperties(decoder, properties) &&
      RunCodec(encoder, source, UINT64_C(131072), NULL, &archive) &&
      RunCodec(decoder, archive, source.size(), &restored_size, &restored) && restored == source;
  if (!ok) { encoder->Release(); decoder->Release(); return 3; }
  std::vector<Byte> empty_archive, empty_restored, malformed = properties, tail = archive, ignored;
  const UInt64 zero = 0, too_large = UINT64_C(134217729);
  CAbortProgress abort;
  CMemoryIn bound_source(source, 13); CMemoryOut bound_destination(11, UINT64_C(131072));
  const UInt64 declared_too_large = UINT64_C(67108865);
  const HRESULT input_bound = encoder->Code(&bound_source, &bound_destination, &declared_too_large,
      NULL, NULL);
  malformed.pop_back(); tail.push_back(0xA5);
  const bool boundaries = RunCodec(encoder, {}, UINT64_C(131072), NULL, &empty_archive) &&
      RunCodec(decoder, empty_archive, 0, &zero, &empty_restored) && empty_restored.empty() &&
      !SetDecoderProperties(decoder, malformed) &&
      CodeResult(decoder, archive, source.size(), NULL, NULL, &ignored) == E_INVALIDARG &&
      CodeResult(decoder, tail, source.size(), &restored_size, NULL, &ignored) != S_OK &&
      CodeResult(decoder, std::vector<Byte>(archive.begin(), archive.end() - 1), source.size(), &restored_size, NULL, &ignored) != S_OK &&
      CodeResult(encoder, source, UINT64_C(131072), NULL, &abort, &ignored) == E_ABORT && abort.calls_ != 0 &&
      CodeResult(decoder, archive, 0, &too_large, NULL, &ignored) == E_OUTOFMEMORY && ignored.empty() &&
      input_bound == E_OUTOFMEMORY && bound_destination.bytes().empty();
  encoder->Release();
  decoder->Release();
  return boundaries ? 0 : 3;
}
