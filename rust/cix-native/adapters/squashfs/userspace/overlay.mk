# Out-of-tree object replacement contract. Append this file to a generated
# Automake Makefile with `make -f Makefile -f overlay.mk TARGET`; do not edit
# the vendor tree or the generated Makefile.
# Pins: squashfs-tools-ng f2a3ad56e40c9711b23371238f9fa07dd24245f1;
#       squashfuse 3f4dd2928ab362f8b20eab2be864d8e622472df5.
CIX_USERSPACE ?= $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
CIX_PROFILE ?= $(abspath $(CIX_USERSPACE)/../profile)
CIX_PROFILE_SRC := $(CIX_PROFILE)/src/cix_squashfs_profile.c
CIX_COMMON_SRC := $(CIX_USERSPACE)/cix_sqfs_userspace.c
CIX_CPPFLAGS := -I$(CIX_USERSPACE)/include -I$(CIX_PROFILE)/include

# Required variables name the exact immutable vendor source trees.
# SQFSNG_SRC is v1.2.0; SQUASHFUSE_SRC is 0.5.0.  CIX_READER must be either
# sqfsng or squashfuse; this prevents the two projects' same-named local
# bridge objects from sharing an accidental recipe.
define cix_sqfsng_compile
	$(LIBTOOL) --tag=CC --mode=compile $(CC) $(DEFS) $(DEFAULT_INCLUDES) $(INCLUDES) $(AM_CPPFLAGS) $(CPPFLAGS) $(libsquashfs_la_CPPFLAGS) $(AM_CFLAGS) $(CFLAGS) $(libsquashfs_la_CFLAGS) $(CIX_CPPFLAGS) $(1) -c -o $@ $(2)
endef
define cix_squashfuse_compile
	$(LIBTOOL) --tag=CC --mode=compile $(CC) $(DEFS) $(DEFAULT_INCLUDES) $(INCLUDES) $(AM_CPPFLAGS) $(CPPFLAGS) $(libsquashfuse_convenience_la_CPPFLAGS) $(AM_CFLAGS) $(CFLAGS) $(CIX_CPPFLAGS) $(1) -c -o $@ $(2)
endef

ifeq ($(CIX_READER),sqfsng)
# Replace exactly compressor.c and read_super.c. Extra CIX objects join
# libsqfs, so rdsquashfs links the actual restricted profile.
libsquashfs_la_OBJECTS += cix_sqfs_userspace.lo cix_squashfs_profile.lo
libsquashfs.la: cix_sqfs_userspace.lo cix_squashfs_profile.lo
lib/sqfs/comp/libsquashfs_la-compressor.lo: $(CIX_USERSPACE)/libsquashfs/cix_sqfsng_compressor.c $(SQFSNG_SRC)/lib/sqfs/comp/compressor.c
	$(call cix_sqfsng_compile,-DCIX_SQFSNG_COMPRESSOR_SOURCE='"$(SQFSNG_SRC)/lib/sqfs/comp/compressor.c"',$<)
lib/sqfs/libsquashfs_la-read_super.lo: $(CIX_USERSPACE)/libsquashfs/cix_sqfsng_read_super.c $(SQFSNG_SRC)/lib/sqfs/read_super.c
	$(call cix_sqfsng_compile,-DCIX_SQFSNG_READ_SUPER_SOURCE='"$(SQFSNG_SRC)/lib/sqfs/read_super.c"',$<)
cix_sqfs_userspace.lo: $(CIX_COMMON_SRC)
	$(call cix_sqfsng_compile,,$<)
cix_squashfs_profile.lo: $(CIX_PROFILE_SRC)
	$(call cix_sqfsng_compile,,$<)
else ifeq ($(CIX_READER),squashfuse)

# Replace exactly decompress.c. Its convenience library supplies both
# squashfuse_extract and public libsquashfuse, including xattr reads.
libsquashfuse_convenience_la_OBJECTS += cix_sqfs_userspace.lo cix_squashfs_profile.lo
libsquashfuse_convenience.la: cix_sqfs_userspace.lo cix_squashfs_profile.lo
libsquashfuse_convenience_la-decompress.lo: $(CIX_USERSPACE)/squashfuse/cix_squashfuse_decode.c $(SQUASHFUSE_SRC)/decompress.c
	$(call cix_squashfuse_compile,-DCIX_SQUASHFUSE_DECOMPRESS_SOURCE='"$(SQUASHFUSE_SRC)/decompress.c"',$<)
cix_sqfs_userspace.lo: $(CIX_COMMON_SRC)
	$(call cix_squashfuse_compile,,$<)
cix_squashfs_profile.lo: $(CIX_PROFILE_SRC)
	$(call cix_squashfuse_compile,,$<)
squashfuse-xattr-contract: libsquashfuse.la $(CIX_USERSPACE)/tests/squashfuse_xattr_contract.c
	$(LIBTOOL) --tag=CC --mode=link $(CC) $(DEFS) $(DEFAULT_INCLUDES) $(INCLUDES) $(AM_CPPFLAGS) $(CPPFLAGS) $(CFLAGS) -I$(SQUASHFUSE_SRC) -o $@ $(CIX_USERSPACE)/tests/squashfuse_xattr_contract.c libsquashfuse.la $(COMPRESSION_LIBS) $(FUSE_LIBS)
squashfuse-xattr-contract-static: $(CIX_USERSPACE)/tests/squashfuse_xattr_contract.c .libs/libsquashfuse.a
	$(LIBTOOL) --tag=CC --mode=link $(CC) $(DEFS) $(DEFAULT_INCLUDES) $(INCLUDES) $(AM_CPPFLAGS) $(CPPFLAGS) $(CFLAGS) -I$(SQUASHFUSE_SRC) -o $@ $(CIX_USERSPACE)/tests/squashfuse_xattr_contract.c .libs/libsquashfuse.a $(COMPRESSION_LIBS) $(FUSE_LIBS)
squashfuse-library-fixture-contract: libsquashfuse.la $(CIX_USERSPACE)/tests/squashfuse_library_fixture_contract.c
	$(LIBTOOL) --tag=CC --mode=link $(CC) $(DEFS) $(DEFAULT_INCLUDES) $(INCLUDES) $(AM_CPPFLAGS) $(CPPFLAGS) $(CFLAGS) -I$(SQUASHFUSE_SRC) -o $@ $(CIX_USERSPACE)/tests/squashfuse_library_fixture_contract.c libsquashfuse.la $(COMPRESSION_LIBS) $(FUSE_LIBS)
# Compile a verifier against a pre-existing static archive without making the
# archive or any vendor/CIX library object a prerequisite. This supports the
# v5 qualification retry without rebuilding its accepted v4 SDK.
squashfuse-library-fixture-contract-static: $(CIX_USERSPACE)/tests/squashfuse_library_fixture_contract.c .libs/libsquashfuse.a
	$(LIBTOOL) --tag=CC --mode=link $(CC) $(DEFS) $(DEFAULT_INCLUDES) $(INCLUDES) $(AM_CPPFLAGS) $(CPPFLAGS) $(CFLAGS) -I$(SQUASHFUSE_SRC) -o $@ $(CIX_USERSPACE)/tests/squashfuse_library_fixture_contract.c .libs/libsquashfuse.a $(COMPRESSION_LIBS) $(FUSE_LIBS)
else
$(error CIX_READER must be sqfsng or squashfuse)
endif
