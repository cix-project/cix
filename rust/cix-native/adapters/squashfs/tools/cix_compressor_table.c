// SPDX-License-Identifier: MIT
/* GPL-2.0-or-later: CIX-owned table replacement for squashfs-tools 4.6.1.
 * Compile this file instead of that revision's compressor.c. */
#include <stdio.h>
#include <string.h>
#include "compressor.h"
#include "squashfs_fs.h"

#define CIX_EXPERIMENTAL_COMPRESSION 65001
#define STUB(NAME, ID) static struct compressor NAME##_comp_ops = { .id = ID, .name = #NAME }
#ifndef GZIP_SUPPORT
STUB(gzip, ZLIB_COMPRESSION);
#else
extern struct compressor gzip_comp_ops;
#endif
#ifndef LZMA_SUPPORT
STUB(lzma, LZMA_COMPRESSION);
#else
extern struct compressor lzma_comp_ops;
#endif
#ifndef LZO_SUPPORT
STUB(lzo, LZO_COMPRESSION);
#else
extern struct compressor lzo_comp_ops;
#endif
#ifndef LZ4_SUPPORT
STUB(lz4, LZ4_COMPRESSION);
#else
extern struct compressor lz4_comp_ops;
#endif
#ifndef XZ_SUPPORT
STUB(xz, XZ_COMPRESSION);
#else
extern struct compressor xz_comp_ops;
#endif
#ifndef ZSTD_SUPPORT
STUB(zstd, ZSTD_COMPRESSION);
#else
extern struct compressor zstd_comp_ops;
#endif
extern struct compressor cix_comp_ops;
STUB(unknown, 0);
struct compressor *compressor[] = { &gzip_comp_ops, &lzo_comp_ops, &lz4_comp_ops,
    &xz_comp_ops, &zstd_comp_ops, &lzma_comp_ops, &cix_comp_ops, &unknown_comp_ops };

struct compressor *lookup_compressor(char *name) { int i; for (i=0; compressor[i]->id; ++i) if (!strcmp(compressor[i]->name,name)) break; return compressor[i]; }
struct compressor *lookup_compressor_id(int id) { int i; for (i=0; compressor[i]->id; ++i) if (compressor[i]->id == id) break; return compressor[i]; }
int valid_compressor(char *name) { return lookup_compressor(name)->supported; }
void display_compressor_usage(FILE *stream, char *def, int cols) { int i; (void)cols; fprintf(stream,"\nCompressors available and compressor specific options:\n"); for(i=0;compressor[i]->id;++i) if(compressor[i]->supported) { fprintf(stream,"\t%s%s\n", compressor[i]->name,!strcmp(compressor[i]->name,def)?" (default)":""); if(compressor[i]->usage) compressor[i]->usage(stream,cols); } }
void print_selected_comp_options(FILE *stream, struct compressor *comp, char *prog) { int cols=80; fprintf(stream,"%s: selected compressor \"%s\". Options supported: %s\n",prog,comp->name,comp->usage?"":"none"); if(comp->usage)comp->usage(stream,cols); }
void print_comp_options(FILE *stream, int cols, char *name, char *prog) { int i; if(!strcmp(name,"all")){display_compressor_usage(stream,COMP_DEFAULT,cols);return;} for(i=0;compressor[i]->id;++i)if(compressor[i]->supported&&!strcmp(compressor[i]->name,name)){print_selected_comp_options(stream,compressor[i],prog);return;} }
