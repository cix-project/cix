// Thin, allocation-owned C ABI for the pinned libzpaq 7.15 implementation.
// This deliberately exposes only byte slices: Rust retains CIX framing,
// checksums, bounded admission and selection while libzpaq supplies its
// level-5 adaptive context-mixing codec in process.
#include "libzpaq.h"

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <new>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
thread_local size_t g_instruction_budget = static_cast<size_t>(-1);
thread_local unsigned g_cancel_poll_countdown = 0;
}

namespace {
using CancelCallback = bool (*)();
thread_local CancelCallback g_cancel_callback = nullptr;
class CancelScope {
 public:
  explicit CancelScope(CancelCallback callback) : previous_(g_cancel_callback) {
    g_cancel_callback = callback;
  }
  ~CancelScope() { g_cancel_callback = previous_; }
 private:
  CancelCallback previous_;
};
}
extern "C" bool cix_zpaq715_cancelled() {
  return g_cancel_callback != nullptr && g_cancel_callback();
}

// Called from the narrowly patched upstream interpreter loop. A valid
// level-5 model executes many instructions per byte, but it must periodically
// produce reconstructed data. Resetting the watchdog from SliceWriter stops a
// malformed program from looping forever before the next output byte.
bool cix_zpaq715_charge_instruction() {
  if (g_instruction_budget == 0) return false;
  --g_instruction_budget;
  // Crossing into Rust on every interpreted instruction would dominate level
  // 5. Poll at most once per 4096 instructions; the local budget remains
  // checked every instruction, retaining a hard stop for a jump loop.
  if (g_cancel_poll_countdown-- == 0) {
    g_cancel_poll_countdown = 4095;
    return !cix_zpaq715_cancelled();
  }
  return true;
}

extern "C" void cix_zpaq715_instruction_progress() {
  g_instruction_budget = 1'000'000;
  g_cancel_poll_countdown = 0;
}

namespace {

class OutputLimit final : public std::runtime_error {
 public:
  OutputLimit() : std::runtime_error("ZPAQ output exceeds bounded payload limit") {}
};

class SliceReader final : public libzpaq::Reader {
 public:
  SliceReader(const uint8_t* input, size_t size) : input_(input), size_(size) {}

  int get() override {
    if (cix_zpaq715_cancelled()) throw std::runtime_error("CIX interrupted");
    return position_ == size_ ? -1 : input_[position_++];
  }

  int read(char* destination, int count) override {
    if (cix_zpaq715_cancelled()) throw std::runtime_error("CIX interrupted");
    if (count <= 0 || position_ == size_) return 0;
    const size_t available = size_ - position_;
    const size_t copied = available < static_cast<size_t>(count)
                              ? available
                              : static_cast<size_t>(count);
    std::memcpy(destination, input_ + position_, copied);
    position_ += copied;
    return static_cast<int>(copied);
  }

  size_t position() const { return position_; }

 private:
  const uint8_t* input_;
  size_t size_;
  size_t position_ = 0;
};

class SliceWriter final : public libzpaq::Writer {
 public:
  SliceWriter(uint8_t* output, size_t capacity) : output_(output), capacity_(capacity) {}

  void put(int byte) override {
    if (size_ == capacity_) throw OutputLimit();
    output_[size_++] = static_cast<uint8_t>(byte);
    cix_zpaq715_instruction_progress();
  }

  void write(const char* input, int count) override {
    if (count < 0 || static_cast<size_t>(count) > capacity_ - size_) {
      throw OutputLimit();
    }
    std::memcpy(output_ + size_, input, static_cast<size_t>(count));
    size_ += static_cast<size_t>(count);
    cix_zpaq715_instruction_progress();
  }

  size_t size() const { return size_; }

 private:
  uint8_t* output_;
  size_t capacity_;
  size_t size_ = 0;
};

class LimitedWriter final : public libzpaq::Writer {
 public:
  explicit LimitedWriter(size_t limit) : limit_(limit) {}

  void put(int byte) override {
    if (bytes_.size() == limit_) throw OutputLimit();
    bytes_.push_back(static_cast<uint8_t>(byte));
  }

  void write(const char* input, int count) override {
    if (count < 0 || static_cast<size_t>(count) > limit_ - bytes_.size()) {
      throw OutputLimit();
    }
    const auto* first = reinterpret_cast<const uint8_t*>(input);
    bytes_.insert(bytes_.end(), first, first + count);
  }

  uint8_t* release(size_t* size) {
    *size = bytes_.size();
    if (bytes_.empty()) return nullptr;
    auto* result = static_cast<uint8_t*>(std::malloc(bytes_.size()));
    if (result == nullptr) throw std::bad_alloc();
    std::memcpy(result, bytes_.data(), bytes_.size());
    return result;
  }

 private:
  size_t limit_;
  std::vector<uint8_t> bytes_;
};

void set_error(char* destination, size_t capacity, const char* message) {
  if (destination == nullptr || capacity == 0) return;
  const size_t n = std::strlen(message);
  const size_t copied = n < capacity - 1 ? n : capacity - 1;
  std::memcpy(destination, message, copied);
  destination[copied] = '\0';
}

template <class Operation>
int run(Operation operation, uint8_t** output, size_t* output_size,
        char* error, size_t error_capacity) {
  if (output == nullptr || output_size == nullptr) {
    set_error(error, error_capacity, "ZPAQ FFI output argument is null");
    return 1;
  }
  *output = nullptr;
  *output_size = 0;
  try {
    return operation();
  } catch (const OutputLimit& e) {
    set_error(error, error_capacity, e.what());
    return 2;
  } catch (const std::bad_alloc&) {
    set_error(error, error_capacity, "ZPAQ allocation failed");
    return 3;
  } catch (const std::exception& e) {
    set_error(error, error_capacity, e.what());
    return 1;
  } catch (...) {
    set_error(error, error_capacity, "unknown ZPAQ failure");
    return 1;
  }
}

}  // namespace

// libzpaq requires this callback. It is converted to an exception and caught
// before control returns over the C ABI.
void libzpaq::error(const char* message) {
  throw std::runtime_error(message == nullptr ? "libzpaq error" : message);
}

extern "C" int cix_zpaq715_compress_l5(const uint8_t* input, size_t input_size,
                                        size_t output_limit, uint8_t** output,
                                        size_t* output_size, char* error,
                                        size_t error_capacity, CancelCallback cancellation) {
  CancelScope cancel_scope(cancellation);
  if (input == nullptr && input_size != 0) {
    set_error(error, error_capacity, "ZPAQ FFI input argument is null");
    return 1;
  }
  return run([&] {
    SliceReader source(input, input_size);
    LimitedWriter destination(output_limit);
    // `5` is libzpaq's published maximum built-in level. The stream embeds
    // all decoder configuration and does not require a source dictionary.
    libzpaq::compress(&source, &destination, "5");
    *output = destination.release(output_size);
    return 0;
  }, output, output_size, error, error_capacity);
}

extern "C" int cix_zpaq715_decompress(const uint8_t* input, size_t input_size,
                                       uint8_t* output, size_t output_capacity,
                                       size_t native_memory_limit,
                                       size_t* output_size, char* error,
                                       size_t error_capacity, CancelCallback cancellation) {
  CancelScope cancel_scope(cancellation);
  if ((input == nullptr && input_size != 0) ||
      (output == nullptr && output_capacity != 0)) {
    set_error(error, error_capacity, "ZPAQ FFI input argument is null");
    return 1;
  }
  if (output_size == nullptr) {
    set_error(error, error_capacity, "ZPAQ FFI output argument is null");
    return 1;
  }
  *output_size = 0;
  try {
    // libzpaq's convenient findBlock() intentionally scans for a tag. CIXB1
    // is a strict single-payload envelope, so do not permit a valid block to
    // hide prefix/trailing junk in that scanner behaviour.
    static const uint8_t kTag[] = {
        0x37, 0x6b, 0x53, 0x74, 0xa0, 0x31, 0x83,
        0xd3, 0x8c, 0xb2, 0x28, 0xb0, 0xd3,
    };
    // CIXB1 backend-6 payloads must contain a framed, tagged ZPAQ stream.
    // Accepting an empty CIXB1 payload would create an unframed alternate
    // spelling and let a forged backend-6 envelope bypass this validation.
    if (input_size == 0) {
      set_error(error, error_capacity, "ZPAQ payload is empty and lacks a stream tag");
      return 1;
    }
    if (input_size < sizeof(kTag) || std::memcmp(input, kTag, sizeof(kTag)) != 0) {
      set_error(error, error_capacity, "ZPAQ payload lacks an initial stream tag");
      return 1;
    }
    SliceReader source(input, input_size);
    SliceWriter destination(output, output_capacity);
    cix_zpaq715_instruction_progress();
    libzpaq::Decompresser decoder;
    decoder.setInput(&source);
    while (true) {
      double block_memory = 0;
      if (!decoder.findBlock(&block_memory)) break;
      if (block_memory < 0 || block_memory > static_cast<double>(native_memory_limit)) {
        set_error(error, error_capacity, "ZPAQ stream model exceeds decoder memory limit");
        return 4;
      }
      while (decoder.findFilename()) {
        decoder.readComment();
        decoder.setOutput(&destination);
        while (decoder.decompress()) {}
        decoder.readSegmentEnd();
      }
      if (source.position() < static_cast<size_t>(decoder.buffered())) {
        set_error(error, error_capacity, "ZPAQ decoder cursor underflow");
        return 1;
      }
      const size_t next = source.position() - static_cast<size_t>(decoder.buffered());
      if (next == input_size) break;
      if (input_size - next < sizeof(kTag) ||
          std::memcmp(input + next, kTag, sizeof(kTag)) != 0) {
        set_error(error, error_capacity, "ZPAQ trailing data");
        return 1;
      }
    }
    // Decompresser owns a read-ahead buffer. Subtract it from the source
    // cursor for an exact payload-consumption assertion after the strict
    // block-boundary checks above.
    if (source.position() < static_cast<size_t>(decoder.buffered()) ||
        source.position() - static_cast<size_t>(decoder.buffered()) != input_size) {
      set_error(error, error_capacity, "ZPAQ trailing data");
      return 1;
    }
    *output_size = destination.size();
    return 0;
  } catch (const OutputLimit& e) {
    set_error(error, error_capacity, e.what());
    return 2;
  } catch (const std::bad_alloc&) {
    set_error(error, error_capacity, "ZPAQ allocation failed");
    return 3;
  } catch (const std::exception& e) {
    set_error(error, error_capacity, e.what());
    return 1;
  } catch (...) {
    set_error(error, error_capacity, "unknown ZPAQ failure");
    return 1;
  }
}

extern "C" void cix_zpaq715_free(void* pointer) { std::free(pointer); }

extern "C" const char* cix_zpaq715_version() { return "7.15"; }
