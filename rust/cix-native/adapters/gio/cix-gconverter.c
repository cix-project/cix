// SPDX-License-Identifier: MIT
#include "cix-gconverter.h"

typedef enum { CIX_ACTION_NONE, CIX_ACTION_FLUSH, CIX_ACTION_FINISH } CixAction;

struct _CixGConverter {
    GObject parent;
    gboolean encoder;
    gboolean finished;
    gint cancelled;
    CixAction action;
    GError *failure;
    union {
        cix_stream_encoder *encoder;
        cix_stream_decoder *decoder;
    } handle;
};

static void converter_interface_init(GConverterIface *iface);
G_DEFINE_TYPE_WITH_CODE(CixGConverter, cix_gconverter, G_TYPE_OBJECT,
                       G_IMPLEMENT_INTERFACE(G_TYPE_CONVERTER, converter_interface_init))

static void cix_gconverter_finalize(GObject *object)
{
    CixGConverter *self = CIX_GCONVERTER(object);
    if (self->encoder)
        cix_stream_encoder_destroy(self->handle.encoder);
    else
        cix_stream_decoder_destroy(self->handle.decoder);
    g_clear_error(&self->failure);
    G_OBJECT_CLASS(cix_gconverter_parent_class)->finalize(object);
}

static void cix_gconverter_class_init(CixGConverterClass *klass)
{
    G_OBJECT_CLASS(klass)->finalize = cix_gconverter_finalize;
}

static void cix_gconverter_init(CixGConverter *self)
{
    (void)self;
}

/* GIO must first report successful progress, then report a terminal error on
 * the next call. Keep the error until reset so it cannot be mistaken for EOF. */
static GConverterResult fail(CixGConverter *self, GIOErrorEnum code,
                             const char *message, gsize used, gsize made, GError **error)
{
    if (self->failure == NULL)
        self->failure = g_error_new_literal(G_IO_ERROR, code, message);
    if (used != 0 || made != 0)
        return G_CONVERTER_CONVERTED;
    g_propagate_error(error, g_error_copy(self->failure));
    return G_CONVERTER_ERROR;
}

static gboolean valid_progress(cix_stream_result_v1 progress, gsize input, gsize output)
{
    return progress.consumed <= input && progress.produced <= output &&
           (progress.state == CIX_STREAM_NEEDS_INPUT ||
            progress.state == CIX_STREAM_NEEDS_OUTPUT ||
            progress.state == CIX_STREAM_FINISHED);
}

static GConverterResult convert(GConverter *base, const void *input, gsize input_size,
                                void *output, gsize output_size, GConverterFlags flags,
                                gsize *used, gsize *made, GError **error)
{
    CixGConverter *self = CIX_GCONVERTER(base);
    cix_stream_result_v1 progress = {0};
    cix_status status;
    *used = *made = 0;
    if (self->failure != NULL) {
        g_propagate_error(error, g_error_copy(self->failure));
        return G_CONVERTER_ERROR;
    }
    if (g_atomic_int_get(&self->cancelled))
        return fail(self, G_IO_ERROR_CANCELLED, "CIX converter cancelled", 0, 0, error);
    if (self->finished)
        return fail(self, G_IO_ERROR_CLOSED, "CIX converter requires reset after finish", 0, 0, error);
    if (output_size == 0) {
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_NO_SPACE, "CIX needs output space");
        return G_CONVERTER_ERROR;
    }
    if (!self->encoder && (flags & G_CONVERTER_FLUSH) && !(flags & G_CONVERTER_INPUT_AT_END)) {
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                            "CIX decoder does not support flush");
        return G_CONVERTER_ERROR;
    }

    if (self->action == CIX_ACTION_NONE) {
        status = self->encoder
            ? cix_stream_encoder_process(self->handle.encoder, input, input_size,
                                         output, output_size, &progress)
            : cix_stream_decoder_process(self->handle.decoder, input, input_size,
                                         output, output_size, &progress);
        if (status != CIX_STATUS_OK)
            return fail(self, G_IO_ERROR_INVALID_DATA, "CIX process failed or exceeded its limits", 0, 0, error);
        if (!valid_progress(progress, input_size, output_size))
            return fail(self, G_IO_ERROR_FAILED, "CIX returned invalid progress", 0, 0, error);
        *used = progress.consumed;
        *made = progress.produced;
        if (progress.state == CIX_STREAM_FINISHED) {
            self->finished = TRUE;
            return G_CONVERTER_FINISHED;
        }
        /* The flags describe the end of the supplied input. Do not finish
         * while any of that input still belongs to the caller. */
        if (*used == input_size) {
            if (flags & G_CONVERTER_INPUT_AT_END)
                self->action = CIX_ACTION_FINISH;
            else if (flags & G_CONVERTER_FLUSH)
                self->action = CIX_ACTION_FLUSH;
        }
    }

    if (self->action != CIX_ACTION_NONE && *made < output_size) {
        guint8 *tail = (guint8 *)output + *made;
        gsize remaining = output_size - *made;
        progress = (cix_stream_result_v1){0};
        if (self->action == CIX_ACTION_FINISH)
            status = self->encoder
                ? cix_stream_encoder_finish(self->handle.encoder, tail, remaining, &progress)
                : cix_stream_decoder_finish(self->handle.decoder, tail, remaining, &progress);
        else
            status = cix_stream_encoder_flush(self->handle.encoder, tail, remaining, &progress);
        if (status != CIX_STATUS_OK)
            return fail(self, G_IO_ERROR_INVALID_DATA,
                        "CIX flush/finish failed, truncated input or resource limit",
                        *used, *made, error);
        if (!valid_progress(progress, 0, remaining))
            return fail(self, G_IO_ERROR_FAILED, "CIX returned invalid terminal progress",
                        *used, *made, error);
        *made += progress.produced;
        if (progress.state == CIX_STREAM_FINISHED) {
            self->finished = TRUE;
            return G_CONVERTER_FINISHED;
        }
        if (self->action == CIX_ACTION_FLUSH && progress.state == CIX_STREAM_NEEDS_INPUT) {
            self->action = CIX_ACTION_NONE;
            return G_CONVERTER_FLUSHED;
        }
    }
    if (*used != 0 || *made != 0)
        return G_CONVERTER_CONVERTED;
    g_set_error_literal(error, G_IO_ERROR,
                        progress.state == CIX_STREAM_NEEDS_OUTPUT ? G_IO_ERROR_NO_SPACE : G_IO_ERROR_PARTIAL_INPUT,
                        progress.state == CIX_STREAM_NEEDS_OUTPUT ? "CIX needs more output space" : "CIX needs more input");
    return G_CONVERTER_ERROR;
}

static void reset(GConverter *base)
{
    CixGConverter *self = CIX_GCONVERTER(base);
    cix_status status = self->encoder
        ? cix_stream_encoder_reset(self->handle.encoder)
        : cix_stream_decoder_reset(self->handle.decoder);
    self->finished = FALSE;
    self->action = CIX_ACTION_NONE;
    g_atomic_int_set(&self->cancelled, FALSE);
    g_clear_error(&self->failure);
    if (status != CIX_STATUS_OK)
        self->failure = g_error_new(G_IO_ERROR, G_IO_ERROR_FAILED, "CIX reset failed: %d", (int)status);
}

void cix_gconverter_cancel(CixGConverter *self)
{
    g_return_if_fail(CIX_IS_GCONVERTER(self));
    g_atomic_int_set(&self->cancelled, TRUE);
}

static void converter_interface_init(GConverterIface *iface)
{
    iface->convert = convert;
    iface->reset = reset;
}

CixGConverter *cix_gconverter_new(gboolean encoder, const cix_options_v1 *options, GError **error)
{
    CixGConverter *self = g_object_new(CIX_TYPE_GCONVERTER, NULL);
    self->encoder = encoder;
    cix_status status = encoder
        ? cix_stream_encoder_create(options, &self->handle.encoder)
        : cix_stream_decoder_create(options, &self->handle.decoder);
    if (status == CIX_STATUS_OK)
        return self;
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED, "CIX create failed: %d", (int)status);
    g_object_unref(self);
    return NULL;
}
