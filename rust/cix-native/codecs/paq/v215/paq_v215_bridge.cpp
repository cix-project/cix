#include "paq_v215_bridge.h"

#include <atomic>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <exception>
#include <sys/stat.h>
#include <string>
#include <vector>

/* Defined by unchanged upstream src/paq8px.cpp. */
int processCommandLine(int argc, char **argv);

namespace {
std::atomic_flag used = ATOMIC_FLAG_INIT;

void message(char *buffer, size_t size, const char *text) {
    if (buffer != nullptr && size != 0) {
        std::snprintf(buffer, size, "%s", text);
    }
}

bool regular_output_missing(const char *path) {
    struct stat state {};
    #if defined(_WIN32)
    const int result = stat(path, &state);
#else
    const int result = lstat(path, &state);
#endif
    return result != 0 && errno == ENOENT;
}

bool regular_output_present(const char *path) {
    struct stat state {};
    return stat(path, &state) == 0 && S_ISREG(state.st_mode);
}
}  // namespace

extern "C" int cix_paq_v215_process(
    int argc,
    const char *const argv[],
    const char *expected_output,
    char *error_buffer,
    size_t error_buffer_size
) {
    message(error_buffer, error_buffer_size, "");
    if (argc < 2 || argv == nullptr || expected_output == nullptr || expected_output[0] == '\0') {
        message(error_buffer, error_buffer_size, "invalid PAQ v215 worker arguments");
        return CIX_PAQ_V215_INVALID_ARGUMENT;
    }
    for (int index = 0; index < argc; ++index) {
        if (argv[index] == nullptr) {
            message(error_buffer, error_buffer_size, "PAQ v215 argument vector contains null");
            return CIX_PAQ_V215_INVALID_ARGUMENT;
        }
    }
    if (!regular_output_missing(expected_output)) {
        message(error_buffer, error_buffer_size, "PAQ v215 output must not exist before invocation");
        return CIX_PAQ_V215_INVALID_ARGUMENT;
    }
    if (used.test_and_set(std::memory_order_acq_rel)) {
        message(error_buffer, error_buffer_size, "PAQ v215 bridge permits one invocation per worker process");
        return CIX_PAQ_V215_ALREADY_USED;
    }

    try {
        // Upstream owns a mutable argv API. Keep caller storage immutable and
        // catch allocation errors before they can cross the C ABI boundary.
        std::vector<std::string> argument_storage;
        argument_storage.reserve(static_cast<size_t>(argc));
        for (int index = 0; index < argc; ++index) {
            argument_storage.emplace_back(argv[index]);
        }
        std::vector<char *> mutable_argv;
        mutable_argv.reserve(static_cast<size_t>(argc) + 1);
        for (auto &argument : argument_storage) {
            mutable_argv.push_back(argument.data());
        }
        mutable_argv.push_back(nullptr);
        const int status = processCommandLine(argc, mutable_argv.data());
        if (status != 0) {
            message(error_buffer, error_buffer_size, "PAQ v215 returned a non-zero status");
            return CIX_PAQ_V215_UPSTREAM_FAILURE;
        }
    } catch (const std::exception &error) {
        message(error_buffer, error_buffer_size, error.what());
        return CIX_PAQ_V215_EXCEPTION;
    } catch (...) {
        message(error_buffer, error_buffer_size, "PAQ v215 raised an unknown exception");
        return CIX_PAQ_V215_EXCEPTION;
    }

    if (!regular_output_present(expected_output)) {
        message(error_buffer, error_buffer_size, "PAQ v215 completed without the requested output");
        return CIX_PAQ_V215_UPSTREAM_FAILURE;
    }
    return CIX_PAQ_V215_OK;
}

extern "C" const char *cix_paq_v215_bridge_version(void) {
    return "cix-paq-v215-bridge/1";
}
