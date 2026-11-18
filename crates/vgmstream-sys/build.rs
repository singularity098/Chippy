//! Builds libvgmstream statically from the vendored submodule, with
//! mpg123 (MPEG audio), libvorbis (Vorbis) and FFmpeg (Opus, ATRAC3/3plus,
//! XMA, WMA, AAC and more in game formats; also Ogg Opus files). These come
//! from the system through pkg-config and are linked dynamically.
use std::path::PathBuf;

/// pkg-config modules vgmstream needs, and the CMake option each one enables.
const DEPS: &[(&str, &[&str])] = &[
    ("USE_MPEG", &["libmpg123"]),
    ("USE_VORBIS", &["vorbisfile", "vorbis", "ogg"]),
    (
        "USE_FFMPEG",
        &["libavformat", "libavcodec", "libavutil", "libswresample"],
    ),
];

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/vgmstream");
    if !root.join("CMakeLists.txt").exists() {
        panic!("third_party/vgmstream is missing: run `git submodule update --init --recursive`");
    }
    println!("cargo:rerun-if-changed=build.rs");

    // Find every library first so a missing one fails with a clear message
    // instead of silently building without the codec.
    let mut libs = Vec::new();
    for (_, modules) in DEPS {
        for m in *modules {
            match pkg_config::Config::new().cargo_metadata(false).probe(m) {
                Ok(lib) => libs.push(lib),
                Err(e) => panic!(
                    "vgmstream needs `{m}` (mpg123, libvorbis and FFmpeg development files).\n\
                     On Debian/Ubuntu: sudo apt install libmpg123-dev libvorbis-dev \
                     libavformat-dev libavcodec-dev libavutil-dev libswresample-dev\n{e}"
                ),
            }
        }
    }

    let mut cfg = cmake::Config::new(&root);
    for (opt, _) in DEPS {
        cfg.define(opt, "ON");
    }
    let dst = cfg
        .define("USE_CELT", "OFF")
        .define("USE_SPEEX", "OFF")
        .define("BUILD_STATIC", "OFF")
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
    // Its dependencies come after it on the link line.
    for lib in &libs {
        for p in &lib.link_paths {
            println!("cargo:rustc-link-search=native={}", p.display());
        }
        for l in &lib.libs {
            println!("cargo:rustc-link-lib={l}");
        }
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=m");
    }
}
