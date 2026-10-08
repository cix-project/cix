# SPDX-License-Identifier: GPL-2.0-only
# Linked by CIX-owned wrapper translation units against a pinned Linux reader.
obj-$(CONFIG_CIX_SQUASHFS) += cix_squashfs.o
ccflags-y += -I$(src)/include
ccflags-y += -I$(src)/profile/include
ccflags-y += -DCIX_PORTABLE_PROFILE_SOURCE=\"$(src)/profile/src/cix_squashfs_profile.c\"
ccflags-y += -DCIX_SQUASHFS_PROFILE_DECODER_ONLY
cix_squashfs-y := cix_profile_portable.o cix_squashfs_decompressor.o cix_squashfs_fs_register.o
cix_squashfs-y += cix_upstream_block.o cix_upstream_cache.o cix_upstream_dir.o cix_upstream_export.o
cix_squashfs-y += cix_upstream_file.o cix_upstream_fragment.o cix_upstream_id.o cix_upstream_inode.o
cix_squashfs-y += cix_upstream_namei.o cix_upstream_super.o cix_upstream_symlink.o cix_upstream_page_actor.o
cix_squashfs-y += cix_upstream_file_direct.o cix_upstream_decompressor_single.o
cix_squashfs-y += cix_upstream_decompressor_multi.o cix_upstream_decompressor_multi_percpu.o
cix_squashfs-y += cix_upstream_xattr.o cix_upstream_xattr_id.o
