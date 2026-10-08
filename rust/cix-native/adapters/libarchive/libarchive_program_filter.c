// SPDX-License-Identifier: MIT
/* Installed libarchive program-filter consumer for CIX. */
#include <archive.h>
#include <archive_entry.h>

#include <stdio.h>
#include <string.h>

enum { CIX_COMMAND_CAP = 4096, CIX_ERROR_CAP = 512 };

static void copy_error(char error[CIX_ERROR_CAP], struct archive *archive) {
    const char *message = archive_error_string(archive);
    (void)snprintf(error, CIX_ERROR_CAP, "%s",
                   message == NULL ? "libarchive did not provide an error" : message);
}

/*
 * Quote one argument for libarchive's command-line parser.
 * libarchive 3.7.2 does not invoke a shell: it recognizes double quotes and
 * a backslash which quotes the following byte.  Single quotes are literal.
 */
static int append_argument(char command[CIX_COMMAND_CAP], size_t *used,
                           const char *value) {
    if (value == NULL || value[0] == '\0') return 1;
    if (*used != 0) {
        if (*used + 1 >= CIX_COMMAND_CAP) return 1;
        command[(*used)++] = ' ';
    }
    if (*used + 1 >= CIX_COMMAND_CAP) return 1;
    command[(*used)++] = '"';
    for (const unsigned char *at = (const unsigned char *)value; *at != '\0'; ++at) {
        if (*at == '"' || *at == '\\') {
            if (*used + 2 >= CIX_COMMAND_CAP) return 1;
            command[(*used)++] = '\\';
        } else if (*used + 1 >= CIX_COMMAND_CAP) {
            return 1;
        }
        command[(*used)++] = (char)*at;
    }
    if (*used + 2 > CIX_COMMAND_CAP) return 1;
    command[(*used)++] = '"';
    command[*used] = '\0';
    return 0;
}

static int filter_command(const char *filter, const char *cix, const char *mode,
                          char command[CIX_COMMAND_CAP]) {
    size_t used = 0;
    if (filter == NULL || filter[0] != '/' || cix == NULL || cix[0] != '/') return 1;
    return append_argument(command, &used, filter) ||
           append_argument(command, &used, "--cix") ||
           append_argument(command, &used, cix) ||
           append_argument(command, &used, mode);
}

static int add_member(struct archive *writer, const char *name, const char *value,
                      char error[CIX_ERROR_CAP]) {
    struct archive_entry *entry = archive_entry_new();
    size_t size = strlen(value);
    if (entry == NULL) {
        (void)snprintf(error, CIX_ERROR_CAP, "cannot allocate archive entry");
        return 1;
    }
    archive_entry_set_pathname(entry, name);
    archive_entry_set_filetype(entry, AE_IFREG);
    archive_entry_set_perm(entry, 0644);
    archive_entry_set_size(entry, (la_int64_t)size);
    if (archive_write_header(writer, entry) != ARCHIVE_OK ||
        archive_write_data(writer, value, size) != (la_ssize_t)size) {
        copy_error(error, writer);
        archive_entry_free(entry);
        return 1;
    }
    archive_entry_free(entry);
    return 0;
}

static int write_demo(const char *filter, const char *cix, const char *output,
                      char error[CIX_ERROR_CAP]) {
    char command[CIX_COMMAND_CAP];
    struct archive *writer;
    int failed = 1;
    error[0] = '\0';
    if (filter_command(filter, cix, "--encode", command)) goto invalid_command;
    writer = archive_write_new();
    if (writer == NULL) goto allocation_failed;
    if (archive_write_set_format_pax_restricted(writer) != ARCHIVE_OK ||
        archive_write_add_filter_program(writer, command) != ARCHIVE_OK ||
        archive_write_open_filename(writer, output) != ARCHIVE_OK ||
        add_member(writer, "first.txt", "first member\n", error) ||
        add_member(writer, "nested/second.txt", "second member\n", error) ||
        archive_write_close(writer) != ARCHIVE_OK) {
        if (error[0] == '\0') copy_error(error, writer);
    } else {
        failed = 0;
    }
    archive_write_free(writer);
    return failed;
allocation_failed:
    (void)snprintf(error, CIX_ERROR_CAP, "cannot allocate libarchive writer");
    return 1;
invalid_command:
    (void)snprintf(error, CIX_ERROR_CAP, "invalid or overlong absolute filter path");
    return 1;
}

static int verify_demo(const char *filter, const char *cix, const char *input,
                       char error[CIX_ERROR_CAP]) {
    static const char *const names[] = {"first.txt", "nested/second.txt"};
    static const char *const values[] = {"first member\n", "second member\n"};
    char command[CIX_COMMAND_CAP], bytes[32];
    struct archive *reader;
    struct archive_entry *entry;
    int index = 0, failed = 1, status;
    error[0] = '\0';
    if (filter_command(filter, cix, "--decode", command)) goto invalid_command;
    reader = archive_read_new();
    if (reader == NULL) goto allocation_failed;
    if (archive_read_support_filter_program(reader, command) != ARCHIVE_OK ||
        archive_read_support_format_tar(reader) != ARCHIVE_OK ||
        archive_read_open_filename(reader, input, 64 * 1024) != ARCHIVE_OK) {
        copy_error(error, reader);
        goto done;
    }
    while ((status = archive_read_next_header(reader, &entry)) == ARCHIVE_OK) {
        const char *pathname = archive_entry_pathname(entry);
        size_t expected, received = 0;
        la_ssize_t size;
        if (index >= 2 || pathname == NULL || strcmp(pathname, names[index]) != 0) {
            (void)snprintf(error, CIX_ERROR_CAP, "unexpected tar member");
            goto done;
        }
        expected = strlen(values[index]);
        if (archive_entry_size(entry) != (la_int64_t)expected || expected >= sizeof(bytes)) {
            (void)snprintf(error, CIX_ERROR_CAP, "unexpected tar member length");
            goto done;
        }
        while (received < expected) {
            size = archive_read_data(reader, bytes + received, expected - received);
            if (size <= 0) {
                copy_error(error, reader);
                goto done;
            }
            received += (size_t)size;
        }
        size = archive_read_data(reader, bytes, 1);
        if (size != 0 || memcmp(bytes, values[index], expected) != 0) {
            (void)snprintf(error, CIX_ERROR_CAP, "unexpected tar member data");
            goto done;
        }
        ++index;
    }
    if (status != ARCHIVE_EOF || index != 2 || archive_read_close(reader) != ARCHIVE_OK) {
        copy_error(error, reader);
        goto done;
    }
    failed = 0;
done:
    archive_read_free(reader);
    return failed;
allocation_failed:
    (void)snprintf(error, CIX_ERROR_CAP, "cannot allocate libarchive reader");
    return 1;
invalid_command:
    (void)snprintf(error, CIX_ERROR_CAP, "invalid or overlong absolute filter path");
    return 1;
}

int main(int argc, char **argv) {
    char error[CIX_ERROR_CAP];
    if (argc != 4) {
        fprintf(stderr, "usage: %s /absolute/cix-tar-filter.sh /absolute/cix archive.tar.cix\n", argv[0]);
        return 64;
    }
    if (write_demo(argv[1], argv[2], argv[3], error) ||
        verify_demo(argv[1], argv[2], argv[3], error)) {
        fprintf(stderr, "libarchive CIX program filter: %.511s\n", error);
        return 1;
    }
    return 0;
}
