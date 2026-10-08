# Generated public-source workspace entry point.
PREFIX ?= /usr/local
DESTDIR ?=
PROFILE ?= release
CARGO ?= cargo
CARGO_TARGET_DIR ?= $(CURDIR)/target
NATIVE_CRATE := rust/cix-native

.PHONY: all build check test install install-only install-sdk uninstall uninstall-sdk clean sdk-configure
all build check test install install-only install-sdk uninstall uninstall-sdk clean sdk-configure:
	$(MAKE) -C $(NATIVE_CRATE) $@ PREFIX="$(PREFIX)" DESTDIR="$(DESTDIR)" PROFILE="$(PROFILE)" CARGO="$(CARGO)" CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)"

# The portable WebAssembly subset is a separate, deliberately limited crate.
portable:
	$(CARGO) build --locked -p cix-portable
