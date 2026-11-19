//! Builds libvgmstream statically from the vendored submodule, with
//! mpg123 (MPEG audio), libvorbis (Vorbis) and FFmpeg (Opus, ATRAC3/3plus,
//! XMA, WMA, AAC and more in game formats; also Ogg Opus files). These come
//! from the system through pkg-config and are linked dynamically; for
//! Android, from the static builds of `android/build-deps.sh` found through
//! `CHIPPY_ANDROID_DEPS`.
use std::path::{Path, PathBuf};

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
    println!("cargo:rerun-if-env-changed=CHIPPY_ANDROID_DEPS");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        let deps = std::env::var_os("CHIPPY_ANDROID_DEPS")
            .map(PathBuf::from)
            .expect(
                "set CHIPPY_ANDROID_DEPS to the prefix android/build-deps.sh built the codecs into",
            );
        build_android(&root, &deps);
        return;
    }

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

    let dst = build(cmake::Config::new(&root));
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

/// Configures and builds the library target with every codec on.
fn build(mut cfg: cmake::Config) -> PathBuf {
    for (opt, _) in DEPS {
        cfg.define(opt, "ON");
    }
    cfg.define("USE_CELT", "OFF")
        .define("USE_SPEEX", "OFF")
        .define("BUILD_STATIC", "OFF")
        .define("BUILD_CLI", "OFF")
        .define("BUILD_V123", "OFF")
        .define("BUILD_AUDACIOUS", "OFF")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .profile("Release")
        .build_target("libvgmstream")
        .build()
}

/// Android: the codecs are static libraries under `deps`; CMake (with the
/// NDK's toolchain, which cmake-rs picks) and pkg-config look only there.
fn build_android(root: &Path, deps: &Path) {
    let pc = deps.join("lib/pkgconfig");
    std::env::set_var("PKG_CONFIG_LIBDIR", &pc);
    std::env::set_var("PKG_CONFIG_PATH", &pc);
    std::env::remove_var("PKG_CONFIG_SYSROOT_DIR");
    let mut cfg = cmake::Config::new(root);
    cfg.define("CMAKE_FIND_ROOT_PATH", deps)
        .define("CMAKE_PREFIX_PATH", deps)
        .define("MPG123_DIR", deps)
        .define("CMAKE_FIND_ROOT_PATH_MODE_PACKAGE", "BOTH");
    let dst = build(cfg);
    println!(
        "cargo:rustc-link-search=native={}",
        dst.join("build/src").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        deps.join("lib").display()
    );
    for l in [
        "vgmstream",
        "avformat",
        "avcodec",
        "swresample",
        "avutil",
        "mpg123",
        "vorbisfile",
        "vorbis",
        "ogg",
    ] {
        println!("cargo:rustc-link-lib=static={l}");
    }
    println!("cargo:rustc-link-lib=m");
}
