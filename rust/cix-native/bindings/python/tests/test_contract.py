import os
from pathlib import Path
import unittest

from cix_native import NativeLibrary, NativeError


class BindingContract(unittest.TestCase):
    def test_rejects_relative_library_path(self):
        with self.assertRaises(ValueError):
            NativeLibrary("libcix_native.so")


@unittest.skipUnless(os.environ.get("CIX_PYTHON_NATIVE_LIBRARY"), "qualified library path not supplied")
class QualifiedNativeContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.library = NativeLibrary(Path(os.environ["CIX_PYTHON_NATIVE_LIBRARY"]))

    def test_buffer_and_standard_format_roundtrips(self):
        payload = b"python native binding payload" * 32
        with self.library.context(output_limit=1 << 20, memory_limit=128 << 20) as context:
            archive = context.encode(payload)
            self.assertEqual(context.decode(archive), payload)
        standard = self.library.format_encode(6, payload, output_limit=4 << 20, memory_limit=512 << 20)
        self.assertEqual(self.library.format_decode(6, standard, output_limit=4 << 20, memory_limit=512 << 20), payload)

    def test_dictionary_export_import_and_wrong_identity(self):
        samples = [(f"sample-{index:03d}-".encode() + b"dictionary material " * 24) for index in range(64)]
        with self.library.train_dictionary(samples, dictionary_capacity=1024, memory_limit=128 << 20) as source:
            identity = source.identity
            persisted = source.export_bytes()
            archive = source.encode(b"dictionary material dictionary material", output_limit=1 << 20, memory_limit=128 << 20)
        with self.library.dictionary_from_bytes(persisted, memory_limit=128 << 20) as imported:
            self.assertEqual(imported.identity, identity)
            self.assertEqual(imported.decode(archive, identity=identity, output_limit=1 << 20, memory_limit=128 << 20), b"dictionary material dictionary material")
            wrong = type(identity)(identity.zstd_id, b"\0" * 32)
            with self.assertRaises(NativeError):
                imported.decode(archive, identity=wrong, output_limit=1 << 20, memory_limit=128 << 20)

    def test_stream_reports_native_progress_and_cancellation(self):
        with self.library.stream_encoder(output_limit=1 << 20, memory_limit=128 << 20) as stream:
            progress, data = stream.process(b"fragment", 4096)
            self.assertLessEqual(progress.consumed, len(b"fragment"))
            self.assertEqual(progress.produced, len(data))
            stream.cancel()
            with self.assertRaises(RuntimeError):
                stream.process(b"more", 4096)
            stream.reset()

class PreallocationContract(unittest.TestCase):
    def test_context_rejects_input_before_wrapper_copy_or_native_call(self):
        from ctypes import c_void_p
        from threading import RLock

        from cix_native import NativeContext

        context = NativeContext.__new__(NativeContext)
        context._lock = RLock()
        context._handle = c_void_p(1)
        context._memory_limit = 1
        context._output_limit = 8
        called = False

        def native_call(*_args):
            nonlocal called
            called = True
            return 0

        with self.assertRaises(ValueError):
            context._call(native_call, b"ab")
        self.assertFalse(called)

    def test_sizing_rejects_simultaneous_input_and_output_before_allocation(self):
        import ctypes
        import cix_native

        calls = 0

        def sizing_call(_prefix, _input, _input_length, _output, _capacity, needed):
            nonlocal calls
            calls += 1
            ctypes.cast(needed, ctypes.POINTER(ctypes.c_size_t)).contents.value = 2
            return 3  # CIX_STATUS_OUTPUT_TOO_SMALL

        with self.assertRaises(NativeError) as raised:
            cix_native._sized_buffer_call(sizing_call, None, b"x", 8, 2)
        self.assertEqual(raised.exception.status, 6)
        self.assertEqual(calls, 1)

    def test_rejects_native_second_pass_length_beyond_allocated_storage(self):
        import cix_native

        with self.assertRaises(NativeError) as raised:
            cix_native._read_output(bytearray(1), 2)
        self.assertEqual(raised.exception.status, 4)

    def test_dictionary_import_rejects_before_ctypes_copy_or_library_call(self):
        from cix_native import ZstdDictionary

        with self.assertRaises(ValueError):
            ZstdDictionary.from_bytes(object(), b"ab", memory_limit=1)

    def test_training_stops_an_oversize_generator_before_later_items(self):
        consumed = []

        def samples():
            consumed.append("first")
            yield b"x" * 1024
            consumed.append("second")
            yield b"never reached"

        from cix_native import ZstdDictionary
        with self.assertRaises(ValueError):
            ZstdDictionary.train(object(), samples(), dictionary_capacity=256, memory_limit=1024)
        self.assertEqual(consumed, ["first"])

    def test_stream_rejects_oversize_output_before_native_or_python_allocation(self):
        from cix_native import NativeStream
        from ctypes import c_void_p
        from threading import Event, RLock

        stream = NativeStream.__new__(NativeStream)
        stream._output_limit = 1
        stream._memory_limit = 1
        stream._total_produced = 0
        stream._handle = c_void_p(1)
        stream._cancelled = Event()
        stream._lock = RLock()
        with self.assertRaises(ValueError):
            stream._stream_call("process", b"", 2)


def _feed_fragments(stream, payload, *, chunk_size=7, output_capacity=4096):
    """Feed actual stream fragments without hiding native progress states."""
    output = bytearray()
    offset = 0
    calls = 0
    while offset < len(payload):
        calls += 1
        if calls > len(payload) * 4 + 32:
            raise AssertionError("fragmented stream process made no bounded progress")
        fragment = payload[offset:offset + chunk_size]
        progress, produced = stream.process(fragment, output_capacity)
        if progress.consumed > len(fragment) or progress.produced != len(produced):
            raise AssertionError("invalid native stream process result")
        output.extend(produced)
        offset += progress.consumed
        if progress.state == "finished":
            if offset != len(payload):
                raise NativeError(4, "native stream finished before trailing input")
            break
        if progress.consumed == 0 and progress.produced == 0:
            raise AssertionError("stream process made no progress")
    return bytes(output)


def _finish_encoder(stream, output_capacity=4096):
    """Drain flush then finish; return immediately if flush has finished."""
    output = bytearray()
    for _ in range(64):
        progress, produced = stream.flush(output_capacity)
        if progress.consumed != 0 or progress.produced != len(produced):
            raise AssertionError("invalid native encoder flush result")
        output.extend(produced)
        if progress.state == "finished":
            return bytes(output)
        if progress.state == "needs_input":
            break
        if progress.produced == 0:
            raise AssertionError("encoder flush requested output without producing bytes")
    else:
        raise AssertionError("encoder flush did not reach a bounded terminal state")
    for _ in range(64):
        progress, produced = stream.finish(output_capacity)
        if progress.consumed != 0 or progress.produced != len(produced):
            raise AssertionError("invalid native encoder finish result")
        output.extend(produced)
        if progress.state == "finished":
            return bytes(output)
        if progress.produced == 0:
            raise AssertionError("encoder finish made no progress")
    raise AssertionError("encoder finish did not terminate")


def _finish_decoder(stream, output_capacity=4096):
    output = bytearray()
    for _ in range(64):
        progress, produced = stream.finish(output_capacity)
        if progress.consumed != 0 or progress.produced != len(produced):
            raise AssertionError("invalid native decoder finish result")
        output.extend(produced)
        if progress.state == "finished":
            return bytes(output)
        if progress.produced == 0:
            raise AssertionError("decoder finish made no progress")
    raise AssertionError("decoder finish did not terminate")


@unittest.skipUnless(os.environ.get("CIX_PYTHON_NATIVE_LIBRARY"), "qualified library path not supplied")
class QualifiedFragmentedStreamContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.library = NativeLibrary(Path(os.environ["CIX_PYTHON_NATIVE_LIBRARY"]))

    def _encode(self, payload):
        with self.library.stream_encoder(output_limit=1 << 20, memory_limit=128 << 20) as encoder:
            archive = bytearray(_feed_fragments(encoder, payload))
            archive.extend(_finish_encoder(encoder))
            return bytes(archive)

    def _decode(self, archive):
        with self.library.stream_decoder(output_limit=1 << 20, memory_limit=128 << 20) as decoder:
            payload = bytearray(_feed_fragments(decoder, archive))
            payload.extend(_finish_decoder(decoder))
            return bytes(payload)

    def test_fragmented_roundtrip_flush_finish_and_empty(self):
        payload = (b"fragmented native CIXG1 stream " * 40) + bytes(range(64))
        archive = self._encode(payload)
        self.assertEqual(self._decode(archive), payload)
        self.assertEqual(self._decode(self._encode(b"")), b"")

    def test_reset_and_trailing_failure(self):
        with self.library.stream_encoder(output_limit=1 << 20, memory_limit=128 << 20) as encoder:
            first = bytearray(_feed_fragments(encoder, b"first stream"))
            first.extend(_finish_encoder(encoder))
            encoder.reset()
            second = bytearray(_feed_fragments(encoder, b"second stream"))
            second.extend(_finish_encoder(encoder))
        self.assertEqual(self._decode(bytes(first)), b"first stream")
        self.assertEqual(self._decode(bytes(second)), b"second stream")
        with self.library.stream_decoder(output_limit=1 << 20, memory_limit=128 << 20) as decoder:
            with self.assertRaises(NativeError):
                _feed_fragments(decoder, bytes(second) + b"trailing")
