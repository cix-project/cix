"""Experimental Zarr v3 BytesBytesCodec, intentionally separate from v2."""
from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

from .numcodecs import CODEC_ID, CIXNativeCodec, configure_native_library

CODEC_NAME = "cix-native-experimental"

try:
    from zarr.abc.codec import BytesBytesCodec as _BytesBytesCodec
except ImportError:  # pragma: no cover - SDK-only installation.
    _BytesBytesCodec = object


class CIXNativeBytesBytesCodec(_BytesBytesCodec):
    """Zarr v3 bytes-to-bytes codec for an explicitly installed CIX SDK.

    It does not implement partial reads or writes, and it makes no statement
    about array dtype or shape because it receives serialized chunk bytes.
    """

    is_fixed_size = False

    def __init__(self, library_path: str | Path | None = None, **configuration: object) -> None:
        if library_path is None:
            self._codec = CIXNativeCodec.from_config({"id": CODEC_ID, **configuration})
        else:
            self._codec = CIXNativeCodec(library_path, **configuration)  # type: ignore[arg-type]

    @classmethod
    def _from_numcodecs(cls, codec: CIXNativeCodec) -> "CIXNativeBytesBytesCodec":
        result = object.__new__(cls)
        result._codec = codec
        return result

    @property
    def configuration(self) -> dict[str, object]:
        values = self._codec.get_config()
        values.pop("id")
        return values

    @classmethod
    def from_dict(cls, data: dict[str, object]) -> "CIXNativeBytesBytesCodec":
        if data.get("name") != CODEC_NAME:
            raise ValueError(f"unsupported Zarr codec name {data.get('name')!r}")
        configuration = data.get("configuration", {})
        if not isinstance(configuration, dict):
            raise TypeError("Zarr codec configuration must be an object")
        return cls._from_numcodecs(CIXNativeCodec.from_config({"id": CODEC_ID, **configuration}))

    def to_dict(self) -> dict[str, object]:
        return {"name": CODEC_NAME, "configuration": self.configuration}

    def _encode_sync(self, chunk_bytes: Any, chunk_spec: Any) -> Any:
        from zarr.core.buffer.cpu import as_numpy_array_wrapper
        return as_numpy_array_wrapper(self._codec.encode, chunk_bytes, chunk_spec.prototype)

    def _decode_sync(self, chunk_bytes: Any, chunk_spec: Any) -> Any:
        from zarr.core.buffer.cpu import as_numpy_array_wrapper
        return as_numpy_array_wrapper(self._codec.decode, chunk_bytes, chunk_spec.prototype)

    async def _encode_single(self, chunk_bytes: Any, chunk_spec: Any) -> Any:
        return await asyncio.to_thread(self._encode_sync, chunk_bytes, chunk_spec)

    async def _decode_single(self, chunk_bytes: Any, chunk_spec: Any) -> Any:
        return await asyncio.to_thread(self._decode_sync, chunk_bytes, chunk_spec)

    def compute_encoded_size(self, _input_byte_length: int, _chunk_spec: Any) -> int:
        raise NotImplementedError("CIX compressed output has variable size")


def register_zarr_v3(library_path: str | Path | None = None) -> type[CIXNativeBytesBytesCodec]:
    """Register the v3 BytesBytesCodec under its v3 ``name`` metadata."""
    if _BytesBytesCodec is object:
        raise ImportError("Zarr v3 is required for this registration")
    if library_path is not None:
        configure_native_library(library_path)
    from zarr.registry import register_codec
    register_codec(CODEC_NAME, CIXNativeBytesBytesCodec)
    return CIXNativeBytesBytesCodec
