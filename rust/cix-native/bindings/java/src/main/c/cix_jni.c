// SPDX-License-Identifier: MIT
#include <jni.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "cix.h"
#include "cix_stream.h"

static void fail_status(JNIEnv *env, const char *where, int status) {
    char message[160];
    snprintf(message, sizeof message, "%s failed with native CIX status %d", where, status);
    jclass exception = (*env)->FindClass(env, "dev/cix/binding/NativeCix$NativeException");
    if (exception != NULL) (*env)->ThrowNew(env, exception, message);
}
static void fail_argument(JNIEnv *env, const char *message) {
    jclass exception = (*env)->FindClass(env, "java/lang/IllegalArgumentException");
    if (exception != NULL) (*env)->ThrowNew(env, exception, message);
}
static int options(jint profile, jint workers, jlong output_limit, jlong memory_limit, cix_options_v1 *out) {
    if (workers <= 0 || output_limit <= 0 || memory_limit <= 0) return 0;
    if (profile != CIX_PROFILE_FAST && profile != CIX_PROFILE_DEFAULT && profile != CIX_PROFILE_BEST) return 0;
    *out = (cix_options_v1){CIX_ABI_VERSION_1, sizeof(cix_options_v1), (uint32_t)profile,
                            (uint32_t)workers, (uint64_t)output_limit, (uint64_t)memory_limit};
    return 1;
}
static void *handle(jlong value) { return value == 0 ? NULL : (void *)(uintptr_t)value; }
static jlong pointer(void *value) { return (jlong)(uintptr_t)value; }

JNIEXPORT jlong JNICALL Java_dev_cix_binding_NativeCix_createContext
  (JNIEnv *env, jclass klass, jint profile, jint workers, jlong output_limit, jlong memory_limit) {
    (void)klass;
    cix_options_v1 native;
    cix_context *context = NULL;
    if (!options(profile, workers, output_limit, memory_limit, &native)) {
        fail_argument(env, "invalid native context options"); return 0;
    }
    int status = cix_context_create(&native, &context);
    if (status != CIX_STATUS_OK || context == NULL) { fail_status(env, "cix_context_create", status); return 0; }
    return pointer(context);
}
JNIEXPORT void JNICALL Java_dev_cix_binding_NativeCix_destroyContext
  (JNIEnv *env, jclass klass, jlong value) { (void)env; (void)klass; cix_context_destroy(handle(value)); }

JNIEXPORT jbyteArray JNICALL Java_dev_cix_binding_NativeCix_bufferCall
  (JNIEnv *env, jclass klass, jlong value, jbyteArray input, jboolean encode, jlong output_limit, jlong memory_limit) {
    (void)klass;
    if (handle(value) == NULL || input == NULL || output_limit <= 0 || memory_limit <= 0
        || (uint64_t)output_limit > SIZE_MAX || (uint64_t)memory_limit > SIZE_MAX) {
        fail_argument(env, "invalid buffer call"); return NULL;
    }
    jsize input_len = (*env)->GetArrayLength(env, input);
    jbyte *input_bytes = input_len == 0 ? NULL : (*env)->GetByteArrayElements(env, input, NULL);
    if (input_len != 0 && input_bytes == NULL) return NULL;
    size_t needed = 0;
    int status = encode ? cix_encode_buffer(handle(value), (uint8_t *)input_bytes, (size_t)input_len, NULL, 0, &needed)
                        : cix_decode_buffer(handle(value), (uint8_t *)input_bytes, (size_t)input_len, NULL, 0, &needed);
    if (status == CIX_STATUS_OK && needed == 0) {
        if (input_bytes != NULL) (*env)->ReleaseByteArrayElements(env, input, input_bytes, JNI_ABORT);
        return (*env)->NewByteArray(env, 0);
    }
    if (status != CIX_STATUS_OUTPUT_TOO_SMALL || needed > (size_t)output_limit || needed > INT32_MAX
        || (size_t)input_len > (size_t)memory_limit || needed > (size_t)memory_limit - (size_t)input_len) {
        if (input_bytes != NULL) (*env)->ReleaseByteArrayElements(env, input, input_bytes, JNI_ABORT);
        fail_status(env, "cix complete-buffer sizing", status); return NULL;
    }
    jbyteArray output = (*env)->NewByteArray(env, (jsize)needed);
    if (output == NULL) { if (input_bytes != NULL) (*env)->ReleaseByteArrayElements(env, input, input_bytes, JNI_ABORT); return NULL; }
    jbyte *output_bytes = (*env)->GetByteArrayElements(env, output, NULL);
    if (output_bytes == NULL) { if (input_bytes != NULL) (*env)->ReleaseByteArrayElements(env, input, input_bytes, JNI_ABORT); return NULL; }
    size_t written = 0;
    status = encode ? cix_encode_buffer(handle(value), (uint8_t *)input_bytes, (size_t)input_len, (uint8_t *)output_bytes, needed, &written)
                    : cix_decode_buffer(handle(value), (uint8_t *)input_bytes, (size_t)input_len, (uint8_t *)output_bytes, needed, &written);
    (*env)->ReleaseByteArrayElements(env, output, output_bytes, status == CIX_STATUS_OK ? 0 : JNI_ABORT);
    if (input_bytes != NULL) (*env)->ReleaseByteArrayElements(env, input, input_bytes, JNI_ABORT);
    if (status != CIX_STATUS_OK || written != needed) { fail_status(env, "cix complete-buffer operation", status); return NULL; }
    return output;
}

JNIEXPORT jlong JNICALL Java_dev_cix_binding_NativeCix_createStream
  (JNIEnv *env, jclass klass, jboolean encoder, jint profile, jint workers, jlong output_limit, jlong memory_limit) {
    (void)klass;
    cix_options_v1 native; void *stream = NULL;
    if (!options(profile, workers, output_limit, memory_limit, &native)) { fail_argument(env, "invalid native stream options"); return 0; }
    int status = encoder ? cix_stream_encoder_create(&native, (cix_stream_encoder **)&stream)
                         : cix_stream_decoder_create(&native, (cix_stream_decoder **)&stream);
    if (status != CIX_STATUS_OK || stream == NULL) { fail_status(env, "cix stream create", status); return 0; }
    return pointer(stream);
}
JNIEXPORT void JNICALL Java_dev_cix_binding_NativeCix_destroyStream
  (JNIEnv *env, jclass klass, jlong value, jboolean encoder) {
    (void)env; (void)klass;
    if (encoder) cix_stream_encoder_destroy((cix_stream_encoder *)handle(value));
    else cix_stream_decoder_destroy((cix_stream_decoder *)handle(value));
}
JNIEXPORT void JNICALL Java_dev_cix_binding_NativeCix_resetStream
  (JNIEnv *env, jclass klass, jlong value, jboolean encoder) {
    (void)klass;
    int status = encoder ? cix_stream_encoder_reset((cix_stream_encoder *)handle(value))
                         : cix_stream_decoder_reset((cix_stream_decoder *)handle(value));
    if (status != CIX_STATUS_OK) fail_status(env, "cix stream reset", status);
}

JNIEXPORT jlongArray JNICALL Java_dev_cix_binding_NativeCix_streamCall
  (JNIEnv *env, jclass klass, jlong value, jboolean encoder, jint operation, jbyteArray input, jbyteArray output) {
    (void)klass;
    if (handle(value) == NULL || output == NULL || (operation == 0 && input == NULL) || operation < 0 || operation > 2) {
        fail_argument(env, "invalid stream call"); return NULL;
    }
    jsize input_len = input == NULL ? 0 : (*env)->GetArrayLength(env, input);
    jsize output_len = (*env)->GetArrayLength(env, output);
    jbyte *input_bytes = input_len == 0 ? NULL : (*env)->GetByteArrayElements(env, input, NULL);
    jbyte *output_bytes = output_len == 0 ? NULL : (*env)->GetByteArrayElements(env, output, NULL);
    if ((input_len != 0 && input_bytes == NULL) || (output_len != 0 && output_bytes == NULL)) goto cleanup;
    cix_stream_result_v1 result = {0, 0, 0};
    int status;
    if (operation == 0) status = encoder
        ? cix_stream_encoder_process((cix_stream_encoder *)handle(value), (uint8_t *)input_bytes, (size_t)input_len, (uint8_t *)output_bytes, (size_t)output_len, &result)
        : cix_stream_decoder_process((cix_stream_decoder *)handle(value), (uint8_t *)input_bytes, (size_t)input_len, (uint8_t *)output_bytes, (size_t)output_len, &result);
    else if (operation == 1 && encoder) status = cix_stream_encoder_flush((cix_stream_encoder *)handle(value), (uint8_t *)output_bytes, (size_t)output_len, &result);
    else if (operation == 2) status = encoder
        ? cix_stream_encoder_finish((cix_stream_encoder *)handle(value), (uint8_t *)output_bytes, (size_t)output_len, &result)
        : cix_stream_decoder_finish((cix_stream_decoder *)handle(value), (uint8_t *)output_bytes, (size_t)output_len, &result);
    else { fail_argument(env, "invalid stream operation"); goto cleanup; }
    if (status != CIX_STATUS_OK || result.consumed > (size_t)input_len || result.produced > (size_t)output_len) { fail_status(env, "cix stream operation", status); goto cleanup; }
    jlong values[3] = {(jlong)result.consumed, (jlong)result.produced, (jlong)result.state};
    jlongArray returned = (*env)->NewLongArray(env, 3);
    if (returned != NULL) (*env)->SetLongArrayRegion(env, returned, 0, 3, values);
    if (output_bytes != NULL) (*env)->ReleaseByteArrayElements(env, output, output_bytes, 0);
    if (input_bytes != NULL) (*env)->ReleaseByteArrayElements(env, input, input_bytes, JNI_ABORT);
    return returned;
cleanup:
    if (output_bytes != NULL) (*env)->ReleaseByteArrayElements(env, output, output_bytes, JNI_ABORT);
    if (input_bytes != NULL) (*env)->ReleaseByteArrayElements(env, input, input_bytes, JNI_ABORT);
    return NULL;
}
