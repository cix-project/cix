// SPDX-License-Identifier: MIT
#pragma once
#include <gio/gio.h>
#include "cix_stream.h"
G_BEGIN_DECLS
#define CIX_TYPE_GCONVERTER (cix_gconverter_get_type())
G_DECLARE_FINAL_TYPE(CixGConverter, cix_gconverter, CIX, GCONVERTER, GObject)

/**
 * cix_gconverter_new:
 * @encoder: %TRUE for an encoder and %FALSE for a decoder
 * @options: options passed to the native stream constructor
 * @error: return location for a #GError, or %NULL
 *
 * Creates a #GConverter backed by one native CIX stream. The returned object
 * owns that stream and is released with g_object_unref().
 *
 * Returns: (transfer full): a new converter, or %NULL after reporting
 *   `G_IO_ERROR` when native stream creation fails.
 */
CixGConverter *cix_gconverter_new(gboolean encoder, const cix_options_v1 *options, GError **error);

/**
 * cix_gconverter_cancel:
 * @converter: a #CixGConverter that remains alive for the call
 *
 * Requests cancellation through an atomic entry fence. An in-flight native
 * codec call is not preempted; a later conversion observes cancellation until
 * the #GConverter reset operation clears it. This does not provide a general
 * concurrent-use guarantee for the object.
 */
void cix_gconverter_cancel(CixGConverter *converter);
G_END_DECLS
