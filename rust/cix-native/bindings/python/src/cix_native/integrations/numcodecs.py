"""Experimental Numcodecs adapter over :mod:`cix_native` buffer contexts.

The adapter accepts only contiguous bytes-like chunks. It deliberately has no
dtype, shape, or array interpretation: those belong to the host's serializer.
"""
from __future__ import annotations

from pathlib import Path
from threading import RLock
from typing import Any

from cix_native import NativeLibrary

CODEC_ID = "cix-native-experimental-v1"
_DEFAULT_OUTPUT_LIMIT = 64 << 20
_DEFAULT_MEMORY_LIMIT = 512 << 20
_TRUSTED_LIBRARY: NativeLibrary | None = None
_TRUSTED_LIBRARY_LOCK = RLock()

try:  # Keep importing the optional SDK usable without host packages.
    from numcodecs.abc import Codec as _CodecBase
except ImportError:  # pragma: no cover - exercised on SDK-only installations.
    _CodecBase = object


def _contiguous_view(value: Any, name: str) -> memoryview:
    try:
        view = memoryview(value)
    except TypeError as error:
        raise TypeError(f"{name} must export a buffer") from error
    if not view.c_contiguous:
        raise ValueError(f"{name} must be C-contiguous")
    return view


def configure_native_library(library_path: str | Path) -> NativeLibrary:
    """Set the process-local, explicitly trusted SDK used by metadata loaders.

    This function is the only integration API that loads a native library from
    a path. Configuration stored by Numcodecs or Zarr cannot change it.
    """
    path = Path(library_path)
    if not path.is_absolute():
        raise ValueError("library_path must be absolute")
    library = NativeLibrary(path)
    global _TRUSTED_LIBRARY
    with _TRUSTED_LIBRARY_LOCK:
        _TRUSTED_LIBRARY = library
    return library


def _trusted_library() -> NativeLibrary:
    with _TRUSTED_LIBRARY_LOCK:
        if _TRUSTED_LIBRARY is None:
            raise RuntimeError(
                "no trusted native SDK configured; call configure_native_library() first"
            )
        return _TRUSTED_LIBRARY


class CIXNativeCodec(_CodecBase):
    """Numcodecs-compatible, byte-preserving experimental CIX codec.

    ``library_path`` is deliberately explicit and absolute. It identifies a
    locally installed native SDK; it is not an archive dependency resolver.
    """

    codec_id = CODEC_ID

    def __init__(
        self,
        library_path: str | Path,
        *,
        profile: str = "default",
        workers: int = 1,
        output_limit: int = _DEFAULT_OUTPUT_LIMIT,
        memory_limit: int = _DEFAULT_MEMORY_LIMIT,
    ) -> None:
        path = Path(library_path)
        if not path.is_absolute():
            raise ValueError("library_path must be absolute")
        if profile not in ("fast", "default", "best"):
            raise ValueError("profile must be fast/default/best")
        if not isinstance(workers, int) or not 1 <= workers <= (1 << 32) - 1:
            raise ValueError("workers must fit uint32 and be positive")
        if not isinstance(output_limit, int) or output_limit <= 0:
            raise ValueError("output_limit must be a positive integer")
        if not isinstance(memory_limit, int) or memory_limit <= 0:
            raise ValueError("memory_limit must be a positive integer")
        self.library_path = str(path)
        self.profile = profile
        self.workers = workers
        self.output_limit = output_limit
        self.memory_limit = memory_limit
        self._library: NativeLibrary | None = None

    def _native_library(self) -> NativeLibrary:
        if self._library is None:
            self._library = NativeLibrary(self.library_path)
        return self._library

    def _context(self):
        return self._native_library().context(
            profile=self.profile,
            workers=self.workers,
            output_limit=self.output_limit,
            memory_limit=self.memory_limit,
        )

    def encode(self, buf: Any) -> bytes:
        view = _contiguous_view(buf, "buf")
        if view.nbytes > self.memory_limit:
            raise ValueError("input exceeds configured memory_limit")
        payload = view.tobytes()
        with self._context() as context:
            return context.encode(payload)

    def decode(self, buf: Any, out: Any | None = None) -> Any:
        view = _contiguous_view(buf, "buf")
        if view.nbytes > self.memory_limit:
            raise ValueError("input exceeds configured memory_limit")
        payload = view.tobytes()
        with self._context() as context:
            restored = context.decode(payload)
        if out is None:
            return restored
        try:
            target = memoryview(out)
        except TypeError as error:
            raise TypeError("out must export a writable buffer") from error
        if target.readonly or not target.c_contiguous or target.nbytes != len(restored):
            raise ValueError("out must be a writable C-contiguous buffer of the exact decoded size")
        target.cast("B")[:] = restored
        return out

    def get_config(self) -> dict[str, object]:
        return {
            "id": self.codec_id,
            "profile": self.profile,
            "workers": self.workers,
            "output_limit": self.output_limit,
            "memory_limit": self.memory_limit,
        }

    @classmethod
    def from_config(cls, config: dict[str, object]) -> "CIXNativeCodec":
        values = dict(config)
        codec_id = values.pop("id", cls.codec_id)
        if codec_id != cls.codec_id:
            raise ValueError(f"unsupported codec id {codec_id!r}")
        expected = {"profile", "workers", "output_limit", "memory_limit"}
        unknown = set(values).difference(expected)
        if unknown:
            raise ValueError(f"unsupported serialized CIX codec keys: {sorted(unknown)!r}")
        return cls(_trusted_library().path, **values)  # type: ignore[arg-type]


def register_numcodecs(library_path: str | Path | None = None) -> type[CIXNativeCodec]:
    """Register this explicit experimental ID with an installed Numcodecs host."""
    if library_path is not None:
        configure_native_library(library_path)
    try:
        from numcodecs.registry import register_codec
    except ImportError as error:
        raise ImportError("Numcodecs is required for this registration") from error
    register_codec(CIXNativeCodec)
    return CIXNativeCodec
