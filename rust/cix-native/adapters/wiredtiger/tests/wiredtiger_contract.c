// SPDX-License-Identifier: MIT
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <wiredtiger.h>

enum { value_size = 65536 };

#ifdef CIX_WT_CONTRACT_TESTING
extern WT_COMPRESSOR *cix_wt_contract_compressor(void);
#endif

static int compression_stat(WT_SESSION *session, int stat_key, int64_t *value) {
    WT_CURSOR *cursor = NULL;
    const char *description;
    const char *printable;
    int ret;

    if (value == NULL) {
        return EINVAL;
    }
    *value = 0;
    ret = session->open_cursor(session, "statistics:table:cix", NULL, NULL, &cursor);
    if (ret == 0) {
        cursor->set_key(cursor, stat_key);
        ret = cursor->search(cursor);
    }
    if (ret == 0) {
        ret = cursor->get_value(cursor, &description, &printable, value);
    }
    if (cursor != NULL) {
        int close_ret = cursor->close(cursor);
        if (ret == 0) {
            ret = close_ret;
        }
    }
    return ret;
}

#ifdef CIX_WT_CONTRACT_TESTING
static int direct_callback_contract(void) {
    static uint8_t source[value_size];
    static uint8_t output[value_size];
    static uint8_t malformed[16];
    WT_COMPRESSOR *compressor = cix_wt_contract_compressor();
    size_t result_len;
    int compression_failed;
    int ret;

    if (compressor == NULL || compressor->pre_size == NULL || compressor->compress == NULL ||
            compressor->decompress == NULL) {
        return EINVAL;
    }
    ret = compressor->pre_size(compressor, NULL, source, 67108865, &result_len);
    if (ret != EFBIG) {
        return EINVAL;
    }
    compression_failed = 0;
    ret = compressor->compress(compressor, NULL, source, sizeof(source), output,
                               1, &result_len, &compression_failed);
    if (ret != 0 || compression_failed == 0 || result_len != 0) {
        return EINVAL;
    }
    ret = compressor->decompress(compressor, NULL, malformed, sizeof(malformed), output,
                                 sizeof(output), &result_len);
    if (ret != EINVAL || result_len != 0) {
        return EINVAL;
    }
    return 0;
}
#endif

static int insert_value(WT_SESSION *session, const char *key, const uint8_t *bytes) {
    WT_CURSOR *cursor = NULL;
    WT_ITEM value;
    int ret;

    memset(&value, 0, sizeof(value));
    value.data = bytes;
    value.size = value_size;
    ret = session->open_cursor(session, "table:cix", NULL, NULL, &cursor);
    if (ret == 0) {
        cursor->set_key(cursor, key);
        cursor->set_value(cursor, &value);
        ret = cursor->insert(cursor);
    }
    if (cursor != NULL) {
        int close_ret = cursor->close(cursor);
        if (ret == 0) {
            ret = close_ret;
        }
    }
    return ret;
}

static int verify_value(WT_SESSION *session, const char *key, const uint8_t *expected) {
    WT_CURSOR *cursor = NULL;
    WT_ITEM value;
    int ret;

    ret = session->open_cursor(session, "table:cix", NULL, NULL, &cursor);
    if (ret == 0) {
        cursor->set_key(cursor, key);
        ret = cursor->search(cursor);
    }
    if (ret == 0) {
        ret = cursor->get_value(cursor, &value);
    }
    if (ret == 0 && (value.size != value_size || memcmp(value.data, expected, value_size) != 0)) {
        ret = EINVAL;
    }
    if (cursor != NULL) {
        int close_ret = cursor->close(cursor);
        if (ret == 0) {
            ret = close_ret;
        }
    }
    return ret;
}

int main(int argc, char **argv) {
    static uint8_t compressible[value_size];
    static uint8_t incompressible[value_size];
    WT_CONNECTION *connection = NULL;
    WT_SESSION *session = NULL;
    uint32_t state = UINT32_C(0x9e3779b9);
    size_t index;
    int64_t stat_value;
    int ret;

    if (argc != 2) {
        return 64;
    }
    memset(compressible, 0x4d, sizeof(compressible));
    for (index = 0; index < sizeof(incompressible); ++index) {
        state = state * UINT32_C(1664525) + UINT32_C(1013904223);
        incompressible[index] = (uint8_t)(state >> 24);
    }
    ret = wiredtiger_open("cix-wt-contract-home", NULL, "create,statistics=(all)", &connection);
    if (ret == 0) {
        ret = connection->load_extension(connection, argv[1],
                                         "entry=wiredtiger_extension_init,terminate=wiredtiger_extension_terminate");
    }
    if (ret == 0) {
        ret = connection->open_session(connection, NULL, NULL, &session);
    }
    if (ret == 0) {
        ret = session->create(session, "table:cix",
                              "key_format=S,value_format=u,block_compressor=cix-v1-experimental");
    }
    if (ret == 0) {
        ret = insert_value(session, "compressible", compressible);
    }
    if (ret == 0) {
        ret = insert_value(session, "incompressible", incompressible);
    }
    if (ret == 0) {
        ret = session->checkpoint(session, NULL);
    }
    if (ret == 0) {
        ret = compression_stat(session, WT_STAT_DSRC_COMPRESS_WRITE, &stat_value);
        if (ret == 0 && stat_value == 0) {
            ret = EINVAL;
        }
    }
    if (session != NULL) {
        int close_ret = session->close(session, NULL);
        if (ret == 0) {
            ret = close_ret;
        }
    }
    if (connection != NULL) {
        int close_ret = connection->close(connection, NULL);
        if (ret == 0) {
            ret = close_ret;
        }
    }
    if (ret != 0) {
        fprintf(stderr, "initial WiredTiger CIX extension phase failed: %d\n", ret);
        return 1;
    }

    connection = NULL;
    session = NULL;
    ret = wiredtiger_open("cix-wt-contract-home", NULL, "statistics=(all)", &connection);
    if (ret == 0) {
        ret = connection->load_extension(connection, argv[1],
                                         "entry=wiredtiger_extension_init,terminate=wiredtiger_extension_terminate");
    }
    if (ret == 0) {
        ret = connection->open_session(connection, NULL, NULL, &session);
    }
    if (ret == 0) {
        ret = verify_value(session, "compressible", compressible);
    }
    if (ret == 0) {
        ret = verify_value(session, "incompressible", incompressible);
    }
    if (ret == 0) {
        ret = compression_stat(session, WT_STAT_DSRC_COMPRESS_READ, &stat_value);
        if (ret == 0 && stat_value == 0) {
            ret = EINVAL;
        }
    }
    if (session != NULL) {
        int close_ret = session->close(session, NULL);
        if (ret == 0) {
            ret = close_ret;
        }
    }
    if (connection != NULL) {
        int close_ret = connection->close(connection, NULL);
        if (ret == 0) {
            ret = close_ret;
        }
    }
    if (ret != 0) {
        fprintf(stderr, "reopen WiredTiger CIX extension phase failed: %d\n", ret);
        return 2;
    }
#ifdef CIX_WT_CONTRACT_TESTING
    ret = direct_callback_contract();
    if (ret != 0) {
        fprintf(stderr, "direct CIX WiredTiger compressor boundary contract failed: %d\n", ret);
        return 3;
    }
#endif
    return 0;
}
