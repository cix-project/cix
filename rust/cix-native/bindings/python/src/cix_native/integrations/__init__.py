"""Optional host adapters for the process-free native Python SDK.

Importing this package does not require Numcodecs or Zarr. Import a concrete
adapter, and call its explicit registration function, only in a host that has
the relevant optional dependency installed.
"""

from .numcodecs import CIXNativeCodec, configure_native_library, register_numcodecs
from .zarr_v2 import CIXNativeV2Codec, register_zarr_v2
from .zarr_v3 import CIXNativeBytesBytesCodec, register_zarr_v3

__all__ = [
    "CIXNativeCodec",
    "CIXNativeV2Codec",
    "CIXNativeBytesBytesCodec",
    "configure_native_library",
    "register_numcodecs",
    "register_zarr_v2",
    "register_zarr_v3",
]
