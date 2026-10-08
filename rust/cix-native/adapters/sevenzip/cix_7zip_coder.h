// SPDX-License-Identifier: MIT
#ifndef CIX_7ZIP_CODER_H
#define CIX_7ZIP_CODER_H

/*
 * CIX-owned, experimental 7-Zip SDK coder contract.  This header deliberately
 * does not register a 7z method with 7-Zip and does not describe a ZIP method.
 */
#include "7zip/ICoder.h"

#define CIX_7ZIP_CODER_ABI_V1 1U
#define CIX_7ZIP_METHOD_VERSION_V1 1U
#define CIX_7ZIP_WIRE_PROPERTIES_SIZE_V1 5U
/* Private development identifier only.  It has no upstream allocation. */
#define CIX_7ZIP_EXPERIMENTAL_METHOD_ID_V1 0x00000000C1580001ULL

struct cix_7zip_coder_options_v1 {
  UInt32 abi_version;
  UInt32 struct_size;
  UInt64 input_limit;
  UInt64 output_limit;
  UInt64 memory_limit;
  UInt32 profile; /* cix_profile: FAST=1, DEFAULT=2, BEST=3 */
  UInt32 workers;
};

/* Returns a separately configured single-direction ICompressCoder.  The host
 * owns the returned COM reference and must Release() it.  These factories are
 * not an automatic 7-Zip registry hook: a host integration must publish the
 * method/version and ship the matching decoder. */
HRESULT Cix7zipCreateExperimentalEncoder(
    const cix_7zip_coder_options_v1 *options, ICompressCoder **coder);
HRESULT Cix7zipCreateExperimentalDecoder(
    const cix_7zip_coder_options_v1 *options, ICompressCoder **coder);

/* Module factories use deployment-local admission defaults.  Archive coder
 * properties contain only the CIX 7-Zip wire-version marker; they never raise
 * these local input/output/memory limits during decode. */
HRESULT Cix7zipCreateExperimentalDefaultEncoder(ICompressCoder **coder);
HRESULT Cix7zipCreateExperimentalDefaultDecoder(ICompressCoder **coder);

/* CodecExports.cpp takes an unreferenced newly allocated implementation and
 * applies AddRef itself.  These are for the CIX-owned module only. */
ICompressCoder *Cix7zipNewExperimentalDefaultEncoder(void);
ICompressCoder *Cix7zipNewExperimentalDefaultDecoder(void);

#endif
