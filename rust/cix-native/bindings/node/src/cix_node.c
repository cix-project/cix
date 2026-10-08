// SPDX-License-Identifier: MIT
#include <cix.h>
#include <cix_stream.h>
#include <math.h>
#include <node_api.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define JS_SAFE_INTEGER UINT64_C(9007199254740991)

#if NAPI_VERSION < 8
#error "CIX Node binding requires N-API version 8 type tags"
#endif

typedef struct {
    cix_context *native;
    cix_options_v1 options;
    int closed;
} context;

typedef struct {
    void *native;
    int decoder;
    int closed;
    int cancelled;
    uint64_t limit;
    uint64_t produced;
    uint64_t memory;
} stream;

/* Stable, distinct tags prevent prototype borrowing from reinterpreting state. */
static const napi_type_tag CONTEXT_TAG = {UINT64_C(0x4bf2511ca1f2d613), UINT64_C(0xb79d235ef0c41a68)};
static const napi_type_tag STREAM_TAG = {UINT64_C(0x7193dce1a605f40b), UINT64_C(0x2af618ce5b749d97)};

static napi_value fail(napi_env e, const char *m) {
    napi_throw_error(e, NULL, m);
    return NULL;
}

static int ok(napi_env e, napi_status s) {
    if (s == napi_ok) {
        return 1;
    }
    fail(e, "Node-API call failed");
    return 0;
}

static int u64(napi_env e, napi_value v, uint64_t *out) {
    napi_valuetype t;
    double x;
    if (!ok(e, napi_typeof(e, v, &t)) || t != napi_number
        || !ok(e, napi_get_value_double(e, v, &x)) || x < 0 || !isfinite(x)
        || x > (double)JS_SAFE_INTEGER || (double)(uint64_t)x != x) {
        fail(e, "expected a non-negative safe integer");
        return 0;
    }
    *out = (uint64_t)x;
    return 1;
}

static int options(napi_env e, napi_value v, cix_options_v1 *o) {
    cix_status s = cix_options_v1_default(o);
    if (s != CIX_STATUS_OK) {
        fail(e, "CIX default options failed");
        return 0;
    }
    if (!v) {
        return 1;
    }

    napi_valuetype t;
    if (!ok(e, napi_typeof(e, v, &t)) || t != napi_object)
        return fail(e, "options must be an object"), 0;

    const char *names[] = {"profile", "workers", "outputLimit", "memoryLimit"};
    for (size_t i = 0; i < 4; i++) {
        bool has;
        napi_value x;
        if (!ok(e, napi_has_named_property(e, v, names[i], &has)) || !has) {
            continue;
        }
        if (!ok(e, napi_get_named_property(e, v, names[i], &x))) {
            return 0;
        }

        uint64_t n;
        if (!u64(e, x, &n)) {
            return 0;
        }
        if (i == 0) {
            if (n < 1 || n > 3) {
                return fail(e, "profile must be 1, 2, or 3"), 0;
            }
            o->profile = (uint32_t)n;
        } else if (i == 1) {
            if (n == 0 || n > UINT32_MAX) {
                return fail(e, "workers is out of range"), 0;
            }
            o->workers = (uint32_t)n;
        } else if (i == 2) {
            o->output_limit = n;
        } else {
            o->memory_limit = n;
        }
    }
    return 1;
}

static int input(napi_env e, napi_value v, uint8_t **p, size_t *n) {
    bool b;
    if (!ok(e, napi_is_buffer(e, v, &b)) || !b)
        return fail(e, "input must be a Buffer"), 0;
    return ok(e, napi_get_buffer_info(e, v, (void **)p, n));
}

static void context_final(napi_env e, void *p, void *h) {
    (void)e;
    (void)h;
    context *x = p;
    if (x) {
        if (x->native) cix_context_destroy(x->native);
        free(x);
    }
}

static void stream_final(napi_env e, void *p, void *h) {
    (void)e;
    (void)h;
    stream *x = p;
    if (x) {
        if (x->native) {
            if (x->decoder) cix_stream_decoder_destroy(x->native);
            else cix_stream_encoder_destroy(x->native);
        }
        free(x);
    }
}

static int unwrap(
    napi_env e,
    napi_callback_info i,
    const napi_type_tag *expected_tag,
    void **p,
    napi_value *argv,
    size_t *argc
) {
    napi_value th;
    bool matches = false;
    if (!ok(e, napi_get_cb_info(e, i, argc, argv, &th, NULL))
        || !ok(e, napi_check_object_type_tag(e, th, expected_tag, &matches))) return 0;
    if (!matches) {
        return fail(e, "invalid CIX receiver type"), 0;
    }
    if (!ok(e, napi_unwrap(e, th, p))) {
        return 0;
    }
    if (*p == NULL) {
        fail(e, "invalid CIX object");
        return 0;
    }
    return 1;
}

static napi_value context_new(napi_env e, napi_callback_info i) {
    size_t n = 1;
    napi_value a[1], th;
    if (!ok(e, napi_get_cb_info(e, i, &n, a, &th, NULL))) {
        return NULL;
    }

    context *x = calloc(1, sizeof(*x));
    if (!x) {
        return fail(e, "allocation failed");
    }
    if (!options(e, n ? a[0] : NULL, &x->options)
        || cix_context_create(&x->options, &x->native) != CIX_STATUS_OK || !x->native) {
        free(x);
        return fail(e, "CIX context creation failed");
    }
    if (!ok(e, napi_type_tag_object(e, th, &CONTEXT_TAG))
        || !ok(e, napi_wrap(e, th, x, context_final, NULL, NULL))) {
        context_final(e, x, NULL);
        return NULL;
    }
    return th;
}

static napi_value buffer_op(napi_env e, napi_callback_info i, int decode) {
    size_t n = 1;
    napi_value a[1];
    context *x;
    if (!unwrap(e, i, &CONTEXT_TAG, (void **)&x, a, &n) || n != 1 || x->closed || !x->native)
        return fail(e, "context is disposed");

    uint8_t *p;
    size_t z, need = 0;
    if (!input(e, a[0], &p, &z)) {
        return NULL;
    }
    cix_status s = decode ? cix_decode_buffer(x->native, p, z, NULL, 0, &need)
                          : cix_encode_buffer(x->native, p, z, NULL, 0, &need);
    if (s != CIX_STATUS_OUTPUT_TOO_SMALL && s != CIX_STATUS_OK)
        return fail(e, "CIX buffer operation failed");
    if (need > x->options.output_limit || need > x->options.memory_limit)
        return fail(e, "CIX output exceeds configured limit");

    size_t expected = need;
    if (z > x->options.memory_limit || expected > x->options.memory_limit - z)
        return fail(e, "CIX input and output exceed configured memory limit");

    void *out;
    napi_value r;
    if (!ok(e, napi_create_buffer(e, expected, &out, &r))) {
        return NULL;
    }
    s = decode ? cix_decode_buffer(x->native, p, z, out, expected, &need)
               : cix_encode_buffer(x->native, p, z, out, expected, &need);
    if (s != CIX_STATUS_OK || need != expected)
        return fail(e, "CIX buffer operation changed required output size");
    return r;
}

static napi_value context_encode(napi_env e, napi_callback_info i) { return buffer_op(e, i, 0); }
static napi_value context_decode(napi_env e, napi_callback_info i) { return buffer_op(e, i, 1); }

static napi_value context_dispose(napi_env e, napi_callback_info i) {
    size_t n = 0;
    context *x;
    if (!unwrap(e, i, &CONTEXT_TAG, (void **)&x, NULL, &n)) {
        return NULL;
    }
    if (!x->closed) {
        cix_context_destroy(x->native);
        x->native = NULL;
        x->closed = 1;
    }
    napi_value u;
    napi_get_undefined(e, &u);
    return u;
}

static napi_value stream_new(napi_env e, napi_callback_info i) {
    size_t n = 1;
    napi_value a[1], th;
    void *kind = NULL;
    if (!ok(e, napi_get_cb_info(e, i, &n, a, &th, &kind))) {
        return NULL;
    }

    stream *x = calloc(1, sizeof(*x));
    cix_options_v1 o;
    if (!x || !options(e, n ? a[0] : NULL, &o)) {
        free(x);
        return NULL;
    }
    x->decoder = kind != NULL;
    x->limit = o.output_limit;
    x->memory = o.memory_limit;
    cix_status s = x->decoder
        ? cix_stream_decoder_create(&o, (cix_stream_decoder **)&x->native)
        : cix_stream_encoder_create(&o, (cix_stream_encoder **)&x->native);
    if (s != CIX_STATUS_OK || !x->native) {
        int decoder = x->decoder;
        free(x);
        return fail(e, decoder ? "CIX stream decoder creation failed" : "CIX stream encoder creation failed");
    }
    if (!ok(e, napi_type_tag_object(e, th, &STREAM_TAG))
        || !ok(e, napi_wrap(e, th, x, stream_final, NULL, NULL))) {
        stream_final(e, x, NULL);
        return NULL;
    }
    return th;
}

static napi_value stream_call(napi_env e, napi_callback_info i, int mode) {
    size_t n = 2;
    napi_value a[2];
    stream *x;
    if (!unwrap(e, i, &STREAM_TAG, (void **)&x, a, &n) || x->closed || !x->native)
        return fail(e, "stream is disposed");
    if (x->cancelled) {
        return fail(e, "stream is cancelled; call reset before reuse");
    }

    uint8_t *in = NULL;
    size_t ilen = 0, cap = 0;
    if (mode == 0) {
        if (n != 2 || !input(e, a[0], &in, &ilen)) {
            return NULL;
        }
        uint64_t q;
        if (!u64(e, a[1], &q) || q > SIZE_MAX)
            return fail(e, "output capacity is out of range");
        cap = (size_t)q;
    } else {
        if (n != 1) {
            return fail(e, "output capacity is required");
        }
        uint64_t q;
        if (!u64(e, a[0], &q) || q > SIZE_MAX)
            return fail(e, "output capacity is out of range");
        cap = (size_t)q;
    }

    /* The exact-length Node result is a second bounded copy. */
    if (cap > x->limit - x->produced || cap > x->memory / 2 || ilen > x->memory - cap * 2)
        return fail(e, "stream buffers exceed configured limits");

    void *out;
    napi_value data;
    if (!ok(e, napi_create_buffer(e, cap, &out, &data))) {
        return NULL;
    }

    cix_stream_result_v1 r = {0};
    cix_status s;
    if (mode == 0) {
        s = x->decoder
            ? cix_stream_decoder_process(x->native, in, ilen, out, cap, &r)
            : cix_stream_encoder_process(x->native, in, ilen, out, cap, &r);
    } else if (mode == 1) {
        if (x->decoder) {
            return fail(e, "decoder streams do not support flush");
        }
        s = cix_stream_encoder_flush(x->native, out, cap, &r);
    } else {
        s = x->decoder
            ? cix_stream_decoder_finish(x->native, out, cap, &r)
            : cix_stream_encoder_finish(x->native, out, cap, &r);
    }
    if (s != CIX_STATUS_OK || r.consumed > ilen || r.produced > cap
        || r.produced > x->limit - x->produced || r.state < 1 || r.state > 3)
        return fail(e, "invalid CIX stream result");

    x->produced += r.produced;
    napi_value result, v;
    if (!ok(e, napi_create_object(e, &result))
        || !ok(e, napi_create_buffer_copy(e, r.produced, out, NULL, &data))) return NULL;
    napi_create_double(e, (double)r.consumed, &v);
    napi_set_named_property(e, result, "consumed", v);
    napi_create_double(e, (double)r.produced, &v);
    napi_set_named_property(e, result, "produced", v);
    napi_create_uint32(e, r.state, &v);
    napi_set_named_property(e, result, "state", v);
    napi_set_named_property(e, result, "data", data);
    return result;
}

static napi_value enc_process(napi_env e, napi_callback_info i) { return stream_call(e, i, 0); }
static napi_value enc_flush(napi_env e, napi_callback_info i) { return stream_call(e, i, 1); }
static napi_value enc_finish(napi_env e, napi_callback_info i) { return stream_call(e, i, 2); }

static napi_value stream_reset(napi_env e, napi_callback_info i) {
    size_t n = 0;
    stream *x;
    if (!unwrap(e, i, &STREAM_TAG, (void **)&x, NULL, &n) || x->closed)
        return fail(e, "stream is disposed");
    cix_status s = x->decoder ? cix_stream_decoder_reset(x->native)
                              : cix_stream_encoder_reset(x->native);
    if (s != CIX_STATUS_OK) {
        return fail(e, "CIX stream reset failed");
    }
    x->cancelled = 0;
    x->produced = 0;
    napi_value u;
    napi_get_undefined(e, &u);
    return u;
}

static napi_value stream_cancel(napi_env e, napi_callback_info i) {
    size_t n = 0;
    stream *x;
    if (!unwrap(e, i, &STREAM_TAG, (void **)&x, NULL, &n)) {
        return NULL;
    }
    x->cancelled = 1;
    napi_value u;
    napi_get_undefined(e, &u);
    return u;
}

static napi_value stream_dispose(napi_env e, napi_callback_info i) {
    size_t n = 0;
    stream *x;
    if (!unwrap(e, i, &STREAM_TAG, (void **)&x, NULL, &n)) {
        return NULL;
    }
    if (!x->closed) {
        if (x->decoder) cix_stream_decoder_destroy(x->native);
        else cix_stream_encoder_destroy(x->native);
        x->native = NULL;
        x->closed = 1;
    }
    napi_value u;
    napi_get_undefined(e, &u);
    return u;
}

static napi_value init(napi_env e, napi_value ex) {
    napi_property_descriptor c[] = {
        {"encode", 0, context_encode, 0, 0, 0, napi_default, 0},
        {"decode", 0, context_decode, 0, 0, 0, napi_default, 0},
        {"dispose", 0, context_dispose, 0, 0, 0, napi_default, 0},
    };
    napi_property_descriptor s[] = {
        {"process", 0, enc_process, 0, 0, 0, napi_default, 0},
        {"flush", 0, enc_flush, 0, 0, 0, napi_default, 0},
        {"finish", 0, enc_finish, 0, 0, 0, napi_default, 0},
        {"reset", 0, stream_reset, 0, 0, 0, napi_default, 0},
        {"cancel", 0, stream_cancel, 0, 0, 0, napi_default, 0},
        {"dispose", 0, stream_dispose, 0, 0, 0, napi_default, 0},
    };
    napi_value k;
    if (!ok(e, napi_define_class(e, "Context", NAPI_AUTO_LENGTH, context_new, 0, 3, c, &k)))
        return NULL;
    napi_set_named_property(e, ex, "Context", k);
    if (!ok(e, napi_define_class(e, "Encoder", NAPI_AUTO_LENGTH, stream_new, NULL, 6, s, &k)))
        return NULL;
    napi_set_named_property(e, ex, "Encoder", k);
    if (!ok(e, napi_define_class(e, "Decoder", NAPI_AUTO_LENGTH, stream_new, (void *)1, 6, s, &k)))
        return NULL;
    napi_set_named_property(e, ex, "Decoder", k);
    return ex;
}

NAPI_MODULE(NODE_GYP_MODULE_NAME, init)
