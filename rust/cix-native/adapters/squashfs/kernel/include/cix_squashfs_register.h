/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef CIX_SQUASHFS_REGISTER_H
#define CIX_SQUASHFS_REGISTER_H

#include <linux/fs.h>

/**
 * @brief Registers @p fs under the private `cix_squashfs` filesystem name.
 *
 * @return The result from the kernel's register_filesystem().
 */
int cix_squashfs_register_filesystem(struct file_system_type *fs);

/**
 * @brief Unregisters a filesystem previously registered by the kernel.
 *
 * @return The result from the kernel's unregister_filesystem().
 */
int cix_squashfs_unregister_filesystem(struct file_system_type *fs);
#endif
