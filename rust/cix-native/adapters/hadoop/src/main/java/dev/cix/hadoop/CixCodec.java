package dev.cix.hadoop;

import dev.cix.binding.NativeCix;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.Objects;
import org.apache.hadoop.conf.Configurable;
import org.apache.hadoop.conf.Configuration;
import org.apache.hadoop.io.compress.CompressionCodec;
import org.apache.hadoop.io.compress.CompressionInputStream;
import org.apache.hadoop.io.compress.CompressionOutputStream;
import org.apache.hadoop.io.compress.Compressor;
import org.apache.hadoop.io.compress.Decompressor;

/** Experimental sequential Hadoop codec backed by the public CIXG1 JNI stream ABI. */
public final class CixCodec implements CompressionCodec, Configurable {
    public static final String NATIVE_LIBRARY = "cix.hadoop.native.library";
    public static final String JNI_LIBRARY = "cix.hadoop.jni.library";
    public static final String PROFILE = "cix.hadoop.profile";
    public static final String WORKERS = "cix.hadoop.workers";
    public static final String OUTPUT_LIMIT = "cix.hadoop.output.limit";
    public static final String MEMORY_LIMIT = "cix.hadoop.memory.limit";

    private static final int DEFAULT_OUTPUT_LIMIT = 64 << 20;
    private static final int DEFAULT_MEMORY_LIMIT = 512 << 20;
    private static final int MAX_CHUNK = 64 << 10;
    private Configuration configuration = new Configuration(false);

    @Override public void setConf(Configuration value) {
        configuration = value == null ? new Configuration(false) : new Configuration(value);
    }
    @Override public Configuration getConf() { return new Configuration(configuration); }

    @Override public CompressionOutputStream createOutputStream(OutputStream out) throws IOException {
        return createOutputStream(out, createCompressor());
    }
    @Override public CompressionOutputStream createOutputStream(OutputStream out, Compressor compressor) throws IOException {
        if (!(compressor instanceof CixCompressor cix)) {
            throw new IOException("CIX Hadoop codec requires its own CixCompressor");
        }
        return new CixOutputStream(out, cix);
    }
    @Override public CompressionInputStream createInputStream(InputStream in) throws IOException {
        return createInputStream(in, createDecompressor());
    }
    @Override public CompressionInputStream createInputStream(InputStream in, Decompressor decompressor) throws IOException {
        if (!(decompressor instanceof CixDecompressor cix)) {
            throw new IOException("CIX Hadoop codec requires its own CixDecompressor");
        }
        return new CixInputStream(in, cix);
    }
    @Override public Class<? extends Compressor> getCompressorType() { return CixCompressor.class; }
    @Override public Compressor createCompressor() { return new CixCompressor(options()); }
    @Override public Class<? extends Decompressor> getDecompressorType() { return CixDecompressor.class; }
    @Override public Decompressor createDecompressor() { return new CixDecompressor(options()); }
    @Override public String getDefaultExtension() { return ".cixg"; }

    private NativeCix.Options options() {
        String nativePath = requireAbsolute(configuration.get(NATIVE_LIBRARY), NATIVE_LIBRARY);
        String jniPath = requireAbsolute(configuration.get(JNI_LIBRARY), JNI_LIBRARY);
        NativeCix.load(Path.of(nativePath), Path.of(jniPath));
        int profile = configuration.getInt(PROFILE, NativeCix.DEFAULT);
        int workers = configuration.getInt(WORKERS, 1);
        long output = configuration.getLong(OUTPUT_LIMIT, DEFAULT_OUTPUT_LIMIT);
        long memory = configuration.getLong(MEMORY_LIMIT, DEFAULT_MEMORY_LIMIT);
        return new NativeCix.Options(profile, workers, output, memory);
    }

    private static String requireAbsolute(String value, String key) {
        if (value == null || value.isBlank() || !Path.of(value).isAbsolute()) {
            throw new IllegalArgumentException(key + " must name an explicitly configured absolute library path");
        }
        return value;
    }

    private abstract static class CixStreamBase {
        final NativeCix.Options options;
        final int inputChunk;
        final int outputChunk;
        long read;
        long written;
        boolean ended;

        CixStreamBase(NativeCix.Options options) {
            this.options = Objects.requireNonNull(options, "options");
            long memory = options.memoryLimit();
            if (memory < 4) throw new IllegalArgumentException("CIX Hadoop memory limit must be at least four bytes");
            inputChunk = (int) Math.min(MAX_CHUNK, Math.max(1, memory / 4));
            outputChunk = inputChunk;
        }
        final void checkRange(byte[] data, int offset, int length) {
            Objects.requireNonNull(data, "data");
            if (offset < 0 || length < 0 || offset > data.length - length) {
                throw new IndexOutOfBoundsException("invalid byte range");
            }
        }
        final IOException nativeFailure(RuntimeException error) {
            return new IOException("CIX native stream operation failed", error);
        }
        final void ensureOpen() throws IOException {
            if (ended) throw new IOException("CIX Hadoop stream is closed");
        }
    }

    /** Real Hadoop Compressor state machine over one CIXG1 encoder. */
    public static final class CixCompressor extends CixStreamBase implements Compressor {
        private NativeCix.Encoder encoder;
        private byte[] pending = new byte[0];
        private int offset;
        private boolean finishing;
        private boolean finished;

        CixCompressor(NativeCix.Options options) { super(options); encoder = NativeCix.encoder(options); }
        @Override public synchronized void setInput(byte[] data, int start, int length) {
            checkRange(data, start, length);
            if (!needsInput()) throw new IllegalStateException("CIX compressor input has not been consumed");
            if (length > inputChunk) throw new IllegalArgumentException("CIX Hadoop input exceeds bounded chunk size");
            pending = Arrays.copyOfRange(data, start, start + length);
            offset = 0;
        }
        @Override public synchronized boolean needsInput() { return offset == pending.length && !finishing; }
        @Override public void setDictionary(byte[] b, int off, int len) { throw new UnsupportedOperationException("CIXG1 has no dictionary input"); }
        @Override public synchronized void finish() { finishing = true; }
        @Override public synchronized boolean finished() { return finished; }
        @Override public synchronized int compress(byte[] output, int start, int length) {
            checkRange(output, start, length);
            if (ended) throw new IllegalStateException("CIX compressor is closed");
            try {
                NativeCix.ProgressAndBytes step;
                if (offset < pending.length) {
                    byte[] input = Arrays.copyOfRange(pending, offset, pending.length);
                    step = encoder.process(input, new byte[length]);
                    offset += step.progress().consumed();
                    read += step.progress().consumed();
                } else if (finishing) {
                    step = encoder.finish(new byte[length]);
                    finished = step.progress().state() == NativeCix.State.FINISHED;
                } else {
                    return 0;
                }
                int produced = step.progress().produced();
                if (produced > length || produced != step.bytes().length) throw new IllegalStateException("invalid native output count");
                System.arraycopy(step.bytes(), 0, output, start, produced);
                written += produced;
                if (produced == 0 && !finished && offset < pending.length) {
                    throw new IllegalStateException("CIX encoder made no progress");
                }
                return produced;
            } catch (RuntimeException error) {
                throw new IllegalStateException("CIX native compressor failure", error);
            }
        }
        synchronized NativeCix.ProgressAndBytes flushNative(byte[] output) {
            checkRange(output, 0, output.length);
            if (ended) throw new IllegalStateException("CIX compressor is closed");
            try {
                NativeCix.ProgressAndBytes step = encoder.flush(output);
                if (step.progress().consumed() != 0 || step.progress().produced() > output.length
                        || step.progress().produced() != step.bytes().length) {
                    throw new IllegalStateException("invalid native flush result");
                }
                System.arraycopy(step.bytes(), 0, output, 0, step.bytes().length);
                written += step.bytes().length;
                return step;
            } catch (RuntimeException error) {
                throw new IllegalStateException("CIX native encoder flush failure", error);
            }
        }
        @Override public synchronized void reset() {
            if (ended) throw new IllegalStateException("CIX compressor is closed");
            encoder.reset(); pending = new byte[0]; offset = 0; finishing = false; finished = false; read = 0; written = 0;
        }
        @Override public synchronized void end() { if (!ended) { encoder.close(); ended = true; } }
        @Override public synchronized void reinit(Configuration ignored) { reset(); }
        @Override public synchronized long getBytesRead() { return read; }
        @Override public synchronized long getBytesWritten() { return written; }
    }

    /** Real Hadoop Decompressor state machine over one CIXG1 decoder. */
    public static final class CixDecompressor extends CixStreamBase implements Decompressor {
        private NativeCix.Decoder decoder;
        private byte[] pending = new byte[0];
        private int offset;
        private boolean inputFinished;
        private boolean finished;

        CixDecompressor(NativeCix.Options options) { super(options); decoder = NativeCix.decoder(options); }
        @Override public synchronized void setInput(byte[] data, int start, int length) {
            checkRange(data, start, length);
            if (!needsInput()) throw new IllegalStateException("CIX decompressor input has not been consumed");
            if (length > inputChunk) throw new IllegalArgumentException("CIX Hadoop input exceeds bounded chunk size");
            pending = Arrays.copyOfRange(data, start, start + length); offset = 0;
        }
        @Override public synchronized boolean needsInput() { return offset == pending.length && !inputFinished; }
        @Override public void setDictionary(byte[] b, int off, int len) { throw new UnsupportedOperationException("CIXG1 has no dictionary input"); }
        @Override public boolean needsDictionary() { return false; }
        @Override public synchronized boolean finished() { return finished; }
        @Override public synchronized int decompress(byte[] output, int start, int length) {
            checkRange(output, start, length);
            if (ended) throw new IllegalStateException("CIX decompressor is closed");
            try {
                NativeCix.ProgressAndBytes step;
                if (offset < pending.length) {
                    byte[] input = Arrays.copyOfRange(pending, offset, pending.length);
                    step = decoder.process(input, new byte[length]);
                    offset += step.progress().consumed(); read += step.progress().consumed();
                } else if (inputFinished) {
                    step = decoder.finish(new byte[length]);
                } else {
                    return 0;
                }
                int produced = step.progress().produced();
                if (produced > length || produced != step.bytes().length) throw new IllegalStateException("invalid native output count");
                System.arraycopy(step.bytes(), 0, output, start, produced);
                written += produced;
                finished = step.progress().state() == NativeCix.State.FINISHED;
                return produced;
            } catch (RuntimeException error) { throw new IllegalStateException("CIX native decompressor failure", error); }
        }
        synchronized void finishInput() { inputFinished = true; }
        @Override public synchronized int getRemaining() { return pending.length - offset; }
        @Override public synchronized void reset() {
            if (ended) throw new IllegalStateException("CIX decompressor is closed");
            decoder.reset(); pending = new byte[0]; offset = 0; inputFinished = false; finished = false; read = 0; written = 0;
        }
        @Override public synchronized void end() { if (!ended) { decoder.close(); ended = true; } }
    }

    private static final class CixOutputStream extends CompressionOutputStream {
        private final CixCompressor compressor;
        private boolean finished;
        private boolean closed;
        CixOutputStream(OutputStream out, CixCompressor compressor) { super(out); this.compressor = compressor; }
        @Override public void write(int value) throws IOException { write(new byte[] {(byte) value}, 0, 1); }
        @Override public void write(byte[] data, int offset, int length) throws IOException {
            compressor.checkRange(data, offset, length); if (closed || finished) throw new IOException("stream is finished or closed");
            while (length > 0) {
                int part = Math.min(length, compressor.inputChunk);
                compressor.setInput(data, offset, part);
                drainInput(); offset += part; length -= part;
            }
        }
        private void drainInput() throws IOException {
            while (!compressor.needsInput()) {
                int produced = callCompress();
                if (produced == 0 && !compressor.needsInput()) throw new IOException("CIX encoder backpressure made no progress");
            }
        }
        private int callCompress() throws IOException {
            byte[] output = new byte[compressor.outputChunk];
            try { int count = compressor.compress(output, 0, output.length); out.write(output, 0, count); return count; }
            catch (RuntimeException error) { throw compressor.nativeFailure(error); }
        }
        @Override public void finish() throws IOException {
            if (finished) return; drainInput(); compressor.finish();
            while (!compressor.finished()) {
                if (callCompress() == 0 && !compressor.finished()) throw new IOException("CIX encoder finish made no progress");
            }
            finished = true;
        }
        @Override public void flush() throws IOException {
            if (closed) throw new IOException("stream is closed");
            drainInput();
            while (true) {
                byte[] output = new byte[compressor.outputChunk];
                final NativeCix.ProgressAndBytes step;
                try { step = compressor.flushNative(output); }
                catch (RuntimeException error) { throw compressor.nativeFailure(error); }
                out.write(output, 0, step.bytes().length);
                if (step.progress().state() == NativeCix.State.NEEDS_OUTPUT && step.bytes().length == 0) {
                    throw new IOException("CIX encoder flush backpressure made no progress");
                }
                if (step.progress().state() != NativeCix.State.NEEDS_OUTPUT) break;
            }
            out.flush();
        }
        @Override public void resetState() throws IOException {
            if (closed) throw new IOException("stream is closed");
            if (!finished) throw new IOException("finish before resetState; discarding an unfinished CIXG1 stream is forbidden");
            compressor.reset(); finished = false;
        }
        @Override public void close() throws IOException {
            if (closed) return; IOException failure = null;
            try { finish(); } catch (IOException error) { failure = error; }
            try { compressor.end(); } catch (RuntimeException error) { if (failure == null) failure = new IOException("CIX compressor close failed", error); else failure.addSuppressed(error); }
            try { out.close(); } catch (IOException error) { if (failure == null) failure = error; else failure.addSuppressed(error); }
            closed = true; if (failure != null) throw failure;
        }
    }

    private static final class CixInputStream extends CompressionInputStream {
        private final CixDecompressor decompressor;
        private boolean sourceEof;
        private boolean closed;
        CixInputStream(InputStream in, CixDecompressor decompressor) throws IOException { super(in); this.decompressor = decompressor; }
        @Override public int read() throws IOException { byte[] one = new byte[1]; int count = read(one, 0, 1); return count < 0 ? -1 : one[0] & 255; }
        @Override public int read(byte[] output, int offset, int length) throws IOException {
            decompressor.checkRange(output, offset, length); if (closed) throw new IOException("stream is closed"); if (length == 0) return 0;
            while (true) {
                if (decompressor.needsInput() && !sourceEof) fillInput();
                if (sourceEof && decompressor.needsInput()) decompressor.finishInput();
                int capacity = Math.min(length, decompressor.outputChunk);
                byte[] temporary = new byte[capacity];
                final int produced;
                try { produced = decompressor.decompress(temporary, 0, temporary.length); }
                catch (RuntimeException error) { throw decompressor.nativeFailure(error); }
                if (produced > 0) { System.arraycopy(temporary, 0, output, offset, produced); return produced; }
                if (decompressor.finished()) return -1;
                if (sourceEof) throw new IOException("truncated or malformed CIXG1 stream");
                if (!decompressor.needsInput()) throw new IOException("CIX decoder backpressure made no progress");
            }
        }
        private void fillInput() throws IOException {
            byte[] input = new byte[decompressor.inputChunk]; int count = in.read(input);
            if (count < 0) { sourceEof = true; return; }
            if (count == 0) { int one = in.read(); if (one < 0) { sourceEof = true; return; } input[0] = (byte) one; count = 1; }
            decompressor.setInput(input, 0, count);
        }
        @Override public void resetState() throws IOException { if (closed) throw new IOException("stream is closed"); decompressor.reset(); sourceEof = false; }
        @Override public void close() throws IOException {
            if (closed) return;
            IOException failure = null;
            try { decompressor.end(); } catch (RuntimeException error) { failure = new IOException("CIX decompressor close failed", error); }
            try { in.close(); } catch (IOException error) { if (failure == null) failure = error; else failure.addSuppressed(error); }
            closed = true;
            if (failure != null) throw failure;
        }
    }
}
