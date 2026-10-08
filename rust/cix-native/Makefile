PREFIX ?= /usr/local
DESTDIR ?=
PROFILE ?= release
CARGO ?= cargo
CARGO_TARGET_DIR ?= target
CMAKE ?= cmake
INSTALL ?= install
LN_S ?= ln -sfn
RM ?= rm -f

BINDIR := $(DESTDIR)$(PREFIX)/bin
MANDIR := $(DESTDIR)$(PREFIX)/share/man/man1
DOCDIR := $(DESTDIR)$(PREFIX)/share/doc/cix
TARGETDIR := $(CARGO_TARGET_DIR)/$(PROFILE)
RUNTIME_BINARY := $(TARGETDIR)/cix
ALIASES := uncix cixcat
SDK_BUILD_DIR ?= $(CARGO_TARGET_DIR)/cix-sdk-$(PROFILE)
LIBDIR ?= lib

UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
SDK_SHARED_LIBRARY := $(TARGETDIR)/libcix_native.dylib
else
SDK_SHARED_LIBRARY := $(TARGETDIR)/libcix_native.so
endif
SDK_STATIC_LIBRARY := $(TARGETDIR)/libcix_native.a

.PHONY: all build check test sdk-configure install install-only install-sdk uninstall uninstall-sdk clean

all: build

# Build the one installed CLI executable and the process-free SDK libraries.
# uncix and cixcat are argv[0]-selected aliases, never separately built tools.
build:
	CARGO_TARGET_DIR=$(CARGO_TARGET_DIR) $(CARGO) build --locked --profile $(PROFILE) --lib --bin cix

check:
	CARGO_TARGET_DIR=$(CARGO_TARGET_DIR) $(CARGO) check --locked --bins --lib

test:
	CARGO_TARGET_DIR=$(CARGO_TARGET_DIR) $(CARGO) test --locked

sdk-configure: build
	$(CMAKE) -S packaging/sdk -B $(SDK_BUILD_DIR) \
		-DCIX_NATIVE_LIBRARY=$(abspath $(SDK_SHARED_LIBRARY)) \
		-DCIX_NATIVE_STATIC_LIBRARY=$(abspath $(SDK_STATIC_LIBRARY)) \
		-DCMAKE_INSTALL_PREFIX=$(PREFIX) \
		-DCMAKE_INSTALL_LIBDIR=$(LIBDIR) \
		-DCMAKE_INSTALL_DOCDIR=share/doc/cix

install: build install-only install-sdk

# Install one physical executable. uncix and cixcat are argv[0]-selected Unix
# symlinks; the CLI owns their distinct modes.
install-only:
	$(INSTALL) -d $(BINDIR) $(MANDIR) $(DOCDIR)
	$(INSTALL) -m 0755 $(RUNTIME_BINARY) $(BINDIR)/cix
	for alias in $(ALIASES); do $(LN_S) cix $(BINDIR)/$$alias; done
	$(INSTALL) -m 0644 man/cix.1 man/uncix.1 man/cixcat.1 $(MANDIR)
	$(INSTALL) -m 0644 README.md LICENSE THIRD_PARTY_NOTICES.txt THIRD_PARTY_NOTICES_RUST.md $(DOCDIR)

# This installs headers, CMake/pkg-config metadata and both selected Rust
# library artifacts. It is a native SDK installation, distinct from optional
# host adapters and the separately staged full-engine runtime providers.
install-sdk: sdk-configure
	DESTDIR=$(DESTDIR) $(CMAKE) --install $(SDK_BUILD_DIR) --prefix $(PREFIX)

uninstall: uninstall-sdk
	$(RM) $(BINDIR)/cix
	for alias in $(ALIASES); do $(RM) $(BINDIR)/$$alias; done
	$(RM) $(MANDIR)/cix.1 $(MANDIR)/uncix.1 $(MANDIR)/cixcat.1
	$(RM) $(DOCDIR)/README.md $(DOCDIR)/LICENSE $(DOCDIR)/THIRD_PARTY_NOTICES.txt $(DOCDIR)/THIRD_PARTY_NOTICES_RUST.md

# CMake has no portable generated uninstall target. Remove only files that
# this source package installs; leave unrelated prefix contents untouched.
uninstall-sdk:
	$(RM) $(DESTDIR)$(PREFIX)/include/cix.h $(DESTDIR)$(PREFIX)/include/cix.hpp $(DESTDIR)$(PREFIX)/include/cix_stream.h $(DESTDIR)$(PREFIX)/include/cix_formats.h $(DESTDIR)$(PREFIX)/include/cix_dictionary.h
	$(RM) $(DESTDIR)$(PREFIX)/$(LIBDIR)/libcix_native.so $(DESTDIR)$(PREFIX)/$(LIBDIR)/libcix_native.dylib $(DESTDIR)$(PREFIX)/$(LIBDIR)/libcix_native.a
	$(RM) $(DESTDIR)$(PREFIX)/$(LIBDIR)/pkgconfig/cix-native.pc
	$(RM) $(DESTDIR)$(PREFIX)/$(LIBDIR)/cmake/CIX/CIXConfig.cmake $(DESTDIR)$(PREFIX)/$(LIBDIR)/cmake/CIX/CIXConfigVersion.cmake
	$(RM) $(DESTDIR)$(PREFIX)/share/doc/cix/README.md $(DESTDIR)$(PREFIX)/share/doc/cix/examples/dictionary.c

clean:
	CARGO_TARGET_DIR=$(CARGO_TARGET_DIR) $(CARGO) clean
