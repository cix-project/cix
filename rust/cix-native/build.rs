fn main() {
    // An explicit build-time dependency prefix supports private/packaged SDKs.
    // This never changes the runtime loader's search policy.
    println!("cargo:rerun-if-env-changed=CIX_NATIVE_LIBRARY_DIR");
    if let Some(directory) = std::env::var_os("CIX_NATIVE_LIBRARY_DIR") {
        let directory = std::path::PathBuf::from(directory);
        assert!(
            directory.is_absolute() && directory.is_dir(),
            "CIX_NATIVE_LIBRARY_DIR must name an existing absolute directory"
        );
        println!("cargo:rustc-link-search=native={}", directory.display());
    }
    println!("cargo:rerun-if-changed=src/zpaq_ffi.cpp");
    println!("cargo:rerun-if-changed=vendor/libzpaq715/libzpaq.cpp");
    println!("cargo:rerun-if-changed=vendor/libzpaq715/libzpaq.h");

    // libzpaq is vendored from the pinned 7.15 source snapshot.  Compile it
    // as portable C++ rather than inheriting the upstream `-march=native`
    // Makefile default: CIX's optional CPU acceleration must remain a runtime
    // decision and the ordinary archive decoder must run on the host baseline.
    let target = std::env::var("TARGET").unwrap_or_default();
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        // ZPAQ archive headers carry executable ZPAQL programs. Keeping the
        // interpreter avoids RWX JIT pages, works on hardened W^X systems,
        // and preserves portable decoder behaviour. It does not change the
        // compressed format or its level-5 model semantics.
        .define("NOJIT", None)
        .flag_if_supported("-O3")
        .flag_if_supported("-fno-strict-aliasing")
        .include("vendor/libzpaq715")
        .file("vendor/libzpaq715/libzpaq.cpp")
        .file("src/zpaq_ffi.cpp");
    if target.contains("windows") {
        // libzpaq's non-Unix random-number helper uses CryptoAPI even though
        // CIX disables its optional JIT with NOJIT.
        println!("cargo:rustc-link-lib=advapi32");
    } else {
        // libzpaq uses this historical macro to select POSIX facilities.
        // Do not define it for Windows targets: that would select mmap and
        // /dev/urandom paths which are unavailable there.
        build.define("unix", None);
    }
    build.compile("cix_zpaq715");
    // `cc` can omit this metadata when the static archive is referenced from
    // a Rust extern block. The final linker still needs the C++ runtime for
    // the bridge's exception and RTTI implementation. MSVC supplies it via
    // its normal toolchain; Unix targets use their platform runtime.
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }

    println!("cargo:rerun-if-changed=src/bsc_ffi.rs");
    println!("cargo:rerun-if-changed=vendor/libbsc/VERSION");
    println!("cargo:rerun-if-changed=vendor/libbsc/libbsc");
    // libbsc's optional OpenMP/CUDA paths are intentionally disabled. CIX
    // owns coarse-grained scheduling, so nested codec worker pools would make
    // the global resource budget inaccurate and reduce throughput under load.
    let mut bsc = cc::Build::new();
    bsc.cpp(true)
        .std("c++17")
        .flag_if_supported("-O3")
        .flag_if_supported("-fno-strict-aliasing")
        .include("vendor/libbsc/libbsc")
        .file("vendor/libbsc/libbsc/adler32/adler32.cpp")
        .file("vendor/libbsc/libbsc/bwt/bwt.cpp")
        .file("vendor/libbsc/libbsc/bwt/libsais/libsais.c")
        .file("vendor/libbsc/libbsc/coder/coder.cpp")
        .file("vendor/libbsc/libbsc/coder/qlfc/qlfc.cpp")
        .file("vendor/libbsc/libbsc/coder/qlfc/qlfc_model.cpp")
        .file("vendor/libbsc/libbsc/filters/detectors.cpp")
        .file("vendor/libbsc/libbsc/filters/preprocessing.cpp")
        .file("vendor/libbsc/libbsc/libbsc/libbsc.cpp")
        .file("vendor/libbsc/libbsc/lzp/lzp.cpp")
        .file("vendor/libbsc/libbsc/platform/platform.cpp")
        .file("vendor/libbsc/libbsc/st/st.cpp")
        .compile("cix_bsc");
}
