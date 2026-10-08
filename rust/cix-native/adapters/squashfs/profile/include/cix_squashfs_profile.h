// SPDX-License-Identifier: MIT
/* Experimental, block-local CIX SquashFS profile. See SPEC.md. */
#ifndef CIX_SQUASHFS_PROFILE_H
#define CIX_SQUASHFS_PROFILE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** @brief Profile block class, which determines the decoded-size limit. */
enum cix_squashfs_profile_kind {
    CIX_SQUASHFS_DATA = 0,     /**< Data or fragment block; limit is 131072 bytes. */
    CIX_SQUASHFS_METADATA = 1  /**< Metadata or xattr block; limit is 8192 bytes. */
};

/** @brief Result values returned by profile encode and decode operations. */
enum cix_squashfs_profile_status {
    CIX_SQUASHFS_OK = 0,                /**< Operation completed. */
    CIX_SQUASHFS_NO_BENEFIT = 1,        /**< Encoding cannot reduce the block. */
    CIX_SQUASHFS_INVALID_ARGUMENT = 2,  /**< Pointer, range, kind, or workspace is invalid. */
    CIX_SQUASHFS_OUTPUT_TOO_SMALL = 3,  /**< Destination cannot hold decoded output. */
    CIX_SQUASHFS_MALFORMED = 4,         /**< Input is not one complete valid profile block. */
    CIX_SQUASHFS_LIMIT = 5              /**< Declared decoded size exceeds the class limit. */
};

/**
 * @brief Returns the decoded-input limit for @p kind.
 *
 * @return 131072 for data, 8192 for metadata, or zero for an invalid kind.
 */
size_t cix_squashfs_profile_max_input(enum cix_squashfs_profile_kind kind);

/**
 * @brief Returns the bytes required for a caller-owned adaptive decode model.
 *
 * The returned workspace must be suitably aligned for `uint32_t`.
 */
size_t cix_squashfs_profile_decode_workspace_size(void);

/**
 * @brief Encodes one block with the experimental profile.
 *
 * @param kind Block class that limits @p src_len.
 * @param src Source range, which must not overlap @p dst.
 * @param src_len Source bytes; @p dst must have capacity for this many bytes.
 * @param dst Caller-owned destination.
 * @param dst_cap Destination capacity.
 * @param written Required caller-owned result length. If non-null, it is zeroed
 * before validation; a null pointer returns #CIX_SQUASHFS_INVALID_ARGUMENT.
 * @return #CIX_SQUASHFS_OK with `*written < src_len`, or
 * #CIX_SQUASHFS_NO_BENEFIT for a caller to store the input uncompressed.
 *
 * This operation has no heap allocation, process state, history, dictionary,
 * or floating-point dependency. Destination contents are unspecified unless it
 * returns #CIX_SQUASHFS_OK.
 */
int cix_squashfs_profile_encode(enum cix_squashfs_profile_kind kind,
    const uint8_t *src, size_t src_len, uint8_t *dst, size_t dst_cap,
    size_t *written);

/**
 * @brief Decodes one complete profile block using stack-local workspace.
 *
 * @param kind Block class used to enforce the decoded-size limit.
 * @param src Complete encoded input, which must not overlap @p dst.
 * @param src_len Encoded input length.
 * @param dst Caller-owned destination.
 * @param dst_cap Destination capacity.
 * @param written Required caller-owned result length. If non-null, it is zeroed
 * before validation and equals the declared length only on
 * #CIX_SQUASHFS_OK; a null pointer returns #CIX_SQUASHFS_INVALID_ARGUMENT.
 * @return A #cix_squashfs_profile_status value.
 */
int cix_squashfs_profile_decode(enum cix_squashfs_profile_kind kind,
    const uint8_t *src, size_t src_len, uint8_t *dst, size_t dst_cap,
    size_t *written);

/**
 * @brief Decodes one complete profile block using caller-owned workspace.
 *
 * @param workspace Adaptive model storage with at least
 * cix_squashfs_profile_decode_workspace_size() bytes and `uint32_t` alignment.
 * It must not overlap @p src or @p dst and is never retained.
 * @param workspace_len Available workspace bytes.
 * @param written Required caller-owned result length. If non-null, it is zeroed
 * before validation; a null pointer returns #CIX_SQUASHFS_INVALID_ARGUMENT.
 * @return A #cix_squashfs_profile_status value.
 *
 * The remaining parameters and successful output contract match
 * cix_squashfs_profile_decode(). This entry point performs no allocation.
 */
int cix_squashfs_profile_decode_with_workspace(
    enum cix_squashfs_profile_kind kind, const uint8_t *src, size_t src_len,
    uint8_t *dst, size_t dst_cap, void *workspace, size_t workspace_len,
    size_t *written);

#ifdef __cplusplus
}
#endif
#endif
