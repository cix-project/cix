/* SPDX-License-Identifier: GPL-2.0-only */
/* CIX-owned, allocation-free decoder for private SquashFS compressor 65001. */
#ifndef CIX_SQUASHFS_KERNEL_H
#define CIX_SQUASHFS_KERNEL_H

#include <linux/types.h>

#define CIX_SQUASHFS_COMPRESSION 65001
#define CIX_SQUASHFS_DATA_LIMIT (128 * 1024)
#define CIX_SQUASHFS_METADATA_LIMIT (8 * 1024)

/**
 * @brief Returns bytes required by the caller-owned profile decode workspace.
 */
extern size_t cix_squashfs_profile_decode_workspace_size(void);

/**
 * @brief Decodes a complete bounded profile block without allocating.
 *
 * @param kind #CIX_SQUASHFS_KERNEL_DATA or #CIX_SQUASHFS_KERNEL_METADATA.
 * @param src Input range that must not overlap @p dst or @p workspace.
 * @param dst Caller-owned output range.
 * @param workspace Caller-owned storage of at least
 * cix_squashfs_profile_decode_workspace_size() bytes.
 * @param written Caller-owned output length, valid after a zero result.
 * @return Zero on success; a nonzero profile status otherwise.
 */
extern int cix_squashfs_profile_decode_with_workspace(int kind, const u8 *src,
	size_t src_len, u8 *dst, size_t dst_cap, void *workspace,
	size_t workspace_len, size_t *written);

/** @brief Kernel block classes used to select the portable profile limit. */
enum cix_squashfs_kernel_kind {
	CIX_SQUASHFS_KERNEL_DATA,      /**< Data or fragment limit: 128 KiB. */
	CIX_SQUASHFS_KERNEL_METADATA,  /**< Metadata or xattr limit: 8 KiB. */
};

#endif
