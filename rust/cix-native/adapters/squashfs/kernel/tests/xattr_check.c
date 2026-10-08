/* SPDX-License-Identifier: GPL-2.0-only */
/* Qualification helper: two exact xattr values read through a mounted reader. */
#include <string.h>
#include <sys/types.h>
#include <sys/xattr.h>

static int check(const char *path, const char *name, const char *expected) {
    char value[64];
    size_t length = strlen(expected);
    ssize_t actual = getxattr(path, name, value, sizeof(value));
    return actual == (ssize_t)length && memcmp(value, expected, length) == 0;
}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    return check(argv[1], "user.cix.fixture", "experimental") &&
           check(argv[2], "user.cix.tiny", "yes") ? 0 : 1;
}
