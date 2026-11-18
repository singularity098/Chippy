//! Compiles libsidplayfp (kode54's 2.x tree, with reSIDfp bundled) and a
//! small C wrapper. The headers autoconf would make are written here.
use std::path::{Path, PathBuf};

fn cpp_files(dir: &Path, out: &mut Vec<PathBuf>, skip: &[&str]) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if skip.contains(&name.as_str()) {
            continue;
        }
        if p.extension().is_some_and(|x| x == "cpp" || x == "cc") {
            out.push(p);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../third_party/libsidplayfp");
    let src = root.join("src");
    if !src.exists() {
        panic!(
            "third_party/libsidplayfp is missing: run `git submodule update --init --recursive`"
        );
    }
    println!("cargo:rerun-if-changed=wrapper.cpp");
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let gen = out.join("gen");
    std::fs::create_dir_all(gen.join("sidplayfp")).unwrap();
    std::fs::create_dir_all(gen.join("residfp")).unwrap();
    std::fs::write(
        gen.join("config.h"),
        "#define HAVE_CXX11 1\n#define HAVE_CXX14 1\n#define HAVE_STRCASECMP 1\n\
         #define HAVE_STRNCASECMP 1\n#define PACKAGE_NAME \"libsidplayfp\"\n\
         #define PACKAGE_VERSION \"2.5.0\"\n#define PACKAGE_URL \"\"\n#define VERSION \"2.5.0\"\n",
    )
    .unwrap();
    let ver = std::fs::read_to_string(src.join("sidplayfp/sidversion.h.in"))
        .unwrap()
        .replace("@LIB_MAJOR@", "2")
        .replace("@LIB_MINOR@", "5")
        .replace("@LIB_LEVEL@", "0")
        .replace("@PACKAGE_VERSION@", "2.5.0");
    std::fs::write(gen.join("sidplayfp/sidversion.h"), ver).unwrap();
    let defs =
        std::fs::read_to_string(src.join("builders/residfp-builder/residfp/siddefs-fp.h.in"))
            .unwrap()
            .replace("@RESID_BRANCH_HINTS@", "1")
            .replace("@HAVE_BUILTIN_EXPECT@", "1")
            .replace("@PACKAGE_VERSION@", "2.5.0")
            .replace("@RESID_INLINING@", "1")
            .replace("@RESID_INLINE@", "inline");
    std::fs::write(gen.join("siddefs-fp.h"), &defs).unwrap();
    std::fs::write(gen.join("residfp/siddefs-fp.h"), &defs).unwrap();
    // The Sidplayer (.mus) drivers need an assembler to build: Chippy does
    // not open .mus files, so they are left empty.
    std::fs::write(gen.join("sidplayer1.bin"), "0\n").unwrap();
    std::fs::write(gen.join("sidplayer2.bin"), "0\n").unwrap();

    let mut files = Vec::new();
    cpp_files(&src, &mut files, &[]);
    for d in [
        "c64",
        "c64/CIA",
        "c64/VIC_II",
        "sidplayfp",
        "sidtune",
        "utils",
    ] {
        cpp_files(&src.join(d), &mut files, &[]);
    }
    cpp_files(&src.join("c64/CPU"), &mut files, &["mos6510debug.cpp"]);
    cpp_files(&src.join("builders/residfp-builder"), &mut files, &[]);
    cpp_files(
        &src.join("builders/residfp-builder/residfp"),
        &mut files,
        &[],
    );
    cpp_files(
        &src.join("builders/residfp-builder/residfp/resample"),
        &mut files,
        &["test.cpp"],
    );
    cpp_files(&src.join("utils/MD5"), &mut files, &[]);
    files.push(manifest.join("wrapper.cpp"));

    let mut b = cc::Build::new();
    b.cpp(true)
        .std("c++14")
        .include(&gen)
        .include(&src)
        .include(src.join("builders/residfp-builder/residfp"))
        .include(src.join("sidplayfp"))
        .define("HAVE_CONFIG_H", "1")
        .warnings(false)
        .opt_level(3)
        .files(&files);
    b.compile("sidplayfp");
}
