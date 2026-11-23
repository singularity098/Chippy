//! Compiles the Audio Overload SDK engines from third_party/aosdk.
use std::path::PathBuf;

const SOURCES: &[&str] = &[
    "ao_notes.c",
    "corlett.c",
    "psfmain.c",
    "eng_dsf/aica.c",
    "eng_dsf/aicadsp.c",
    "eng_dsf/arm7.c",
    "eng_dsf/arm7i.c",
    "eng_dsf/dc_hw.c",
    "eng_dsf/eng_dsf.c",
    "eng_psf/eng_psf.c",
    "eng_psf/eng_psf2.c",
    "eng_psf/eng_spu.c",
    "eng_psf/peops/spu.c",
    "eng_psf/peops2/dma2.c",
    "eng_psf/peops2/registers2.c",
    "eng_psf/peops2/spu2.c",
    "eng_psf/psx.c",
    "eng_psf/psx_hw.c",
    "eng_qsf/eng_qsf.c",
    "eng_qsf/kabuki.c",
    "eng_qsf/qsound.c",
    "eng_qsf/z80.c",
    "eng_qsf/z80dasm.c",
    "eng_ssf/eng_ssf.c",
    "eng_ssf/m68kcpu.c",
    "eng_ssf/m68kopac.c",
    "eng_ssf/m68kopdm.c",
    "eng_ssf/m68kopnz.c",
    "eng_ssf/m68kops.c",
    "eng_ssf/sat_hw.c",
    "eng_ssf/scsp.c",
    "eng_ssf/scspdsp.c",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../../third_party/aosdk");
    println!("cargo:rerun-if-changed={}", src.display());
    let mut b = cc::Build::new();
    b.include(&src)
        .include(src.join("eng_ssf"))
        .include(src.join("eng_qsf"))
        .include(src.join("eng_dsf"))
        .define("HAS_PSXCPU", "1")
        .warnings(false)
        .flag_if_supported("-fno-strict-aliasing")
        .flag_if_supported("-Wno-implicit-function-declaration")
        .flag_if_supported("-Wno-int-conversion")
        .flag_if_supported("-Wno-incompatible-pointer-types");
    if std::env::var("CARGO_CFG_TARGET_ENDIAN").as_deref() == Ok("big") {
        b.define("WORDS_BIGENDIAN", "1");
    }
    if std::env::var("CARGO_CFG_TARGET_POINTER_WIDTH").as_deref() == Ok("64")
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
    {
        b.define("LONG_IS_64BIT", "1");
    }
    if let Some(z) = std::env::var_os("DEP_Z_INCLUDE") {
        b.include(z);
    }
    for s in SOURCES {
        b.file(src.join(s));
    }
    b.compile("aosdk");
}
