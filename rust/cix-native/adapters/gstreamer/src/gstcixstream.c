// SPDX-License-Identifier: MIT
#include "cix_stream.h"
#include <gst/gst.h>
#include <string.h>

#define GST_TYPE_CIX_STREAM (gst_cix_stream_get_type())
G_DECLARE_FINAL_TYPE(GstCixStream, gst_cix_stream, GST, CIX_STREAM, GstElement)
typedef enum { CIX_ACTIVE, CIX_FINISHED, CIX_FAILED } CixState;
struct _GstCixStream {
  GstElement parent;
  GstPad *sink, *src;
  gboolean encoder;
  guint64 max_input, max_output, output_limit, memory_limit;
  gint cancelled;
  GMutex lock;
  guint generation;
  gboolean have_segment;
  CixState state;
  union {
    cix_stream_encoder *encoder;
    cix_stream_decoder *decoder;
  } handle;
};
G_DEFINE_TYPE(GstCixStream, gst_cix_stream, GST_TYPE_ELEMENT)
enum {
  PROP_0,
  PROP_ENCODER,
  PROP_MAX_INPUT,
  PROP_MAX_OUTPUT,
  PROP_OUTPUT_LIMIT,
  PROP_MEMORY_LIMIT
};
static GstStaticPadTemplate sink_template = GST_STATIC_PAD_TEMPLATE(
    "sink", GST_PAD_SINK, GST_PAD_ALWAYS, GST_STATIC_CAPS_ANY);
static GstStaticPadTemplate src_template = GST_STATIC_PAD_TEMPLATE(
    "src", GST_PAD_SRC, GST_PAD_ALWAYS, GST_STATIC_CAPS_ANY);

static void destroy_locked(GstCixStream *s) {
  if (s->encoder) {
    if (s->handle.encoder)
      cix_stream_encoder_destroy(s->handle.encoder);
    s->handle.encoder = NULL;
  } else {
    if (s->handle.decoder)
      cix_stream_decoder_destroy(s->handle.decoder);
    s->handle.decoder = NULL;
  }
}
/* SDK memory accounting excludes buffers supplied by this element. Reserve a
 * mapped input buffer and one output GstBuffer before creating the handle. */
static gboolean create_locked(GstCixStream *s) {
  cix_options_v1 o;
  cix_status status;
  guint64 reserve;
  if (s->max_input > G_MAXSIZE || s->max_output > G_MAXSIZE ||
      s->max_output > s->output_limit ||
      s->max_input > G_MAXUINT64 - s->max_output)
    return FALSE;
  reserve = s->max_input + s->max_output;
  if (reserve >= s->memory_limit || cix_options_v1_default(&o) != CIX_STATUS_OK)
    return FALSE;
  o.workers = 1;
  o.output_limit = s->output_limit;
  o.memory_limit = s->memory_limit - reserve;
  status = s->encoder ? cix_stream_encoder_create(&o, &s->handle.encoder)
                      : cix_stream_decoder_create(&o, &s->handle.decoder);
  return status == CIX_STATUS_OK;
}
static gboolean reset_locked(GstCixStream *s) {
  cix_status status;
  if ((s->encoder && !s->handle.encoder) || (!s->encoder && !s->handle.decoder))
    return create_locked(s);
  status = s->encoder ? cix_stream_encoder_reset(s->handle.encoder)
                      : cix_stream_decoder_reset(s->handle.decoder);
  return status == CIX_STATUS_OK;
}
static gboolean valid(cix_stream_result_v1 p, gsize in, gsize out) {
  return p.consumed <= in && p.produced <= out &&
         (p.state == CIX_STREAM_NEEDS_INPUT ||
          p.state == CIX_STREAM_NEEDS_OUTPUT || p.state == CIX_STREAM_FINISHED);
}
static void fail(GstCixStream *s, const gchar *where, cix_status status) {
  g_mutex_lock(&s->lock);
  s->state = CIX_FAILED;
  g_mutex_unlock(&s->lock);
  GST_ELEMENT_ERROR(s, STREAM, FAILED, ("CIX byte stream %s failed", where),
                    ("CIX status %d", (gint)status));
}
/* A flush can reset the handle after a native call returns but before its
 * result is inspected.  Never turn that new generation into a late failure. */
static gboolean fail_current(GstCixStream *s, guint generation,
                             const gchar *where, cix_status status) {
  gboolean current;
  g_mutex_lock(&s->lock);
  current = generation == s->generation;
  if (current)
    s->state = CIX_FAILED;
  g_mutex_unlock(&s->lock);
  if (current)
    GST_ELEMENT_ERROR(s, STREAM, FAILED, ("CIX byte stream %s failed", where),
                      ("CIX status %d", (gint)status));
  return current;
}
/* A native call is serialized, but downstream is never invoked under lock. */
static GstFlowReturn native_call(GstCixStream *s, guint generation,
                                 const guint8 *in, gsize n, gboolean finishing,
                                 GstBuffer **buffer_out,
                                 cix_stream_result_v1 *progress) {
  GstBuffer *buffer = gst_buffer_new_allocate(NULL, (gsize)s->max_output, NULL);
  GstMapInfo map;
  cix_status status;
  *buffer_out = NULL;
  memset(progress, 0, sizeof(*progress));
  if (!buffer || !gst_buffer_map(buffer, &map, GST_MAP_WRITE)) {
    if (buffer)
      gst_buffer_unref(buffer);
    return GST_FLOW_ERROR;
  }
  g_mutex_lock(&s->lock);
  if (generation != s->generation || g_atomic_int_get(&s->cancelled)) {
    g_mutex_unlock(&s->lock);
    gst_buffer_unmap(buffer, &map);
    gst_buffer_unref(buffer);
    return GST_FLOW_FLUSHING;
  }
  if (s->state != CIX_ACTIVE) {
    g_mutex_unlock(&s->lock);
    gst_buffer_unmap(buffer, &map);
    gst_buffer_unref(buffer);
    return GST_FLOW_ERROR;
  }
  status =
      finishing
          ? (s->encoder ? cix_stream_encoder_finish(s->handle.encoder, map.data,
                                                    map.size, progress)
                        : cix_stream_decoder_finish(s->handle.decoder, map.data,
                                                    map.size, progress))
          : (s->encoder
                 ? cix_stream_encoder_process(s->handle.encoder, in, n,
                                              map.data, map.size, progress)
                 : cix_stream_decoder_process(s->handle.decoder, in, n,
                                              map.data, map.size, progress));
  g_mutex_unlock(&s->lock);
  gst_buffer_unmap(buffer, &map);
  if (status != CIX_STATUS_OK ||
      !valid(*progress, finishing ? 0 : n, (gsize)s->max_output)) {
    gst_buffer_unref(buffer);
    return fail_current(s, generation, finishing ? "finish" : "process", status)
               ? GST_FLOW_ERROR
               : GST_FLOW_FLUSHING;
  }
  gst_buffer_set_size(buffer, progress->produced);
  *buffer_out = buffer;
  return GST_FLOW_OK;
}
/* The input and compressed-output byte positions are unrelated.  Always emit
 * a new full output segment starting at zero with an unknown stop. */
static gboolean push_output_segment(GstCixStream *s) {
  GstSegment segment;
  (void)s;
  gst_segment_init(&segment, GST_FORMAT_BYTES);
  segment.start = 0;
  segment.position = 0;
  segment.stop = GST_CLOCK_TIME_NONE;
  return gst_pad_push_event(s->src, gst_event_new_segment(&segment));
}
/* appsrc may end an empty byte stream without emitting a segment. Native
 * finish can still produce bytes, so establish one before the first buffer. */
static gboolean ensure_segment(GstCixStream *s) {
  gboolean needed;
  g_mutex_lock(&s->lock);
  needed = !s->have_segment;
  if (needed)
    s->have_segment = TRUE;
  g_mutex_unlock(&s->lock);
  if (!needed)
    return TRUE;
  return push_output_segment(s);
}
static GstFlowReturn push(GstCixStream *s, guint generation,
                          GstBuffer *buffer) {
  if (gst_buffer_get_size(buffer) == 0) {
    gst_buffer_unref(buffer);
    return GST_FLOW_OK;
  }
  if (!ensure_segment(s)) {
    gst_buffer_unref(buffer);
    return GST_FLOW_ERROR;
  }
  g_mutex_lock(&s->lock);
  if (generation != s->generation || g_atomic_int_get(&s->cancelled)) {
    g_mutex_unlock(&s->lock);
    gst_buffer_unref(buffer);
    return GST_FLOW_FLUSHING;
  }
  g_mutex_unlock(&s->lock);
  return gst_pad_push(s->src, buffer);
}
static GstFlowReturn run(GstCixStream *s, const guint8 *in, gsize n,
                         gboolean finishing) {
  gsize offset = 0;
  guint generation;
  g_mutex_lock(&s->lock);
  generation = s->generation;
  if (s->state != CIX_ACTIVE) {
    gboolean finished = s->state == CIX_FINISHED;
    g_mutex_unlock(&s->lock);
    if (finished)
      GST_ELEMENT_ERROR(s, STREAM, FAILED,
                        ("CIX byte stream received input after FINISHED"),
                        ("reset the element before reusing it"));
    return GST_FLOW_ERROR;
  }
  g_mutex_unlock(&s->lock);
  for (guint steps = 0; steps < 100000; ++steps) {
    GstBuffer *buffer;
    cix_stream_result_v1 p;
    GstFlowReturn flow =
        native_call(s, generation, in ? in + offset : NULL,
                    finishing ? 0 : n - offset, finishing, &buffer, &p);
    if (flow != GST_FLOW_OK)
      return flow;
    if (!finishing && p.state == CIX_STREAM_FINISHED &&
        p.consumed != n - offset) {
      gst_buffer_unref(buffer);
      return fail_current(s, generation, "process (trailing bytes)",
                          CIX_STATUS_CODEC_ERROR)
                 ? GST_FLOW_ERROR
                 : GST_FLOW_FLUSHING;
    }
    flow = push(s, generation, buffer);
    if (flow != GST_FLOW_OK)
      return flow;
    offset += p.consumed;
    if (p.state == CIX_STREAM_FINISHED) {
      g_mutex_lock(&s->lock);
      if (generation == s->generation)
        s->state = CIX_FINISHED;
      g_mutex_unlock(&s->lock);
      return GST_FLOW_OK;
    }
    if (!finishing && offset == n)
      return GST_FLOW_OK;
    if (p.consumed == 0 && p.produced == 0)
      return fail_current(s, generation, finishing ? "finish" : "process",
                          CIX_STATUS_OUTPUT_TOO_SMALL)
                 ? GST_FLOW_ERROR
                 : GST_FLOW_FLUSHING;
  }
  return fail_current(s, generation, "progress", CIX_STATUS_RESOURCE_LIMIT)
             ? GST_FLOW_ERROR
             : GST_FLOW_FLUSHING;
}
static GstFlowReturn chain(GstPad *pad, GstObject *parent, GstBuffer *buffer) {
  GstCixStream *s = GST_CIX_STREAM(parent);
  GstMapInfo map;
  GstFlowReturn flow;
  (void)pad;
  if (gst_buffer_get_size(buffer) > s->max_input) {
    gst_buffer_unref(buffer);
    fail(s, "input limit", CIX_STATUS_RESOURCE_LIMIT);
    return GST_FLOW_ERROR;
  }
  if (g_atomic_int_get(&s->cancelled)) {
    gst_buffer_unref(buffer);
    return GST_FLOW_FLUSHING;
  }
  if (!gst_buffer_map(buffer, &map, GST_MAP_READ)) {
    gst_buffer_unref(buffer);
    return GST_FLOW_ERROR;
  }
  flow = run(s, map.data, map.size, FALSE);
  gst_buffer_unmap(buffer, &map);
  gst_buffer_unref(buffer);
  return flow;
}
static gboolean sink_event(GstPad *pad, GstObject *parent, GstEvent *event) {
  GstCixStream *s = GST_CIX_STREAM(parent);
  gboolean ok;
  (void)pad;
  if (GST_EVENT_TYPE(event) == GST_EVENT_FLUSH_START) {
    /* Forward first: this unblocks a downstream pad push before waiting for a
     * native call. */
    g_atomic_int_set(&s->cancelled, TRUE);
    ok = gst_pad_push_event(s->src, event);
    g_mutex_lock(&s->lock);
    s->generation++;
    s->have_segment = FALSE;
    g_mutex_unlock(&s->lock);
    return ok;
  }
  if (GST_EVENT_TYPE(event) == GST_EVENT_FLUSH_STOP) {
    g_mutex_lock(&s->lock);
    s->generation++;
    s->state = reset_locked(s) ? CIX_ACTIVE : CIX_FAILED;
    g_atomic_int_set(&s->cancelled, FALSE);
    ok = s->state == CIX_ACTIVE;
    g_mutex_unlock(&s->lock);
    if (!ok) {
      gst_event_unref(event);
      fail(s, "reset", CIX_STATUS_CODEC_ERROR);
      return FALSE;
    }
    return gst_pad_push_event(s->src, event);
  }
  if (GST_EVENT_TYPE(event) == GST_EVENT_EOS) {
    g_mutex_lock(&s->lock);
    ok = s->state == CIX_FINISHED;
    g_mutex_unlock(&s->lock);
    if (!ok)
      ok = !g_atomic_int_get(&s->cancelled) &&
           run(s, NULL, 0, TRUE) == GST_FLOW_OK;
    if (!ok) {
      gst_event_unref(event);
      return FALSE;
    }
    return gst_pad_push_event(s->src, event);
  }
  if (GST_EVENT_TYPE(event) == GST_EVENT_SEGMENT) {
    const GstSegment *segment;
    GstFormat format;
    gst_event_parse_segment(event, &segment);
    if (!segment || segment->format != GST_FORMAT_BYTES) {
      format = segment ? segment->format : GST_FORMAT_UNDEFINED;
      gst_event_unref(event);
      GST_ELEMENT_ERROR(s, STREAM, FORMAT,
                        ("CIX byte stream requires GST_FORMAT_BYTES segments"),
                        ("received segment format %d", format));
      return FALSE;
    }
    g_mutex_lock(&s->lock);
    s->have_segment = TRUE;
    g_mutex_unlock(&s->lock);
    gst_event_unref(event);
    return push_output_segment(s);
  }
  return gst_pad_push_event(s->src, event);
}
static GstStateChangeReturn change_state(GstElement *element,
                                         GstStateChange transition) {
  GstCixStream *s = GST_CIX_STREAM(element);
  GstStateChangeReturn result;
  if (transition == GST_STATE_CHANGE_NULL_TO_READY) {
    g_mutex_lock(&s->lock);
    s->state = create_locked(s) ? CIX_ACTIVE : CIX_FAILED;
    g_mutex_unlock(&s->lock);
    if (s->state == CIX_FAILED) {
      fail(s, "initialization", CIX_STATUS_INVALID_OPTIONS);
      return GST_STATE_CHANGE_FAILURE;
    }
  }
  if (transition == GST_STATE_CHANGE_READY_TO_PAUSED) {
    g_mutex_lock(&s->lock);
    s->generation++;
    s->have_segment = FALSE;
    s->state = reset_locked(s) ? CIX_ACTIVE : CIX_FAILED;
    g_atomic_int_set(&s->cancelled, FALSE);
    g_mutex_unlock(&s->lock);
    if (s->state == CIX_FAILED)
      return GST_STATE_CHANGE_FAILURE;
  }
  result = GST_ELEMENT_CLASS(gst_cix_stream_parent_class)
               ->change_state(element, transition);
  if (result != GST_STATE_CHANGE_FAILURE &&
      transition == GST_STATE_CHANGE_READY_TO_NULL) {
    g_mutex_lock(&s->lock);
    destroy_locked(s);
    s->generation++;
    g_mutex_unlock(&s->lock);
  }
  return result;
}
static void set_property(GObject *o, guint id, const GValue *v, GParamSpec *p) {
  GstCixStream *s = GST_CIX_STREAM(o);
  (void)p;
  if (id == PROP_ENCODER)
    s->encoder = g_value_get_boolean(v);
  else if (id == PROP_MAX_INPUT)
    s->max_input = g_value_get_uint64(v);
  else if (id == PROP_MAX_OUTPUT)
    s->max_output = g_value_get_uint64(v);
  else if (id == PROP_OUTPUT_LIMIT)
    s->output_limit = g_value_get_uint64(v);
  else if (id == PROP_MEMORY_LIMIT)
    s->memory_limit = g_value_get_uint64(v);
  else
    G_OBJECT_WARN_INVALID_PROPERTY_ID(o, id, p);
}
static void get_property(GObject *o, guint id, GValue *v, GParamSpec *p) {
  GstCixStream *s = GST_CIX_STREAM(o);
  (void)p;
  if (id == PROP_ENCODER)
    g_value_set_boolean(v, s->encoder);
  else if (id == PROP_MAX_INPUT)
    g_value_set_uint64(v, s->max_input);
  else if (id == PROP_MAX_OUTPUT)
    g_value_set_uint64(v, s->max_output);
  else if (id == PROP_OUTPUT_LIMIT)
    g_value_set_uint64(v, s->output_limit);
  else if (id == PROP_MEMORY_LIMIT)
    g_value_set_uint64(v, s->memory_limit);
  else
    G_OBJECT_WARN_INVALID_PROPERTY_ID(o, id, p);
}
static void finalize(GObject *o) {
  GstCixStream *s = GST_CIX_STREAM(o);
  g_mutex_lock(&s->lock);
  destroy_locked(s);
  g_mutex_unlock(&s->lock);
  g_mutex_clear(&s->lock);
  G_OBJECT_CLASS(gst_cix_stream_parent_class)->finalize(o);
}
static void gst_cix_stream_class_init(GstCixStreamClass *k) {
  GObjectClass *g = G_OBJECT_CLASS(k);
  GstElementClass *e = GST_ELEMENT_CLASS(k);
  g->set_property = set_property;
  g->get_property = get_property;
  g->finalize = finalize;
  e->change_state = change_state;
  g_object_class_install_property(
      g, PROP_ENCODER,
      g_param_spec_boolean("encoder", "Encoder", "TRUE encodes CIXG1", TRUE,
                           G_PARAM_READWRITE | G_PARAM_CONSTRUCT_ONLY));
  g_object_class_install_property(
      g, PROP_MAX_INPUT,
      g_param_spec_uint64("max-input", "Maximum input",
                          "Maximum bytes per GstBuffer", 1, G_MAXUINT64,
                          1 << 20, G_PARAM_READWRITE | G_PARAM_CONSTRUCT_ONLY));
  g_object_class_install_property(
      g, PROP_MAX_OUTPUT,
      g_param_spec_uint64("max-output-buffer", "Maximum output",
                          "Maximum emitted output bytes", 1, G_MAXUINT64,
                          1 << 20, G_PARAM_READWRITE | G_PARAM_CONSTRUCT_ONLY));
  g_object_class_install_property(
      g, PROP_OUTPUT_LIMIT,
      g_param_spec_uint64("output-limit", "Output limit",
                          "Native stream output limit", 1, G_MAXUINT64, 4 << 20,
                          G_PARAM_READWRITE | G_PARAM_CONSTRUCT_ONLY));
  g_object_class_install_property(
      g, PROP_MEMORY_LIMIT,
      g_param_spec_uint64(
          "memory-limit", "Memory limit", "Total element reservation", 1,
          G_MAXUINT64, 128 << 20, G_PARAM_READWRITE | G_PARAM_CONSTRUCT_ONLY));
  gst_element_class_add_static_pad_template(e, &sink_template);
  gst_element_class_add_static_pad_template(e, &src_template);
  gst_element_class_set_static_metadata(e, "CIX byte stream", "Codec/Encoder",
                                        "Bounded CIXG1 byte stream", "CIX");
}
static void gst_cix_stream_init(GstCixStream *s) {
  s->encoder = TRUE;
  s->max_input = 1 << 20;
  s->max_output = 1 << 20;
  s->output_limit = 4 << 20;
  s->memory_limit = 128 << 20;
  s->state = CIX_FAILED;
  g_mutex_init(&s->lock);
  s->sink = gst_pad_new_from_static_template(&sink_template, "sink");
  s->src = gst_pad_new_from_static_template(&src_template, "src");
  gst_pad_set_chain_function(s->sink, GST_DEBUG_FUNCPTR(chain));
  gst_pad_set_event_function(s->sink, GST_DEBUG_FUNCPTR(sink_event));
  gst_element_add_pad(GST_ELEMENT(s), s->sink);
  gst_element_add_pad(GST_ELEMENT(s), s->src);
}
static gboolean plugin_init(GstPlugin *p) {
  return gst_element_register(p, "cixstream", GST_RANK_NONE,
                              GST_TYPE_CIX_STREAM);
}
GST_PLUGIN_DEFINE(GST_VERSION_MAJOR, GST_VERSION_MINOR, cixstream,
                  "bounded CIX byte stream", plugin_init, "0.1", "Proprietary",
                  "CIX", "https://example.invalid/cix")
