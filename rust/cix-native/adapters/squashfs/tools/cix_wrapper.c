// SPDX-License-Identifier: MIT
/* GPL-2.0-or-later: experimental squashfs-tools 4.6.1 overlay only. */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "compressor.h" /* supplied by the pinned squashfs-tools checkout */
#include "cix_squashfs_profile.h"

#define CIX_EXPERIMENTAL_COMPRESSION 65001

static int cix_init(void **stream, int block_size, int datablock)
{
    size_t limit = cix_squashfs_profile_max_input(datablock ? CIX_SQUASHFS_DATA : CIX_SQUASHFS_METADATA);
    if (block_size <= 0 || (size_t)block_size > limit)
        return -1;
    /* The profile is state-free; the sentinel preserves the upstream ABI. */
    *stream = (void *)(uintptr_t)(datablock ? 1 : 2);
    return 0;
}

static int cix_compress(void *stream, void *dest, void *src, int size,
    int block_size, int *error)
{
    size_t written = 0;
    enum cix_squashfs_profile_kind kind = stream == (void *)(uintptr_t)1 ? CIX_SQUASHFS_DATA : CIX_SQUASHFS_METADATA;
    int result;
    if (size < 0 || block_size < 0) { *error = CIX_SQUASHFS_INVALID_ARGUMENT; return -1; }
    result = cix_squashfs_profile_encode(kind, src, (size_t)size, dest,
        (size_t)block_size, &written);
    if (result == CIX_SQUASHFS_OK) return (int)written;
    if (result == CIX_SQUASHFS_NO_BENEFIT || result == CIX_SQUASHFS_OUTPUT_TOO_SMALL) return 0;
    *error = result;
    return -1;
}

static int cix_uncompress(void *dest, void *src, int size, int block_size,
    int *error)
{
    size_t written = 0;
    /* Upstream supplies the configured data block size or metadata block size. */
    enum cix_squashfs_profile_kind kind = block_size <= 8192 ? CIX_SQUASHFS_METADATA : CIX_SQUASHFS_DATA;
    int result;
    if (size < 0 || block_size < 0) { *error = CIX_SQUASHFS_INVALID_ARGUMENT; return -1; }
    result = cix_squashfs_profile_decode(kind, src, (size_t)size, dest,
        (size_t)block_size, &written);
    if (result == CIX_SQUASHFS_OK) return (int)written;
    *error = result;
    return -1;
}

static int cix_options(char **argv, int argc) { (void)argv; (void)argc; return -1; }
static int cix_options_post(int block_size) { return block_size > 0 && block_size <= 131072 ? 0 : -1; }
static void *cix_dump_options(int block_size, int *size) { (void)block_size; *size = 0; return NULL; }
static int cix_extract_options(int block_size, void *buffer, int size) { (void)block_size; (void)buffer; return size == 0 ? 0 : -1; }
static int cix_check_options(int block_size, void *buffer, int size) { return cix_extract_options(block_size, buffer, size); }
static void cix_display_options(void *buffer, int size) { (void)buffer; (void)size; printf("\texperimental CIX profile v1; data <= 131072, metadata <= 8192\n"); }
static void cix_usage(FILE *stream, int cols) { (void)cols; fprintf(stream, "\t  no CIX-specific options; requires -b <= 131072\n"); }
static int cix_option_args(char *option) { (void)option; return 0; }

struct compressor cix_comp_ops = {
    .id = CIX_EXPERIMENTAL_COMPRESSION, .name = "cix", .supported = 1,
    .init = cix_init, .compress = cix_compress, .uncompress = cix_uncompress,
    .options = cix_options, .options_post = cix_options_post,
    .dump_options = cix_dump_options, .extract_options = cix_extract_options,
    .check_options = cix_check_options, .display_options = cix_display_options,
    .usage = cix_usage, .option_args = cix_option_args
};
