"""Explicit-path, stdlib-only bindings for the installed CIX native C ABI.

This package never searches PATH, compiles code, downloads dependencies, or is
loaded by the CIX executable.  Pass an absolute path to the qualified native
library to :class:`NativeLibrary`.
"""
from __future__ import annotations

import ctypes as _ct
from dataclasses import dataclass
from pathlib import Path
from threading import Event, RLock
from typing import Iterable, Literal

__all__ = [
    "CancelledError",
    "DictionaryIdentity",
    "Format",
    "NativeContext",
    "NativeError",
    "NativeLibrary",
    "StreamProgress",
    "ZstdDictionary",
]

_OK = 0
_INVALID_ARGUMENT = 1
_INVALID_OPTIONS = 2
_OUTPUT_TOO_SMALL = 3
_CODEC_ERROR = 4
_PANIC = 5
_RESOURCE_LIMIT = 6
_ABI_V1 = 1
_STREAM_NEEDS_INPUT = 1
_STREAM_NEEDS_OUTPUT = 2
_STREAM_FINISHED = 3
_U64_MAX = (1 << 64) - 1
_SIZE_T_MAX = (1 << (_ct.sizeof(_ct.c_size_t) * 8)) - 1
_MAX_C_UINT = (1 << 32) - 1
_MAX_FORMAT_INPUT = 128 << 20


class NativeError(RuntimeError):
    """A status returned by the native C ABI."""

    def __init__(self, status: int, message: str = "native CIX call failed") -> None:
        super().__init__(f"{message} (status {status})")
        self.status = status


class CancelledError(RuntimeError):
    """The Python stream wrapper was cancelled before a native call began."""


class _COptions(_ct.Structure):
    _fields_ = [
        ("abi_version", _ct.c_uint32),
        ("struct_size", _ct.c_uint32),
        ("profile", _ct.c_uint32),
        ("workers", _ct.c_uint32),
        ("output_limit", _ct.c_uint64),
        ("memory_limit", _ct.c_uint64),
    ]


class _CFormatOptions(_ct.Structure):
    _fields_ = [
        ("abi_version", _ct.c_uint32),
        ("struct_size", _ct.c_uint32),
        ("format", _ct.c_uint32),
        ("level", _ct.c_uint32),
        ("output_limit", _ct.c_uint64),
        ("memory_limit", _ct.c_uint64),
    ]


class _CDictionaryOptions(_ct.Structure):
    _fields_ = [
        ("abi_version", _ct.c_uint32),
        ("struct_size", _ct.c_uint32),
        ("level", _ct.c_int32),
        ("reserved", _ct.c_uint32),
        ("output_limit", _ct.c_uint64),
        ("memory_limit", _ct.c_uint64),
    ]


class _CDictionaryIdentity(_ct.Structure):
    _fields_ = [("zstd_id", _ct.c_uint32), ("sha256", _ct.c_ubyte * 32)]


class _CStreamResult(_ct.Structure):
    _fields_ = [
        ("consumed", _ct.c_size_t),
        ("produced", _ct.c_size_t),
        ("state", _ct.c_uint32),
    ]


@dataclass(frozen=True)
class DictionaryIdentity:
    """The exact Zstd dictionary identity required for decoding."""

    zstd_id: int
    sha256: bytes

    def __post_init__(self) -> None:
        if not isinstance(self.zstd_id, int) or not 0 <= self.zstd_id <= _MAX_C_UINT:
            raise ValueError("zstd_id must fit uint32")
        digest = bytes(self.sha256)
        if len(digest) != 32:
            raise ValueError("sha256 must contain exactly 32 bytes")
        object.__setattr__(self, "sha256", digest)


@dataclass(frozen=True)
class StreamProgress:
    """Bytes consumed/produced by one genuine native stream call."""

    consumed: int
    produced: int
    state: Literal["needs_input", "needs_output", "finished"]


class Format:
    """Integer identifiers defined by ``cix_formats.h``."""

    GZIP = 1
    ZLIB = 2
    DEFLATE = 3
    BZIP2 = 4
    XZ = 5
    ZSTD = 6
    BROTLI = 7
    LZ4_FRAME = 8
    SNAPPY_FRAMED = 9


def _u64(value: int, name: str, *, nonzero: bool = True) -> int:
    if not isinstance(value, int) or value < 0 or value > _U64_MAX or (nonzero and value == 0):
        raise ValueError(f"{name} must be a {'non-zero ' if nonzero else ''}unsigned 64-bit integer")
    return value


def _bytes(value: bytes | bytearray | memoryview, name: str) -> bytes:
    try:
        return bytes(value)
    except (TypeError, ValueError) as error:
        raise TypeError(f"{name} must be bytes-like") from error


def _byte_length(value: bytes | bytearray | memoryview, name: str) -> int:
    """Read a buffer length before making Python or ctypes copies."""
    try:
        return memoryview(value).nbytes
    except TypeError as error:
        raise TypeError(f"{name} must be bytes-like") from error


def _admit_input(value: bytes | bytearray | memoryview, memory_limit: int, name: str) -> int:
    """Reject an input before a wrapper copy can exceed its declared budget."""
    length = _byte_length(value, name)
    if length > memory_limit:
        raise ValueError(f"{name} exceeds the requested memory_limit")
    return length


def _admit_buffer_pair(input_length: int, output_length: int, memory_limit: int) -> None:
    """Check simultaneous caller-side input and output buffers without overflow."""
    if output_length > memory_limit - input_length:
        raise NativeError(_RESOURCE_LIMIT, "input plus output exceeds the requested memory_limit")


def _input(value: bytes) -> tuple[object | None, _ct.POINTER(_ct.c_ubyte) | None]:
    if not value:
        return None, None
    storage = (_ct.c_ubyte * len(value)).from_buffer_copy(value)
    return storage, _ct.cast(storage, _ct.POINTER(_ct.c_ubyte))


def _output(capacity: int) -> tuple[object | None, _ct.POINTER(_ct.c_ubyte) | None]:
    if capacity < 0:
        raise ValueError("output capacity must not be negative")
    if capacity == 0:
        return None, None
    try:
        storage = (_ct.c_ubyte * capacity)()
    except (MemoryError, OverflowError) as error:
        raise NativeError(_RESOURCE_LIMIT, "Python output allocation failed") from error
    return storage, _ct.cast(storage, _ct.POINTER(_ct.c_ubyte))


def _read_output(storage: object | None, count: int) -> bytes:
    if count == 0:
        return b""
    if storage is None:
        raise NativeError(_INVALID_ARGUMENT, "native library reported data for a zero output buffer")
    if count > len(storage):  # type: ignore[arg-type]
        raise NativeError(_CODEC_ERROR, "native library reported output beyond the supplied buffer")
    return bytes(storage[:count])  # type: ignore[index]


def _state(value: int) -> Literal["needs_input", "needs_output", "finished"]:
    states = {
        _STREAM_NEEDS_INPUT: "needs_input",
        _STREAM_NEEDS_OUTPUT: "needs_output",
        _STREAM_FINISHED: "finished",
    }
    try:
        return states[value]
    except KeyError as error:
        raise NativeError(_CODEC_ERROR, f"unknown native stream state {value}") from error


class NativeLibrary:
    """A library loaded only from an explicit, absolute qualified path."""

    def __init__(self, library_path: str | Path) -> None:
        path = Path(library_path)
        if not path.is_absolute():
            raise ValueError("library_path must be an absolute path")
        if not path.is_file():
            raise FileNotFoundError(path)
        self.path = path.resolve(strict=True)
        self._cdll = _ct.CDLL(str(self.path))
        self._bind()

    def _bind(self) -> None:
        void_p = _ct.c_void_p
        byte_p = _ct.POINTER(_ct.c_ubyte)
        size_p = _ct.POINTER(_ct.c_size_t)
        status = _ct.c_int
        lib = self._cdll
        lib.cix_options_v1_default.argtypes = [_ct.POINTER(_COptions)]
        lib.cix_options_v1_default.restype = status
        lib.cix_context_create.argtypes = [_ct.POINTER(_COptions), _ct.POINTER(void_p)]
        lib.cix_context_create.restype = status
        lib.cix_context_destroy.argtypes = [void_p]
        lib.cix_context_destroy.restype = None
        lib.cix_encode_buffer.argtypes = [void_p, byte_p, _ct.c_size_t, byte_p, _ct.c_size_t, size_p]
        lib.cix_encode_buffer.restype = status
        lib.cix_decode_buffer.argtypes = [void_p, byte_p, _ct.c_size_t, byte_p, _ct.c_size_t, size_p]
        lib.cix_decode_buffer.restype = status
        lib.cix_context_last_error.argtypes = [void_p, _ct.POINTER(_ct.c_char), _ct.c_size_t, size_p]
        lib.cix_context_last_error.restype = status

        lib.cix_format_encode_v1.argtypes = [_ct.POINTER(_CFormatOptions), byte_p, _ct.c_size_t, byte_p, _ct.c_size_t, size_p]
        lib.cix_format_encode_v1.restype = status
        lib.cix_format_decode_v1.argtypes = [_ct.POINTER(_CFormatOptions), byte_p, _ct.c_size_t, byte_p, _ct.c_size_t, size_p]
        lib.cix_format_decode_v1.restype = status

        for prefix in ("encoder", "decoder"):
            getattr(lib, f"cix_stream_{prefix}_create").argtypes = [_ct.POINTER(_COptions), _ct.POINTER(void_p)]
            getattr(lib, f"cix_stream_{prefix}_create").restype = status
            getattr(lib, f"cix_stream_{prefix}_destroy").argtypes = [void_p]
            getattr(lib, f"cix_stream_{prefix}_destroy").restype = None
            getattr(lib, f"cix_stream_{prefix}_reset").argtypes = [void_p]
            getattr(lib, f"cix_stream_{prefix}_reset").restype = status
            getattr(lib, f"cix_stream_{prefix}_finish").argtypes = [void_p, byte_p, _ct.c_size_t, _ct.POINTER(_CStreamResult)]
            getattr(lib, f"cix_stream_{prefix}_finish").restype = status
            if prefix == "encoder":
                lib.cix_stream_encoder_flush.argtypes = [void_p, byte_p, _ct.c_size_t, _ct.POINTER(_CStreamResult)]
                lib.cix_stream_encoder_flush.restype = status
            getattr(lib, f"cix_stream_{prefix}_process").argtypes = [void_p, byte_p, _ct.c_size_t, byte_p, _ct.c_size_t, _ct.POINTER(_CStreamResult)]
            getattr(lib, f"cix_stream_{prefix}_process").restype = status

        lib.cix_zstd_dictionary_create_v1.argtypes = [byte_p, _ct.c_size_t, _ct.c_uint64, _ct.POINTER(void_p)]
        lib.cix_zstd_dictionary_create_v1.restype = status
        lib.cix_zstd_dictionary_train_v1.argtypes = [byte_p, _ct.c_size_t, _ct.POINTER(_ct.c_size_t), _ct.c_size_t, _ct.c_size_t, _ct.c_uint64, _ct.POINTER(void_p)]
        lib.cix_zstd_dictionary_train_v1.restype = status
        lib.cix_zstd_dictionary_free.argtypes = [void_p]
        lib.cix_zstd_dictionary_free.restype = None
        lib.cix_zstd_dictionary_identity_v1.argtypes = [void_p, _ct.POINTER(_CDictionaryIdentity)]
        lib.cix_zstd_dictionary_identity_v1.restype = status
        lib.cix_zstd_dictionary_bytes_v1.argtypes = [void_p, byte_p, _ct.c_size_t, size_p]
        lib.cix_zstd_dictionary_bytes_v1.restype = status
        lib.cix_zstd_dictionary_encode_v1.argtypes = [void_p, _ct.POINTER(_CDictionaryOptions), byte_p, _ct.c_size_t, byte_p, _ct.c_size_t, size_p]
        lib.cix_zstd_dictionary_encode_v1.restype = status
        lib.cix_zstd_dictionary_decode_v1.argtypes = [void_p, _ct.POINTER(_CDictionaryOptions), _ct.POINTER(_CDictionaryIdentity), byte_p, _ct.c_size_t, byte_p, _ct.c_size_t, size_p]
        lib.cix_zstd_dictionary_decode_v1.restype = status

    def context(self, *, profile: Literal["fast", "default", "best"] = "default", workers: int = 1,
                output_limit: int = 64 << 20, memory_limit: int = 512 << 20) -> "NativeContext":
        return NativeContext(self, profile, workers, output_limit, memory_limit)

    def stream_encoder(self, **options: object) -> "NativeStream":
        return NativeStream(self, "encoder", **options)

    def stream_decoder(self, **options: object) -> "NativeStream":
        return NativeStream(self, "decoder", **options)

    def format_encode(self, format_id: int, data: bytes | bytearray | memoryview, *, level: int = 6,
                      output_limit: int = 64 << 20, memory_limit: int = 512 << 20) -> bytes:
        return self._format(False, format_id, data, level, output_limit, memory_limit)

    def format_decode(self, format_id: int, data: bytes | bytearray | memoryview, *, level: int = 6,
                      output_limit: int = 64 << 20, memory_limit: int = 512 << 20) -> bytes:
        return self._format(True, format_id, data, level, output_limit, memory_limit)

    def _format(self, decode: bool, format_id: int, data: bytes | bytearray | memoryview,
                level: int, output_limit: int, memory_limit: int) -> bytes:
        if not isinstance(format_id, int) or not 1 <= format_id <= 9 or not isinstance(level, int) or not 0 <= level <= 9:
            raise ValueError("format_id must be 1..9 and level must be 0..9")
        options = _CFormatOptions(_ABI_V1, _ct.sizeof(_CFormatOptions), format_id, level,
                                  _u64(output_limit, "output_limit"), _u64(memory_limit, "memory_limit"))
        _admit_input(data, int(options.memory_limit), "data")
        function = self._cdll.cix_format_decode_v1 if decode else self._cdll.cix_format_encode_v1
        payload = _bytes(data, "data")
        if len(payload) > _MAX_FORMAT_INPUT:
            raise ValueError("standard-format input exceeds the native 128 MiB limit")
        return _sized_buffer_call(function, _ct.byref(options), payload, output_limit, int(options.memory_limit))

    def dictionary_from_bytes(self, data: bytes | bytearray | memoryview, *, memory_limit: int = 128 << 20) -> "ZstdDictionary":
        return ZstdDictionary.from_bytes(self, data, memory_limit=memory_limit)

    def train_dictionary(self, samples: Iterable[bytes | bytearray | memoryview], *, dictionary_capacity: int = 112640,
                         memory_limit: int = 128 << 20) -> "ZstdDictionary":
        return ZstdDictionary.train(self, samples, dictionary_capacity=dictionary_capacity, memory_limit=memory_limit)


def _sized_buffer_call(function: object, prefix: object, data: bytes, output_limit: int,
                       memory_limit: int) -> bytes:
    _admit_buffer_pair(len(data), 0, memory_limit)
    storage, input_ptr = _input(data)
    needed = _ct.c_size_t(0)
    status = function(prefix, input_ptr, len(data), None, 0, _ct.byref(needed))  # type: ignore[operator]
    if status == _OK:
        return b""
    if status != _OUTPUT_TOO_SMALL:
        _raise(status)
    if needed.value > output_limit:
        raise NativeError(_RESOURCE_LIMIT, "native result exceeds requested output_limit")
    _admit_buffer_pair(len(data), needed.value, memory_limit)
    output_storage, output_ptr = _output(needed.value)
    status = function(prefix, input_ptr, len(data), output_ptr, needed.value, _ct.byref(needed))  # type: ignore[operator]
    _raise(status)
    return _read_output(output_storage, needed.value)


def _raise(status: int, message: str = "native CIX call failed") -> None:
    if status != _OK:
        raise NativeError(status, message)


class NativeContext:
    """A reusable complete-buffer native context; use it as a context manager."""

    def __init__(self, library: NativeLibrary, profile: str, workers: int, output_limit: int, memory_limit: int) -> None:
        profiles = {"fast": 1, "default": 2, "best": 3}
        if profile not in profiles or not isinstance(workers, int) or not 1 <= workers <= (1 << 32) - 1:
            raise ValueError("profile must be fast/default/best and workers must fit uint32")
        self._library, self._lock, self._handle = library, RLock(), _ct.c_void_p()
        self._output_limit = _u64(output_limit, "output_limit")
        self._memory_limit = _u64(memory_limit, "memory_limit")
        self._options = _COptions(_ABI_V1, _ct.sizeof(_COptions), profiles[profile], workers,
                                  self._output_limit, self._memory_limit)
        _raise(library._cdll.cix_context_create(_ct.byref(self._options), _ct.byref(self._handle)))

    def __enter__(self) -> "NativeContext": return self
    def __exit__(self, *_: object) -> None: self.close()

    def close(self) -> None:
        with self._lock:
            if self._handle.value:
                self._library._cdll.cix_context_destroy(self._handle)
                self._handle = _ct.c_void_p()

    def encode(self, data: bytes | bytearray | memoryview) -> bytes:
        return self._call(self._library._cdll.cix_encode_buffer, data)

    def decode(self, data: bytes | bytearray | memoryview) -> bytes:
        return self._call(self._library._cdll.cix_decode_buffer, data)

    def last_error(self) -> str:
        with self._lock:
            self._ensure_open()
            needed = _ct.c_size_t(0)
            status = self._library._cdll.cix_context_last_error(self._handle, None, 0, _ct.byref(needed))
            if status == _OK:
                return ""
            if status != _OUTPUT_TOO_SMALL:
                raise NativeError(status, "could not read native context diagnostic")
            output = _ct.create_string_buffer(needed.value)
            _raise(self._library._cdll.cix_context_last_error(self._handle, output, needed.value, _ct.byref(needed)))
            return output.value.decode("utf-8", "replace")

    def _call(self, function: object, data: bytes | bytearray | memoryview) -> bytes:
        _admit_input(data, self._memory_limit, "data")
        payload = _bytes(data, "data")
        with self._lock:
            self._ensure_open()
            storage, input_ptr = _input(payload)
            needed = _ct.c_size_t(0)
            status = function(self._handle, input_ptr, len(payload), None, 0, _ct.byref(needed))  # type: ignore[operator]
            if status == _OK:
                return b""
            if status != _OUTPUT_TOO_SMALL:
                _raise(status, self.last_error() or "native buffer operation failed")
            if needed.value > self._output_limit:
                raise NativeError(_RESOURCE_LIMIT, "native result exceeds requested output_limit")
            _admit_buffer_pair(len(payload), needed.value, self._memory_limit)
            output_storage, output_ptr = _output(needed.value)
            status = function(self._handle, input_ptr, len(payload), output_ptr, needed.value, _ct.byref(needed))  # type: ignore[operator]
            if status != _OK:
                _raise(status, self.last_error() or "native buffer operation failed")
            return _read_output(output_storage, needed.value)

    def _ensure_open(self) -> None:
        if not self._handle.value:
            raise RuntimeError("native context is closed")


class NativeStream:
    """Native CIXG1 independent-block stream with explicit backpressure."""

    def __init__(self, library: NativeLibrary, direction: Literal["encoder", "decoder"], **options: object) -> None:
        if direction not in ("encoder", "decoder"):
            raise ValueError("direction must be encoder or decoder")
        profile = options.pop("profile", "default")
        workers = options.pop("workers", 1)
        output_limit = options.pop("output_limit", 64 << 20)
        memory_limit = options.pop("memory_limit", 512 << 20)
        if options:
            raise TypeError(f"unknown stream options: {', '.join(options)}")
        profiles = {"fast": 1, "default": 2, "best": 3}
        if profile not in profiles or not isinstance(workers, int) or not 1 <= workers <= (1 << 32) - 1:
            raise ValueError("profile must be fast/default/best and workers must fit uint32")
        self._library, self._direction, self._lock = library, direction, RLock()
        self._cancelled, self._handle = Event(), _ct.c_void_p()
        self._total_produced = 0
        self._output_limit = _u64(output_limit, "output_limit")
        self._memory_limit = _u64(memory_limit, "memory_limit")
        self._options = _COptions(_ABI_V1, _ct.sizeof(_COptions), profiles[profile], workers,
                                  self._output_limit, self._memory_limit)
        create = getattr(library._cdll, f"cix_stream_{direction}_create")
        _raise(create(_ct.byref(self._options), _ct.byref(self._handle)))

    def __enter__(self) -> "NativeStream": return self
    def __exit__(self, *_: object) -> None: self.close()

    def cancel(self) -> None:
        """Stop future wrapper calls. The v1 C ABI cannot interrupt a call in flight."""
        self._cancelled.set()

    def reset(self) -> None:
        with self._lock:
            self._ensure_callable(ignore_cancel=True)
            _raise(getattr(self._library._cdll, f"cix_stream_{self._direction}_reset")(self._handle))
            self._total_produced = 0
            self._cancelled.clear()

    def close(self) -> None:
        with self._lock:
            if self._handle.value:
                getattr(self._library._cdll, f"cix_stream_{self._direction}_destroy")(self._handle)
                self._handle = _ct.c_void_p()

    def process(self, data: bytes | bytearray | memoryview, output_capacity: int) -> tuple[StreamProgress, bytes]:
        return self._stream_call("process", _bytes(data, "data"), output_capacity)

    def flush(self, output_capacity: int) -> tuple[StreamProgress, bytes]:
        if self._direction != "encoder":
            raise RuntimeError("only an encoder can flush")
        return self._stream_call("flush", None, output_capacity)

    def finish(self, output_capacity: int) -> tuple[StreamProgress, bytes]:
        return self._stream_call("finish", None, output_capacity)

    def _stream_call(self, operation: str, data: bytes | None, output_capacity: int) -> tuple[StreamProgress, bytes]:
        if not isinstance(output_capacity, int) or output_capacity < 0:
            raise ValueError("output_capacity must be a non-negative integer")
        payload = data or b""
        if output_capacity > self._output_limit - self._total_produced:
            raise ValueError("output_capacity exceeds the remaining stream output_limit")
        if len(payload) > self._memory_limit or output_capacity > self._memory_limit - len(payload):
            raise ValueError("input plus output_capacity exceeds the stream memory_limit")
        with self._lock:
            self._ensure_callable()
            input_storage, input_ptr = _input(payload)
            output_storage, output_ptr = _output(output_capacity)
            result = _CStreamResult()
            function = getattr(self._library._cdll, f"cix_stream_{self._direction}_{operation}")
            if operation == "process":
                status = function(self._handle, input_ptr, len(payload), output_ptr, output_capacity, _ct.byref(result))
            else:
                status = function(self._handle, output_ptr, output_capacity, _ct.byref(result))
            _raise(status, f"native stream {operation} failed")
            if result.produced > output_capacity:
                raise NativeError(_CODEC_ERROR, "native stream produced beyond caller capacity")
            if operation == "process" and result.consumed > len(payload):
                raise NativeError(_CODEC_ERROR, "native stream consumed beyond supplied input")
            if operation != "process" and result.consumed != 0:
                raise NativeError(_CODEC_ERROR, "native flush/finish reported input consumption")
            self._total_produced += result.produced
            return StreamProgress(result.consumed, result.produced, _state(result.state)), _read_output(output_storage, result.produced)

    def _ensure_callable(self, *, ignore_cancel: bool = False) -> None:
        if not self._handle.value:
            raise RuntimeError("native stream is closed")
        if self._cancelled.is_set() and not ignore_cancel:
            raise CancelledError("stream was cancelled; reset or close it before reuse")


class ZstdDictionary:
    """Owned native Zstd dictionary handle with explicit persistence bytes."""

    def __init__(self, library: NativeLibrary, handle: _ct.c_void_p) -> None:
        self._library, self._handle, self._lock = library, handle, RLock()

    def __enter__(self) -> "ZstdDictionary": return self
    def __exit__(self, *_: object) -> None: self.close()

    @classmethod
    def from_bytes(cls, library: NativeLibrary, data: bytes | bytearray | memoryview, *, memory_limit: int) -> "ZstdDictionary":
        memory_limit = _u64(memory_limit, "memory_limit")
        _admit_input(data, memory_limit, "data")
        payload = _bytes(data, "data")
        storage, pointer = _input(payload)
        handle = _ct.c_void_p()
        _raise(library._cdll.cix_zstd_dictionary_create_v1(pointer, len(payload), memory_limit, _ct.byref(handle)))
        return cls(library, handle)

    @classmethod
    def train(cls, library: NativeLibrary, samples: Iterable[bytes | bytearray | memoryview], *, dictionary_capacity: int, memory_limit: int) -> "ZstdDictionary":
        if not isinstance(dictionary_capacity, int) or not 1 <= dictionary_capacity <= _SIZE_T_MAX:
            raise ValueError("dictionary_capacity must fit size_t and be positive")
        memory_limit = _u64(memory_limit, "memory_limit")
        total = 0
        values: list[bytes] = []
        sizes: list[int] = []
        for sample in samples:
            try:
                sample_size = memoryview(sample).nbytes
            except TypeError as error:
                raise TypeError("sample must be bytes-like") from error
            if sample_size > _SIZE_T_MAX or len(values) >= _MAX_C_UINT:
                raise ValueError("sample size or sample count exceeds the native ABI")
            next_total = total + sample_size
            table_bytes = (len(values) + 1) * _ct.sizeof(_ct.c_size_t)
            # Python retains one sample copy and builds one flat byte string;
            # ctypes then makes the input copy used for the C call. These are
            # additional to, rather than substitutes for, native admission.
            if 3 * next_total + table_bytes + dictionary_capacity > memory_limit:
                raise ValueError("samples, table, and Python copies exceed the requested memory_limit")
            values.append(bytes(sample))
            sizes.append(sample_size)
            total = next_total
        if total > _SIZE_T_MAX:
            raise ValueError("training bytes exceed size_t")
        # The bounded admission above occurs before this potentially large join.
        blob = b"".join(values)
        del values
        size_table = (_ct.c_size_t * len(sizes))(*sizes)
        blob_storage, blob_pointer = _input(blob)
        handle = _ct.c_void_p()
        _raise(library._cdll.cix_zstd_dictionary_train_v1(
            blob_pointer, len(blob), size_table if sizes else None, len(sizes),
            dictionary_capacity, memory_limit, _ct.byref(handle)))
        return cls(library, handle)

    @property
    def identity(self) -> DictionaryIdentity:
        with self._lock:
            self._ensure_open()
            raw = _CDictionaryIdentity()
            _raise(self._library._cdll.cix_zstd_dictionary_identity_v1(self._handle, _ct.byref(raw)))
            return DictionaryIdentity(raw.zstd_id, bytes(raw.sha256))

    def export_bytes(self, *, max_bytes: int = 128 << 20) -> bytes:
        max_bytes = _u64(max_bytes, "max_bytes")
        with self._lock:
            self._ensure_open()
            needed = _ct.c_size_t(0)
            status = self._library._cdll.cix_zstd_dictionary_bytes_v1(self._handle, None, 0, _ct.byref(needed))
            if status == _OK:
                return b""
            if status != _OUTPUT_TOO_SMALL:
                _raise(status, "native dictionary export failed")
            if needed.value > max_bytes:
                raise NativeError(_RESOURCE_LIMIT, "dictionary export exceeds max_bytes")
            output_storage, output_ptr = _output(needed.value)
            _raise(self._library._cdll.cix_zstd_dictionary_bytes_v1(self._handle, output_ptr, needed.value, _ct.byref(needed)))
            return _read_output(output_storage, needed.value)

    def encode(self, data: bytes | bytearray | memoryview, *, level: int = 3, output_limit: int = 64 << 20, memory_limit: int = 128 << 20) -> bytes:
        return self._codec(False, data, level, output_limit, memory_limit, None)

    def decode(self, data: bytes | bytearray | memoryview, *, identity: DictionaryIdentity, level: int = 3,
               output_limit: int = 64 << 20, memory_limit: int = 128 << 20) -> bytes:
        return self._codec(True, data, level, output_limit, memory_limit, identity)

    def close(self) -> None:
        with self._lock:
            if self._handle.value:
                self._library._cdll.cix_zstd_dictionary_free(self._handle)
                self._handle = _ct.c_void_p()

    def _codec(self, decode: bool, data: bytes | bytearray | memoryview, level: int, output_limit: int,
               memory_limit: int, identity: DictionaryIdentity | None) -> bytes:
        if not isinstance(level, int) or not -131072 <= level <= 22:
            raise ValueError("level must be a supported Zstd level")
        options = _CDictionaryOptions(_ABI_V1, _ct.sizeof(_CDictionaryOptions), level, 0,
                                      _u64(output_limit, "output_limit"), _u64(memory_limit, "memory_limit"))
        _admit_input(data, int(options.memory_limit), "data")
        payload = _bytes(data, "data")
        with self._lock:
            self._ensure_open()
            input_storage, input_ptr = _input(payload)
            needed = _ct.c_size_t(0)
            if decode:
                if identity is None or len(identity.sha256) != 32:
                    raise ValueError("decode requires a 32-byte DictionaryIdentity")
                raw_identity = _CDictionaryIdentity(identity.zstd_id, (_ct.c_ubyte * 32).from_buffer_copy(identity.sha256))
                function = self._library._cdll.cix_zstd_dictionary_decode_v1
                args = (self._handle, _ct.byref(options), _ct.byref(raw_identity), input_ptr, len(payload))
            else:
                function = self._library._cdll.cix_zstd_dictionary_encode_v1
                args = (self._handle, _ct.byref(options), input_ptr, len(payload))
            status = function(*args, None, 0, _ct.byref(needed))
            if status == _OK:
                return b""
            if status != _OUTPUT_TOO_SMALL:
                _raise(status, "native dictionary operation failed")
            if needed.value > output_limit:
                raise NativeError(_RESOURCE_LIMIT, "native result exceeds requested output_limit")
            _admit_buffer_pair(len(payload), needed.value, int(options.memory_limit))
            output_storage, output_ptr = _output(needed.value)
            _raise(function(*args, output_ptr, needed.value, _ct.byref(needed)), "native dictionary operation failed")
            return _read_output(output_storage, needed.value)

    def _ensure_open(self) -> None:
        if not self._handle.value:
            raise RuntimeError("native dictionary is closed")
