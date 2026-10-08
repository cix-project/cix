/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Compatibility prelude for compiling the qualified portable profile in a
 * kernel translation unit.  It supplies only spelling aliases; it does not
 * change profile/src/cix_squashfs_profile.c or its byte-level algorithm.
 */
#ifndef CIX_PROFILE_KERNEL_SHIM_H
#define CIX_PROFILE_KERNEL_SHIM_H

#include <linux/kernel.h>
#include <linux/string.h>
#include <linux/types.h>

/* Dedicated CIX include shims satisfy the portable source's C library names. */

#endif
