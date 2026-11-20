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
/// changed (as Carlita does), while anything moves. Drawn on demand, frames
/// came whenever the clock timer asked for one, out of step with the
/// display: animations stuttered, and on some X servers input showed up only
/// with the next event. The buffer swap waits for the display, so this draws
/// at its rate; frames are still kept at least 4 ms apart should a driver
/// not wait.
///
/// It draws continuously only while something moves (an animation, a
/// scroll, the music playing) and, on the desktop, for a moment after the
/// last input (pointer, keys, wheel); otherwise it draws on demand, so an
/// idle window costs no CPU. (Changes that come without input, such as a
/// blinking text cursor, are drawn on demand.) On Android this is also what
/// reaches 90 or 120 Hz: drawn on demand, Slint's event loop sleeps 10 ms
/// between animation frames and then waits for the display, which halves
/// the frame rate.
/// `CHIPPY_LAZY_REDRAW=1` always draws on demand.
fn redraw_every_frame(ui: &MainWindow) {
    use slint::ComponentHandle;
    use std::cell::Cell;
    use std::time::{Duration, Instant};
    const MIN_FRAME: Duration = Duration::from_micros(4_150);
    // How long the desktop keeps drawing every frame after the last input.
    const GRACE: Duration = Duration::from_millis(1500);
    let weak = ui.as_weak();
    let last = Cell::new(Instant::now());
    let result = ui.window().set_rendering_notifier(move |state, _| {
        if !matches!(state, slint::RenderingState::AfterRendering) {
            return;
        }
        let Some(ui) = weak.upgrade() else { return };
        let moving = ui.window().has_active_animations() || ui.global::<Now>().get_state() == 1;
        let now = Instant::now();
        let recent_input = !cfg!(target_os = "android")
            && LAST_INPUT.with(|t| now.duration_since(t.get()) < GRACE);
        if !moving && !recent_input {
            return;
        }
        let since = last.replace(now).elapsed();
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

/// Lowers the calling thread's priority, for work in the background
/// (reading tags, which can mean running a whole track through its
/// emulator; album art; searching; exporting): the player thread and the
/// window come first, so a busy moment does not starve the sound.
pub(crate) fn background_thread() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: plain system calls on this thread; nice values are per thread
    // on Linux.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
}

thread_local! {
    /// When the user last moved the pointer, pressed or turned something.
    static LAST_INPUT: std::cell::Cell<std::time::Instant> =
        std::cell::Cell::new(std::time::Instant::now());
}

/// Notes input: the window draws every frame for a moment after it.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub(crate) fn note_input() {
    LAST_INPUT.with(|t| t.set(std::time::Instant::now()));
}
