//! Builds libxmp statically from the vendored submodule, and Chippy's shim.
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../third_party/libxmp");
    if !root.join("CMakeLists.txt").exists() {
        panic!("third_party/libxmp is missing: run `git submodule update --init --recursive`");
    }
    println!("cargo:rerun-if-changed=shim.c");
    println!("cargo:rerun-if-changed=build.rs");
    let dst = cmake::Config::new(&root)
        .define("BUILD_STATIC", "ON")
        .define("BUILD_SHARED", "OFF")
        .define("WITH_UNIT_TESTS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .profile("Release")
        .build();
    cc::Build::new()
        .include(root.join("include"))
        .define("LIBXMP_STATIC", None)
        .file(manifest.join("shim.c"))
        .compile("cxmp");
    for sub in ["lib", "lib64"] {
        println!("cargo:rustc-link-search=native={}", dst.join(sub).display());
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("msvc") {
        println!("cargo:rustc-link-lib=static=libxmp-static");
    } else {
        println!("cargo:rustc-link-lib=static=xmp");
    }
}
