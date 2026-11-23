//! Compiles Mednafen's WonderSwan core (trimmed for WSR) and Chippy's shim.
use std::path::PathBuf;

const SOURCES: &[&str] = &[
    "error.cpp",
    "sound/Blip_Buffer.cpp",
    "wsr_player.cpp",
    "wswan/gfx.cpp",
    "wswan/interrupt.cpp",
    "wswan/main.cpp",
    "wswan/memory.cpp",
    "wswan/sound.cpp",
    "wswan/tcache.cpp",
    "wswan/v30mz.cpp",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../third_party/wsr");
    let src = root.join("mednafen");
    println!("cargo:rerun-if-changed=shim.cpp");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", root.display());
    // The player, then a second copy for finding notes while a song plays
    // (its namespaces, classes and functions renamed).
    for notes in [false, true] {
        let mut b = cc::Build::new();
        b.cpp(true)
            .std("c++20")
            .include(&root)
            .include(&src)
            .warnings(false)
            .opt_level(3);
        let prefix = if notes { "WSRN_" } else { "WSR_" };
        // GME has its own Blip_Buffer classes: keep these apart.
        for c in [
            "Blip_Buffer",
            "Blip_Reader",
            "Blip_Synth",
            "Blip_Synth_",
            "Blip_Synth_Fast_",
            "Silent_Blip_Buffer",
            "blip_eq_t",
        ] {
            b.define(c, format!("{prefix}{c}").as_str());
        }
        if notes {
            for n in ["MDFN_IEN_WSWAN", "rePlayer", "Mednafen"] {
                b.define(n, format!("{n}_notes").as_str());
            }
            b.define("WSR_FN(name)", "wsrn_##name");
        }
        if std::env::var("CARGO_CFG_TARGET_ENDIAN").as_deref() == Ok("little") {
            b.define("LSB_FIRST", None);
        }
        for s in SOURCES {
            b.file(src.join(s));
        }
        b.file(manifest.join("shim.cpp"));
        b.compile(if notes { "wsrnotes" } else { "wsr" });
    }
}
