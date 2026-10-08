package dev.cix.binding;

import java.io.ByteArrayOutputStream;
import java.nio.file.Path;
import java.util.Arrays;

/** Plain-javac contract executable; environment paths must name qualified artifacts. */
public final class Contract {
    private static final int CAPACITY = 4096;
    private Contract() {}

    public static void main(String[] args) {
        String nativeLibrary = System.getenv("CIX_NATIVE_LIBRARY");
        String jniLibrary = System.getenv("CIX_JNI_LIBRARY");
        if (nativeLibrary == null || jniLibrary == null) {
            throw new IllegalStateException("CIX_NATIVE_LIBRARY and CIX_JNI_LIBRARY are required");
        }
        NativeCix.load(Path.of(nativeLibrary), Path.of(jniLibrary));
        expect(IllegalArgumentException.class, () -> new NativeCix.Options(NativeCix.DEFAULT, 0, 1, 1));
        NativeCix.Options options = new NativeCix.Options(NativeCix.DEFAULT, 1, 1L << 20, 128L << 20);
        byte[] payload = "JNI native contract ".repeat(128).getBytes(java.nio.charset.StandardCharsets.UTF_8);
        try (NativeCix.Context context = NativeCix.context(options)) {
            check(Arrays.equals(payload, context.decode(context.encode(payload))), "complete-buffer roundtrip");
        }
        byte[] archive;
        try (NativeCix.Encoder encoder = NativeCix.encoder(options)) {
            byte[] alias = new byte[8];
            expect(IllegalArgumentException.class, () -> encoder.process(alias, alias));
            archive = encode(encoder, payload);
            encoder.reset();
            check(Arrays.equals(archive, encode(encoder, payload)), "reset deterministic stream");
        }
        try (NativeCix.Decoder decoder = NativeCix.decoder(options)) {
            check(Arrays.equals(payload, decode(decoder, archive)), "fragmented stream roundtrip");
        }
    }

    private static byte[] encode(NativeCix.Encoder encoder, byte[] input) {
        ByteArrayOutputStream archive = new ByteArrayOutputStream();
        feed(encoder, input, archive);
        for (int tries = 0; tries < 64; tries++) {
            NativeCix.ProgressAndBytes step = encoder.flush(new byte[CAPACITY]);
            archive.writeBytes(step.bytes());
            if (step.progress().state() == NativeCix.State.FINISHED) return archive.toByteArray();
            if (step.progress().state() == NativeCix.State.NEEDS_INPUT) break;
            check(step.progress().produced() > 0, "flush backpressure progress");
        }
        for (int tries = 0; tries < 64; tries++) {
            NativeCix.ProgressAndBytes step = encoder.finish(new byte[CAPACITY]);
            archive.writeBytes(step.bytes());
            if (step.progress().state() == NativeCix.State.FINISHED) return archive.toByteArray();
            check(step.progress().produced() > 0, "finish progress");
        }
        throw new AssertionError("encoder did not finish");
    }

    private static byte[] decode(NativeCix.Decoder decoder, byte[] archive) {
        ByteArrayOutputStream restored = new ByteArrayOutputStream();
        feed(decoder, archive, restored);
        for (int tries = 0; tries < 64; tries++) {
            NativeCix.ProgressAndBytes step = decoder.finish(new byte[CAPACITY]);
            restored.writeBytes(step.bytes());
            if (step.progress().state() == NativeCix.State.FINISHED) return restored.toByteArray();
            check(step.progress().produced() > 0, "decoder finish progress");
        }
        throw new AssertionError("decoder did not finish");
    }

    private static void feed(NativeCix.Stream stream, byte[] input, ByteArrayOutputStream output) {
        int offset = 0;
        for (int tries = 0; offset < input.length && tries < input.length * 4 + 32; tries++) {
            byte[] fragment = Arrays.copyOfRange(input, offset, Math.min(input.length, offset + 7));
            NativeCix.ProgressAndBytes step = stream.process(fragment, new byte[CAPACITY]);
            check(step.progress().consumed() <= fragment.length, "native consumed count");
            check(step.progress().produced() == step.bytes().length, "native produced count");
            output.writeBytes(step.bytes());
            offset += step.progress().consumed();
            if (step.progress().state() == NativeCix.State.FINISHED) {
                check(offset == input.length, "finished before all input");
                return;
            }
            check(step.progress().consumed() != 0 || step.progress().produced() != 0, "stream process progress");
        }
        check(offset == input.length, "fragmented feed did not finish input");
    }

    private static void expect(Class<? extends Throwable> type, Runnable operation) {
        try { operation.run(); }
        catch (Throwable error) {
            if (type.isInstance(error)) return;
            throw new AssertionError("unexpected exception", error);
        }
        throw new AssertionError("expected " + type.getSimpleName());
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
}
