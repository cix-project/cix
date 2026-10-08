// SPDX-License-Identifier: MIT
#include "cix_compression_manager.h"

#include <cix.h>
#include <rocksdb/slice.h>
#include <rocksdb/status.h>

#include <array>
#include <atomic>
#include <cstring>
#include <limits>
#include <memory>

namespace rocksdb {
namespace {
constexpr CompressionType kCixType = kCustomCompression80;
constexpr char kMagic[] = {'C', 'I', 'X', 'R'};
constexpr size_t kHeader = 16;
constexpr char kCompatibility[] = "CIX_RocksDB_Experimental_v1";
struct Stats { std::atomic<uint64_t> encode{0}, cix{0}, raw{0}, decode{0}; };

Status cix_status(cix_status value, const char* operation) {
  if (value == CIX_STATUS_RESOURCE_LIMIT) return Status::MemoryLimit(operation);
  if (value == CIX_STATUS_OUTPUT_TOO_SMALL) return Status::Corruption(operation);
  return Status::Corruption(operation);
}
void put_u64(char* out, uint64_t value) {
  for (unsigned i = 0; i != 8; ++i) out[i] = static_cast<char>(value >> (8 * i));
}
uint64_t get_u64(const char* in) {
  uint64_t value = 0;
  for (unsigned i = 0; i != 8; ++i) value |= uint64_t(static_cast<unsigned char>(in[i])) << (8 * i);
  return value;
}
cix_context* make_context(size_t input, size_t output, size_t cap) {
  constexpr size_t kWorkspace = 64U << 20;
  cix_options_v1 options;
  if (cix_options_v1_default(&options) != CIX_STATUS_OK) return nullptr;
  options.profile = CIX_PROFILE_FAST;
  options.workers = 1;
  options.output_limit = output;
  if (input > cap || output > cap || input > std::numeric_limits<size_t>::max() - kWorkspace ||
      output > (std::numeric_limits<size_t>::max() - input - kWorkspace) / 2) return nullptr;
  // Host destination and native temporary output are both live. The fixed
  // native workspace allowance also covers the default block model state.
  options.memory_limit = input + 2 * output + kWorkspace;
  cix_context* context = nullptr;
  return cix_context_create(&options, &context) == CIX_STATUS_OK ? context : nullptr;
}

class CixDecompressor final : public Decompressor {
 public:
  CixDecompressor(size_t cap, std::shared_ptr<Stats> stats) : cap_(cap), stats_(std::move(stats)) {}
  const char* Name() const override { return kCompatibility; }
  Status ExtractUncompressedSize(Args& args) override {
    try {
    if (args.compression_type != kCixType || args.compressed_data.size() < kHeader ||
        std::memcmp(args.compressed_data.data(), kMagic, sizeof(kMagic)) != 0 ||
        static_cast<unsigned char>(args.compressed_data.data()[4]) != 1 ||
        args.compressed_data.data()[5] || args.compressed_data.data()[6] || args.compressed_data.data()[7])
      return Status::Corruption("invalid CIX RocksDB frame");
    const uint64_t size = get_u64(args.compressed_data.data() + 8);
    if (size > cap_ || size > std::numeric_limits<size_t>::max()) return Status::Corruption("CIX output exceeds cap");
    args.uncompressed_size = size;
    args.compressed_data.remove_prefix(kHeader);
    return Status::OK();
    } catch (...) { return Status::Corruption("CIX size callback threw"); }
  }
  Status DecompressBlock(const Args& args, char* output) override {
    try {
    ++stats_->decode;
    if (args.uncompressed_size > cap_ || output == nullptr) return Status::Corruption("invalid CIX output");
    cix_context* context = make_context(args.compressed_data.size(), args.uncompressed_size, cap_);
    if (!context) return Status::MemoryLimit("CIX context admission failed");
    size_t written = 0;
    const auto result = cix_decode_buffer(context,
        reinterpret_cast<const uint8_t*>(args.compressed_data.data()), args.compressed_data.size(),
        reinterpret_cast<uint8_t*>(output), args.uncompressed_size, &written);
    cix_context_destroy(context);
    if (result != CIX_STATUS_OK || written != args.uncompressed_size) return cix_status(result, "CIX decode failed");
    return Status::OK();
    } catch (...) { return Status::Corruption("CIX decode callback threw"); }
  }
 private: size_t cap_; std::shared_ptr<Stats> stats_;
};

class CixCompressor final : public Compressor {
 public:
  CixCompressor(size_t cap, std::shared_ptr<Stats> stats) : cap_(cap), stats_(std::move(stats)) {}
  const char* Name() const override { return kCompatibility; }
  std::unique_ptr<Compressor> Clone() const override {
    try { return std::make_unique<CixCompressor>(cap_, stats_); } catch (...) { return nullptr; }
  }
  Status CompressBlock(Slice input, char* output, size_t* output_size,
                       CompressionType* output_type, ManagedWorkingArea*) override {
    try {
    ++stats_->encode;
    if (!output_size || !output_type || !output || input.size() > cap_) return Status::InvalidArgument("invalid CIX block");
    if (*output_size < kHeader || input.empty()) {
      ++stats_->raw; *output_size = 0; *output_type = kNoCompression; return Status::OK();
    }
    const size_t capacity = *output_size - kHeader;
    cix_context* context = make_context(input.size(), capacity, cap_);
    if (!context) return Status::MemoryLimit("CIX context admission failed");
    size_t written = 0;
    const auto result = cix_encode_buffer(context, reinterpret_cast<const uint8_t*>(input.data()), input.size(),
                                          reinterpret_cast<uint8_t*>(output + kHeader), capacity, &written);
    cix_context_destroy(context);
    if (result == CIX_STATUS_OUTPUT_TOO_SMALL || result == CIX_STATUS_RESOURCE_LIMIT) {
      ++stats_->raw; *output_size = 0; *output_type = kNoCompression; return Status::OK();
    }
    if (result != CIX_STATUS_OK) return cix_status(result, "CIX encode failed");
    if (written + kHeader >= input.size()) { ++stats_->raw; *output_size = 0; *output_type = kNoCompression; return Status::OK(); }
    std::memcpy(output, kMagic, sizeof(kMagic)); output[4] = 1; output[5] = output[6] = output[7] = 0;
    put_u64(output + 8, input.size()); *output_size = written + kHeader; *output_type = kCixType; ++stats_->cix;
    return Status::OK();
    } catch (...) { return Status::Corruption("CIX encode callback threw"); }
  }
 private: size_t cap_; std::shared_ptr<Stats> stats_;
};

class CixManager final : public CompressionManager {
 public:
  static const char* kClassName() { return kCompatibility; }
  explicit CixManager(size_t cap) : stats_(std::make_shared<Stats>()), decompressor_(std::make_shared<CixDecompressor>(cap, stats_)), cap_(cap) {}
  const char* Name() const override { return kCompatibility; }
  const char* CompatibilityName() const override { return kCompatibility; }
  std::shared_ptr<CompressionManager> FindCompatibleCompressionManager(Slice name) override {
    try { return name == kCompatibility ? shared_from_this() : nullptr; } catch (...) { return nullptr; }
  }
  bool SupportsCompressionType(CompressionType type) const override { return type == kCixType; }
  std::unique_ptr<Compressor> GetCompressor(const CompressionOptions&, CompressionType type) override {
    try { return type == kCixType ? std::make_unique<CixCompressor>(cap_, stats_) : nullptr; } catch (...) { return nullptr; }
  }
  std::shared_ptr<Decompressor> GetDecompressor() override { return decompressor_; }
  CixCompressionManagerStatsV1 stats() const { return {stats_->encode, stats_->cix, stats_->raw, stats_->decode}; }
 private: std::shared_ptr<Stats> stats_; std::shared_ptr<Decompressor> decompressor_; size_t cap_;
};
}  // namespace
std::shared_ptr<CompressionManager> NewCixCompressionManagerV1(size_t max_block_bytes) {
  try { return max_block_bytes == 0 || max_block_bytes > (128U << 20) ? nullptr : std::make_shared<CixManager>(max_block_bytes); }
  catch (...) { return nullptr; }
}
bool GetCixCompressionManagerStatsV1(const std::shared_ptr<CompressionManager>& manager,
                                     CixCompressionManagerStatsV1* out) {
  try {
    auto cix = manager ? manager->CheckedCast<CixManager>() : nullptr;
    if (!cix || !out) return false;
    *out = cix->stats(); return true;
  } catch (...) { return false; }
}
}  // namespace rocksdb
