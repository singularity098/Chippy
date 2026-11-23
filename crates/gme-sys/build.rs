//! Builds libgme statically from the vendored submodule. Compressed VGZ files
//! are inflated in Rust, so libgme is built without zlib.
//!
//! The submodule is built from a copy with a few lines added ([`PATCHES`]):
//! hook calls for the register writes libgme has no hook points for, so
//! Chippy can see the notes of AY tunes, KSS's SCC and FM, and NSF
//! cartridge sound chips.
use std::path::{Path, PathBuf};

/// (file in `gme/`, text found once, text put in its place).
const PATCHES: &[(&str, &str, &str)] = &[
    // The end of each block of emulation, so a write's time within its
    // block (below) can be placed in the tune.
    (
        "Classic_Emu.cpp",
        "\t\t\tRETURN_ERR( run_clocks( clocks_emulated, msec ) );",
        "\t\t\tRETURN_ERR( run_clocks( clocks_emulated, msec ) );\n\t\t\tGME_BLOCK_HOOK( clocks_emulated, clock_rate_ );",
    ),
    // The APU writes libgme already hooks, given their time.
    (
        "Hes_Emu.cpp",
        "GME_APU_HOOK( this, addr - apu.start_addr, data );",
        "GME_APU_HOOK_T( time(), addr - apu.start_addr, data );",
    ),
    (
        "gb_cpu_io.h",
        "GME_APU_HOOK( this, addr - Gb_Apu::start_addr, data );",
        "GME_APU_HOOK_T( clock(), addr - Gb_Apu::start_addr, data );",
    ),
    (
        "Sap_Emu.cpp",
        "GME_APU_HOOK( this, addr - Sap_Apu::start_addr, data );",
        "GME_APU_HOOK_T( time() & time_mask, addr - Sap_Apu::start_addr, data );",
    ),
    (
        "Sap_Emu.cpp",
        "GME_APU_HOOK( this, addr - 0x10 - Sap_Apu::start_addr + 10, data );",
        "GME_APU_HOOK_T( time() & time_mask, addr - 0x10 - Sap_Apu::start_addr + 10, data );",
    ),
    (
        "Kss_Emu.cpp",
        "GME_APU_HOOK( &emu, emu.ay_latch, data );",
        "GME_APU_HOOK_T( time, emu.ay_latch, data );",
    ),
    (
        "Kss_Emu.cpp",
        "GME_APU_HOOK( &emu, 16, data );",
        "GME_APU_HOOK_T( time, 16, data );",
    ),
    // AY: the Spectrum's and the Amstrad CPC's AY writes (the CPC's with
    // bit 8 set, as its AY runs at another clock).
    (
        "Ay_Emu.cpp",
        "\t\t\tspectrum_mode = true;\n\t\t\tapu.write( time, apu_addr, data );",
        "\t\t\tspectrum_mode = true;\n\t\t\tGME_APU_HOOK_T( time, apu_addr, data );\n\t\t\tapu.write( time, apu_addr, data );",
    ),
    (
        "Ay_Emu.cpp",
        "\t\t\t\tapu.write( time, apu_addr, cpc_latch );",
        "\t\t\t\tGME_APU_HOOK_T( time, apu_addr | 0x100, cpc_latch );\n\t\t\t\tapu.write( time, apu_addr, cpc_latch );",
    ),
    // KSS: Konami SCC writes (0x100 + register), and every OUT (the port),
    // which includes the FM chips libgme doesn't play.
    (
        "Kss_Emu.cpp",
        "\t\tscc.write( time(), scc_addr, data );",
        "\t\tGME_OTHER_HOOK( time(), 0x100 + scc_addr, data );\n\t\tscc.write( time(), scc_addr, data );",
    ),
    (
        "Kss_Emu.cpp",
        "\tdata &= 0xFF;\n\tKss_Emu& emu = STATIC_CAST(Kss_Emu&,*cpu);",
        "\tdata &= 0xFF;\n\tGME_OTHER_HOOK( time, addr & 0xFF, data );\n\tKss_Emu& emu = STATIC_CAST(Kss_Emu&,*cpu);",
    ),
    // NSF: writes outside the 2A03 (the cartridge sound chips), by address.
    (
        "Nsf_Emu.cpp",
        "void Nsf_Emu::cpu_write_misc( nes_addr_t addr, int data )\n{",
        "void Nsf_Emu::cpu_write_misc( nes_addr_t addr, int data )\n{\n\tGME_OTHER_HOOK( time(), addr, data );",
    ),
];

/// Copies `from` to `to` (leaving out `.git`).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_name() == ".git" {
            continue;
        }
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), &dst).unwrap();
        }
    }
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/game-music-emu");
    if !root.join("CMakeLists.txt").exists() {
        panic!(
            "third_party/game-music-emu is missing: run `git submodule update --init --recursive`"
        );
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=hooks");
    println!("cargo:rerun-if-changed={}", root.join("gme").display());
    let src = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("gme-src");
    let _ = std::fs::remove_dir_all(&src);
    copy_tree(&root, &src);
    for (file, find, put) in PATCHES {
        let path = src.join("gme").join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.matches(find).count(),
            1,
            "{file}: the text to patch isn't there once: {find}"
        );
        std::fs::write(&path, text.replacen(find, put, 1)).unwrap();
    }
    // hooks/config.h switches on libgme's register-write hook points; their
    // other end, hooks.cpp, is built below and linked after libgme.
    let hooks = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("hooks");
    let dst = cmake::Config::new(&src)
        .define("GME_BUILD_SHARED", "OFF")
        .define("GME_BUILD_STATIC", "ON")
        .define("GME_BUILD_TESTING", "OFF")
        .define("GME_BUILD_EXAMPLES", "OFF")
        .define("GME_ZLIB", "OFF")
        .define("GME_ENABLE_UBSAN", "OFF")
        .define("ENABLE_UBSAN", "OFF")
        .define("BUILD_TESTING", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .cxxflag("-DHAVE_CONFIG_H")
        .cxxflag(format!("-I\"{}\"", hooks.display()))
        .profile("Release")
        .build();
    for sub in ["lib", "lib64"] {
        println!("cargo:rustc-link-search=native={}", dst.join(sub).display());
    }
    println!("cargo:rustc-link-lib=static=gme");
    cc::Build::new()
        .cpp(true)
        .file(hooks.join("hooks.cpp"))
        .include(&hooks)
        .compile("chippy_gme_hooks");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else if target.contains("android") {
        println!("cargo:rustc-link-lib=c++_shared");
    } else if target.contains("windows-gnu") {
        // Linked statically by .cargo/config.toml: Windows has no
        // libstdc++ DLL.
    } else if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=stdc++");
    }
}
