//! Chippy on the desktop: the command line, then the window.

// No console window behind Chippy's on Windows (--help and --version print
// to the one they were typed in, if any).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    attach_console();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut opts = chippy::StartOptions::default();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--fullscreen") => opts.fullscreen = Some(true),
            Some("--windowed") => opts.fullscreen = Some(false),
            Some("--version") => {
                println!("chippy {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("--help" | "-h") => {
                println!(
                    "Usage: chippy [--fullscreen|--windowed] [FILE or FOLDER]\n\n\
                     A FILE plays at once (with the rest of its folder, unless turned off in\n\
                     Settings); a FOLDER opens in the file browser. Paths may run into archives\n\
                     and multi-track files: chippy ~/music/nes.zip/MegaMan2.nsfe\n\n\
                     Environment:\n  CHIPPY_HOME     keep settings below this folder\n  \
                     SLINT_BACKEND   e.g. winit-software when no OpenGL is available"
                );
                return Ok(());
            }
            _ => opts.open = Some(std::path::PathBuf::from(arg)),
        }
    }
    chippy::run(opts)
}

/// A GUI program on Windows has no console of its own: borrow the one it
/// was started from, if any, so --help, --version and the log reach it.
#[cfg(windows)]
fn attach_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
