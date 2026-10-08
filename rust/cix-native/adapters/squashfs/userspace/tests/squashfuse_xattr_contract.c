// SPDX-License-Identifier: MIT
/* Direct public-library xattr read check for the pinned Squashfuse overlay.
 * Usage: IMAGE PATH XATTR_NAME VALUE_HEX. Extraction does not restore xattrs. */
#include "fs.h"
#include "traverse.h"
#include "util.h"
#include "xattr.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int hex_value(const char *text, unsigned char **out, size_t *size)
{
    size_t i, length = strlen(text);
    unsigned char *result;
    if ((length & 1) != 0) return -1;
    result = malloc(length / 2 ? length / 2 : 1);
    if (result == NULL) return -1;
    for (i = 0; i < length; i += 2) {
        unsigned int value;
        if (sscanf(text + i, "%2x", &value) != 1) { free(result); return -1; }
        result[i / 2] = (unsigned char)value;
    }
    *out = result; *size = length / 2;
    return 0;
}

int main(int argc, char **argv)
{
    sqfs fs; sqfs_traverse traversal; sqfs_err error = SQFS_OK;
    unsigned char *expected = NULL, *actual = NULL;
    size_t expected_size, actual_size; int found = 0, result = 1;
    if (argc != 5 || hex_value(argv[4], &expected, &expected_size) != 0) return 2;
    if (sqfs_open_image(&fs, argv[1], 0) != SQFS_OK) goto done;
    if (sqfs_traverse_open(&traversal, &fs, sqfs_inode_root(&fs)) != SQFS_OK) goto close_fs;
    while (sqfs_traverse_next(&traversal, &error)) {
        sqfs_inode inode;
        if (traversal.dir_end || strcmp(traversal.path, argv[2]) != 0) continue;
        found = 1;
        if (sqfs_inode_get(&fs, &inode, traversal.entry.inode) != SQFS_OK) break;
        actual_size = 0;
        if (sqfs_xattr_lookup(&fs, &inode, argv[3], NULL, &actual_size) != SQFS_OK || actual_size != expected_size) break;
        actual = malloc(actual_size ? actual_size : 1);
        if (actual == NULL || sqfs_xattr_lookup(&fs, &inode, argv[3], actual, &actual_size) != SQFS_OK || actual_size != expected_size || memcmp(actual, expected, expected_size) != 0) break;
        result = 0; break;
    }
    sqfs_traverse_close(&traversal);
close_fs:
    sqfs_destroy(&fs); sqfs_fd_close(fs.fd);
done:
    free(actual); free(expected);
    return found ? result : 1;
}
