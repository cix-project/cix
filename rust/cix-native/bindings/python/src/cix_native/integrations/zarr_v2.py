"""Zarr v2 uses the Numcodecs adapter and its distinct v2 ``id`` metadata."""
from __future__ import annotations

from pathlib import Path

from .numcodecs import CIXNativeCodec, register_numcodecs

CIXNativeV2Codec = CIXNativeCodec


def register_zarr_v2(library_path: str | Path | None = None) -> type[CIXNativeCodec]:
    """Register the Numcodecs codec used by Zarr v2 compressor metadata."""
    return register_numcodecs(library_path)
