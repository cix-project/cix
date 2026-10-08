// SPDX-License-Identifier: MIT
/* Replacement TU for squashfs-tools-ng v1.2.0 compressor.c. */
#define SQFS_BUILDING_DLL
#include "config.h"
#include <stdlib.h>
#include <string.h>
#include "cix_squashfs_profile.h"

#define CIX_ID 65001u
#ifndef CIX_SQFSNG_COMPRESSOR_SOURCE
#error "define CIX_SQFSNG_COMPRESSOR_SOURCE to pinned lib/sqfs/comp/compressor.c"
#endif
#define sqfs_compressor_create cix_upstream_compressor_create
#define sqfs_compressor_name_from_id cix_upstream_compressor_name_from_id
#define sqfs_compressor_id_from_name cix_upstream_compressor_id_from_name
#define sqfs_compressor_config_init cix_upstream_compressor_config_init
#include CIX_SQFSNG_COMPRESSOR_SOURCE
#undef sqfs_compressor_create
#undef sqfs_compressor_name_from_id
#undef sqfs_compressor_id_from_name
#undef sqfs_compressor_config_init

typedef struct { sqfs_compressor_t base; sqfs_u32 block_size; int decode; } cix_cmp_t;
static void cix_destroy(sqfs_object_t *obj) { free(obj); }
static sqfs_object_t *cix_copy(const sqfs_object_t *obj) { cix_cmp_t *copy = malloc(sizeof(*copy)); if (copy) memcpy(copy,obj,sizeof(*copy)); return (sqfs_object_t *)copy; }
static void cix_config(const sqfs_compressor_t *base, sqfs_compressor_config_t *cfg) { const cix_cmp_t *cmp=(const cix_cmp_t *)base; memset(cfg,0,sizeof(*cfg));cfg->id=CIX_ID;cfg->block_size=cmp->block_size;if(cmp->decode)cfg->flags=SQFS_COMP_FLAG_UNCOMPRESS; }
static int cix_options_write(sqfs_compressor_t *base, sqfs_file_t *file) { (void)base;(void)file;return 0; }
static int cix_options_read(sqfs_compressor_t *base, sqfs_file_t *file) { (void)base;(void)file;return 0; }
static sqfs_s32 cix_block(sqfs_compressor_t *base,const sqfs_u8 *in,sqfs_u32 n,sqfs_u8 *out,sqfs_u32 cap) { cix_cmp_t *cmp=(cix_cmp_t *)base;size_t z=0;enum cix_squashfs_profile_kind kind=cap<=8192?CIX_SQUASHFS_METADATA:CIX_SQUASHFS_DATA;int r=cmp->decode?cix_squashfs_profile_decode(kind,in,n,out,cap,&z):cix_squashfs_profile_encode(kind,in,n,out,cap,&z);if(r==CIX_SQUASHFS_OK)return z;if(!cmp->decode&&(r==CIX_SQUASHFS_NO_BENEFIT||r==CIX_SQUASHFS_OUTPUT_TOO_SMALL))return 0;return cmp->decode&&r==CIX_SQUASHFS_OUTPUT_TOO_SMALL?0:SQFS_ERROR_COMPRESSOR; }
static int cix_create(const sqfs_compressor_config_t *cfg,sqfs_compressor_t **out) { cix_cmp_t *cmp;if(cfg->block_size==0||cfg->block_size>131072||cfg->flags&~SQFS_COMP_FLAG_UNCOMPRESS)return SQFS_ERROR_UNSUPPORTED;cmp=calloc(1,sizeof(*cmp));if(!cmp)return SQFS_ERROR_ALLOC;cmp->block_size=cfg->block_size;cmp->decode=(cfg->flags&SQFS_COMP_FLAG_UNCOMPRESS)!=0;cmp->base.get_configuration=cix_config;cmp->base.write_options=cix_options_write;cmp->base.read_options=cix_options_read;cmp->base.do_block=cix_block;cmp->base.base.destroy=cix_destroy;cmp->base.base.copy=cix_copy;*out=&cmp->base;return 0; }
int sqfs_compressor_create(const sqfs_compressor_config_t *cfg,sqfs_compressor_t **out) { if(!cfg||!out)return SQFS_ERROR_ARG_INVALID;if(cfg->id==CIX_ID)return cix_create(cfg,out);return cix_upstream_compressor_create(cfg,out); }
const char *sqfs_compressor_name_from_id(SQFS_COMPRESSOR id) { return (sqfs_u16)id==CIX_ID?"cix-experimental":cix_upstream_compressor_name_from_id(id); }
int sqfs_compressor_id_from_name(const char *name) { return !strcmp(name,"cix-experimental")?CIX_ID:cix_upstream_compressor_id_from_name(name); }
int sqfs_compressor_config_init(sqfs_compressor_config_t *cfg,SQFS_COMPRESSOR id,size_t block,sqfs_u16 flags) { if((sqfs_u16)id!=CIX_ID)return cix_upstream_compressor_config_init(cfg,id,block,flags);if(!cfg||block==0||block>131072||flags&~SQFS_COMP_FLAG_UNCOMPRESS)return SQFS_ERROR_UNSUPPORTED;memset(cfg,0,sizeof(*cfg));cfg->id=CIX_ID;cfg->block_size=block;cfg->flags=flags;return 0; }
