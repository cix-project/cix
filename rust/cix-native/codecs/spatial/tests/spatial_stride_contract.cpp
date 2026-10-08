#include "cix_spatial_bridge.h"

#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <vector>

namespace {

bool expect(bool condition, const char* message) {
  if (condition) return true;
  std::cerr << message << '\n';
  return false;
}

bool reject_overflow_before_buffer_use() {
  const uint32_t width = std::numeric_limits<uint32_t>::max();
  const size_t claimed_bytes = static_cast<size_t>(width) * 2;
  uint8_t one_byte = 0;
  cix_spatial_buffer encoded{&one_byte, 9};
  if (!expect(cix_spatial_encode_u16_gray(CIX_SPATIAL_JPEGLS, &one_byte,
      claimed_bytes, width, 1, 16, std::numeric_limits<size_t>::max(),
      &encoded) == CIX_SPATIAL_INVALID_ARGUMENT, "encode overflow was accepted")) return false;
  if (!expect(encoded.data == nullptr && encoded.size == 0,
              "encode overflow mutated output")) return false;
  if (!expect(cix_spatial_decode_u16_gray(CIX_SPATIAL_JPEGLS, &one_byte, 1,
      width, 1, 16, &one_byte, claimed_bytes) == CIX_SPATIAL_INVALID_ARGUMENT,
      "decode overflow was accepted")) return false;
  return true;
}

bool round_trip(uint32_t bits, const std::vector<uint8_t>& input) {
  cix_spatial_buffer encoded{nullptr, 0};
  const int encode = cix_spatial_encode_u16_gray(CIX_SPATIAL_JPEGLS,
      input.data(), input.size(), 2, 2, bits, 1024 * 1024, &encoded);
  if (!expect(encode == CIX_SPATIAL_OK && encoded.data != nullptr,
              "small CharLS encode failed")) return false;
  std::vector<uint8_t> decoded(input.size());
  const int decode = cix_spatial_decode_u16_gray(CIX_SPATIAL_JPEGLS,
      encoded.data, encoded.size, 2, 2, bits, decoded.data(), decoded.size());
  cix_spatial_free_buffer(&encoded);
  return expect(decode == CIX_SPATIAL_OK && decoded == input,
                "small CharLS round trip changed bytes");
}

}  // namespace

int main() {
  const std::vector<uint8_t> eight_bit = {3, 0, 255, 0, 17, 0, 99, 0};
  const std::vector<uint8_t> sixteen_bit = {3, 2, 255, 1, 17, 128, 99, 64};
  return reject_overflow_before_buffer_use() && round_trip(8, eight_bit) &&
         round_trip(16, sixteen_bit) ? 0 : 1;
}
