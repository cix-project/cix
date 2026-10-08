// SPDX-License-Identifier: MIT
/*
 * CIX-owned 7-Zip SDK module registry.  CodecExports.cpp is compiled unchanged
 * from the selected official SDK and consumes these exact globals.
 */
// Instantiate the official SDK interface GUIDs once for this module.
#include "Common/MyInitGuid.h"
#include "cix_7zip_coder.h"
#include "7zip/Common/RegisterCodec.h"

namespace {

void *CreateCixDecoder() {
  return static_cast<void *>(Cix7zipNewExperimentalDefaultDecoder());
}

void *CreateCixEncoder() {
  return static_cast<void *>(Cix7zipNewExperimentalDefaultEncoder());
}

const CCodecInfo kCixCodec = {
    CreateCixDecoder,
    CreateCixEncoder,
    CIX_7ZIP_EXPERIMENTAL_METHOD_ID_V1,
    "CIX-EXPERIMENTAL-v1",
    1,
    false};

}  // namespace

/* Names and types are the official CodecExports.cpp link contract. */
unsigned g_NumCodecs = 1;
const CCodecInfo *g_Codecs[] = {&kCixCodec};
unsigned g_NumHashers = 0;
const CHasherInfo *g_Hashers[] = {NULL};
