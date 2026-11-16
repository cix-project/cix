#ifndef CIX_PRESENTATION_H
#define CIX_PRESENTATION_H

#include "cix.h"

#ifdef __cplusplus
extern "C" {
#endif

#define CIX_PRESENTATION_ABI_VERSION_1 UINT32_C(1)

/**
 * Immutable borrowed pattern for sdk.buffer.output_too_small (no parameters).
 * The caller resolves locale externally. Set abi_version to 1 and struct_size
 * to sizeof(cix_buffer_presentation_v1); later trailing fields are ignored.
 * Length is UTF-8 bytes excluding a terminator. CIX retains no caller pointer.
 */
typedef struct cix_buffer_presentation_v1 {
    uint32_t abi_version;
    uint32_t struct_size;
    const char *output_too_small_pattern;
    size_t output_too_small_pattern_len;
} cix_buffer_presentation_v1;

/**
 * Copy the then-current diagnostic using the caller's per-call pattern.
 *
 * NULL presentation delegates cix_context_last_error exactly. Otherwise only
 * an error originating at the selected C buffer short-capacity setter is
 * eligible; identical English set by a legacy path is not translated.
 *
 * Normal Localizer validation and brace processing apply: for example,
 * "memoire {{courte}}" renders "memoire {courte}". Missing, empty, over-511-byte,
 * invalid UTF-8, interior-NUL, malformed-brace or undeclared-placeholder
 * candidates use the whole retained English. A rendered translated leaf must
 * be nonempty, NUL-free UTF-8 of at most 511 bytes. Unselected/cleared legacy
 * text, including empty text or interior NUL, is preserved byte for byte.
 *
 * needed includes one appended NUL. OUTPUT_TOO_SMALL (3) writes only needed;
 * output remains untouched. OK (0) copies exactly the bytes and one NUL.
 * An empty diagnostic needs 1 byte: capacity 0 returns 3, capacity 1 returns 0.
 * Invalid version, record size, pointer/range or overlap returns
 * INVALID_ARGUMENT (1) without output/needed writes; NULL pattern with a
 * nonzero length is invalid. Diagnostic lock poison/contained unwind returns
 * PANIC (5). Internal message arguments make the leaf ineligible and fall back
 * with ordinary 0/3 capacity behavior; they do not add a caller ABI error.
 *
 * The caller supplies a live context not being destroyed, properly aligned
 * writable needed storage, and writable output for its capacity (NULL only at
 * capacity 0). A non-NULL presentation has a readable eight-byte prefix and
 * its advertised complete v1 prefix. Pattern storage is readable/immutable
 * throughout the call. Output and needed do not overlap each other, context,
 * record or pattern. Input record/pattern do not alias the context.
 * Overlong candidates are not read. English and owned identity are snapshotted
 * together; the diagnostic lock is released before rendering. The temporary
 * catalog dies at return. Independent contexts may share immutable patterns.
 * Each query/copy snapshots independently; no stable diagnostic is promised
 * across calls. Existing context concurrency/destruction rules remain.
 */
cix_status cix_context_last_error_presented_v1(
    const cix_context *context,
    const cix_buffer_presentation_v1 *presentation,
    char *output, size_t output_capacity, size_t *needed);

#ifdef __cplusplus
}
#endif

#endif /* CIX_PRESENTATION_H */
