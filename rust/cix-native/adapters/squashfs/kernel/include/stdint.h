/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef CIX_KERNEL_STDINT_H
#define CIX_KERNEL_STDINT_H
#include <linux/types.h>
typedef u8 uint8_t;
typedef u32 uint32_t;
typedef u64 uint64_t;
typedef unsigned long uintptr_t;
#ifndef UINT64_C
#define UINT64_C(value) value##ULL
#endif
#ifndef UINT32_MAX
#define UINT32_MAX 0xffffffffU
#endif
#ifndef UINTPTR_MAX
#define UINTPTR_MAX (~(uintptr_t)0)
#endif
#endif
