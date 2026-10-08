// SPDX-License-Identifier: MIT
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <hdf5.h>

#ifndef CIX_HDF5_FILTER_ID
#error "CIX_HDF5_FILTER_ID must be supplied by CMake"
#endif

#define PARAMETER_COUNT 5U
#define CHECK(condition)                                                                          \
    do {                                                                                          \
        if (!(condition)) {                                                                       \
            fprintf(stderr, "%s:%d: %s\n", __func__, __LINE__, #condition);                    \
            goto done;                                                                            \
        }                                                                                         \
    } while (0)

static int close_if_valid(hid_t value, herr_t (*closer)(hid_t))
{
    return value >= 0 && closer(value) < 0;
}

static hid_t create_dataset(hid_t file, const char *name, hsize_t count,
                            const unsigned parameters[], unsigned flags)
{
    hid_t space = -1, dcpl = -1, dataset = -1;

    if ((space = H5Screate_simple(1, &count, NULL)) < 0 ||
        (dcpl = H5Pcreate(H5P_DATASET_CREATE)) < 0 || H5Pset_chunk(dcpl, 1, &count) < 0 ||
        H5Pset_filter(dcpl, (H5Z_filter_t)CIX_HDF5_FILTER_ID, flags, PARAMETER_COUNT,
                      parameters) < 0 ||
        (dataset = H5Dcreate2(file, name, H5T_STD_U32LE, space, H5P_DEFAULT, dcpl,
                              H5P_DEFAULT)) < 0)
        dataset = -1;
    (void)close_if_valid(dcpl, H5Pclose);
    (void)close_if_valid(space, H5Sclose);
    return dataset;
}

static int stored_chunk(hid_t dataset, hsize_t raw_size, unsigned expected_mask,
                        int must_shrink, hsize_t *stored_out)
{
    hsize_t stored = 0;
    unsigned filter_mask = 0;
    void *raw = NULL;
    int result = 1;

    if (H5Dget_chunk_storage_size(dataset, (hsize_t[]){0}, &stored) < 0 || stored == 0 ||
        stored > SIZE_MAX || (must_shrink && stored >= raw_size) ||
        (!must_shrink && stored != raw_size))
        return 1;
    raw = malloc((size_t)stored);
    if (raw == NULL || H5Dread_chunk(dataset, H5P_DEFAULT, (hsize_t[]){0}, &filter_mask, raw) < 0 ||
        filter_mask != expected_mask)
        goto done;
    *stored_out = stored;
    result = 0;
done:
    free(raw);
    return result;
}

static int read_exact(hid_t dataset, const uint32_t *input, size_t bytes)
{
    uint32_t *output = malloc(bytes);
    int result = 1;

    if (output != NULL &&
        H5Dread(dataset, H5T_NATIVE_UINT32, H5S_ALL, H5S_ALL, H5P_DEFAULT, output) >= 0 &&
        memcmp(input, output, bytes) == 0)
        result = 0;
    free(output);
    return result;
}

/* Chunked writes are deferred by the raw chunk cache.  Test a required filter
 * in a separate file and accept the documented failure point: write, flush, or
 * dataset close.  Suppress this expected HDF5 error stack only in this helper. */
static int required_parameters_fail(const char *path, const uint32_t *input,
                                    const unsigned parameters[], const char *label,
                                    int valid_control_created)
{
    hsize_t count = 1024;
    H5E_auto2_t old_callback = NULL;
    void *old_data = NULL;
    hid_t file = -1, dataset = -1;
    herr_t write_status = 0, flush_status = 0, close_status = 0;
    int dataset_created = 0;
    int result = 0;

    if (!valid_control_created) {
        fprintf(stderr, "%s: valid filter control was not created before invalid-parameter test\n",
                __func__);
        return 0;
    }
    file = H5Fcreate(path, H5F_ACC_TRUNC, H5P_DEFAULT, H5P_DEFAULT);
    if (file < 0) {
        fprintf(stderr, "%s: H5Fcreate failed for %s control file\n", __func__, label);
        return 0;
    }
    if (H5Eget_auto2(H5E_DEFAULT, &old_callback, &old_data) < 0 ||
        H5Eset_auto2(H5E_DEFAULT, NULL, NULL) < 0) {
        (void)close_if_valid(file, H5Fclose);
        fprintf(stderr, "%s: unable to suppress expected HDF5 filter error stack\n", __func__);
        return 0;
    }
    dataset = create_dataset(file, "rejected", count, parameters, 0);
    /* Some HDF5 versions reject unavailable cd_values at dataset creation; after
     * a valid plugin control has loaded, that is the intended result. */
    if (dataset < 0) {
        result = 1;
    }
    if (dataset >= 0) {
        dataset_created = 1;
        write_status = H5Dwrite(dataset, H5T_NATIVE_UINT32, H5S_ALL, H5S_ALL, H5P_DEFAULT, input);
        flush_status = H5Dflush(dataset);
        close_status = H5Dclose(dataset);
        dataset = -1;
        result = write_status < 0 || flush_status < 0 || close_status < 0;
    }
    (void)close_if_valid(dataset, H5Dclose);
    (void)close_if_valid(file, H5Fclose);
    (void)H5Eset_auto2(H5E_DEFAULT, old_callback, old_data);
    if (!result) {
        fprintf(stderr,
                "%s: %s filter request unexpectedly succeeded (created=%d write=%d flush=%d close=%d)\n",
                __func__, label, dataset_created, (int)write_status, (int)flush_status,
                (int)close_status);
        H5Eprint2(H5E_DEFAULT, stderr);
    }
    return result;
}

int main(int argc, char **argv)
{
    enum { small_count = 1024, limited_count = 300000 };
    unsigned parameters[PARAMETER_COUNT] = {1, 2, 1, 8, 64};
    unsigned invalid_parameters[PARAMETER_COUNT] = {99, 2, 1, 8, 64};
    unsigned over_limit_parameters[PARAMETER_COUNT] = {1, 2, CIX_HDF5_MAX_WORKERS + 1U, 8, 64};
    unsigned limited_parameters[PARAMETER_COUNT] = {1, 2, 1, 1, 64};
    char invalid_path[4096];
    char over_limit_path[4096];
    uint32_t *compressible = NULL, *incompressible = NULL, *limited = NULL;
    hid_t file = -1, compressed = -1, raw = -1, corrupt = -1, limit = -1;
    hsize_t stored = 0;
    void *chunk = NULL;
    unsigned corrupt_mask = 0;
    uint32_t state = UINT32_C(0x6d2b79f5);
    size_t i;
    int result = 1;

    if (argc != 2) {
        fprintf(stderr, "usage: %s OUTPUT.h5\n", argv[0]);
        return 2;
    }
    CHECK(snprintf(invalid_path, sizeof(invalid_path), "%s.invalid.h5", argv[1]) > 0 &&
          strlen(invalid_path) < sizeof(invalid_path) - 1);
    CHECK(snprintf(over_limit_path, sizeof(over_limit_path), "%s.over-limit.h5", argv[1]) > 0 &&
          strlen(over_limit_path) < sizeof(over_limit_path) - 1);
    compressible = malloc(sizeof(*compressible) * small_count);
    incompressible = malloc(sizeof(*incompressible) * small_count);
    limited = calloc(limited_count, sizeof(*limited));
    CHECK(compressible != NULL && incompressible != NULL && limited != NULL);
    for (i = 0; i < small_count; ++i) {
        compressible[i] = (uint32_t)(i % 11U);
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        incompressible[i] = state;
    }

    CHECK((file = H5Fcreate(argv[1], H5F_ACC_TRUNC, H5P_DEFAULT, H5P_DEFAULT)) >= 0);
    /* H5Pset_filter below triggers H5PL discovery through HDF5_PLUGIN_PATH. */
    CHECK((compressed = create_dataset(file, "compressed", small_count, parameters,
                                       H5Z_FLAG_OPTIONAL)) >= 0);
    CHECK(H5Zfilter_avail((H5Z_filter_t)CIX_HDF5_FILTER_ID) > 0);
    CHECK(required_parameters_fail(invalid_path, compressible, invalid_parameters,
                                   "invalid metadata", compressed >= 0));
    CHECK(required_parameters_fail(over_limit_path, compressible, over_limit_parameters,
                                   "over-deployment worker cap", compressed >= 0));
    CHECK((raw = create_dataset(file, "raw", small_count, parameters, H5Z_FLAG_OPTIONAL)) >= 0);
    CHECK((corrupt = create_dataset(file, "corrupt", small_count, parameters,
                                    H5Z_FLAG_OPTIONAL)) >= 0);
    CHECK((limit = create_dataset(file, "limit", limited_count, limited_parameters,
                                  H5Z_FLAG_OPTIONAL)) >= 0);
    CHECK(H5Dwrite(compressed, H5T_NATIVE_UINT32, H5S_ALL, H5S_ALL, H5P_DEFAULT, compressible) >= 0);
    CHECK(H5Dwrite(raw, H5T_NATIVE_UINT32, H5S_ALL, H5S_ALL, H5P_DEFAULT, incompressible) >= 0);
    CHECK(H5Dwrite(corrupt, H5T_NATIVE_UINT32, H5S_ALL, H5S_ALL, H5P_DEFAULT, compressible) >= 0);
    CHECK(H5Dwrite(limit, H5T_NATIVE_UINT32, H5S_ALL, H5S_ALL, H5P_DEFAULT, limited) >= 0);
    CHECK(H5Fflush(file, H5F_SCOPE_GLOBAL) >= 0);

    CHECK(H5Dget_chunk_storage_size(corrupt, (hsize_t[]){0}, &stored) >= 0 && stored > 0 &&
          stored <= SIZE_MAX && stored < sizeof(*compressible) * small_count);
    chunk = malloc((size_t)stored);
    CHECK(chunk != NULL);
    CHECK(H5Dread_chunk(corrupt, H5P_DEFAULT, (hsize_t[]){0}, &corrupt_mask, chunk) >= 0 &&
          corrupt_mask == 0);
    ((uint8_t *)chunk)[0] ^= UINT8_C(0xff);
    CHECK(H5Dwrite_chunk(corrupt, H5P_DEFAULT, 0, (hsize_t[]){0}, stored, chunk) >= 0);
    free(chunk);
    chunk = NULL;
    CHECK(H5Fflush(file, H5F_SCOPE_GLOBAL) >= 0);

    /* Close every handle before reads, so cache state cannot satisfy decoding. */
    CHECK(H5Dclose(limit) >= 0);
    limit = -1;
    CHECK(H5Dclose(corrupt) >= 0);
    corrupt = -1;
    CHECK(H5Dclose(raw) >= 0);
    raw = -1;
    CHECK(H5Dclose(compressed) >= 0);
    compressed = -1;
    CHECK(H5Fclose(file) >= 0);
    file = -1;

    CHECK((file = H5Fopen(argv[1], H5F_ACC_RDONLY, H5P_DEFAULT)) >= 0);
    CHECK((compressed = H5Dopen2(file, "compressed", H5P_DEFAULT)) >= 0);
    CHECK((raw = H5Dopen2(file, "raw", H5P_DEFAULT)) >= 0);
    CHECK((corrupt = H5Dopen2(file, "corrupt", H5P_DEFAULT)) >= 0);
    CHECK((limit = H5Dopen2(file, "limit", H5P_DEFAULT)) >= 0);
    CHECK(!stored_chunk(compressed, sizeof(*compressible) * small_count, 0, 1, &stored));
    CHECK(!read_exact(compressed, compressible, sizeof(*compressible) * small_count));
    /* An optional non-shrinking result has filter bit zero set in the mask. */
    CHECK(!stored_chunk(raw, sizeof(*incompressible) * small_count, 1, 0, &stored));
    CHECK(!read_exact(raw, incompressible, sizeof(*incompressible) * small_count));
    /* The decode cap is enforced from persisted cd_values after reopen. */
    CHECK(H5Dread(limit, H5T_NATIVE_UINT32, H5S_ALL, H5S_ALL, H5P_DEFAULT, limited) < 0);
    /* A corrupted compressed chunk must fail rather than produce partial output. */
    CHECK(H5Dread(corrupt, H5T_NATIVE_UINT32, H5S_ALL, H5S_ALL, H5P_DEFAULT, compressible) < 0);
    result = 0;

done:
    free(chunk);
    result |= close_if_valid(limit, H5Dclose);
    result |= close_if_valid(corrupt, H5Dclose);
    result |= close_if_valid(raw, H5Dclose);
    result |= close_if_valid(compressed, H5Dclose);
    result |= close_if_valid(file, H5Fclose);
    free(limited);
    free(incompressible);
    free(compressible);
    return result;
}
