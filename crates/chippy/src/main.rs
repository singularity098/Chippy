//! Chippy on the desktop: the command line, then the window.

fn main() -> anyhow::Result<()> {
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
