// SPDX-License-Identifier: GPL-2.0-only
/* Registration adapter for the separately named experimental filesystem. */
#include <linux/fs.h>
#include "include/cix_squashfs_register.h"

int cix_squashfs_register_filesystem(struct file_system_type *fs)
{
	fs->name = "cix_squashfs";
	return register_filesystem(fs);
}

int cix_squashfs_unregister_filesystem(struct file_system_type *fs)
{
	return unregister_filesystem(fs);
}
