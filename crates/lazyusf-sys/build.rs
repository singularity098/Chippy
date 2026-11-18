//! Compiles lazyusf2 (a trimmed mupen64plus) from the vendored submodule.
use std::path::PathBuf;

const SOURCES: &[&str] = &[
    "ai/ai_controller.c",
    "api/callbacks.c",
    "debugger/dbg_decoder.c",
    "main/main.c",
    "main/rom.c",
    "main/savestates.c",
    "main/util.c",
    "memory/memory.c",
    "pi/cart_rom.c",
    "pi/pi_controller.c",
    "r4300/cached_interp.c",
    "r4300/cp0.c",
    "r4300/cp1.c",
    "r4300/exception.c",
    "r4300/interupt.c",
    "r4300/mi_controller.c",
    "r4300/pure_interp.c",
    "r4300/r4300.c",
    "r4300/r4300_core.c",
    "r4300/recomp.c",
    "r4300/reset.c",
    "r4300/tlb.c",
    "rdp/rdp_core.c",
    "ri/rdram.c",
    "ri/rdram_detection_hack.c",
    "ri/ri_controller.c",
    "rsp/rsp_core.c",
    "rsp_hle/alist.c",
    "rsp_hle/alist_audio.c",
    "rsp_hle/alist_naudio.c",
    "rsp_hle/alist_nead.c",
    "rsp_hle/audio.c",
    "rsp_hle/cicx105.c",
    "rsp_hle/hle.c",
    "rsp_hle/hvqm.c",
    "rsp_hle/jpeg.c",
    "rsp_hle/memory.c",
    "rsp_hle/mp3.c",
    "rsp_hle/musyx.c",
    "rsp_hle/plugin.c",
    "rsp_hle/re2.c",
    "si/cic.c",
    "si/game_controller.c",
    "si/n64_cic_nus_6105.c",
    "si/pif.c",
    "si/si_controller.c",
    "usf/usf.c",
    "usf/barray.c",
    "usf/resampler.c",
    "vi/vi_controller.c",
];

const DYNAREC_X64: &[&str] = &[
    "assemble.c",
    "gbc.c",
    "gcop0.c",
    "gcop1.c",
    "gcop1_d.c",
    "gcop1_l.c",
    "gcop1_s.c",
    "gcop1_w.c",
    "gr4300.c",
    "gregimm.c",
    "gspecial.c",
    "gtlb.c",
    "regcache.c",
    "rjump.c",
];

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/lazyusf2");
    if !root.join("usf/usf.h").exists() {
        panic!("third_party/lazyusf2 is missing: run `git submodule update --init --recursive`");
    }
    println!("cargo:rerun-if-changed=build.rs");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // The x86-64 recompiler is several times faster than the interpreter;
    // CHIPPY_USF_NO_DYNAREC=1 builds without it.
    let dynarec =
        arch == "x86_64" && os != "windows" && std::env::var_os("CHIPPY_USF_NO_DYNAREC").is_none();
    println!("cargo:rerun-if-env-changed=CHIPPY_USF_NO_DYNAREC");

    let mut rsp = cc::Build::new();
    rsp.include(&root)
        .opt_level(2)
        .warnings(false)
        .file(root.join("rsp_lle/rsp.c"));
    match arch.as_str() {
        "x86_64" => {
            rsp.define("ARCH_MIN_SSE2", None);
        }
        "aarch64" => {
            rsp.define("ARCH_MIN_ARM_NEON", None);
        }
        _ => {}
    }
    rsp.compile("lazyusf_rsp");

    let mut b = cc::Build::new();
    b.include(&root)
        .opt_level(2)
        .warnings(false)
        .flag_if_supported("-fno-strict-aliasing");
    for s in SOURCES {
        b.file(root.join(s));
    }
    if dynarec {
        b.define("DYNAREC", None);
        for s in DYNAREC_X64 {
            b.file(root.join("r4300/x86_64").join(s));
        }
    }
    b.compile("lazyusf");
    if os != "windows" {
        println!("cargo:rustc-link-lib=m");
    }
}
