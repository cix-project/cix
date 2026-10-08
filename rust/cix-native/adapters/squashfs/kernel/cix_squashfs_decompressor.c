// SPDX-License-Identifier: GPL-2.0-only
/* CIX-owned replacement of the pinned decompressor dispatch translation unit. */
#include "include/cix_squashfs_prefix.h"
#include "include/cix_squashfs_kernel.h"
#include <linux/bio.h>
#include <linux/slab.h>
#include <linux/vmalloc.h>

#include "upstream/squashfs_fs.h"
#include "upstream/squashfs_fs_sb.h"
#include "upstream/decompressor.h"
#include "upstream/squashfs.h"
#include "upstream/page_actor.h"

struct cix_squashfs_stream {
	u8 *input;
	u8 *output;
	void *workspace;
	size_t workspace_size;
	size_t capacity;
};

static void *cix_squashfs_init(struct squashfs_sb_info *msblk, void *opts)
{
	struct cix_squashfs_stream *stream;
	size_t capacity = max_t(size_t, msblk->block_size, SQUASHFS_METADATA_SIZE);

	if (opts || msblk->block_size > CIX_SQUASHFS_DATA_LIMIT)
		return ERR_PTR(-EINVAL);
	stream = kzalloc(sizeof(*stream), GFP_KERNEL);
	if (!stream)
		return ERR_PTR(-ENOMEM);
	stream->input = vmalloc(capacity);
	stream->output = vmalloc(capacity);
	stream->workspace_size = cix_squashfs_profile_decode_workspace_size();
	stream->workspace = kmalloc(stream->workspace_size, GFP_KERNEL);
	if (!stream->input || !stream->output || !stream->workspace) {
		vfree(stream->input);
		vfree(stream->output);
		kfree(stream->workspace);
		kfree(stream);
		return ERR_PTR(-ENOMEM);
	}
	stream->capacity = capacity;
	return stream;
}

static void cix_squashfs_free(void *ptr)
{
	struct cix_squashfs_stream *stream = ptr;

	if (stream) {
		vfree(stream->input);
		vfree(stream->output);
		kfree(stream->workspace);
	}
	kfree(stream);
}

static int cix_squashfs_copy_output(struct squashfs_page_actor *actor,
	const u8 *src, size_t length)
{
	void *page = cix_squashfs_first_page(actor);
	size_t copied = 0;

	while (page && copied < length) {
		size_t chunk = min_t(size_t, PAGE_SIZE, length - copied);
		/* A page actor may deliberately skip a cached page. */
		if (!IS_ERR(page))
			memcpy(page, src + copied, chunk);
		copied += chunk;
		if (copied < length)
			page = cix_squashfs_next_page(actor);
	}
	return copied == length ? 0 : -EIO;
}

static int cix_squashfs_decompress(struct squashfs_sb_info *msblk, void *ptr,
	struct bio *bio, int offset, int length, struct squashfs_page_actor *actor)
{
	struct cix_squashfs_stream *stream = ptr;
	struct bvec_iter_all all = {};
	struct bio_vec *bvec = bvec_init_iter_all(&all);
	size_t written = 0;
	int remaining = length;
	int ret;

	if (length < 0 || (size_t)length > stream->capacity ||
	    actor->length < 0 || (size_t)actor->length > stream->capacity)
		return -EIO;
	while (bio_next_segment(bio, &all)) {
		int available = min(remaining, (int)bvec->bv_len - offset);
		if (available < 0)
			return -EIO;
		memcpy(stream->input + length - remaining, bvec_virt(bvec) + offset,
		       available);
		remaining -= available;
		offset = 0;
	}
	if (remaining)
		return -EIO;

	ret = cix_squashfs_profile_decode_with_workspace(actor->length <= CIX_SQUASHFS_METADATA_LIMIT ?
		CIX_SQUASHFS_KERNEL_METADATA : CIX_SQUASHFS_KERNEL_DATA,
		stream->input, length, stream->output, actor->length, stream->workspace,
		stream->workspace_size, &written);
	if (ret || written > actor->length)
		return -EIO;
	ret = cix_squashfs_copy_output(actor, stream->output, written);
	cix_squashfs_finish_page(actor);
	if (ret)
		return -EIO;
	return written;
}

static const struct squashfs_decompressor cix_squashfs_comp_ops = {
	.init = cix_squashfs_init,
	.free = cix_squashfs_free,
	.decompress = cix_squashfs_decompress,
	.id = CIX_SQUASHFS_COMPRESSION,
	.name = "cix-experimental-65001",
	.alloc_buffer = 0,
	.supported = 1,
};

static const struct squashfs_decompressor cix_squashfs_unknown_comp_ops = {
	.id = 0,
	.name = "unknown",
};

const struct squashfs_decompressor *cix_squashfs_lookup_decompressor(int id)
{
	/* The pinned reader passes its 16-bit wire ID through a signed short. */
	return (u16)id == CIX_SQUASHFS_COMPRESSION ? &cix_squashfs_comp_ops :
		&cix_squashfs_unknown_comp_ops;
}

void *cix_squashfs_decompressor_setup(struct super_block *sb,
	unsigned short flags)
{
	struct squashfs_sb_info *msblk = sb->s_fs_info;

	if (SQUASHFS_COMP_OPTS(flags))
		return ERR_PTR(-EIO);
	return msblk->thread_ops->create(msblk, NULL);
}
