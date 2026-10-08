// SPDX-License-Identifier: MIT
/* Replacement translation unit for Squashfuse 0.5.0 (3f4dd292...).
 * Compile this instead of decompress.c, defining CIX_SQUASHFUSE_DECOMPRESS_SOURCE
 * to that unchanged source file. */
#include "cix_sqfs_userspace.h"
#include "decompress.h"

#ifndef CIX_SQUASHFUSE_DECOMPRESS_SOURCE
#error "define CIX_SQUASHFUSE_DECOMPRESS_SOURCE to the pinned decompress.c"
#endif

#define sqfs_decompressor_get cix_upstream_decompressor_get
#define sqfs_compression_name cix_upstream_compression_name
#define sqfs_compression_supported cix_upstream_compression_supported
#include CIX_SQUASHFUSE_DECOMPRESS_SOURCE
#undef sqfs_decompressor_get
#undef sqfs_compression_name
#undef sqfs_compression_supported

static sqfs_err cix_decompressor(void *in, size_t insz, void *out, size_t *outsz)
{
    size_t written = 0;
    enum cix_sqfs_block_class cls = *outsz <= 8192 ? CIX_SQFS_BLOCK_METADATA : CIX_SQFS_BLOCK_DATA;
    if (cix_sqfs_userspace_decode(CIX_SQFS_EXPERIMENTAL_COMPRESSION_ID, cls,
        in, insz, out, *outsz, &written) != 0)
        return SQFS_ERR;
    *outsz = written;
    return SQFS_OK;
}

sqfs_decompressor sqfs_decompressor_get(sqfs_compression_type type)
{
    return type == CIX_SQFS_EXPERIMENTAL_COMPRESSION_ID ? cix_decompressor : cix_upstream_decompressor_get(type);
}

char *sqfs_compression_name(sqfs_compression_type type)
{
    return type == CIX_SQFS_EXPERIMENTAL_COMPRESSION_ID ? "cix-experimental" : cix_upstream_compression_name(type);
}

void sqfs_compression_supported(sqfs_compression_type *types)
{
    /* The upstream fixed 16-entry reporting array cannot represent 65001.
     * Dispatch above is authoritative; do not write outside its ABI. */
    cix_upstream_compression_supported(types);
}

int cix_squashfuse_decode_block(uint16_t method, int metadata_or_xattr,
    int fragment, const void *input, size_t input_size, void *output,
    size_t output_size)
{
    enum cix_sqfs_block_class block_class = metadata_or_xattr ?
        CIX_SQFS_BLOCK_METADATA : (fragment ? CIX_SQFS_BLOCK_FRAGMENT : CIX_SQFS_BLOCK_DATA);
    return cix_sqfs_userspace_decode(method, block_class, input, input_size,
        output, output_size, &output_size);
}
