// SPDX-License-Identifier: MIT
/* Experimental userspace-only SquashFS CIX decoder bridge. */
#ifndef CIX_SQFS_USERSPACE_H
#define CIX_SQFS_USERSPACE_H

#include <stddef.h>
#include <stdint.h>

#define CIX_SQFS_EXPERIMENTAL_COMPRESSION_ID 65001u

/** @brief SquashFS source block class supplied by the reader overlay. */
enum cix_sqfs_block_class {
    CIX_SQFS_BLOCK_DATA = 0,      /**< Regular data block. */
    CIX_SQFS_BLOCK_FRAGMENT = 1,  /**< Fragment data block. */
    CIX_SQFS_BLOCK_METADATA = 2,  /**< Metadata table block. */
    CIX_SQFS_BLOCK_XATTR = 3      /**< Extended-attribute block. */
};

/**
 * @brief Decodes one CIX experimental SquashFS compressed block.
 *
 * @param compression_id Superblock method; only
 * #CIX_SQFS_EXPERIMENTAL_COMPRESSION_ID is accepted.
 * @param block_class Actual source block class, which chooses the profile cap.
 * @param compressed Input bytes for one complete compressed block.
 * @param compressed_size Input length.
 * @param output Caller-owned output range.
 * @param output_capacity Output capacity.
 * @param output_size Caller-owned destination for bytes decoded.
 * @return Zero only after a complete bounded profile decode; `-1` for an
 * unsupported method or class, invalid output-size destination, or profile
 * decode failure.
 *
 * The adapter must pass the real superblock method and block class.
 */
int cix_sqfs_userspace_decode(uint16_t compression_id,
    enum cix_sqfs_block_class block_class, const void *compressed,
    size_t compressed_size, void *output, size_t output_capacity,
    size_t *output_size);

#endif
