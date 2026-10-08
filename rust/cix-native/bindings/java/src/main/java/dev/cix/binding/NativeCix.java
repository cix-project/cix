package dev.cix.binding;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Objects;

/** Optional JNI consumer for the installed native CIX C ABI. */
public final class NativeCix {
    public static final int FAST = 1;
    public static final int DEFAULT = 2;
    public static final int BEST = 3;
    private static volatile boolean loaded;
    private static Path loadedNativeLibrary;
    private static Path loadedJniLibrary;

    private NativeCix() {}

    /**
     * Loads exactly the two supplied absolute shared libraries. This does not
     * search PATH, invoke the CLI, compile code, or discover a host library.
     */
    public static synchronized void load(Path cixNativeLibrary, Path jniLibrary) {
        Path nativePath = canonicalRegularFile(cixNativeLibrary);
        Path jniPath = canonicalRegularFile(jniLibrary);
        if (loaded) {
            if (!loadedNativeLibrary.equals(nativePath) || !loadedJniLibrary.equals(jniPath)) {
                throw new IllegalStateException("NativeCix is already loaded with different library paths");
            }
            return;
        }
        System.load(nativePath.toString());
        System.load(jniPath.toString());
        loadedNativeLibrary = nativePath;
        loadedJniLibrary = jniPath;
        loaded = true;
    }

    public static Context context(Options options) {
        requireLoaded();
        return new Context(options);
    }

    public static Encoder encoder(Options options) {
        requireLoaded();
        return new Encoder(options);
    }

    public static Decoder decoder(Options options) {
        requireLoaded();
        return new Decoder(options);
    }

    private static Path canonicalRegularFile(Path path) {
        Objects.requireNonNull(path, "library path");
        if (!path.isAbsolute() || !Files.isRegularFile(path)) {
            throw new IllegalArgumentException("library path must be an existing absolute regular file");
        }
        try {
            return path.toRealPath();
        } catch (java.io.IOException error) {
            throw new IllegalArgumentException("could not canonicalize library path", error);
        }
    }

    private static void requireLoaded() {
        if (!loaded) throw new IllegalStateException("call NativeCix.load with explicit absolute library paths first");
    }

    public record Options(int profile, int workers, long outputLimit, long memoryLimit) {
        public Options {
            if (profile != FAST && profile != DEFAULT && profile != BEST) {
                throw new IllegalArgumentException("profile must be FAST, DEFAULT, or BEST");
            }
            if (workers <= 0 || outputLimit <= 0 || memoryLimit <= 0) {
                throw new IllegalArgumentException("workers, outputLimit, and memoryLimit must be positive");
            }
        }
        public static Options defaults() { return new Options(DEFAULT, 1, 64L << 20, 512L << 20); }
    }

    public record Progress(int consumed, int produced, State state) {}
    public enum State { NEEDS_INPUT, NEEDS_OUTPUT, FINISHED }

    /** Native C ABI failure; the message includes the unmodified C status. */
    public static final class NativeException extends RuntimeException {
        NativeException(String message) { super(message); }
    }
    /** Local cancellation fence; the v1 C ABI cannot interrupt an in-flight call. */
    public static final class CancelledException extends IllegalStateException {
        CancelledException() { super("stream was cancelled before entering the native ABI"); }
    }

    public static final class Context implements AutoCloseable {
        private final Options options;
        private long handle;
        private Context(Options options) {
            this.options = Objects.requireNonNull(options, "options");
            this.handle = createContext(options.profile, options.workers, options.outputLimit, options.memoryLimit);
            if (handle == 0) throw new NativeException("native context creation returned a null handle");
        }
        public synchronized byte[] encode(byte[] input) { return buffer(input, true); }
        public synchronized byte[] decode(byte[] input) { return buffer(input, false); }
        private byte[] buffer(byte[] input, boolean encode) {
            requireOpen();
            Objects.requireNonNull(input, "input");
            if ((long) input.length > options.memoryLimit) {
                throw new IllegalArgumentException("input exceeds context memory limit");
            }
            return bufferCall(handle, input, encode, options.outputLimit, options.memoryLimit);
        }
        private void requireOpen() { if (handle == 0) throw new IllegalStateException("context is closed"); }
        @Override public synchronized void close() { if (handle != 0) { destroyContext(handle); handle = 0; } }
    }

    public abstract static class Stream implements AutoCloseable {
        final Options options;
        final boolean encoder;
        long handle;
        long totalProduced;
        boolean cancelled;
        Stream(Options options, boolean encoder) {
            this.options = Objects.requireNonNull(options, "options");
            this.encoder = encoder;
            this.handle = createStream(encoder, options.profile, options.workers, options.outputLimit, options.memoryLimit);
            if (handle == 0) throw new NativeException("native stream creation returned a null handle");
        }
        /** Stops later wrapper calls; it cannot interrupt a native call already running. */
        public synchronized void cancel() { cancelled = true; }
        public synchronized void reset() {
            requireOpen(true);
            resetStream(handle, encoder);
            totalProduced = 0;
            cancelled = false;
        }
        public synchronized ProgressAndBytes process(byte[] input, byte[] output) {
            requireOpen(false);
            Objects.requireNonNull(input, "input");
            if (input == output) throw new IllegalArgumentException("stream input and output arrays must be distinct");
            return call(input, output, 0);
        }
        public synchronized ProgressAndBytes finish(byte[] output) {
            requireOpen(false);
            return call(null, output, 2);
        }
        public synchronized ProgressAndBytes flush(byte[] output) {
            if (!encoder) throw new IllegalStateException("only an encoder can flush");
            requireOpen(false);
            return call(null, output, 1);
        }
        private ProgressAndBytes call(byte[] input, byte[] output, int operation) {
            Objects.requireNonNull(output, "output");
            int inputLength = input == null ? 0 : input.length;
            if ((long) output.length > options.outputLimit - totalProduced) {
                throw new IllegalArgumentException("output buffer exceeds remaining stream output limit");
            }
            if ((long) inputLength + output.length > options.memoryLimit) {
                throw new IllegalArgumentException("input plus output buffer exceeds stream memory limit");
            }
            long[] nativeResult = streamCall(handle, encoder, operation, input, output);
            if (nativeResult.length != 3 || nativeResult[0] < 0 || nativeResult[0] > inputLength
                    || nativeResult[1] < 0 || nativeResult[1] > output.length) {
                throw new NativeException("native stream returned invalid consumed or produced counts");
            }
            if (operation != 0 && nativeResult[0] != 0) throw new NativeException("native flush/finish consumed input");
            State state = switch ((int) nativeResult[2]) {
                case 1 -> State.NEEDS_INPUT;
                case 2 -> State.NEEDS_OUTPUT;
                case 3 -> State.FINISHED;
                default -> throw new NativeException("native stream returned an unknown state");
            };
            totalProduced += nativeResult[1];
            byte[] produced = new byte[(int) nativeResult[1]];
            System.arraycopy(output, 0, produced, 0, produced.length);
            return new ProgressAndBytes(new Progress((int) nativeResult[0], (int) nativeResult[1], state), produced);
        }
        private void requireOpen(boolean allowCancelled) {
            if (handle == 0) throw new IllegalStateException("stream is closed");
            if (cancelled && !allowCancelled) throw new CancelledException();
        }
        @Override public synchronized void close() { if (handle != 0) { destroyStream(handle, encoder); handle = 0; } }
    }

    public static final class Encoder extends Stream { private Encoder(Options options) { super(options, true); } }
    public static final class Decoder extends Stream { private Decoder(Options options) { super(options, false); } }
    public record ProgressAndBytes(Progress progress, byte[] bytes) {}

    private static native long createContext(int profile, int workers, long outputLimit, long memoryLimit);
    private static native void destroyContext(long handle);
    private static native byte[] bufferCall(long handle, byte[] input, boolean encode, long outputLimit, long memoryLimit);
    private static native long createStream(boolean encoder, int profile, int workers, long outputLimit, long memoryLimit);
    private static native void destroyStream(long handle, boolean encoder);
    private static native void resetStream(long handle, boolean encoder);
    private static native long[] streamCall(long handle, boolean encoder, int operation, byte[] input, byte[] output);
}
