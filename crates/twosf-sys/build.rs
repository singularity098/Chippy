//! Compiles vio2sf (DeSmuME's Nintendo DS core, trimmed for sound) and
//! Chippy's shim.
use std::path::PathBuf;

const SOURCES: &[&str] = &[
    "armcpu.c",
    "arm_instructions.c",
    "barray.c",
    "bios.c",
    "cp15.c",
    "FIFO.c",
    "GPU.c",
    "isqrt.c",
    "matrix.c",
    "mc.c",
    "MMU.c",
    "NDSSystem.c",
    "resampler.c",
    "state.c",
    "thumb_instructions.c",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../../third_party/vio2sf/desmume");
    println!("cargo:rerun-if-changed=shim.c");
    println!("cargo:rerun-if-changed=build.rs");
    let mut c = cc::Build::new();
    c.include(&src)
        // libopus (linked statically for exporting) has an isqrt32 too.
        .define("isqrt32", "vio2sf_isqrt32")
        .define("isqrt64", "vio2sf_isqrt64")
        .warnings(false)
        .opt_level(3)
        .flag_if_supported("-fno-strict-aliasing")
        .flag_if_supported("-Wno-implicit-function-declaration")
        .flag_if_supported("-Wno-int-conversion")
        .flag_if_supported("-Wno-incompatible-pointer-types");
    for s in SOURCES {
        c.file(src.join(s));
    }
    c.file(manifest.join("shim.c"));
    c.compile("vio2sf");
    cc::Build::new()
        .cpp(true)
        .include(&src)
        .define("isqrt32", "vio2sf_isqrt32")
        .define("isqrt64", "vio2sf_isqrt64")
        .warnings(false)
        .opt_level(3)
        .file(src.join("SPU.cpp"))
        .compile("vio2sf_spu");
}
