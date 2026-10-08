#include "cix_jxl_bridge.h"

#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <vector>

namespace {

void require(bool value, const char* message) {
  if (!value) {
    std::cerr << message << '\n';
    std::exit(1);
  }
}

void require_gray(const std::vector<uint8_t>& pixels, uint32_t bits,
                  uint32_t sample_type) {
  cix_jxl_gray_image image{pixels.data(), pixels.size(), 3, 2, bits,
                            sample_type, 7};
  cix_jxl_buffer archive{};
  require(cix_jxl_encode_gray_2d(&image, 1 << 20, &archive) == CIX_JXL_OK,
          "gray encode failed");
  std::vector<uint8_t> saved(archive.data, archive.data + archive.size);
  const size_t archive_size = saved.size();
  require(archive_size != 0, "gray archive is empty");
  std::vector<uint8_t> restored(pixels.size());
  require(cix_jxl_decode_gray_2d(saved.data(), saved.size(), 3, 2, bits,
                                  sample_type, restored.data(), restored.size()) ==
              CIX_JXL_OK &&
              restored == pixels,
          "gray round trip differs");
  saved.push_back(0);
  require(cix_jxl_decode_gray_2d(saved.data(), saved.size(), 3, 2, bits,
                                  sample_type, restored.data(), restored.size()) !=
              CIX_JXL_OK,
          "gray trailing data was accepted");
  cix_jxl_free_buffer(&archive);

  uint8_t sentinel = 0;
  cix_jxl_buffer capped{&sentinel, 9};
  require(cix_jxl_encode_gray_2d(&image, archive_size - 1, &capped) ==
              CIX_JXL_OUTPUT_LIMIT &&
              capped.data == nullptr && capped.size == 0,
          "gray output cap did not reset output");
}

void require_planar() {
  const std::vector<uint8_t> pixels = {
      1, 2, 3, 4, 5, 6,  // gray plane
      7, 8, 9, 10, 11, 12,  // optional plane one
      13, 14, 15, 16, 17, 18,  // optional plane two
  };
  cix_jxl_planar_image image{pixels.data(), pixels.size(), 3, 3, 2, 8,
                              CIX_JXL_UINT8, 7};
  cix_jxl_buffer archive{};
  require(cix_jxl_encode_planar(&image, 1 << 20, &archive) == CIX_JXL_OK,
          "planar encode failed");
  std::vector<uint8_t> saved(archive.data, archive.data + archive.size);
  const size_t archive_size = saved.size();
  require(archive_size != 0, "planar archive is empty");
  std::vector<uint8_t> restored(pixels.size());
  require(cix_jxl_decode_planar(saved.data(), saved.size(), 3, 3, 2, 8,
                                 CIX_JXL_UINT8, restored.data(), restored.size()) ==
              CIX_JXL_OK &&
              restored == pixels,
          "planar round trip differs");
  saved.push_back(0);
  require(cix_jxl_decode_planar(saved.data(), saved.size(), 3, 3, 2, 8,
                                 CIX_JXL_UINT8, restored.data(), restored.size()) !=
              CIX_JXL_OK,
          "planar trailing data was accepted");
  cix_jxl_free_buffer(&archive);

  uint8_t sentinel = 0;
  cix_jxl_buffer capped{&sentinel, 9};
  require(cix_jxl_encode_planar(&image, archive_size - 1, &capped) ==
              CIX_JXL_OUTPUT_LIMIT &&
              capped.data == nullptr && capped.size == 0,
          "planar output cap did not reset output");
}

}  // namespace

int main() {
  require_gray({0, 17, 255, 3, 128, 42}, 8, CIX_JXL_UINT8);
  require_gray({0, 0, 1, 0, 0xff, 0x7f, 0x34, 0x12, 0xff, 0xff, 2, 0},
               16, CIX_JXL_UINT16);
  require_planar();
  return 0;
}
