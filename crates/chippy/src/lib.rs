//! Chippy: a chiptune-focused music player with a file browser instead of a
//! library. The desktop binary (`main.rs`) and the Android library
//! (`crates/chippy-android`) both start it from here.

mod app;
mod art;
mod audio;
#[cfg(not(target_os = "android"))]
mod desktop;
mod export;
mod icons;
mod media;
mod player;
mod spectrum;

#[cfg(target_os = "android")]
pub mod android;

slint::include_modules!();

pub use app::StartOptions;

/// Opens the window and runs until it is closed.
pub fn run(opts: StartOptions) -> anyhow::Result<()> {
    #[cfg(not(target_os = "android"))]
    desktop::select_backend(chippy_core::settings::Settings::load().native_wayland);
    let ui = MainWindow::new()?;
    icons::install(&ui);
    // CHIPPY_MOBILE=1 tries the phone's layout on the desktop.
    if cfg!(target_os = "android") || std::env::var_os("CHIPPY_MOBILE").is_some() {
        use slint::ComponentHandle;
        ui.global::<Theme>().set_mobile(true);
    }
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
///
/// On Android this is what reaches 90 or 120 Hz: drawn on demand, Slint's
/// event loop sleeps 10 ms between animation frames and then waits for the
/// display, which halves the frame rate. To save the battery it draws
/// continuously only while something moves (an animation, a scroll, the
/// spectrum of the music playing).
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
        if cfg!(target_os = "android") {
            let Some(ui) = weak.upgrade() else { return };
            let moving = ui.window().has_active_animations() || ui.global::<Now>().get_state() == 1;
            if !moving {
                return;
            }
        }
        let since = last.replace(Instant::now()).elapsed();
        let weak = weak.clone();
        let redraw = move || {
            if let Some(ui) = weak.upgrade() {
                ui.window().request_redraw();
            }
        };
        if since >= MIN_FRAME && !cfg!(target_os = "android") {
            redraw();
        } else {
            // On Android the timer (even one due now) is also what wakes
            // the event loop at once.
            slint::Timer::single_shot(MIN_FRAME.saturating_sub(since), redraw);
        }
    });
    if let Err(e) = result {
        log::warn!("cannot draw every frame ({e:?}); drawing on demand");
    }
}
