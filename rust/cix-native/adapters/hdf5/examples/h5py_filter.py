"""Optional h5py integration for the experimental CIX HDF5 filter.

Build H5Zcix with the deployment's chosen CIX_HDF5_FILTER_ID, put that shared
library in HDF5_PLUGIN_PATH before Python starts, then provide the same ID here.
"""
import os

import h5py
import numpy as np

FILTER_ID = int(os.environ["CIX_HDF5_FILTER_ID"])
# [parameter ABI, profile, workers, output MiB, memory MiB]
FILTER_PARAMETERS = (1, 2, 1, 8, 64)

if not h5py.h5z.filter_avail(FILTER_ID):
    raise RuntimeError("CIX HDF5 plugin was not loaded; set HDF5_PLUGIN_PATH before starting Python")

data = (np.arange(1024, dtype=np.uint32) % 11).copy()
with h5py.File("cix-filter-example.h5", "w") as handle:
    space = h5py.h5s.create_simple(data.shape)
    dcpl = h5py.h5p.create(h5py.h5p.DATASET_CREATE)
    dcpl.set_chunk(data.shape)
    dcpl.set_filter(FILTER_ID, h5py.h5z.FLAG_OPTIONAL, FILTER_PARAMETERS)
    dataset = h5py.h5d.create(handle.id, b"values", h5py.h5t.NATIVE_UINT32, space, dcpl=dcpl)
    dataset.write(h5py.h5s.ALL, h5py.h5s.ALL, data)

with h5py.File("cix-filter-example.h5", "r") as handle:
    dataset = handle["values"]
    mask, encoded = dataset.id.read_direct_chunk((0,))
    assert mask == 0 and len(encoded) < data.nbytes
    assert np.array_equal(dataset[:], data)
