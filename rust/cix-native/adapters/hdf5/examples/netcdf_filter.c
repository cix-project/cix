// SPDX-License-Identifier: MIT
/* Optional netCDF-C integration. Requires a netCDF-C build exposing
 * nc_def_var_filter (netCDF-C 4.9+) over an HDF5 backend with the CIX plugin
 * already visible through HDF5_PLUGIN_PATH. */
#include <netcdf.h>
#include <netcdf_filter.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef CIX_HDF5_FILTER_ID
#error "Compile with the same CIX_HDF5_FILTER_ID used for H5Zcix"
#endif

#define NC_CHECK(call)                                                                            \
    do {                                                                                          \
        int cix_netcdf_status = (call);                                                          \
        if (cix_netcdf_status != NC_NOERR) {                                                     \
            fprintf(stderr, "%s: %s\n", #call, nc_strerror(cix_netcdf_status));                \
            return EXIT_FAILURE;                                                                 \
        }                                                                                         \
    } while (0)

int main(void)
{
    int ncid, dimension, variable;
    size_t length = 1024;
    unsigned parameters[] = {1, 2, 1, 8, 64};
    unsigned persisted[sizeof(parameters) / sizeof(parameters[0])];
    unsigned persisted_id = 0;
    size_t persisted_count = sizeof(persisted) / sizeof(persisted[0]);
    unsigned values[1024];
    unsigned restored[1024];
    size_t i;

    for (i = 0; i < length; ++i)
        values[i] = (unsigned)(i % 11U);
    NC_CHECK(nc_create("cix-filter-example.nc", NC_NETCDF4 | NC_CLOBBER, &ncid));
    NC_CHECK(nc_def_dim(ncid, "sample", length, &dimension));
    NC_CHECK(nc_def_var(ncid, "values", NC_UINT, 1, &dimension, &variable));
    NC_CHECK(nc_def_var_chunking(ncid, variable, NC_CHUNKED, &length));
    NC_CHECK(nc_def_var_filter(ncid, variable, CIX_HDF5_FILTER_ID,
                               sizeof(parameters) / sizeof(parameters[0]), parameters));
    NC_CHECK(nc_enddef(ncid));
    NC_CHECK(nc_put_var_uint(ncid, variable, values));
    NC_CHECK(nc_close(ncid));
    NC_CHECK(nc_open("cix-filter-example.nc", NC_NOWRITE, &ncid));
    NC_CHECK(nc_inq_varid(ncid, "values", &variable));
    NC_CHECK(nc_inq_var_filter(ncid, variable, &persisted_id, &persisted_count, persisted));
    if (persisted_id != CIX_HDF5_FILTER_ID ||
        persisted_count != sizeof(parameters) / sizeof(parameters[0]) ||
        memcmp(parameters, persisted, sizeof(parameters)) != 0) {
        fprintf(stderr, "persisted CIX filter metadata differs from the requested parameters\n");
        (void)nc_close(ncid);
        return EXIT_FAILURE;
    }
    NC_CHECK(nc_get_var_uint(ncid, variable, restored));
    NC_CHECK(nc_close(ncid));
    if (memcmp(values, restored, sizeof(values)) != 0) {
        fprintf(stderr, "netCDF CIX filter round-trip mismatch\n");
        return EXIT_FAILURE;
    }
    return EXIT_SUCCESS;
}
