"""Bounded binary file adapters for the public native CIXG1 stream API."""
from __future__ import annotations

from . import CancelledError, NativeError, NativeLibrary

__all__ = ["encode_file", "decode_file"]


def _positive(value, name):
    if type(value) is not int or value <= 0:
        raise ValueError(f"{name} must be a positive integer")
    return value


def _cancel_check(cancel):
    if cancel is None:
        return
    predicate = cancel if callable(cancel) else getattr(cancel, "is_set", None)
    if not callable(predicate):
        raise TypeError("cancel must be callable or expose is_set()")
    if predicate():
        raise CancelledError("file operation cancelled between native calls")


def _read(reader, size):
    data = reader.read(size)
    if not isinstance(data, (bytes, bytearray, memoryview)):
        raise TypeError("read(size) must return binary bytes, never None or text")
    view = memoryview(data)
    if not view.c_contiguous or view.ndim != 1 or view.itemsize != 1:
        raise TypeError("read(size) must return a contiguous one-dimensional byte buffer")
    if view.nbytes > size:
        raise ValueError("reader returned more bytes than requested")
    # An empty binary read is EOF; nonblocking readers must raise instead.
    return bytes(view)


def _write(writer, data):
    view = memoryview(data)
    offset = 0
    while offset < len(view):
        count = writer.write(view[offset:])
        if type(count) is not int or not 0 < count <= len(view) - offset:
            raise ValueError("write() must return a positive count within the offered buffer")
        offset += count


def _progress(result, supplied, capacity):
    progress, data = result
    if not isinstance(data, bytes):
        raise TypeError("native stream output must be bytes")
    if (type(progress.consumed) is not int or type(progress.produced) is not int
            or not 0 <= progress.consumed <= supplied
            or not 0 <= progress.produced <= capacity
            or progress.produced != len(data)
            or progress.state not in ("needs_input", "needs_output", "finished")):
        raise RuntimeError("invalid native stream counts or state")
    # State is advisory; retain unconsumed bytes and honor exact consumed count.
    return progress, data


def _transfer(library, reader, writer, decode, read_chunk, output_chunk,
              cancel, profile, workers, output_limit, memory_limit):
    _positive(read_chunk, "read_chunk")
    _positive(output_chunk, "output_chunk")
    _positive(output_limit, "output_limit")
    _positive(memory_limit, "memory_limit")
    if read_chunk + output_chunk > memory_limit:
        raise ValueError("read_chunk plus output_chunk exceeds memory_limit")
    if not callable(getattr(reader, "read", None)) or not callable(getattr(writer, "write", None)):
        raise TypeError("reader and writer must expose binary read/write methods")
    _cancel_check(cancel)
    factory = library.stream_decoder if decode else library.stream_encoder
    total = 0
    idle = 0
    pending = b""
    eof = False
    state = "needs_input"
    with factory(profile=profile, workers=workers, output_limit=output_limit,
                 memory_limit=memory_limit) as stream:
        while True:
            _cancel_check(cancel)
            if not pending and not eof and state != "needs_output":
                pending = _read(reader, read_chunk)
                eof = not pending
            capacity = min(output_chunk, output_limit - total)
            _cancel_check(cancel)
            finishing = eof and not pending
            result = stream.finish(capacity) if finishing else stream.process(pending, capacity)
            progress, data = _progress(result, 0 if finishing else len(pending), capacity)
            if finishing and progress.state == "needs_input":
                raise RuntimeError("native finish requested input after EOF")
            pending = pending[progress.consumed:]
            total += progress.produced
            _write(writer, data)
            idle = idle + 1 if not progress.consumed and not progress.produced else 0
            state = progress.state
            if state == "finished":
                if not decode and not finishing:
                    raise RuntimeError("encoder finished before EOF")
                if pending:
                    raise ValueError("trailing input after archive terminator")
                if not eof:
                    _cancel_check(cancel)
                    if _read(reader, read_chunk):
                        raise ValueError("trailing input after archive terminator")
                if not finishing:
                    # Completion still passes through the public finish validator.
                    eof = True
                    continue
                return total
            if capacity == 0 and state == "needs_output":
                raise NativeError(6, "file operation exceeds output_limit")
            if idle >= 8:
                raise RuntimeError("native stream made no byte progress in eight calls")


def encode_file(library: NativeLibrary, reader, writer, *, read_chunk=65536,
                output_chunk=65536, cancel=None, profile="default", workers=1,
                output_limit=64 << 20, memory_limit=512 << 20) -> int:
    """Encode to a caller-owned binary writer; return the bytes written.

    Reads and writes may be short. Empty reads mean EOF. Neither handle is
    closed or flushed. Cancellation is cooperative between native calls;
    it cannot interrupt native work or blocking I/O already in flight.
    Errors may leave a partial destination; no rollback is attempted.
    """
    return _transfer(library, reader, writer, False, read_chunk, output_chunk,
                     cancel, profile, workers, output_limit, memory_limit)


def decode_file(library: NativeLibrary, reader, writer, *, read_chunk=65536,
                output_chunk=65536, cancel=None, profile="default", workers=1,
                output_limit=64 << 20, memory_limit=512 << 20) -> int:
    """Decode exactly one archive, rejecting truncation and trailing input.

    Ownership, cancellation, return value and partial-error output follow
    encode_file. Native memory admission is not a process RSS ceiling.
    """
    return _transfer(library, reader, writer, True, read_chunk, output_chunk,
                     cancel, profile, workers, output_limit, memory_limit)
