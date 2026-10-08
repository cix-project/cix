// SPDX-License-Identifier: MIT
/* Installed-plugin contract: appsrc ! cixstream ! appsink, with no private
 * CIX or plugin headers.  Every terminal wait is bounded. */
#include <gst/app/gstappsink.h>
#include <gst/app/gstappsrc.h>
#include <gst/gst.h>
#include <stdint.h>
#include <string.h>

typedef struct {
  guint8 *data;
  gsize size, capacity;
} Bytes;
typedef enum { TERMINAL_EOS, TERMINAL_ERROR, TERMINAL_TIMEOUT } Terminal;

static void diagnostic(const gchar *test_case, const gchar *detail) {
  g_printerr("cix-gstream-contract[%s]: %s\n", test_case, detail);
}

static gboolean append(Bytes *b, const guint8 *part, gsize length) {
  guint8 *grown;
  gsize next;
  if (length > G_MAXSIZE - b->size)
    return FALSE;
  if (b->size + length > b->capacity) {
    next = b->capacity ? b->capacity : 128;
    while (next < b->size + length) {
      if (next > G_MAXSIZE / 2) {
        next = b->size + length;
        break;
      }
      next *= 2;
    }
    grown = g_realloc(b->data, next);
    if (!grown)
      return FALSE;
    b->data = grown;
    b->capacity = next;
  }
  memcpy(b->data + b->size, part, length);
  b->size += length;
  return TRUE;
}

static Terminal drain(GstElement *pipeline, GstAppSink *sink, Bytes *output,
                      GstObject *expected_error_source,
                      const gchar *test_case) {
  GstBus *bus = gst_element_get_bus(pipeline);
  for (guint attempt = 0; attempt != 100; ++attempt) {
    GstSample *sample = gst_app_sink_try_pull_sample(sink, 20 * GST_MSECOND);
    GstMessage *message = gst_bus_pop_filtered(bus, GST_MESSAGE_ERROR);
    if (message) {
      GError *error = NULL;
      gboolean expected = TRUE;
      gst_message_parse_error(message, &error, NULL);
      g_printerr("cix-gstream-contract[%s]: bus error from %s: %s\n",
                 test_case, GST_OBJECT_NAME(GST_MESSAGE_SRC(message)),
                 error ? error->message : "unknown error");
      if (expected_error_source)
        expected = GST_MESSAGE_SRC(message) == expected_error_source && error &&
                   error->domain == GST_STREAM_ERROR &&
                   error->code == GST_STREAM_ERROR_FAILED;
      g_clear_error(&error);
      gst_message_unref(message);
      gst_object_unref(bus);
      return expected ? TERMINAL_ERROR : TERMINAL_TIMEOUT;
    }
    if (sample) {
      GstBuffer *buffer = gst_sample_get_buffer(sample);
      GstMapInfo map;
      if (!gst_buffer_map(buffer, &map, GST_MAP_READ)) {
        diagnostic(test_case, "could not map appsink output");
        gst_sample_unref(sample);
        gst_object_unref(bus);
        return TERMINAL_ERROR;
      }
      if (!append(output, map.data, map.size)) {
        diagnostic(test_case, "could not retain appsink output");
        gst_buffer_unmap(buffer, &map);
        gst_sample_unref(sample);
        gst_object_unref(bus);
        return TERMINAL_ERROR;
      }
      gst_buffer_unmap(buffer, &map);
      gst_sample_unref(sample);
      continue;
    }
    if (gst_app_sink_is_eos(sink)) {
      gst_object_unref(bus);
      return TERMINAL_EOS;
    }
  }
  g_printerr("cix-gstream-contract[%s]: terminal timeout after 2 seconds "
             "(no appsink EOS or bus error)\n", test_case);
  gst_object_unref(bus);
  return TERMINAL_TIMEOUT;
}

/* gst_parse_launch creates cixstream and applies its construct-only options
 * during construction. g_object_set after gst_element_factory_make cannot do
 * that for G_PARAM_CONSTRUCT_ONLY properties. */
static GstElement *new_pipeline(gboolean encoder, GstElement **source_out,
                                GstElement **codec_out, GstElement **sink_out) {
  GError *error = NULL;
  gchar *description = g_strdup_printf(
      "appsrc name=source format=bytes ! cixstream name=codec encoder=%s "
      "max-input=64 "
      "max-output-buffer=1024 output-limit=1048576 memory-limit=134217728 ! "
      "appsink name=sink sync=false",
      encoder ? "true" : "false");
  GstElement *pipeline = gst_parse_launch(description, &error);
  g_free(description);
  if (!pipeline || error) {
    if (error)
      g_printerr("cix-gstream-contract[pipeline setup]: %s\n", error->message);
    if (error)
      g_error_free(error);
    if (pipeline)
      gst_object_unref(pipeline);
    return NULL;
  }
  *source_out = gst_bin_get_by_name(GST_BIN(pipeline), "source");
  *codec_out = gst_bin_get_by_name(GST_BIN(pipeline), "codec");
  *sink_out = gst_bin_get_by_name(GST_BIN(pipeline), "sink");
  if (!*source_out || !*codec_out || !*sink_out) {
    if (*source_out)
      gst_object_unref(*source_out);
    if (*codec_out)
      gst_object_unref(*codec_out);
    if (*sink_out)
      gst_object_unref(*sink_out);
    gst_object_unref(pipeline);
    return NULL;
  }
  {
    gboolean actual_encoder;
    guint64 max_input, max_output, output_limit, memory_limit;
    g_object_get(*codec_out, "encoder", &actual_encoder, "max-input",
                 &max_input, "max-output-buffer", &max_output, "output-limit",
                 &output_limit, "memory-limit", &memory_limit, NULL);
    if (actual_encoder != encoder || max_input != 64 || max_output != 1024 ||
        output_limit != 1048576 || memory_limit != 134217728) {
      gst_object_unref(*source_out);
      gst_object_unref(*codec_out);
      gst_object_unref(*sink_out);
      gst_object_unref(pipeline);
      return NULL;
    }
  }
  return pipeline;
}

static gboolean feed(GstAppSrc *source, const guint8 *input, gsize length,
                     gsize piece) {
  for (gsize offset = 0; offset < length;) {
    gsize n = MIN(piece, length - offset);
    GstBuffer *buffer = gst_buffer_new_allocate(NULL, n, NULL);
    GstMapInfo map;
    if (!buffer || !gst_buffer_map(buffer, &map, GST_MAP_WRITE)) {
      if (buffer)
        gst_buffer_unref(buffer);
      return FALSE;
    }
    memcpy(map.data, input + offset, n);
    gst_buffer_unmap(buffer, &map);
    if (gst_app_src_push_buffer(source, buffer) != GST_FLOW_OK)
      return FALSE;
    offset += n;
  }
  return gst_app_src_end_of_stream(source) == GST_FLOW_OK;
}

static gboolean transform(gboolean encoder, const guint8 *input, gsize length,
                          gsize piece, Bytes *output, gboolean expect_error) {
  GstElement *source = NULL, *codec = NULL, *sink = NULL,
             *pipeline = new_pipeline(encoder, &source, &codec, &sink);
  gboolean ok = FALSE;
  const gchar *test_case = expect_error ? "expected-error transform" :
                                           (encoder ? "encode transform" : "decode transform");
  if (!pipeline) {
    diagnostic(test_case, "pipeline creation or construct-property readback failed");
    goto done;
  }
  if (gst_element_set_state(pipeline, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE) {
    diagnostic(test_case, "pipeline failed to enter PLAYING");
    goto done;
  }
  if (!feed(GST_APP_SRC(source), input, length, piece)) {
    diagnostic(test_case, "appsrc rejected input or EOS");
    goto done;
  }
  ok = drain(pipeline, GST_APP_SINK(sink), output,
             expect_error ? GST_OBJECT(codec) : NULL, test_case) ==
       (expect_error ? TERMINAL_ERROR : TERMINAL_EOS);
  if (!ok)
    diagnostic(test_case, "unexpected terminal result");
done:
  if (source)
    gst_object_unref(source);
  if (codec)
    gst_object_unref(codec);
  if (sink)
    gst_object_unref(sink);
  if (pipeline) {
    gst_element_set_state(pipeline, GST_STATE_NULL);
    gst_object_unref(pipeline);
  }
  return ok;
}

static gboolean flush_reset(void) {
  static const guint8 fresh[] = {'r', 'e', 's', 'e', 't'};
  GstElement *source = NULL, *codec = NULL, *sink = NULL,
             *pipeline = new_pipeline(TRUE, &source, &codec, &sink);
  GstPad *src_pad = NULL;
  GstSegment segment;
  Bytes output = {0}, decoded = {0};
  gboolean ok = FALSE;
  if (!pipeline || gst_element_set_state(pipeline, GST_STATE_PLAYING) ==
                       GST_STATE_CHANGE_FAILURE) {
    diagnostic("flush reset", "pipeline creation or PLAYING transition failed");
    goto done;
  }
  src_pad = gst_element_get_static_pad(source, "src");
  /* These downstream events traverse the linked, running cixstream pads. */
  if (!src_pad || !gst_pad_push_event(src_pad, gst_event_new_flush_start()) ||
      !gst_pad_push_event(src_pad, gst_event_new_flush_stop(TRUE))) {
    diagnostic("flush reset", "flush event was rejected");
    goto done;
  }
  gst_segment_init(&segment, GST_FORMAT_BYTES);
  if (!gst_pad_push_event(src_pad, gst_event_new_segment(&segment))) {
    diagnostic("flush reset", "post-flush segment was rejected");
    goto done;
  }
  if (!feed(GST_APP_SRC(source), fresh, sizeof(fresh), 2)) {
    diagnostic("flush reset", "appsrc rejected fresh data or EOS");
    goto done;
  }
  ok = drain(pipeline, GST_APP_SINK(sink), &output, NULL, "flush reset") == TERMINAL_EOS &&
       output.size != 0 &&
       transform(FALSE, output.data, output.size, 2, &decoded, FALSE) &&
       decoded.size == sizeof(fresh) &&
       !memcmp(decoded.data, fresh, sizeof(fresh));
  if (!ok)
    diagnostic("flush reset", "fresh stream did not round-trip exactly");
done:
  g_free(output.data);
  g_free(decoded.data);
  if (src_pad)
    gst_object_unref(src_pad);
  if (source)
    gst_object_unref(source);
  if (codec)
    gst_object_unref(codec);
  if (sink)
    gst_object_unref(sink);
  if (pipeline) {
    gst_element_set_state(pipeline, GST_STATE_NULL);
    gst_object_unref(pipeline);
  }
  return ok;
}

int main(int argc, char **argv) {
  guint8 source[4099];
  Bytes archive = {0}, restored = {0}, ignored = {0};
  gboolean ok;
  GstPlugin *plugin;
  if (argc != 2)
    return 2;
  g_setenv("GST_PLUGIN_PATH", argv[1], TRUE);
  gst_init(&argc, &argv);
  plugin = gst_registry_find_plugin(gst_registry_get(), "cixstream");
  if (!plugin) {
    diagnostic("plugin discovery", "cixstream was not found in GST_PLUGIN_PATH");
    return 1;
  }
  gst_object_unref(plugin);
  for (gsize i = 0; i < sizeof(source); ++i)
    source[i] = (guint8)((i * 31u) ^ (i >> 3));
  ok = transform(TRUE, source, sizeof(source), 13, &archive, FALSE);
  if (!ok || archive.size == 0) {
    diagnostic("fragmented encode", "no usable archive was produced");
    ok = FALSE;
  }
  if (ok) {
    ok = transform(FALSE, archive.data, archive.size, 11, &restored, FALSE) &&
         restored.size == sizeof(source) &&
         !memcmp(restored.data, source, sizeof(source));
    if (!ok)
      diagnostic("fragmented decode", "decoded bytes do not exactly match source");
  }
  if (ok) {
    Bytes empty_archive = {0}, empty = {0};
    ok = transform(TRUE, NULL, 0, 1, &empty_archive, FALSE) &&
         transform(FALSE, empty_archive.data, empty_archive.size, 1, &empty,
                   FALSE) &&
         empty.size == 0;
    g_free(empty_archive.data);
    g_free(empty.data);
    if (!ok)
      diagnostic("empty stream", "empty encode/decode did not terminate exactly");
  }
  if (ok && archive.size > 1) {
    ok = transform(FALSE, archive.data, archive.size - 1, 7, &ignored, TRUE);
    if (!ok)
      diagnostic("truncated archive", "expected CIX terminal error was not observed");
  }
  if (ok) {
    guint8 *tail = g_malloc(archive.size + 1);
    memcpy(tail, archive.data, archive.size);
    tail[archive.size] = 0xa5;
    ok = transform(FALSE, tail, archive.size + 1, 11, &ignored, TRUE);
    g_free(tail);
    if (!ok)
      diagnostic("trailing archive byte", "expected post-FINISHED CIX error was not observed");
  }
  if (ok) {
    ok = flush_reset();
    if (!ok)
      diagnostic("flush reset", "post-reset exact round-trip failed");
  }
  g_free(archive.data);
  g_free(restored.data);
  g_free(ignored.data);
  return ok ? 0 : 1;
}
