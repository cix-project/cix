# CIX tar and libarchive program-filter adapter

This is a bounded **installed-consumer** contract. It uses the ordinary
installed `cix` executable as a byte-stream filter; it neither links CLI
internals nor registers CIX as a stock libarchive compression format.

GNU tar's `--use-compress-program=COMMAND` (`-I COMMAND`) invokes a command on
standard input/output. Its documented contract is that no argument encodes and
`-d` decodes. `cix-tar-filter.sh` implements that interface. See [GNU tar's
filter contract](https://www.gnu.org/software/tar/manual/html_node/gzip.html).

libarchive provides external-program filters through
`archive_write_add_filter_program` and
`archive_read_support_filter_program`. `libarchive_program_filter.c` uses those
public APIs with separate encode/decode commands. Current upstream headers
declare both APIs in [archive.h](https://github.com/libarchive/libarchive/blob/master/libarchive/archive.h).
This is deliberately distinct from GNU tar: libarchive 3.7.2 parses a command
itself and executes it directly; it does not run a shell. Its
[command parser](https://github.com/libarchive/libarchive/blob/v3.7.2/libarchive/archive_cmdline.c)
accepts double quotes and backslash escapes, while treating single quotes as
ordinary bytes. The C consumer therefore double-quotes the executable path and
escapes embedded `"` and `\\`. It supports absolute wrapper paths containing
spaces, single quotes, double quotes, and backslashes within its 4096-byte
command limit.

On POSIX, libarchive 3.7.2 invokes its program filter with a null child
environment, so `CIX_TAR_CIX` is not available to that child. The C consumer
therefore passes `--cix /absolute/path/to/cix` explicitly. The wrapper accepts
that form first, then `CIX_TAR_CIX` for GNU tar, then an absolute installed
sibling `cix` next to the wrapper. Every route requires an absolute executable
path and never searches `PATH`.

There is no public libarchive API to register a third-party in-process filter.
This is an external program filter only; it does not claim registered CIX
codec support. A future native host API adapter must use only installed
`cix.h`/`cix_stream.h`, never Rust CLI internals.

## Contract and limits

For GNU tar, set `CIX_TAR_CIX` to the selected installed `cix` binary's
**absolute** executable path. For libarchive, pass that same path explicitly
to the C consumer; do not depend on `CIX_TAR_CIX`. The wrapper never searches
`PATH`.
`CIX_TAR_MEMORY` defaults to `512MiB` and is passed to ordinary streaming CIX.

`-I` receives a shell command. Quote the wrapper as one shell word to preserve
spaces and metacharacters in its installed path:

```sh
filter="'$(printf %s "$PWD/cix-tar-filter.sh" | sed "s/'/'\\\\''/g")'"
tar -C input -I "$filter" -cf backup.tar.cix .
tar -C restored -I "$filter" -xf backup.tar.cix
```

Do not pass that GNU-tar shell-quoted value to libarchive. Pass the raw,
absolute wrapper pathname as the first argument to `libarchive_program_filter`;
its adapter applies libarchive's different quoting rules itself.

The interface carries tar bytes in one CIX archive. It does not support tar
update, append, delete, concatenate, or random access in place. Tar preserves
empty archives, multiple members, and member metadata.

The wrapper permits only empty, `-d`, `--encode`, or `--decode` arguments;
diagnostics stay on stderr and any CIX error returns a nonzero child status.
The C consumer limits command construction to 4096 bytes and exported host
diagnostics to 512 bytes. libarchive owns its child and exposes neither
structured child stderr nor CIX-style process RSS/time controls; CIX's memory
limit applies to CIX and the caller must impose host-level process limits.

`qualify.sh` is not automatic. It covers `tar -I`, stdin/stdout pipelines,
empty and multiple-member archives, exact restoration, and truncated-decoder
failure. It requires GNU tar 1.28+ (record the actual version), a selected CIX
binary, and POSIX utilities. The C example additionally requires libarchive
development headers and linker metadata that declare both program-filter APIs.
The current machine has runtime-only libarchive evidence, so this directory is
not host-qualified yet. The script writes a bounded receipt directory (or the
new directory named by `CIX_TAR_RECEIPT_DIR`) with command text, identities,
three archive artifacts capped at 16 MiB, SHA-256 hashes, and the first 4096
bytes of the expected decoder-failure diagnostic. It never deletes that receipt.

After root provisions dependencies, run:

```sh
cd rust/cix-native/adapters/libarchive
CIX_TAR_CIX=/absolute/prefix/bin/cix ./qualify.sh
cc -std=c11 -Wall -Wextra -Werror libarchive_program_filter.c \
  $(pkg-config --cflags --libs libarchive) -o libarchive-program-filter
./libarchive-program-filter /absolute/path/cix-tar-filter.sh \
  /absolute/prefix/bin/cix archive.tar.cix
tar -I "'/absolute/path/cix-tar-filter.sh'" -tf archive.tar.cix
```

Keep the GNU tar, libarchive header/runtime, CIX binary, command, resource,
archive-hash, exact-restore, and failure receipts together.
