//! Builds libgme statically from the vendored submodule. Compressed VGZ files
//! are inflated in Rust, so libgme is built without zlib.
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/game-music-emu");
    if !root.join("CMakeLists.txt").exists() {
        panic!(
            "third_party/game-music-emu is missing: run `git submodule update --init --recursive`"
        );
    }
    println!("cargo:rerun-if-changed=build.rs");
    let dst = cmake::Config::new(&root)
        .define("GME_BUILD_SHARED", "OFF")
        .define("GME_BUILD_STATIC", "ON")
        .define("GME_BUILD_TESTING", "OFF")
        .define("GME_BUILD_EXAMPLES", "OFF")
        .define("GME_ZLIB", "OFF")
        .define("GME_ENABLE_UBSAN", "OFF")
        .define("ENABLE_UBSAN", "OFF")
        .define("BUILD_TESTING", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .profile("Release")
        .build();
    for sub in ["lib", "lib64"] {
        println!("cargo:rustc-link-search=native={}", dst.join(sub).display());
    }
    println!("cargo:rustc-link-lib=static=gme");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=stdc++");
    }
}
