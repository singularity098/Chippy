//! Compiles the GBA core of mGBA (as trimmed for GSF players) and Chippy's
//! shim.
use std::path::{Path, PathBuf};

fn c_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            c_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "c") {
            out.push(p);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../third_party/mgba-gsf");
    println!("cargo:rerun-if-changed=shim.c");
    println!("cargo:rerun-if-changed=build.rs");
    let mut files = Vec::new();
    c_files(&root.join("src"), &mut files);
    files.push(manifest.join("shim.c"));
    let mut b = cc::Build::new();
    b.include(root.join("include"))
        .include(root.join("src"))
        .define("__BUILD_MGBA__", None)
        .define("DISABLE_THREADING", None)
        .define("MINIMAL_CORE", "3")
        .define("M_CORE_GBA", None)
        .define("MGBA_STANDALONE", None)
        .define("HAVE_STRDUP", None)
        .define("HAVE_STRNDUP", None)
        .flag_if_supported("-std=gnu11")
        .flag_if_supported("-fno-strict-aliasing")
        .warnings(false)
        .opt_level(3)
        .files(&files);
    // locale_t is POSIX; Windows has neither it nor uselocale.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        b.define("HAVE_LOCALE", None);
        b.define("HAVE_SETLOCALE", None);
    }
    b.compile("mgba_gsf");
}
