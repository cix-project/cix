// SPDX-License-Identifier: MIT
#ifndef CIX_BLOSC2_ADAPTER_H
#define CIX_BLOSC2_ADAPTER_H

#include <blosc2.h>

#ifdef __cplusplus
extern "C" {
#endif

/* `compcode_meta` required on every CIX Blosc2 chunk. */
#define CIX_BLOSC2_ADAPTER_METADATA_V1 UINT8_C(1)
#define CIX_BLOSC2_ADAPTER_CODEC_VERSION UINT8_C(1)

/**
 * @brief Registers the configured CIX user codec in this process's Blosc2 registry.
 *
 * Applications choose the registration point; the CMake configuration selects
 * the codec ID.
 *
 * @return The result of `blosc2_register_codec()`.
 */
#if defined(_WIN32)
# if defined(CIX_BLOSC2_BUILDING)
#  define CIX_BLOSC2_EXPORT __declspec(dllexport)
# else
#  define CIX_BLOSC2_EXPORT __declspec(dllimport)
# endif
#else
# define CIX_BLOSC2_EXPORT __attribute__((visibility("default")))
#endif
CIX_BLOSC2_EXPORT int cix_blosc2_register(void);

#ifdef __cplusplus
}
#endif

#endif /* CIX_BLOSC2_ADAPTER_H */
