//! Builds libarchive statically from the vendored submodule, without any of
//! the system libraries it can use: Chippy reads RAR archives with it, and
//! those need none.
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/libarchive");
    if !root.join("CMakeLists.txt").exists() {
        panic!("third_party/libarchive is missing: run `git submodule update --init --recursive`");
    }
    println!("cargo:rerun-if-changed=build.rs");
    let mut c = cmake::Config::new(&root);
    for off in [
        "BUILD_SHARED_LIBS",
        "ENABLE_MBEDTLS",
        "ENABLE_NETTLE",
        "ENABLE_OPENSSL",
        "ENABLE_LIBB2",
        "ENABLE_LZ4",
        "ENABLE_LZO",
        "ENABLE_LZMA",
        "ENABLE_ZSTD",
        "ENABLE_ZLIB",
        "ENABLE_BZip2",
        "ENABLE_LIBXML2",
        "ENABLE_BSDXML",
        "ENABLE_EXPAT",
        "ENABLE_WIN32_XMLLITE",
        "ENABLE_PCREPOSIX",
        "ENABLE_PCRE2POSIX",
        "ENABLE_LIBGCC",
        "ENABLE_CNG",
        "ENABLE_TAR",
        "ENABLE_CPIO",
        "ENABLE_CAT",
        "ENABLE_UNZIP",
        "ENABLE_XATTR",
        "ENABLE_ACL",
        "ENABLE_ICONV",
        "ENABLE_TEST",
        "ENABLE_INSTALL",
    ] {
        c.define(off, "OFF");
    }
    let dst = c
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_DISABLE_FIND_PACKAGE_ZLIB", "ON")
        .define("CMAKE_DISABLE_FIND_PACKAGE_BZip2", "ON")
        .define("CMAKE_DISABLE_FIND_PACKAGE_LibLZMA", "ON")
        .profile("Release")
        .build_target("archive_static")
        .build();
    let lib = dst.join("build/libarchive");
    println!("cargo:rustc-link-search=native={}", lib.display());
    for sub in ["Release", "Debug"] {
        println!("cargo:rustc-link-search=native={}", lib.join(sub).display());
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("msvc") {
        println!("cargo:rustc-link-lib=static=archive_static");
    } else {
        println!("cargo:rustc-link-lib=static=archive");
    }
}
