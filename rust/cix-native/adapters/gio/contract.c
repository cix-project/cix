// SPDX-License-Identifier: MIT
#include "cix-gconverter.h"
#include <string.h>

static cix_options_v1 options(void)
{
    cix_options_v1 value;
    g_assert_cmpint(cix_options_v1_default(&value), ==, CIX_STATUS_OK);
    value.memory_limit = 64U << 20;
    value.output_limit = 1U << 20;
    value.workers = 1;
    return value;
}

static CixGConverter *converter(gboolean encoder)
{
    cix_options_v1 value = options();
    GError *error = NULL;
    CixGConverter *result = cix_gconverter_new(encoder, &value, &error);
    g_assert_no_error(error);
    g_assert_nonnull(result);
    return result;
}

static GBytes *encode_stream(const guint8 *source, gsize length)
{
    CixGConverter *codec = converter(TRUE);
    GError *error = NULL;
    GOutputStream *memory = g_memory_output_stream_new_resizable();
    GOutputStream *stream = g_converter_output_stream_new(memory, G_CONVERTER(codec));
    gsize split = length / 2;
    g_assert_true(g_output_stream_write_all(stream, source, split, NULL, NULL, &error));
    g_assert_no_error(error);
    g_assert_true(g_output_stream_flush(stream, NULL, &error));
    g_assert_no_error(error);
    g_assert_true(g_output_stream_write_all(stream, source + split, length - split, NULL, NULL, &error));
    g_assert_no_error(error);
    g_assert_true(g_output_stream_close(stream, NULL, &error));
    g_assert_no_error(error);
    GBytes *archive = g_memory_output_stream_steal_as_bytes(G_MEMORY_OUTPUT_STREAM(memory));
    g_object_unref(stream);
    g_object_unref(memory);
    g_object_unref(codec);
    return archive;
}

static gboolean decode_stream(GBytes *archive, GByteArray *restored, GError **error)
{
    CixGConverter *codec = converter(FALSE);
    gsize length;
    const guint8 *data = g_bytes_get_data(archive, &length);
    GInputStream *memory = g_memory_input_stream_new_from_data(data, length, NULL);
    GInputStream *stream = g_converter_input_stream_new(memory, G_CONVERTER(codec));
    guint8 buffer[19];
    gssize count;
    while ((count = g_input_stream_read(stream, buffer, sizeof(buffer), NULL, error)) > 0)
        g_byte_array_append(restored, buffer, (guint)count);
    gboolean ok = count == 0 && g_input_stream_close(stream, NULL, error);
    g_object_unref(stream);
    g_object_unref(memory);
    g_object_unref(codec);
    return ok;
}

static void round_trip(void)
{
    guint8 source[8193];
    for (gsize i = 0; i < sizeof(source); ++i)
        source[i] = (guint8)(i * 19U + i / 7U);
    for (guint empty = 0; empty < 2; ++empty) {
        gsize size = empty ? 0 : sizeof(source);
        GBytes *archive = encode_stream(source, size);
        GByteArray *restored = g_byte_array_new();
        GError *error = NULL;
        g_assert_true(decode_stream(archive, restored, &error));
        g_assert_no_error(error);
        g_assert_cmpmem(restored->data, restored->len, source, size);
        g_byte_array_unref(restored);
        g_bytes_unref(archive);
    }
}

static void drive(CixGConverter *codec, const guint8 *input, gsize size,
                  GConverterFlags flags, GConverterResult terminal, GByteArray *output)
{
    gsize offset = 0;
    for (guint iteration = 0; iteration < 100000; ++iteration) {
        guint8 byte;
        gsize used, made;
        GError *error = NULL;
        GConverterResult result = g_converter_convert(G_CONVERTER(codec), input + offset,
            size - offset, &byte, 1, flags, &used, &made, &error);
        g_assert_no_error(error);
        g_assert_cmpuint(used, <=, size - offset);
        g_assert_cmpuint(made, <=, 1);
        offset += used;
        if (made != 0)
            g_byte_array_append(output, &byte, 1);
        if (result == terminal) {
            g_assert_cmpuint(offset, ==, size);
            return;
        }
        g_assert_cmpint(result, ==, G_CONVERTER_CONVERTED);
        g_assert_true(used != 0 || made != 0);
    }
    g_error("converter failed to finish within the bounded progress loop");
}

static void backpressure_and_reset(void)
{
    static const guint8 source[] = "native stream with tiny output and nonempty end/flush";
    CixGConverter *codec = converter(TRUE);
    for (guint repeat = 0; repeat < 2; ++repeat) {
        GByteArray *bytes = g_byte_array_new();
        drive(codec, source, 13, G_CONVERTER_FLUSH, G_CONVERTER_FLUSHED, bytes);
        drive(codec, source + 13, sizeof(source) - 13, G_CONVERTER_INPUT_AT_END,
              G_CONVERTER_FINISHED, bytes);
        GBytes *archive = g_byte_array_free_to_bytes(bytes);
        GByteArray *restored = g_byte_array_new();
        GError *error = NULL;
        g_assert_true(decode_stream(archive, restored, &error));
        g_assert_no_error(error);
        g_assert_cmpmem(restored->data, restored->len, source, sizeof(source));
        g_byte_array_unref(restored);
        g_bytes_unref(archive);
        g_converter_reset(G_CONVERTER(codec));
    }
    g_object_unref(codec);
}

static void truncated_input(void)
{
    static const guint8 source[] = "truncate this exact framed stream";
    GBytes *archive = encode_stream(source, sizeof(source));
    gsize size;
    const guint8 *data = g_bytes_get_data(archive, &size);
    GBytes *cut = g_bytes_new(data, size - 1);
    GByteArray *restored = g_byte_array_new();
    GError *error = NULL;
    g_assert_false(decode_stream(cut, restored, &error));
    g_assert_nonnull(error);
    g_clear_error(&error);
    g_byte_array_unref(restored);
    g_bytes_unref(cut);
    g_bytes_unref(archive);
}

static void cancelled_output(void)
{
    CixGConverter *codec = converter(TRUE);
    guint8 output[64];
    gsize used, made;
    GError *error = NULL;
    cix_gconverter_cancel(codec);
    GConverterResult result = g_converter_convert(G_CONVERTER(codec), "x", 1,
        output, sizeof(output), G_CONVERTER_NO_FLAGS, &used, &made, &error);
    g_assert_cmpint(result, ==, G_CONVERTER_ERROR);
    g_assert_error(error, G_IO_ERROR, G_IO_ERROR_CANCELLED);
    g_assert_cmpuint(used, ==, 0);
    g_assert_cmpuint(made, ==, 0);
    g_clear_error(&error);
    g_converter_reset(G_CONVERTER(codec));
    result = g_converter_convert(G_CONVERTER(codec), "x", 1, output, sizeof(output),
        G_CONVERTER_NO_FLAGS, &used, &made, &error);
    g_assert_no_error(error);
    g_assert_cmpint(result, ==, G_CONVERTER_CONVERTED);
    g_object_unref(codec);
}

int main(int argc, char **argv)
{
    g_test_init(&argc, &argv, NULL);
    g_test_add_func("/cix-gio/round-trip", round_trip);
    g_test_add_func("/cix-gio/backpressure-reset", backpressure_and_reset);
    g_test_add_func("/cix-gio/truncated", truncated_input);
    g_test_add_func("/cix-gio/cancelled-output", cancelled_output);
    return g_test_run();
}
