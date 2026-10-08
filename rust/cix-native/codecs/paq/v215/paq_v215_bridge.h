/* CIX-owned worker-only boundary for the unchanged PAQ v215 sources. */
#ifndef CIX_PAQ_V215_BRIDGE_H
#define CIX_PAQ_V215_BRIDGE_H

#include <stddef.h>

#if defined(_WIN32)
#  if defined(CIX_PAQ_V215_BUILDING)
#    define CIX_PAQ_V215_API __declspec(dllexport)
#  else
#    define CIX_PAQ_V215_API __declspec(dllimport)
#  endif
#else
#  define CIX_PAQ_V215_API __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

/** Result returned by the isolated PAQ v215 worker boundary. */
enum cix_paq_v215_status {
    /** The upstream invocation completed and created the requested regular file. */
    CIX_PAQ_V215_OK = 0,
    /** The supplied worker arguments or pre-invocation output path were invalid. */
    CIX_PAQ_V215_INVALID_ARGUMENT = 1,
    /** A previous invocation attempt consumed this worker's one-shot admission slot. */
    CIX_PAQ_V215_ALREADY_USED = 2,
    /** Upstream returned failure or did not create the requested regular output file. */
    CIX_PAQ_V215_UPSTREAM_FAILURE = 3,
    /** A C++ exception was caught before it could cross this C ABI. */
    CIX_PAQ_V215_EXCEPTION = 4
};

/**
 * Invoke PAQ v215's unchanged command-line implementation exactly once in an
 * isolated CIX worker. `argv` is passed verbatim to processCommandLine.
 * `expected_output` must name a non-existent ordinary output file before the
 * call and is checked after it. This converts PAQ's intentional-error return
 * convention (which can be zero) into a stable status for the worker.
 *
 * The CIX bridge itself does not fork, call exit, change environment variables,
 * or install signal/resource handlers. Upstream PAQ contains LSTM error paths
 * that call exit(1), so this API is deliberately unsuitable for a native SDK:
 * PAQ v215 retains process-lifetime static model state. The caller owns the
 * isolated same-CIX worker, directory, environment, rlimits and cancellation.
 *
 * @param argc Number of non-null entries in @p argv; it must be at least two.
 * @param argv Immutable argument values copied into a mutable upstream argv.
 * @param expected_output Non-empty path absent before the call and regular on success.
 * @param error_buffer Optional writable diagnostic buffer, NUL-terminated when non-null with capacity above zero.
 * @param error_buffer_size Capacity of @p error_buffer in bytes.
 * @return A value from #cix_paq_v215_status.
 */
CIX_PAQ_V215_API int cix_paq_v215_process(
    int argc,
    const char *const argv[],
    const char *expected_output,
    char *error_buffer,
    size_t error_buffer_size
);

/** Return a process-lifetime static identifier for this CIX worker bridge. */
CIX_PAQ_V215_API const char *cix_paq_v215_bridge_version(void);

#ifdef __cplusplus
}
#endif
#endif
