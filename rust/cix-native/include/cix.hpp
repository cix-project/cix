#ifndef CIX_HPP
#define CIX_HPP

#include <cstdint>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include "cix.h"

namespace cix {

/** Exception that retains the #cix_status reported by the native ABI. */
class StatusError : public std::runtime_error {
public:
    StatusError(cix_status status, std::string message)
        : std::runtime_error(std::move(message)), status_(status) {}

    cix_status status() const noexcept { return status_; }

private:
    cix_status status_;
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
            throw StatusError(status, "cix_context_create failed");
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
            throw StatusError(status, "cix_options_v1_default failed");
        }
        return options;
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
        throw StatusError(status, std::string(operation_name) + ": " + last_error());
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
