// SPDX-License-Identifier: MIT
#ifndef CIX_PRESENTATION_HPP
#define CIX_PRESENTATION_HPP

#include <cstddef>
#include <string>
#include <utility>
#include <vector>

#include "cix.hpp"

namespace cix::presentation {

/** New opt-in C++ source API. Not part of the C ABI or existing owner layout. */
enum class DiagnosticOrigin { not_captured, opaque_native, missing_diagnostic };

/** Pair only with the StatusError from the same encode_into call. */
struct DiagnosticPresentation {
    DiagnosticOrigin origin = DiagnosticOrigin::not_captured;
    const char *message_id() const noexcept {
        return origin == DiagnosticOrigin::missing_diagnostic
            ? "sdk.native.missing_diagnostic" : nullptr;
    }
    // The selected leaf has exactly zero named arguments.
};

namespace detail {

/** Same fetch conditions/string construction as the pinned Context::last_error. */
inline std::string last_error(const cix_context *context, DiagnosticPresentation &presentation) {
    size_t needed = 0;
    const cix_status first = cix_context_last_error(context, nullptr, 0, &needed);
    if (first != CIX_STATUS_OUTPUT_TOO_SMALL || needed == 0) {
        presentation.origin = DiagnosticOrigin::missing_diagnostic;
        return "native CIX operation failed without a diagnostic";
    }
    std::vector<char> text(needed);
    const cix_status second = cix_context_last_error(context, text.data(), text.size(), &needed);
    if (second != CIX_STATUS_OK || text.empty()) {
        presentation.origin = DiagnosticOrigin::missing_diagnostic;
        return "native CIX operation failed without a diagnostic";
    }
    presentation.origin = DiagnosticOrigin::opaque_native;
    return std::string(text.data());
}

}  // namespace detail

/** Explicit alternative to Context::encode_into. One call, no hidden retry.
 * Existing Context methods and exception metadata remain unchanged.
 * Success/OUTPUT_TOO_SMALL clears capture and keeps the original result contract.
 */
inline Context::BufferResult encode_into(
    Context &context, const std::uint8_t *input, std::size_t length,
    std::uint8_t *output, std::size_t capacity, DiagnosticPresentation &presentation) {
    presentation.origin = DiagnosticOrigin::not_captured;
    std::size_t written = 0;
    const cix_status status = cix_encode_buffer(
        context.native_handle(), input, length, output, capacity, &written);
    if (status != CIX_STATUS_OK && status != CIX_STATUS_OUTPUT_TOO_SMALL) {
        const std::string diagnostic = detail::last_error(context.native_handle(), presentation);
        throw StatusError(status, std::string("cix_encode_buffer") + ": " + diagnostic,
                          ErrorMessage::operation_failed, "cix_encode_buffer");
    }
    return {status, written};
}

/** Host callback receives only the zero-argument leaf ID and a fresh output.
 * It returns true only for a complete, validated, nonempty UTF-8 leaf.
 * Missing/invalid catalog data must return false. No opaque detail is supplied.
 * The operation Code and separator stay invariant. No original error is changed.
 */
template<class RenderZeroArgumentLeaf>
std::string render_missing_diagnostic(
    const StatusError &error, const DiagnosticPresentation &presentation,
    RenderZeroArgumentLeaf &&render) {
    const char *id = presentation.message_id();
    if (id == nullptr) return error.what();
    std::string rendered;
    if (!std::forward<RenderZeroArgumentLeaf>(render)(id, rendered) || rendered.empty()) {
        return error.what();
    }
    return error.operation() + ": " + rendered;
}

}  // namespace cix::presentation

#endif  // CIX_PRESENTATION_HPP
