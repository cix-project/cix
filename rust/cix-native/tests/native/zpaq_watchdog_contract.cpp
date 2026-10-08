#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <vector>

// Compile the exact private wrapper into this contract translation unit. This
// intentionally exercises no new public ABI.
#include "../../src/zpaq_ffi.cpp"

namespace {
int cancellations = 0;
bool cancelled() {
  ++cancellations;
  return cancellations >= 2;
}

bool require(bool value, const char* message) {
  if (!value) std::fprintf(stderr, "%s\n", message);
  return value;
}
}

int main() {
  cix_zpaq715_instruction_progress();
  for (size_t index = 0; index != 1'000'000; ++index) {
    if (!require(cix_zpaq715_charge_instruction(), "watchdog admitted too few instructions")) return 1;
  }
  if (!require(!cix_zpaq715_charge_instruction(), "watchdog did not stop at budget")) return 1;
  if (!require(!cix_zpaq715_charge_instruction(), "exhausted watchdog wrapped")) return 1;

  cix_zpaq715_instruction_progress();
  if (!require(cix_zpaq715_charge_instruction(), "progress did not reset watchdog")) return 1;

  {
    CancelScope scope(cancelled);
    cix_zpaq715_instruction_progress();
    if (!require(cix_zpaq715_charge_instruction(), "first cancellation poll failed")) return 1;
    for (size_t index = 0; index != 4095; ++index) {
      if (!require(cix_zpaq715_charge_instruction(), "cancellation cadence polled early")) return 1;
    }
    if (!require(!cix_zpaq715_charge_instruction(), "cancellation cadence missed poll")) return 1;
  }

  const std::vector<uint8_t> input(8192, 0x5a);
  uint8_t* archive = nullptr;
  size_t archive_size = 0;
  char error[256]{};
  if (!require(cix_zpaq715_compress_l5(input.data(), input.size(), input.size() * 2,
                                        &archive, &archive_size, error, sizeof(error), nullptr) == 0,
               "normal wrapper compression failed")) return 1;
  std::vector<uint8_t> output(input.size());
  size_t output_size = 0;
  const bool decoded = cix_zpaq715_decompress(archive, archive_size, output.data(), output.size(),
                                               1024ULL * 1024ULL * 1024ULL, &output_size,
                                               error, sizeof(error), nullptr) == 0;
  const bool exact = output_size == input.size() && output == input;
  std::vector<uint8_t> trailing(archive, archive + archive_size);
  trailing.push_back(0);
  const bool rejected_trailing = cix_zpaq715_decompress(
      trailing.data(), trailing.size(), output.data(), output.size(), 1024ULL * 1024ULL * 1024ULL,
      &output_size, error, sizeof(error), nullptr) != 0;
  cix_zpaq715_free(archive);
  return require(decoded && exact, "normal wrapper round trip failed") &&
                 require(rejected_trailing, "wrapper accepted trailing data") ? 0 : 1;
}
