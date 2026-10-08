// SPDX-License-Identifier: MIT
#include "cix_sqfs_userspace.h"
#include "cix_squashfs_profile.h"

int cix_sqfs_userspace_decode(uint16_t compression_id,
    enum cix_sqfs_block_class block_class, const void *compressed,
    size_t compressed_size, void *output, size_t output_capacity,
    size_t *output_size)
{
    enum cix_squashfs_profile_kind kind;
    int status;
    if (output_size == 0 || compression_id != CIX_SQFS_EXPERIMENTAL_COMPRESSION_ID)
        return -1;
    switch (block_class) {
    case CIX_SQFS_BLOCK_DATA:
    case CIX_SQFS_BLOCK_FRAGMENT:
        kind = CIX_SQUASHFS_DATA;
        break;
    case CIX_SQFS_BLOCK_METADATA:
    case CIX_SQFS_BLOCK_XATTR:
        kind = CIX_SQUASHFS_METADATA;
        break;
    default:
        return -1;
    }
    status = cix_squashfs_profile_decode(kind, compressed, compressed_size,
        output, output_capacity, output_size);
    return status == CIX_SQUASHFS_OK ? 0 : -1;
}
