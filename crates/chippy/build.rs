fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/main.slint", config).expect("Slint UI failed to compile");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            mingw_static_runtime();
        }
    }
}

/// MinGW: the emulators' C++ runtime (and the threads library it uses)
/// linked into chippy.exe, which then needs only Windows' own DLLs. rustc
/// would link it as a DLL among the libraries (.cargo/config.toml stops the
/// cc crate asking, gme-sys does not), and lists MinGW's C runtime before
/// these arguments, so the C runtime is named again, in one group with them.
fn mingw_static_runtime() {
    for arg in [
        "-Wl,--start-group",
        "-l:libstdc++.a",
        "-l:libwinpthread.a",
        "-lmingwex",
        "-lmingw32",
        "-lmsvcrt",
        "-lgcc",
        "-lgcc_eh",
        "-lkernel32",
        "-luser32",
        "-ladvapi32",
        "-Wl,--end-group",
    ] {
        println!("cargo:rustc-link-arg={arg}");
    }
}

/// Builds chippy.exe's icon (resource 1, which the window also uses),
/// version info (shown in Properties and the uninstall list) and manifest.
fn windows_resources() {
    use std::path::PathBuf;
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = manifest.join("../../packaging/windows");
    let dir = dir.canonicalize().unwrap_or(dir);
    println!(
        "cargo:rerun-if-changed={}",
        dir.join("chippy.ico").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        dir.join("chippy.exe.manifest").display()
    );

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    let numeric = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);
    // windres and rc.exe both take forward slashes, and neither wants
    // backslashes unescaped.
    let path = |f: &str| dir.join(f).display().to_string().replace('\\', "/");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Chippy"
      VALUE "FileDescription", "Chippy"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "chippy"
      VALUE "LegalCopyright", "GPL-3.0-or-later"
      VALUE "OriginalFilename", "chippy.exe"
      VALUE "ProductName", "Chippy"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = path("chippy.ico"),
        manifest = path("chippy.exe.manifest"),
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("chippy.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
