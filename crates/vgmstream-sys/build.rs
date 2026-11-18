//! Builds libvgmstream statically from the vendored submodule, with its
//! built-in codecs only. MP3 and Vorbis streams in game formats would need
//! mpg123/libvorbis; plain .mp3/.ogg files are decoded by Symphonia instead.
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/vgmstream");
    if !root.join("CMakeLists.txt").exists() {
        panic!("third_party/vgmstream is missing: run `git submodule update --init --recursive`");
    }
    println!("cargo:rerun-if-changed=build.rs");
    let dst = cmake::Config::new(&root)
        .define("USE_MPEG", "OFF")
        .define("USE_VORBIS", "OFF")
        .define("USE_FFMPEG", "OFF")
        .define("USE_CELT", "OFF")
        .define("USE_SPEEX", "OFF")
        .define("BUILD_CLI", "OFF")
        .define("BUILD_V123", "OFF")
        .define("BUILD_AUDACIOUS", "OFF")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .profile("Release")
        .build_target("libvgmstream")
        .build();
    // The library target is not installed; it is left in the build tree.
    println!(
        "cargo:rustc-link-search=native={}",
        dst.join("build/src").display()
    );
    println!("cargo:rustc-link-lib=static=vgmstream");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=m");
    }
}
