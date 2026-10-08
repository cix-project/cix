Name:           cix
Version:        0.1.0
Release:        1%{?dist}
Summary:        Lossless command-line compressor
License:        MIT AND Apache-2.0
Source0:        %{name}-%{version}.tar.gz
%global cix_source_root rust/cix-native
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  cmake
BuildRequires:  gcc-c++
BuildRequires:  zlib-devel
BuildRequires:  bzip2-devel
BuildRequires:  xz-devel
BuildRequires:  libzstd-devel
BuildRequires:  brotli-devel
BuildRequires:  lz4-devel
BuildRequires:  snappy-devel

%description
CIX creates and restores lossless, self-describing CIX archives. The base
package installs one native executable; uncix and cixcat are command aliases.
Optional host adapters and full-engine bridge providers are not implied.

%package devel
Summary:        Native CIX development headers and libraries
Requires:       %{name}%{?_isa} = %{version}-%{release}

%description devel
CIX headers, shared and static native libraries, CMake package metadata,
pkg-config metadata, and a small C embedding example. This is the process-free
native SDK, not an unqualified external-host adapter or full-engine bridge
bundle.

%prep
%autosetup -n cix-%{version}

%build
CARGO_TARGET_DIR=$PWD/target cargo build --manifest-path %{cix_source_root}/Cargo.toml --release --locked --lib --bin cix
cmake -S %{cix_source_root}/packaging/sdk -B cix-sdk-build \
  -DCIX_NATIVE_LIBRARY="$PWD/target/release/libcix_native.so" \
  -DCIX_NATIVE_STATIC_LIBRARY="$PWD/target/release/libcix_native.a" \
  -DCMAKE_INSTALL_PREFIX=%{_prefix} \
  -DCMAKE_INSTALL_LIBDIR=%{_lib} \
  -DCMAKE_INSTALL_DOCDIR=share/doc/%{name}-devel

%install
install -Dpm 0755 target/release/cix %{buildroot}%{_bindir}/cix
ln -s cix %{buildroot}%{_bindir}/uncix
ln -s cix %{buildroot}%{_bindir}/cixcat
install -Dpm 0644 %{cix_source_root}/man/cix.1 %{buildroot}%{_mandir}/man1/cix.1
install -Dpm 0644 %{cix_source_root}/man/uncix.1 %{buildroot}%{_mandir}/man1/uncix.1
install -Dpm 0644 %{cix_source_root}/man/cixcat.1 %{buildroot}%{_mandir}/man1/cixcat.1
DESTDIR=%{buildroot} cmake --install cix-sdk-build

%files
%license LICENSE THIRD_PARTY_NOTICES.txt THIRD_PARTY_NOTICES_RUST.md
%doc README.md
%{_bindir}/cix
%{_bindir}/uncix
%{_bindir}/cixcat
%{_mandir}/man1/cix.1*
%{_mandir}/man1/uncix.1*
%{_mandir}/man1/cixcat.1*

%files devel
%{_includedir}/cix.h
%{_includedir}/cix.hpp
%{_includedir}/cix_stream.h
%{_includedir}/cix_formats.h
%{_includedir}/cix_dictionary.h
%{_libdir}/libcix_native.so
%{_libdir}/libcix_native.a
%{_libdir}/pkgconfig/cix-native.pc
%{_libdir}/cmake/CIX/CIXConfig.cmake
%{_libdir}/cmake/CIX/CIXConfigVersion.cmake
%doc %{_docdir}/%{name}-devel/README.md
%doc %{_docdir}/%{name}-devel/examples/dictionary.c

%changelog
* Sun Oct 05 2026 maldous <matthew.aldous@gmail.com> - 0.1.0-1
- Initial native source package.
