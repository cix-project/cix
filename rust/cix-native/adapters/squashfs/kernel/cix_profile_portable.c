// SPDX-License-Identifier: GPL-2.0-only
/*
 * Compile the qualified CIX-owned portable profile unchanged.  The shim above
 * only maps its fixed-width C spellings to Linux kernel types.
 */
#include "include/cix_profile_kernel_shim.h"
#ifndef CIX_PORTABLE_PROFILE_SOURCE
#error "CIX_PORTABLE_PROFILE_SOURCE must name the qualified profile source"
#endif
#include CIX_PORTABLE_PROFILE_SOURCE
