// SPDX-License-Identifier: MIT
/* Experimental CIX HDF5 chunk filter.  This source depends only on the
 * installed HDF5 C API and the public installed CIX SDK; it has no Python
 * runtime or full-engine dependency. */
#include <limits.h>
#include <stddef.h>
#include <stdint.h>

#include <hdf5.h>
#include <H5PLextern.h>
#include <cix.h>

#ifndef CIX_HDF5_FILTER_ID
#error "CIX_HDF5_FILTER_ID must be supplied by the configuring deployment"
#endif
#ifndef CIX_HDF5_MAX_CHUNK_MIB
#error "CIX_HDF5_MAX_CHUNK_MIB must be supplied by the configuring deployment"
#endif
#ifndef CIX_HDF5_MAX_MEMORY_MIB
#error "CIX_HDF5_MAX_MEMORY_MIB must be supplied by the configuring deployment"
#endif
#ifndef CIX_HDF5_MAX_WORKERS
#error "CIX_HDF5_MAX_WORKERS must be supplied by the configuring deployment"
#endif

#define CIX_HDF5_PARAMETER_VERSION 1U
#define CIX_HDF5_PARAMETER_COUNT 5U
#define CIX_HDF5_MIB ((uint64_t)1024U * 1024U)
#define CIX_HDF5_MAX_CHUNK_BYTES ((uint64_t)CIX_HDF5_MAX_CHUNK_MIB * CIX_HDF5_MIB)

/* cd_values is durable file metadata:
 *   [0] parameter ABI version (1)
 *   [1] CIX profile (CIX_PROFILE_*)
 *   [2] CIX worker count (>= 1)
 *   [3] complete-output limit in MiB (>= 1)
 *   [4] native working-memory limit in MiB (>= 1)
 * No library path, host setting, or process-local configuration is serialized.
 */
static int
cix_hdf5_options(size_t count, const unsigned values[], cix_options_v1 *options)
{
    uint64_t output_limit;
    uint64_t memory_limit;

    if (values == NULL || options == NULL || count != CIX_HDF5_PARAMETER_COUNT ||
        values[0] != CIX_HDF5_PARAMETER_VERSION || values[1] < CIX_PROFILE_FAST ||
        values[1] > CIX_PROFILE_BEST || values[2] == 0 || values[2] > CIX_HDF5_MAX_WORKERS ||
        values[3] == 0 || values[3] > CIX_HDF5_MAX_CHUNK_MIB || values[4] == 0 ||
        values[4] > CIX_HDF5_MAX_MEMORY_MIB ||
        values[3] > UINT64_MAX / CIX_HDF5_MIB || values[4] > UINT64_MAX / CIX_HDF5_MIB)
        return 0;

    output_limit = (uint64_t)values[3] * CIX_HDF5_MIB;
    memory_limit = (uint64_t)values[4] * CIX_HDF5_MIB;
    if (output_limit > SIZE_MAX || memory_limit > SIZE_MAX)
        return 0;

    if (cix_options_v1_default(options) != CIX_STATUS_OK)
        return 0;
    options->profile = values[1];
    options->workers = values[2];
    options->output_limit = output_limit;
    options->memory_limit = memory_limit;
    return 1;
}

static size_t
cix_hdf5_filter(unsigned flags, size_t cd_nelmts, const unsigned cd_values[], size_t nbytes,
                size_t *buf_size, void **buf)
{
    cix_context *context = NULL;
    cix_options_v1 options;
    cix_status status;
    size_t needed = 0;
    size_t written = 0;
    void *replacement = NULL;
    const uint8_t *input;

    if (buf_size == NULL || buf == NULL || *buf_size < nbytes || nbytes > CIX_HDF5_MAX_CHUNK_BYTES ||
        (nbytes != 0 && *buf == NULL) ||
        !cix_hdf5_options(cd_nelmts, cd_values, &options))
        return 0;
    input = (const uint8_t *)*buf;
    if (cix_context_create(&options, &context) != CIX_STATUS_OK || context == NULL)
        return 0;

    if ((flags & H5Z_FLAG_REVERSE) != 0)
        status = cix_decode_buffer(context, input, nbytes, NULL, 0, &needed);
    else
        status = cix_encode_buffer(context, input, nbytes, NULL, 0, &needed);
    if (status != CIX_STATUS_OUTPUT_TOO_SMALL || needed == 0 || needed > options.output_limit)
        goto done;

    /* Optional HDF5 filters use a zero return to record an unfiltered chunk.
     * We never write an ambiguous raw payload under this filter's mask. */
    if ((flags & H5Z_FLAG_REVERSE) == 0 && needed >= nbytes)
        goto done;

    replacement = H5allocate_memory(needed, 0);
    if (replacement == NULL)
        goto done;
    if ((flags & H5Z_FLAG_REVERSE) != 0)
        status = cix_decode_buffer(context, input, nbytes, (uint8_t *)replacement, needed, &written);
    else
        status = cix_encode_buffer(context, input, nbytes, (uint8_t *)replacement, needed, &written);
    if (status != CIX_STATUS_OK || written != needed)
        goto done;

    /* H5allocate_memory/H5free_memory preserve HDF5 allocator ownership across
     * an ABI boundary; do not hand HDF5 a CIX-owned allocation. */
    (void)H5free_memory(*buf);
    *buf = replacement;
    *buf_size = needed;
    cix_context_destroy(context);
    return needed;

done:
    if (replacement != NULL)
        (void)H5free_memory(replacement);
    cix_context_destroy(context);
    return 0;
}

static const H5Z_class2_t cix_hdf5_filter_class = {
    H5Z_CLASS_T_VERS,
    (H5Z_filter_t)CIX_HDF5_FILTER_ID,
    1,
    1,
    "CIX experimental native chunk filter v1",
    NULL,
    NULL,
    cix_hdf5_filter,
};

H5PL_type_t
H5PLget_plugin_type(void)
{
    return H5PL_TYPE_FILTER;
}

const void *
H5PLget_plugin_info(void)
{
    return &cix_hdf5_filter_class;
}
