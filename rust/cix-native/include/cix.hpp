#ifndef CIX_HPP
#define CIX_HPP

#include <cstdint>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include "cix.h"

namespace cix {

/** Locale-independent classification for native SDK error presentation. */
enum class ErrorMessage { legacy_error, operation_failed, moved_from };
/** Typed named parameters: operation is invariant Code, status is Number, and
 * detail is untranslated Text. Pointers remain valid only while the error lives. */
struct ErrorParameters {
    const char *operation;
    cix_status status;
    const char *detail;
};

/** Retains native status and the exact legacy English diagnostic. No catalog
 * lookup, locale/environment access or automatic translation occurs here.
 * Hosts may render a complete validated immutable catalog message; on missing
 * IDs/keys/types they retain what() as the complete English fallback. */
class StatusError : public std::runtime_error {
public:
    StatusError(cix_status status, std::string message)
        : StatusError(status, std::move(message), ErrorMessage::legacy_error, {}) {}
    StatusError(cix_status status, std::string message, ErrorMessage kind,
                std::string operation)
        : std::runtime_error(std::move(message)), status_(status), kind_(kind),
          operation_(std::move(operation)) {}
    cix_status status() const noexcept { return status_; }
    const char *message_id() const noexcept {
        switch (kind_) {
        case ErrorMessage::operation_failed: return "sdk.native.operation_failed";
        case ErrorMessage::moved_from: return "sdk.native.moved_from";
        default: return "sdk.native.legacy_error";
        }
    }
    const std::string &operation() const noexcept { return operation_; }
    const char *detail() const noexcept { return what(); }
    ErrorParameters parameters() const noexcept {
        return {operation_.c_str(), status_, what()};
    }
private:
    cix_status status_;
    ErrorMessage kind_;
    std::string operation_;
};

/** Move-only RAII owner for one native complete-buffer CIX context. */
class Context {
public:
    /** Construct a context using native v1 defaults. */
    Context() : Context(default_options()) {}

    /** Construct a context from a complete validated v1 options record. */
    explicit Context(const cix_options_v1 &options) {
        cix_context *created = nullptr;
        const cix_status status = cix_context_create(&options, &created);
        if (status != CIX_STATUS_OK) {
            throw StatusError(status, "cix_context_create failed", ErrorMessage::operation_failed, "cix_context_create");
        }
        context_ = created;
    }

    ~Context() { cix_context_destroy(context_); }

    Context(const Context &) = delete;
    Context &operator=(const Context &) = delete;

    Context(Context &&other) noexcept : context_(std::exchange(other.context_, nullptr)) {}

    Context &operator=(Context &&other) noexcept {
        if (this != &other) {
            cix_context_destroy(context_);
            context_ = std::exchange(other.context_, nullptr);
        }
        return *this;
    }

    /** Return the native ABI's complete v1 default options record. */
    static cix_options_v1 default_options() {
        cix_options_v1 options{};
        const cix_status status = cix_options_v1_default(&options);
        if (status != CIX_STATUS_OK) {
            throw StatusError(status, "cix_options_v1_default failed", ErrorMessage::operation_failed, "cix_options_v1_default");
        }
        return options;
    }

    /** Native caller-buffer result. OUTPUT_TOO_SMALL preserves required size
     * and leaves output unchanged. Other statuses retain StatusError/diagnostic.
     */
    struct BufferResult { cix_status status; std::size_t written; };

    /** Encode once into caller-owned, disjoint live buffers. NULL is allowed
     * only for zero-length ranges. No output allocation or retry is hidden.
     * Resource limits/serialization and native complete-buffer bounds apply.
     */
    BufferResult encode_into(const std::uint8_t *input, std::size_t length,
                             std::uint8_t *output, std::size_t capacity) {
        return process_into(input, length, output, capacity, cix_encode_buffer, "cix_encode_buffer");
    }

    /** Decode once with the same caller-buffer/status/count contract. */
    BufferResult decode_into(const std::uint8_t *input, std::size_t length,
                             std::uint8_t *output, std::size_t capacity) {
        return process_into(input, length, output, capacity, cix_decode_buffer, "cix_decode_buffer");
    }

    /** Encode one complete byte vector, allocating its returned result. */
    std::vector<std::uint8_t> encode(const std::vector<std::uint8_t> &input) {
        return process(input, cix_encode_buffer, "cix_encode_buffer");
    }

    /** Decode one complete archive vector, allocating its returned result. */
    std::vector<std::uint8_t> decode(const std::vector<std::uint8_t> &input) {
        return process(input, cix_decode_buffer, "cix_decode_buffer");
    }

    cix_context *native_handle() noexcept { return context_; }
    const cix_context *native_handle() const noexcept { return context_; }

private:
    using BufferOperation = cix_status (*)(cix_context *, const std::uint8_t *, size_t,
                                            std::uint8_t *, size_t, size_t *);

    BufferResult process_into(const std::uint8_t *input, std::size_t length,
                              std::uint8_t *output, std::size_t capacity,
                              BufferOperation operation, const char *operation_name) {
        std::size_t written = 0;
        const cix_status status = operation(context_, input, length, output, capacity, &written);
        if (status != CIX_STATUS_OK && status != CIX_STATUS_OUTPUT_TOO_SMALL) {
            throw_status(status, operation_name);
        }
        return {status, written};
    }

    std::vector<std::uint8_t> process(const std::vector<std::uint8_t> &input,
                                      BufferOperation operation,
                                      const char *operation_name) {
        size_t needed = 0;
        const std::uint8_t *source = input.empty() ? nullptr : input.data();
        cix_status status = operation(context_, source, input.size(), nullptr, 0, &needed);
        if (status == CIX_STATUS_OK && needed == 0) {
            return {};
        }
        if (status != CIX_STATUS_OUTPUT_TOO_SMALL) {
            throw_status(status, operation_name);
        }
        std::vector<std::uint8_t> output(needed);
        std::uint8_t *destination = output.empty() ? nullptr : output.data();
        status = operation(context_, source, input.size(), destination, output.size(), &needed);
        if (status != CIX_STATUS_OK) {
            throw_status(status, operation_name);
        }
        output.resize(needed);
        return output;
    }

    [[noreturn]] void throw_status(cix_status status, const char *operation_name) const {
        throw StatusError(status, std::string(operation_name) + ": " + last_error(),
                          ErrorMessage::operation_failed, operation_name);
    }

    std::string last_error() const {
        size_t needed = 0;
        const cix_status first = cix_context_last_error(context_, nullptr, 0, &needed);
        if (first != CIX_STATUS_OUTPUT_TOO_SMALL || needed == 0) {
            return "native CIX operation failed without a diagnostic";
        }
        std::vector<char> text(needed);
        const cix_status second = cix_context_last_error(context_, text.data(), text.size(), &needed);
        if (second != CIX_STATUS_OK || text.empty()) {
            return "native CIX operation failed without a diagnostic";
        }
        return std::string(text.data());
    }

    cix_context *context_ = nullptr;
};

}  // namespace cix

#endif  // CIX_HPP
