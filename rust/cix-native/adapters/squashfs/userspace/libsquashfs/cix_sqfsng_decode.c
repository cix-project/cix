// SPDX-License-Identifier: MIT
/* CIX-owned call-site bridge for the pinned squashfs-tools-ng/libsqfs overlay.
 * It decodes actual SquashFS compressed payload bytes, never CIX archives. */
#include "cix_sqfs_userspace.h"

int cix_sqfsng_decode_block(uint16_t method, int table_or_xattr,
    int fragment, const void *input, size_t input_size, void *output,
    size_t output_size)
{
    enum cix_sqfs_block_class block_class = table_or_xattr ?
        CIX_SQFS_BLOCK_METADATA : (fragment ? CIX_SQFS_BLOCK_FRAGMENT : CIX_SQFS_BLOCK_DATA);
    return cix_sqfs_userspace_decode(method, block_class, input, input_size,
        output, output_size, &output_size);
}
