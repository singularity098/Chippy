//! Chippy: a chiptune-focused music player with a file browser instead of a
//! library.

mod app;
mod art;
mod audio;
mod icons;
mod player;
mod spectrum;

slint::include_modules!();

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut opts = app::StartOptions::default();
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

    let ui = MainWindow::new()?;
    icons::install(&ui);
    if std::env::var_os("CHIPPY_LAZY_REDRAW").is_none() {
        redraw_every_frame(&ui);
    }
    let app = app::App::new(&ui, opts)?;
    ui.run()?;
    app.borrow_mut().shutdown();
    Ok(())
}

/// Draws the window on every display frame, not only when something on it
/// changed (as Carlita does). Drawn on demand, frames came whenever the
/// clock timer asked for one, out of step with the display: animations
/// stuttered, and on some X servers input showed up only with the next
/// event. The buffer swap waits for the display, so this draws at its
/// rate; frames are still kept at least 4 ms apart should a driver not wait.
/// `CHIPPY_LAZY_REDRAW=1` goes back to drawing on demand.
fn redraw_every_frame(ui: &MainWindow) {
    use slint::ComponentHandle;
    use std::cell::Cell;
    use std::time::{Duration, Instant};
    const MIN_FRAME: Duration = Duration::from_micros(4_150);
    let weak = ui.as_weak();
    let last = Cell::new(Instant::now());
    let result = ui.window().set_rendering_notifier(move |state, _| {
        if !matches!(state, slint::RenderingState::AfterRendering) {
            return;
        }
        let since = last.replace(Instant::now()).elapsed();
        let weak = weak.clone();
        let redraw = move || {
            if let Some(ui) = weak.upgrade() {
                ui.window().request_redraw();
            }
        };
        if since >= MIN_FRAME {
            redraw();
        } else {
            slint::Timer::single_shot(MIN_FRAME - since, redraw);
        }
    });
    if let Err(e) = result {
        log::warn!("cannot draw every frame ({e:?}); drawing on demand");
    }
}
