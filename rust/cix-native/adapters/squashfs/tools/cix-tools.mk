# GNU-make overlay for the pinned squashfs-tools commit db038ef265ed00bb17aaf695a72f9b0c3489eb20.
# Invoke from an empty build directory: make -f /abs/.../cix-tools.mk cix-tools UPSTREAM=/abs/squashfs-tools
ifndef UPSTREAM
$(error set UPSTREAM to the pinned squashfs-tools checkout root)
endif
PROFILE ?= $(abspath ../profile)
CIX_TOOLS := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
VPATH := $(UPSTREAM)
EXTRA_CFLAGS += -I$(UPSTREAM) -I$(PROFILE)/include
include $(UPSTREAM)/Makefile

CIX_OBJECTS := cix_wrapper.o cix_compressor_table.o cix_squashfs_profile.o
MKSQUASHFS_CIX_OBJS := $(filter-out compressor.o,$(MKSQUASHFS_OBJS)) $(CIX_OBJECTS)
UNSQUASHFS_CIX_OBJS := $(filter-out compressor.o,$(UNSQUASHFS_OBJS)) $(CIX_OBJECTS)

.PHONY: cix-tools
cix-tools: cix-mksquashfs cix-unsquashfs
cix-mksquashfs: $(MKSQUASHFS_CIX_OBJS)
	$(CC) $(LDFLAGS) $(EXTRA_LDFLAGS) $^ $(LIBS) $(MKSQUASHFS_LIBS) -o $@
cix-unsquashfs: $(UNSQUASHFS_CIX_OBJS)
	$(CC) $(LDFLAGS) $(EXTRA_LDFLAGS) $^ $(LIBS) -o $@
cix_wrapper.o: $(CIX_TOOLS)cix_wrapper.c
	$(CC) -c $(CFLAGS) $(CPPFLAGS) $(EXTRA_CFLAGS) $< -o $@
cix_compressor_table.o: $(CIX_TOOLS)cix_compressor_table.c
	$(CC) -c $(CFLAGS) $(CPPFLAGS) $(EXTRA_CFLAGS) $< -o $@
cix_squashfs_profile.o: $(PROFILE)/src/cix_squashfs_profile.c
	$(CC) -c $(CFLAGS) $(CPPFLAGS) $(EXTRA_CFLAGS) $< -o $@
