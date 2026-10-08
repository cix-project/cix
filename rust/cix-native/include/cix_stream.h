#ifndef CIX_STREAM_H
#define CIX_STREAM_H

#include "cix.h"

#ifdef __cplusplus
extern "C" {
#endif

#define CIX_STREAM_ABI_VERSION_1 UINT32_C(1)
/** Opaque encoder for independent native CIXG1 blocks. */
typedef struct cix_stream_encoder cix_stream_encoder;
/** Opaque decoder for independent native CIXG1 blocks. */
typedef struct cix_stream_decoder cix_stream_decoder;
/** Progress state returned with each stream call. */
typedef enum cix_stream_state { CIX_STREAM_NEEDS_INPUT=1, CIX_STREAM_NEEDS_OUTPUT=2, CIX_STREAM_FINISHED=3 } cix_stream_state;
/** Caller-owned progress record; consumed and produced are byte counts for this call. */
typedef struct cix_stream_result_v1 { size_t consumed; size_t produced; uint32_t state; } cix_stream_result_v1;

/* Native independent CIXG1 blocks only: no full-engine routes or retained-history streams.
 * Each handle is internally serialized. Destroying it concurrently with a call is invalid.
 * `result` must point to one complete writable result record and must not overlap input or
 * output; input and output must also be disjoint. NULL buffers are allowed only at length 0. */
/** Create an encoder using a validated #cix_options_v1 snapshot. */
cix_status cix_stream_encoder_create(const cix_options_v1 *, cix_stream_encoder **);
/** Create a decoder using a validated #cix_options_v1 snapshot. */
cix_status cix_stream_decoder_create(const cix_options_v1 *, cix_stream_decoder **);
/** Destroy an encoder after all calls using it have completed. */
void cix_stream_encoder_destroy(cix_stream_encoder *);
/** Destroy a decoder after all calls using it have completed. */
void cix_stream_decoder_destroy(cix_stream_decoder *);
/** Consume available encoder input and emit available output without flushing a block. */
cix_status cix_stream_encoder_process(cix_stream_encoder *, const uint8_t *, size_t, uint8_t *, size_t, cix_stream_result_v1 *);
/** Consume available decoder input and emit available output. */
cix_status cix_stream_decoder_process(cix_stream_decoder *, const uint8_t *, size_t, uint8_t *, size_t, cix_stream_result_v1 *);
/** End the current encoder block while retaining supported stream history. */
cix_status cix_stream_encoder_flush(cix_stream_encoder *, uint8_t *, size_t, cix_stream_result_v1 *);
/** Emit the encoder terminator; repeat until the returned state is FINISHED. */
cix_status cix_stream_encoder_finish(cix_stream_encoder *, uint8_t *, size_t, cix_stream_result_v1 *);
/** Validate decoder completion; repeat with output space until FINISHED. */
cix_status cix_stream_decoder_finish(cix_stream_decoder *, uint8_t *, size_t, cix_stream_result_v1 *);
/** Discard encoder state after an error or completed archive. */
cix_status cix_stream_encoder_reset(cix_stream_encoder *);
/** Discard decoder state after an error or completed archive. */
cix_status cix_stream_decoder_reset(cix_stream_decoder *);

#ifdef __cplusplus
}
#endif
#endif
