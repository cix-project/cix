package dev.cix.hadoop;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.Arrays;
import org.apache.hadoop.conf.Configuration;
import org.apache.hadoop.io.compress.CompressionInputStream;
import org.apache.hadoop.io.compress.CompressionOutputStream;

/** Scheduler-only host contract; it is intentionally not a Maven/JUnit default test. */
public final class HadoopContract {
    private HadoopContract() {}

    public static void main(String[] args) throws Exception {
        String nativeLibrary = requireEnvironment("CIX_HADOOP_NATIVE_LIBRARY");
        String jniLibrary = requireEnvironment("CIX_HADOOP_JNI_LIBRARY");
        CixCodec codec = new CixCodec();
        Configuration configuration = new Configuration(false);
        configuration.set(CixCodec.NATIVE_LIBRARY, nativeLibrary);
        configuration.set(CixCodec.JNI_LIBRARY, jniLibrary);
        configuration.setLong(CixCodec.MEMORY_LIMIT, 128L << 20);
        configuration.setLong(CixCodec.OUTPUT_LIMIT, 8L << 20);
        codec.setConf(configuration);

        byte[] source = "CIX Hadoop sequential stream ".repeat(8192).getBytes(StandardCharsets.UTF_8);
        byte[] archive = encode(codec, source);
        require(Arrays.equals(source, decode(codec, archive)), "sequential round trip");

        try (CompressionOutputStream resettable = codec.createOutputStream(new ByteArrayOutputStream())) {
            resettable.write(source, 0, 257);
            resettable.finish();
            resettable.resetState();
        }

        CixCodec.CixCompressor compressor = (CixCodec.CixCompressor) codec.createCompressor();
        compressor.setInput(source, 0, Math.min(source.length, 1024));
        compressor.reset();
        compressor.end();

        byte[] truncated = Arrays.copyOf(archive, Math.max(0, archive.length - 1));
        try { decode(codec, truncated); throw new AssertionError("truncated stream was accepted"); }
        catch (IOException expected) { }
    }

    private static byte[] encode(CixCodec codec, byte[] source) throws IOException {
        ByteArrayOutputStream destination = new ByteArrayOutputStream();
        try (CompressionOutputStream stream = codec.createOutputStream(destination)) {
            boolean flushed = false;
            for (int offset = 0; offset < source.length;) {
                int length = Math.min(131, source.length - offset);
                stream.write(source, offset, length);
                offset += length;
                if (!flushed && offset >= source.length / 2) {
                    stream.flush();
                    flushed = true;
                }
            }
            stream.finish();
        }
        return destination.toByteArray();
    }

    private static byte[] decode(CixCodec codec, byte[] archive) throws IOException {
        ByteArrayOutputStream restored = new ByteArrayOutputStream();
        try (CompressionInputStream stream = codec.createInputStream(new ByteArrayInputStream(archive))) {
            byte[] buffer = new byte[257];
            for (int count; (count = stream.read(buffer)) >= 0;) restored.write(buffer, 0, count);
        }
        return restored.toByteArray();
    }

    private static String requireEnvironment(String name) {
        String value = System.getenv(name);
        if (value == null || value.isBlank() || !Path.of(value).isAbsolute()) {
            throw new IllegalStateException(name + " must name an absolute qualified library");
        }
        return value;
    }
    private static void require(boolean value, String label) { if (!value) throw new AssertionError(label); }
}
