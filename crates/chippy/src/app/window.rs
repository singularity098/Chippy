//! The window: its size, place, maximised or fullscreen (put back at the
//! next start and remembered as they change), and the zoom.

use slint::ComponentHandle;
use std::cell::Cell;
use std::time::Instant;

use super::{ss, App};
use crate::Prefs;

/// The zoom steps Ctrl+plus and Ctrl+minus go through.
const ZOOMS: [f32; 10] = [0.8, 0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.75, 2.0];

/// Where the window is in being put back as it was.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WinPhase {
    /// Not on screen yet.
    Waiting,
    /// Put back at this time. The window manager may still move and size
    /// it for a moment; that is not taken for the user's doing. Whether the
    /// size has been checked again.
    Settling(Instant, bool),
    /// Moves, resizes and maximising are remembered as they happen.
    Ready,
}

thread_local! {
    /// The window moved or changed size (by the user or the window manager).
    pub(super) static WINDOW_CHANGED: Cell<bool> = const { Cell::new(false) };
}

/// Notes that the window moved or changed size.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub(super) fn window_changed() {
    WINDOW_CHANGED.with(|c| c.set(true));
}

impl App {
    pub(super) fn zoom(&self) -> f32 {
        if cfg!(target_os = "android") {
            return 1.0;
        }
        let z = self.settings.zoom;
        if z.is_finite() {
            z.clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1])
        } else {
            1.0
        }
    }

    /// The scale factor the system gives the window (logical to physical
    /// pixels), before the zoom.
    pub(super) fn system_scale(&self) -> f32 {
        if let Some(sf) = std::env::var("SLINT_SCALE_FACTOR")
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|s| *s > 0.0)
        {
            return sf;
        }
        #[cfg(not(target_os = "android"))]
        {
            let mut sf = None;
            self.with_winit(|w| sf = Some(w.scale_factor() as f32));
            if let Some(sf) = sf {
                return sf;
            }
        }
        self.ui().window().scale_factor() / self.zoom()
    }

    /// The remembered size of the window when neither maximised nor
    /// fullscreen (kept in the system's logical pixels, so it does not
    /// change with the zoom).
    pub(super) fn normal_size(&self) -> slint::PhysicalSize {
        let sf = self.system_scale();
        slint::PhysicalSize::new(
            (self.settings.window_width as f32 * sf).round() as u32,
            (self.settings.window_height as f32 * sf).round() as u32,
        )
    }

    /// Draws everything at the system's scale times the zoom: text and
    /// controls grow or shrink, and the layout fills the window anew.
    pub(super) fn apply_zoom(&self) {
        if cfg!(target_os = "android") {
            return;
        }
        let ui = self.ui();
        let w = ui.window();
        let sf = self.system_scale() * self.zoom();
        ui.set_zoom(self.zoom());
        if (w.scale_factor() - sf).abs() < 1e-4 {
            return;
        }
        let size = w.size();
        w.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: sf });
        w.dispatch_event(slint::platform::WindowEvent::Resized {
            size: size.to_logical(sf),
        });
    }

    /// Ctrl+plus (1), Ctrl+minus (-1) and Ctrl+0 (0).
    pub(super) fn zoom_by(&mut self, d: i32) {
        let z = self.zoom();
        let nearest = ZOOMS
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - z).abs().total_cmp(&(b.1 - z).abs()))
            .map_or(2, |(i, _)| i);
        let next = match d {
            0 => 1.0,
            _ => ZOOMS[(nearest as i32 + d.signum()).clamp(0, ZOOMS.len() as i32 - 1) as usize],
        };
        self.settings.zoom = next;
        self.apply_zoom();
        self.sync_prefs();
        self.mark_dirty();
        self.toast(&format!("Zoom {}", zoom_label(next)), false);
    }

    pub(super) fn sync_zoom_pref(&self) {
        self.ui()
            .global::<Prefs>()
            .set_zoom(ss(zoom_label(self.zoom())));
    }

    /// Called on every tick: puts the window back as it was once it is on
    /// screen, then remembers how the user leaves it.
    #[cfg(not(target_os = "android"))]
    pub(super) fn window_tick(&mut self, now: Instant) {
        match self.win_phase {
            WinPhase::Waiting => {
                let mut shown = false;
                self.with_winit(|w| shown = w.is_visible() != Some(false));
                if shown {
                    self.restore_window();
                    self.win_phase = WinPhase::Settling(now, false);
                }
            }
            WinPhase::Settling(t, checked) => {
                let since = now.duration_since(t);
                if !checked && since >= std::time::Duration::from_millis(300) {
                    self.win_phase = WinPhase::Settling(t, true);
                    self.check_restored_size();
                } else if since >= std::time::Duration::from_secs(1) {
                    self.win_phase = WinPhase::Ready;
                    WINDOW_CHANGED.with(|c| c.set(false));
                    self.sync_maximized();
                }
            }
            WinPhase::Ready => {
                if WINDOW_CHANGED.with(|c| c.replace(false)) {
                    self.sync_maximized();
                    let before = (
                        self.settings.maximized,
                        self.settings.window_width,
                        self.settings.window_height,
                        self.settings.window_x,
                        self.settings.window_y,
                    );
                    self.remember_window();
                    let after = (
                        self.settings.maximized,
                        self.settings.window_width,
                        self.settings.window_height,
                        self.settings.window_x,
                        self.settings.window_y,
                    );
                    if before != after {
                        self.mark_dirty();
                    }
                }
            }
        }
    }

    /// The maximise button shows restore while the window is maximised,
    /// however it got there (the button, or the window manager).
    #[cfg(not(target_os = "android"))]
    fn sync_maximized(&self) {
        let ui = self.ui();
        let m = ui.window().is_maximized();
        if ui.get_window_maximized() != m {
            ui.set_window_maximized(m);
        }
    }

    /// Puts the window back as it was: its size and place, maximised, or
    /// fullscreen (over its normal size and place, for leaving it). Done
    /// once the window is on screen: asked before, the window manager may
    /// ignore it (X11 window managers do not maximise a window that is not
    /// mapped yet).
    #[cfg(not(target_os = "android"))]
    fn restore_window(&mut self) {
        self.apply_zoom();
        let ui = self.ui();
        let w = ui.window();
        // The normal size and place first, also under maximised and
        // fullscreen: what the window manager goes back to from them.
        w.set_size(self.normal_size());
        if let Some((x, y)) = self.saved_place() {
            w.set_position(slint::PhysicalPosition::new(x, y));
        }
        if self.settings.maximized {
            w.set_maximized(true);
            ui.set_window_maximized(true);
        }
        if self.settings.fullscreen {
            w.set_fullscreen(true);
        }
    }

    /// On X11 a resize to the preferred size made while the window was
    /// being shown can land after ours: asks again once it has settled.
    #[cfg(not(target_os = "android"))]
    fn check_restored_size(&self) {
        if self.settings.fullscreen || self.settings.maximized {
            return;
        }
        let ui = self.ui();
        let w = ui.window();
        if w.is_maximized() || w.is_fullscreen() {
            return;
        }
        let want = self.normal_size();
        let now = w.size();
        let off = |a: u32, b: u32| a.abs_diff(b) > 3;
        if off(now.width, want.width) || off(now.height, want.height) {
            w.set_size(want);
            if let Some((x, y)) = self.saved_place() {
                w.set_position(slint::PhysicalPosition::new(x, y));
            }
        }
    }

    /// The remembered place, if it is still on one of the screens (one may
    /// have been unplugged since).
    #[cfg(not(target_os = "android"))]
    pub(super) fn saved_place(&self) -> Option<(i32, i32)> {
        let (x, y) = self.settings.window_x.zip(self.settings.window_y)?;
        let mut on_screen = true;
        self.with_winit(|w| {
            let monitors: Vec<_> = w.available_monitors().collect();
            if monitors.is_empty() {
                return;
            }
            // Enough of the window's top left to grab it by.
            let (px, py) = (x + 60, y + 20);
            on_screen = monitors.iter().any(|m| {
                let (p, s) = (m.position(), m.size());
                px >= p.x && py >= p.y && px < p.x + s.width as i32 && py < p.y + s.height as i32
            });
        });
        on_screen.then_some((x, y))
    }
}

fn zoom_label(z: f32) -> String {
    format!("{} %", (z * 100.0).round() as i32)
}
