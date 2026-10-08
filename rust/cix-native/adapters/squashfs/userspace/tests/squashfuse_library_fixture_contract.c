// SPDX-License-Identifier: MIT
/* Full public-libsquashfuse reader contract, independent of demo extraction.
 * Usage: IMAGE EXPECTED_ROOT EXPECTED_REGULAR_FILES EXPECTED_DIRECTORIES.
 * It deliberately accepts every upstream inode representation supported by
 * sqfs_inode_get and uses its mode plus sqfs_read_range, rather than the
 * demo's incomplete numeric inode-type switch. */
#include "file.h"
#include "fs.h"
#include "traverse.h"
#include "util.h"

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define READ_CHUNK 8192

static int parse_count(const char *text, size_t *out)
{
    char *end = NULL;
    unsigned long long value = strtoull(text, &end, 10);
    if (text[0] == '\0' || *end != '\0' || value > SIZE_MAX)
        return -1;
    *out = (size_t)value;
    return 0;
}

static int read_regular(sqfs *fs, sqfs_inode *inode, const char *root,
                        const char *relative)
{
    char expected_path[PATH_MAX];
    unsigned char decoded[READ_CHUNK], expected[READ_CHUNK];
    uint64_t offset = 0, total = inode->xtra.reg.file_size;
    int fd;
    struct stat expected_stat;

    if (relative[0] == '/' || strstr(relative, "../") != NULL ||
        snprintf(expected_path, sizeof(expected_path), "%s/%s", root, relative) >=
            (int)sizeof(expected_path))
        return -1;
    fd = open(expected_path, O_RDONLY | O_CLOEXEC);
    if (fd < 0)
        return -1;
    if (fstat(fd, &expected_stat) != 0 || !S_ISREG(expected_stat.st_mode) ||
        (uint64_t)expected_stat.st_size != total) {
        close(fd);
        return -1;
    }
    while (offset < total) {
        sqfs_off_t wanted = (sqfs_off_t)((total - offset) > READ_CHUNK ?
                                         READ_CHUNK : total - offset);
        ssize_t got;
        if (sqfs_read_range(fs, inode, (sqfs_off_t)offset, &wanted, decoded) != SQFS_OK ||
            wanted <= 0 || wanted > READ_CHUNK) {
            close(fd);
            return -1;
        }
        got = pread(fd, expected, (size_t)wanted, (off_t)offset);
        if (got != wanted || memcmp(decoded, expected, (size_t)wanted) != 0) {
            close(fd);
            return -1;
        }
        offset += (uint64_t)wanted;
    }
    close(fd);
    return 0;
}

int main(int argc, char **argv)
{
    sqfs fs;
    sqfs_traverse traversal;
    sqfs_err error = SQFS_OK;
    size_t expected_files, expected_directories, files = 0, directories = 0;
    int result = 1, opened = 0, traversing = 0;

    if (argc != 5 || parse_count(argv[3], &expected_files) != 0 ||
        parse_count(argv[4], &expected_directories) != 0)
        return 2;
    if (sqfs_open_image(&fs, argv[1], 0) != SQFS_OK)
        goto done;
    opened = 1;
    if (sqfs_traverse_open(&traversal, &fs, sqfs_inode_root(&fs)) != SQFS_OK)
        goto done;
    traversing = 1;
    while (sqfs_traverse_next(&traversal, &error)) {
        sqfs_inode inode;
        if (traversal.dir_end)
            continue;
        if (sqfs_inode_get(&fs, &inode, traversal.entry.inode) != SQFS_OK)
            goto done;
        if (S_ISREG(inode.base.mode)) {
            if (read_regular(&fs, &inode, argv[2], traversal.path) != 0)
                goto done;
            ++files;
        } else if (S_ISDIR(inode.base.mode)) {
            ++directories;
        } else {
            /* The fixture receipt is file-only; unsupported entry kinds make
             * this contract fail rather than silently being skipped. */
            goto done;
        }
    }
    if (error == SQFS_OK && files == expected_files && directories == expected_directories)
        result = 0;
done:
    if (traversing)
        sqfs_traverse_close(&traversal);
    if (opened) {
        sqfs_destroy(&fs);
        sqfs_fd_close(fs.fd);
    }
    return result;
}
