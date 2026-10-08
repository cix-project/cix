// SPDX-License-Identifier: MIT
/* Replacement TU for squashfs-tools-ng v1.2.0 read_super.c.
 * Header guards let the range check be widened only while compiling the
 * unchanged upstream implementation; no vendor source file is edited. */
#define SQFS_BUILDING_DLL
#include "config.h"
#include "sqfs/super.h"
#include "sqfs/error.h"
#include "sqfs/io.h"
#include "util/util.h"
#ifndef CIX_SQFSNG_READ_SUPER_SOURCE
#error "define CIX_SQFSNG_READ_SUPER_SOURCE to pinned lib/sqfs/read_super.c"
#endif
#define sqfs_super_read cix_upstream_super_read
#define SQFS_COMP_MIN 1
#define SQFS_COMP_MAX 65001
#include CIX_SQFSNG_READ_SUPER_SOURCE
#undef SQFS_COMP_MIN
#undef SQFS_COMP_MAX
#undef sqfs_super_read

int sqfs_super_read(sqfs_super_t *super, sqfs_file_t *file)
{
    int ret = cix_upstream_super_read(super, file);
    if (ret)
        return ret;
    /* The widened compile-time bound is only a bridge for the private ID. */
    if ((super->compression_id >= 1 && super->compression_id <= 6) ||
        super->compression_id == 65001)
        return 0;
    return SQFS_ERROR_UNSUPPORTED;
}
