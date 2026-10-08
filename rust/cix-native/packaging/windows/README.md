# Windows source-build recipe

Install a native Rust toolchain and a C++17 compiler. A practical source-build
route is vcpkg with a matching native triplet:

```powershell
vcpkg install zlib bzip2 liblzma zstd brotli --triplet x64-windows
$env:VCPKG_ROOT = "C:\\path\\to\\vcpkg"
./packaging/windows/build.ps1
```

The script adds the selected vcpkg include and library directories to the
current process before invoking Cargo. It does not establish that every
library name or ABI is compatible with the selected compiler; that must be
verified by the native build and test receipt.

The script builds one `cix.exe` executable and the native library. It does not
create a Windows installer or duplicate `uncix.exe`/`cixcat.exe` aliases; that
layout needs a Windows staging qualification.

The script builds source only. It does not download, sign, package, or claim
verified Windows binaries. A clean-machine round-trip and malformed-archive
test are required before publishing an archive, MSI, winget manifest, or Scoop
manifest.
