// SPDX-License-Identifier: MIT
// CONTENT ONLY / UNRUN. Fake C ABI; no codec, provider or installed-SDK qualification.
#include <cix_presentation.hpp>

#include <cstring>
#include <string>
#include <type_traits>

struct cix_context {};
static cix_context fake_context;

struct Fixture {
    cix_status operation = CIX_STATUS_CODEC_ERROR;
    cix_status sizing = CIX_STATUS_OUTPUT_TOO_SMALL;
    std::size_t sized_bytes = 1;
    cix_status retrieval = CIX_STATUS_OK;
    std::string text;
    unsigned operations = 0;
    unsigned reads = 0;
};
static Fixture fixture;  // Test stub state; never locale/catalog state.

extern "C" {
cix_status cix_options_v1_default(cix_options_v1 *out) {
    *out = {};
    out->abi_version = CIX_ABI_VERSION_1;
    out->struct_size = sizeof(*out);
    out->profile = CIX_PROFILE_FAST;
    out->workers = 1;
    return CIX_STATUS_OK;
}
cix_status cix_context_create(const cix_options_v1 *, cix_context **out) {
    *out = &fake_context;
    return CIX_STATUS_OK;
}
void cix_context_destroy(cix_context *) {}

cix_status cix_encode_buffer(cix_context *, const uint8_t *, size_t,
                             uint8_t *, size_t, size_t *needed) {
    ++fixture.operations;
    *needed = 17;
    return fixture.operation;
}
cix_status cix_decode_buffer(cix_context *, const uint8_t *, size_t,
                             uint8_t *, size_t, size_t *needed) {
    ++fixture.operations;
    *needed = 17;
    return fixture.operation;
}
cix_status cix_context_last_error(const cix_context *, char *output,
                                  size_t capacity, size_t *needed) {
    ++fixture.reads;
    if (output == nullptr) {
        *needed = fixture.sized_bytes;
        return fixture.sizing;
    }
    *needed = fixture.text.size() + 1;
    if (fixture.retrieval != CIX_STATUS_OK) return fixture.retrieval;
    if (capacity < *needed) return CIX_STATUS_OUTPUT_TOO_SMALL;
    std::memcpy(output, fixture.text.c_str(), *needed);
    return CIX_STATUS_OK;
}
}

using ExistingEncodeSignature = cix::Context::BufferResult (cix::Context::*)(
    const std::uint8_t *, std::size_t, std::uint8_t *, std::size_t);
static_assert(std::is_same_v<decltype(&cix::Context::encode_into), ExistingEncodeSignature>);

#define REQUIRE(condition) do { if (!(condition)) return 1; } while (false)

static int failure_case(cix::Context &context, cix_status first, std::size_t needed,
                        cix_status second, const std::string &opaque,
                        cix::presentation::DiagnosticOrigin expected_origin,
                        unsigned expected_reads) {
    using namespace cix::presentation;
    const std::string fallback = "native CIX operation failed without a diagnostic";
    fixture = {CIX_STATUS_CODEC_ERROR, first, needed, second, opaque, 0, 0};
    DiagnosticPresentation presentation;
    try {
        encode_into(context, nullptr, 0, nullptr, 0, presentation);
        return 1;
    } catch (const cix::StatusError &error) {
        const std::string bare = expected_origin == DiagnosticOrigin::missing_diagnostic
            ? fallback : opaque;
        const std::string english = "cix_encode_buffer: " + bare;
        const auto args = error.parameters();
        REQUIRE(error.status() == CIX_STATUS_CODEC_ERROR);
        REQUIRE(args.status == CIX_STATUS_CODEC_ERROR);
        REQUIRE(std::strcmp(error.message_id(), "sdk.native.operation_failed") == 0);
        REQUIRE(error.operation() == "cix_encode_buffer");
        REQUIRE(std::strcmp(args.operation, "cix_encode_buffer") == 0);
        REQUIRE(std::string(error.what()) == english);
        REQUIRE(std::string(error.detail()) == english);
        REQUIRE(std::string(args.detail) == english);
        REQUIRE(presentation.origin == expected_origin);
        REQUIRE(fixture.operations == 1 && fixture.reads == expected_reads);

        unsigned rendered = 0;
        const std::string display_leaf = "synthetic e\xcc\x81 {literal}";
        const auto display = render_missing_diagnostic(error, presentation,
            [&](const char *id, std::string &out) {
                ++rendered;
                if (std::strcmp(id, "sdk.native.missing_diagnostic") != 0) return false;
                out = display_leaf;
                return true;
            });
        if (expected_origin == DiagnosticOrigin::missing_diagnostic) {
            REQUIRE(rendered == 1);
            REQUIRE(display == "cix_encode_buffer: " + display_leaf);
            REQUIRE(std::strcmp(presentation.message_id(), "sdk.native.missing_diagnostic") == 0);
        } else {
            REQUIRE(rendered == 0 && display == english);
            REQUIRE(presentation.message_id() == nullptr);
        }
        REQUIRE(render_missing_diagnostic(error, presentation,
            [&](const char *, std::string &out) { out = fallback; return true; }) == english);
        REQUIRE(render_missing_diagnostic(error, presentation,
            [](const char *, std::string &out) { out = "discard partial"; return false; }) == english);
        REQUIRE(render_missing_diagnostic(error, presentation,
            [](const char *, std::string &) { return true; }) == english);
        REQUIRE(std::string(error.what()) == english && std::string(error.parameters().detail) == english);
        REQUIRE(fixture.reads == expected_reads);  // Rendering never re-fetches diagnostics.
    }
    return 0;
}

int main() {
    using namespace cix::presentation;
    cix::Context context;
    const std::string fallback = "native CIX operation failed without a diagnostic";
    const std::string opaque = "opaque UTF-8: \xc3\xa9 {operation}";
    REQUIRE(failure_case(context, CIX_STATUS_INVALID_ARGUMENT, 49, CIX_STATUS_OK, "",
                         DiagnosticOrigin::missing_diagnostic, 1) == 0);
    REQUIRE(failure_case(context, CIX_STATUS_OUTPUT_TOO_SMALL, 0, CIX_STATUS_OK, "",
                         DiagnosticOrigin::missing_diagnostic, 1) == 0);
    REQUIRE(failure_case(context, CIX_STATUS_OUTPUT_TOO_SMALL, 49, CIX_STATUS_CODEC_ERROR, "",
                         DiagnosticOrigin::missing_diagnostic, 2) == 0);
    REQUIRE(failure_case(context, CIX_STATUS_OUTPUT_TOO_SMALL, fallback.size() + 1,
                         CIX_STATUS_OK, fallback, DiagnosticOrigin::opaque_native, 2) == 0);
    REQUIRE(failure_case(context, CIX_STATUS_OUTPUT_TOO_SMALL, opaque.size() + 1,
                         CIX_STATUS_OK, opaque, DiagnosticOrigin::opaque_native, 2) == 0);
    REQUIRE(failure_case(context, CIX_STATUS_OUTPUT_TOO_SMALL, 1,
                         CIX_STATUS_OK, "", DiagnosticOrigin::opaque_native, 2) == 0);

    // Reuse must not carry an earlier missing-leaf classification into success.
    DiagnosticPresentation presentation{DiagnosticOrigin::missing_diagnostic};
    for (const cix_status status : {CIX_STATUS_OK, CIX_STATUS_OUTPUT_TOO_SMALL}) {
        presentation.origin = DiagnosticOrigin::missing_diagnostic;
        fixture = {};
        fixture.operation = status;
        const auto result = encode_into(context, nullptr, 0, nullptr, 0, presentation);
        REQUIRE(result.status == status && result.written == 17);
        REQUIRE(presentation.origin == DiagnosticOrigin::not_captured);
        REQUIRE(presentation.message_id() == nullptr);
        REQUIRE(fixture.operations == 1 && fixture.reads == 0);
    }

    // The original method keeps its existing error text/metadata.
    fixture = {CIX_STATUS_CODEC_ERROR, CIX_STATUS_INVALID_ARGUMENT, 49, CIX_STATUS_OK, "", 0, 0};
    try { context.encode_into(nullptr, 0, nullptr, 0); return 1; }
    catch (const cix::StatusError &error) {
        REQUIRE(std::string(error.what()) == "cix_encode_buffer: " + fallback);
        REQUIRE(std::strcmp(error.message_id(), "sdk.native.operation_failed") == 0);
        REQUIRE(std::string(error.parameters().detail) == error.what());
    }
    return 0;
}
