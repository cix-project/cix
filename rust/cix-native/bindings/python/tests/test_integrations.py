from __future__ import annotations

import os
from pathlib import Path

import pytest

from cix_native.integrations.numcodecs import CIXNativeCodec, CODEC_ID


def native_library_path() -> Path:
    value = os.environ.get("CIX_NATIVE_LIBRARY")
    if not value:
        pytest.skip("requires qualified installed CIX_NATIVE_LIBRARY")
    path = Path(value)
    if not path.is_absolute() or not path.is_file():
        pytest.skip("CIX_NATIVE_LIBRARY must name an installed absolute library")
    return path


def test_numcodecs_config_is_json_safe_and_roundtrips_qualified_sdk() -> None:
    pytest.importorskip("numcodecs")
    from cix_native.integrations.numcodecs import register_numcodecs

    library_path = native_library_path()
    codec = CIXNativeCodec(library_path, output_limit=1 << 20, memory_limit=8 << 20)
    register_numcodecs(library_path)
    assert codec.get_config()["id"] == CODEC_ID
    source = bytes((index * 29 + 7) & 255 for index in range(4099))
    assert codec.decode(codec.encode(source)) == source


def test_zarr_v2_reuses_numcodecs_codec_with_qualified_sdk() -> None:
    pytest.importorskip("numcodecs")
    zarr = pytest.importorskip("zarr")
    numpy = pytest.importorskip("numpy")
    from cix_native.integrations.zarr_v2 import CIXNativeV2Codec, register_zarr_v2

    library_path = native_library_path()
    register_zarr_v2(library_path)
    codec = CIXNativeV2Codec(library_path, output_limit=1 << 20, memory_limit=8 << 20)
    source = b"zarr-v2-byte-neutral" * 300
    store = zarr.storage.MemoryStore()
    array = zarr.create(
        store=store,
        shape=(len(source),),
        chunks=(len(source),),
        dtype="u1",
        compressor=codec,
        zarr_format=2,
    )
    array[:] = numpy.frombuffer(source, dtype="u1")
    reopened = zarr.open_array(store=store, zarr_format=2)
    assert reopened[:].tobytes() == source


def test_zarr_v3_bytes_bytes_codec_with_qualified_sdk() -> None:
    zarr = pytest.importorskip("zarr")
    numpy = pytest.importorskip("numpy")
    from cix_native.integrations.zarr_v3 import (
        CODEC_NAME,
        CIXNativeBytesBytesCodec,
        register_zarr_v3,
    )

    library_path = native_library_path()
    register_zarr_v3(library_path)
    codec = CIXNativeBytesBytesCodec(
        library_path=str(library_path), output_limit=1 << 20, memory_limit=8 << 20
    )
    assert codec.to_dict()["name"] == CODEC_NAME
    restored = CIXNativeBytesBytesCodec.from_dict(codec.to_dict())
    assert restored.configuration == codec.configuration
    source = b"zarr-v3-byte-neutral" * 300
    store = zarr.storage.MemoryStore()
    array = zarr.create_array(
        store=store,
        shape=(len(source),),
        chunks=(len(source),),
        dtype="u1",
        serializer=zarr.codecs.BytesCodec(),
        compressors=[codec],
        zarr_format=3,
    )
    array[:] = numpy.frombuffer(source, dtype="u1")
    reopened = zarr.open_array(store=store, zarr_format=3)
    assert reopened[:].tobytes() == source


def test_serialized_metadata_cannot_select_a_native_library() -> None:
    with pytest.raises(ValueError, match="serialized CIX codec keys"):
        CIXNativeCodec.from_config({"id": CODEC_ID, "library_path": "/tmp/untrusted.so"})
    with pytest.raises(ValueError, match="serialized CIX codec keys"):
        CIXNativeCodec.from_config({"id": CODEC_ID, "unexpected": True})
