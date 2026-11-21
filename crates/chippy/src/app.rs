//! The application: everything between the UI and the engine. The file
//! browser's state, the playlist, playback, and the settings live here; a
//! 60 Hz timer moves the clock, the spectrum and anything that finished in
//! the background (tags, album art) onto the screen.

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chippy_core::decode::midi;
use chippy_core::meta::{self, format_ms, TrackMeta};
use chippy_core::playlist::Playlist as Queue;
use chippy_core::settings::{default_columns, Repeat, Session, Settings};
use chippy_core::vfs::{self, Entry, EntryKind, Node, TrackRef};
use chippy_core::{decode, formats, DecodeOptions};

use crate::art::{self, Picture};
use crate::audio::Output;
use crate::media::{Media, Playback, Request};
use crate::player::{Cmd, Event, Player};
use crate::spectrum::{Spectrum, FFT_LEN};
use crate::{
    Browser, ColumnItem, Crumb, Export, FileItem, Info, Lyrics, MainWindow, MusicDir, Now, PlRow,
    Playlist, Prefs, Tabs, Theme,
};

mod tools;
mod window;
use tools::{ExportState, Picked};
use window::WinPhase;

#[derive(Default)]
pub struct StartOptions {
    pub fullscreen: Option<bool>,
    pub open: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Stopped,
    Playing,
    Paused,
}

fn ss(s: impl AsRef<str>) -> SharedString {
    SharedString::from(s.as_ref())
}

pub struct App {
    ui: slint::Weak<MainWindow>,
    settings: Settings,
    settings_dirty: Option<Instant>,
    win_phase: WinPhase,
    output: Output,
    /// Times the sound ran dry, as last logged.
    underruns: u64,
    player: Player,
    media: Media,

    // ---- file browser
    node: Option<Node>,
    entries: Vec<Entry>,
    /// Indices into `entries` that pass the filter, as listed.
    shown: Vec<usize>,
    filter: String,
    history: Vec<PathBuf>,
    hist_pos: usize,
    files: Rc<VecModel<FileItem>>,
    /// Where the browser last was inside each music folder, so switching
    /// back to one returns there.
    dir_places: HashMap<PathBuf, PathBuf>,
    /// The search below the folder: its words, when it is due to start
    /// (shortly after typing stops), and whether the listing shows its
    /// results. Each search has a number; results of older ones are dropped
    /// and their threads stop.
    search: String,
    search_due: Option<Instant>,
    in_results: bool,
    searching: bool,
    search_gen: Arc<AtomicU64>,
    search_tx: Sender<(u64, Option<Vec<Entry>>)>,
    search_rx: Receiver<(u64, Option<Vec<Entry>>)>,

    // ---- playlist
    queue: Queue,
    queue_gen: u64,
    meta_cache: HashMap<TrackRef, TrackMeta>,
    meta_tx: Sender<(u64, usize, TrackRef, TrackMeta)>,
    meta_rx: Receiver<(u64, usize, TrackRef, TrackMeta)>,
    rows: Rc<VecModel<PlRow>>,
    columns: Rc<VecModel<ColumnItem>>,
    all_columns: Rc<VecModel<ColumnItem>>,

    // ---- playback
    state: State,
    generation: u64,
    started: bool,
    duration_ms: Option<u64>,
    failures: usize,
    /// The track was brought back from the last session, paused: if it
    /// cannot be played, nothing else starts in its place.
    resumed: bool,
    /// Where the track loading now starts.
    start_ms: u64,
    /// The playlist or the place in it changed since the session was saved.
    session_dirty: Option<Instant>,
    session_saved: Instant,
    muted: bool,
    art_tx: Sender<(u64, Option<Picture>)>,
    art_rx: Receiver<(u64, Option<Picture>)>,
    art_for: Option<vfs::Location>,
    spectrum: Spectrum,
    bars: Rc<VecModel<f32>>,
    peaks: Rc<VecModel<f32>>,
    last_tick: Instant,
    toast_until: Option<Instant>,

    // ---- MIDI
    /// The SF2 files of the SoundFont folder.
    soundfonts: Vec<PathBuf>,
    /// Recently used SoundFonts, kept loaded so switching back is instant.
    font_cache: Vec<(PathBuf, Arc<rustysynth::SoundFont>)>,
    /// Bumped for each load, so only the last one asked for is used.
    font_request: Arc<AtomicU64>,
    font_tx: Sender<FontLoaded>,
    font_rx: Receiver<FontLoaded>,

    // ---- dialogs
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pick_tx: Sender<Picked>,
    pick_rx: Receiver<Picked>,
    /// A file picker is showing (one at a time).
    picker_open: bool,
    export: ExportState,
    export_tx: Sender<crate::export::Progress>,
    export_rx: Receiver<crate::export::Progress>,
    lyrics: tools::LyricsState,
    tabs: tools::TabsState,
}

/// A SoundFont finished loading (or failed to).
struct FontLoaded {
    path: PathBuf,
    result: Result<Arc<rustysynth::SoundFont>, String>,
}

macro_rules! on {
    ($app:expr, $target:expr, $method:ident, |$a:ident $(, $arg:ident : $t:ty)*| $body:expr) => {{
        let weak = Rc::downgrade(&$app);
        $target.$method(move |$($arg: $t),*| {
            if let Some(app) = weak.upgrade() {
                match app.try_borrow_mut() {
                    Ok(mut guard) => {
                        let $a: &mut App = &mut guard;
                        $body;
                    }
                    Err(_) => log::warn!("ignored a re-entrant call to {}", stringify!($method)),
                }
            }
        });
    }};
}

impl App {
    pub fn new(ui: &MainWindow, opts: StartOptions) -> Result<Rc<RefCell<App>>> {
        let mut settings = Settings::load();
        if let Some(fs) = opts.fullscreen {
            settings.fullscreen = fs;
        }
        let output = Output::open();
        output.shared.lock().unwrap().volume = settings.volume;
        let player = Player::spawn(
            output.shared.clone(),
            output.rate,
            settings.decode_options(output.rate),
        );
        let (meta_tx, meta_rx) = crossbeam_channel::unbounded();
        let (art_tx, art_rx) = crossbeam_channel::unbounded();
        let bands = settings.spectrum_bars as usize;
        let (font_tx, font_rx) = crossbeam_channel::unbounded();
        let (pick_tx, pick_rx) = crossbeam_channel::unbounded();
        let (export_tx, export_rx) = crossbeam_channel::unbounded();
        let (search_tx, search_rx) = crossbeam_channel::unbounded();

        let app = App {
            ui: ui.as_weak(),
            settings,
            settings_dirty: None,
            win_phase: WinPhase::Waiting,
            output,
            underruns: 0,
            player,
            media: Media::connect(),
            node: None,
            entries: Vec::new(),
            shown: Vec::new(),
            filter: String::new(),
            history: Vec::new(),
            hist_pos: 0,
            files: Rc::new(VecModel::default()),
            dir_places: HashMap::new(),
            search: String::new(),
            search_due: None,
            in_results: false,
            searching: false,
            search_gen: Arc::new(AtomicU64::new(0)),
            search_tx,
            search_rx,
            queue: Queue::default(),
            queue_gen: 0,
            meta_cache: HashMap::new(),
            meta_tx,
            meta_rx,
            rows: Rc::new(VecModel::default()),
            columns: Rc::new(VecModel::default()),
            all_columns: Rc::new(VecModel::default()),
            state: State::Stopped,
            generation: 0,
            started: false,
            duration_ms: None,
            failures: 0,
            resumed: false,
            start_ms: 0,
            session_dirty: None,
            session_saved: Instant::now(),
            muted: false,
            art_tx,
            art_rx,
            art_for: None,
            spectrum: Spectrum::new(bands),
            bars: Rc::new(VecModel::from(vec![0.0; bands])),
            peaks: Rc::new(VecModel::from(vec![0.0; bands])),
            last_tick: Instant::now(),
            toast_until: None,
            soundfonts: Vec::new(),
            font_cache: Vec::new(),
            font_request: Arc::new(AtomicU64::new(0)),
            font_tx,
            font_rx,
            pick_tx,
            pick_rx,
            picker_open: false,
            export: ExportState::default(),
            lyrics: Default::default(),
            tabs: Default::default(),
            export_tx,
            export_rx,
        };
        let app = Rc::new(RefCell::new(app));
        Self::wire(&app, ui);
        {
            let mut a = app.borrow_mut();
            a.init_ui();
            a.scan_soundfonts();
            a.load_soundfont();
            let start = a.home_dir();
            a.navigate(&start, true);
            if let Some(p) = opts.open {
                a.open_external(&p);
            } else if a.settings.restore_session {
                a.restore_session();
            }
        }
        // The clock: the position, the spectrum and the meter move on every
        // tick.
        // With nothing moving it ticks only 20 times a second, for what
        // finishes in the background, so an idle window costs no CPU.
        let timer = Rc::new(slint::Timer::default());
        let weak = Rc::downgrade(&app);
        let weak_timer = Rc::downgrade(&timer);
        let busy = std::cell::Cell::new(true);
        timer.start(slint::TimerMode::Repeated, TICK_BUSY, move || {
            let Some(app) = weak.upgrade() else { return };
            let Ok(mut a) = app.try_borrow_mut() else {
                return;
            };
            let now_busy = a.tick();
            if now_busy != busy.replace(now_busy) {
                if let Some(t) = weak_timer.upgrade() {
                    t.set_interval(if now_busy { TICK_BUSY } else { TICK_IDLE });
                }
            }
        });
        // Keep the timer alive as long as the window.
        std::mem::forget(timer);
        Ok(app)
    }

    fn ui(&self) -> MainWindow {
        self.ui.upgrade().expect("window")
    }

    // ------------------------------------------------------------- wiring

    fn wire(app: &Rc<RefCell<App>>, ui: &MainWindow) {
        let b = ui.global::<Browser>();
        on!(app, b, on_activate, |a, i: i32| a.activate(i as usize));
        on!(app, b, on_add, |a, i: i32| a.add_to_queue(i as usize));
        on!(app, b, on_back, |a| a.go_history(-1));
        on!(app, b, on_forward, |a| a.go_history(1));
        on!(app, b, on_up, |a| a.go_up());
        on!(app, b, on_refresh, |a| a.refresh());
        on!(app, b, on_home, |a| {
            let h = a.home_dir();
            a.navigate(&h, true);
        });
        on!(app, b, on_go, |a, t: SharedString| {
            let p = expand(&t);
            a.navigate(&p, true);
        });
        on!(app, b, on_set_filter, |a, t: SharedString| {
            a.filter = t.to_string();
            a.apply_filter();
        });
        on!(app, b, on_go_music_dir, |a, i: i32| a.go_music_dir(i));
        on!(app, b, on_set_search, |a, t: SharedString| a.set_search(&t));
        on!(app, b, on_run_search, |a| {
            if a.search_due.is_some() {
                a.start_search();
            }
        });
        on!(app, b, on_toggle_show_all, |a| {
            let v = !a.settings.show_all_files;
            a.set_flag("show-all", v);
        });

        let p = ui.global::<Playlist>();
        on!(app, p, on_play, |a, i: i32| {
            if i >= 0 {
                a.play_index(i as usize);
            }
        });
        on!(app, p, on_remove, |a, i: i32| a.remove_from_queue(i));
        on!(app, p, on_clear, |a| a.clear_queue());
        on!(app, p, on_reveal, |a, i: i32| a.reveal(i));
        on!(app, p, on_toggle_column, |a, id: SharedString| a
            .toggle_column(&id));
        on!(app, p, on_resize_column, |a, id: SharedString, w: f32| a
            .resize_column(&id, w));
        on!(app, p, on_show_info, |a, i: i32| a.show_info(i));
        on!(app, p, on_export, |a, i: i32| a.open_export(i));
        on!(app, p, on_open_file, |a| a.pick_open());
        on!(app, p, on_reset_columns, |a| {
            a.settings.columns = default_columns();
            a.columns_changed();
        });

        let n = ui.global::<Now>();
        on!(app, n, on_play_pause, |a| a.play_pause());
        on!(app, n, on_stop, |a| a.stop());
        on!(app, n, on_next, |a| a.next(false));
        on!(app, n, on_previous, |a| a.previous());
        on!(app, n, on_seek, |a, f: f32| {
            if let Some(d) = a.duration_ms {
                a.seek((d as f64 * f as f64) as u64);
            }
        });
        on!(app, n, on_set_volume, |a, v: f32| a.set_volume(v));
        on!(app, n, on_toggle_mute, |a| {
            a.muted = !a.muted;
            a.apply_volume();
        });
        on!(app, n, on_toggle_shuffle, |a| {
            a.settings.shuffle = !a.settings.shuffle;
            a.queue.set_shuffle(a.settings.shuffle);
            a.ui().global::<Now>().set_shuffle(a.settings.shuffle);
            a.update_now();
            a.mark_dirty();
        });
        on!(app, n, on_cycle_repeat, |a| {
            a.settings.repeat = a.settings.repeat.next();
            a.ui().global::<Now>().set_repeat(a.settings.repeat.index());
            a.update_now();
            a.mark_dirty();
        });
        {
            let weak = Rc::downgrade(app);
            n.on_time_at(move |f| {
                let d = weak
                    .upgrade()
                    .and_then(|a| a.try_borrow().ok().and_then(|a| a.duration_ms));
                ss(format_ms((d.unwrap_or(0) as f64 * f as f64) as u64))
            });
        }

        let s = ui.global::<Prefs>();
        on!(app, s, on_add_music_dir, |a, t: SharedString| {
            if !t.trim().is_empty() {
                a.add_music_dir(expand(&t));
            }
        });
        on!(app, s, on_remove_music_dir, |a, i: i32| a
            .remove_music_dir(i));
        on!(app, s, on_set_home_dir, |a, i: i32| {
            if let Some(d) = a.settings.music_dirs.get(i.max(0) as usize) {
                a.settings.music_dir = Some(d.clone());
                a.mark_dirty();
                a.sync_prefs();
            }
        });
        on!(app, s, on_use_browser_dir, |a| {
            if let Some(n) = &a.node {
                // A folder inside an archive cannot be a home: use the
                // archive's folder.
                let mut node = n.clone();
                while !matches!(node, Node::Dir(_)) {
                    match node.parent() {
                        Some(p) => node = p,
                        None => break,
                    }
                }
                a.add_music_dir(node.vpath());
            }
        });
        on!(app, s, on_set_flag, |a, id: SharedString, v: bool| a
            .set_flag(&id, v));
        on!(app, s, on_step, |a, id: SharedString, d: i32| a
            .step(&id, d));
        on!(app, s, on_set_soundfont_dir, |a, t: SharedString| {
            let p = expand(&t);
            if p.is_dir() {
                a.set_soundfont_dir(p);
            } else {
                a.toast(&format!("{} is not a folder", tilde(&p)), true);
                a.sync_prefs();
            }
        });
        on!(app, s, on_soundfont_use_browser_dir, |a| {
            if let Some(Node::Dir(d)) = &a.node {
                let d = d.clone();
                a.set_soundfont_dir(d);
            } else {
                a.toast("SoundFonts have to be in a folder on disk", true);
            }
        });
        on!(app, s, on_pick_soundfont, |a, i: i32| a.pick_soundfont(i));
        on!(app, s, on_choose_dir, |a, which: SharedString| a
            .choose_dir(&which));

        let info = ui.global::<Info>();
        on!(app, info, on_close, |a| a.close_info());
        let ex = ui.global::<Export>();
        on!(app, ex, on_set_format, |a, i: i32| a.set_export_format(i));
        on!(app, ex, on_step_bitrate, |a, d: i32| a
            .step_export_bitrate(d));
        on!(app, ex, on_choose_folder, |a| a.choose_dir("export"));
        on!(app, ex, on_start, |a| a.start_export());
        on!(app, ex, on_cancel, |a| a.cancel_export());
        on!(app, ex, on_close, |a| a.close_export());
        on!(app, n, on_cycle_soundfont, |a, d: i32| a.cycle_soundfont(d));
        on!(app, n, on_show_lyrics, |a| a.show_lyrics());
        on!(app, n, on_show_tabs, |a| a.show_tabs());
        let ly = ui.global::<Lyrics>();
        on!(app, ly, on_close, |a| a.close_lyrics());
        on!(app, ly, on_seek_line, |a, i: i32| a.seek_lyrics_line(i));
        let tb = ui.global::<Tabs>();
        on!(app, tb, on_close, |a| a.close_tabs());
        on!(app, tb, on_step_part, |a, d: i32| a.step_tab_part(d));
        on!(app, tb, on_set_part, |a, i: i32| a.set_tab_part(i));
        on!(app, tb, on_set_tuning, |a, i: i32| a.set_tab_tuning(i));
        on!(app, tb, on_set_width, |a, w: i32| a.set_tab_width(w));
        on!(app, tb, on_seek_line, |a, i: i32| a.seek_tab_line(i));
        on!(app, s, on_set_theme, |a, i: i32| {
            let names: Vec<String> = a
                .ui()
                .global::<Theme>()
                .get_names()
                .iter()
                .map(|s| s.to_string())
                .collect();
            if let Some(n) = names.get(i as usize) {
                a.settings.theme = n.clone();
                a.ui().global::<Theme>().set_index(i);
                a.sync_prefs();
                a.mark_dirty();
            }
        });

        #[cfg(not(target_os = "android"))]
        on!(app, ui, on_start_drag, |a| {
            a.with_winit(|w| {
                let _ = w.drag_window();
            });
            a.release_pointer();
        });
        #[cfg(not(target_os = "android"))]
        on!(app, ui, on_start_resize, |a, dir: SharedString| {
            use slint::winit_030::winit::window::ResizeDirection as D;
            let d = match dir.as_str() {
                "n" => D::North,
                "s" => D::South,
                "e" => D::East,
                "w" => D::West,
                "ne" => D::NorthEast,
                "nw" => D::NorthWest,
                "se" => D::SouthEast,
                _ => D::SouthWest,
            };
            a.with_winit(|w| {
                let _ = w.drag_resize_window(d);
            });
            a.release_pointer();
        });
        on!(app, ui, on_minimize_window, |a| a
            .ui()
            .window()
            .set_minimized(true));
        on!(app, ui, on_toggle_maximize, |a| {
            let ui = a.ui();
            let m = !ui.window().is_maximized();
            if m {
                // The size and place to come back to.
                a.remember_window();
            }
            ui.window().set_maximized(m);
            ui.set_window_maximized(m);
            a.settings.maximized = m;
            a.mark_dirty();
        });
        on!(app, ui, on_close_window, |a| {
            a.remember_window();
            let _ = slint::quit_event_loop();
        });
        on!(app, ui, on_zoom_by, |a, d: i32| a.zoom_by(d));
        on!(app, ui, on_rescaled, |a| a.apply_zoom());
        on!(app, ui, on_toggle_fullscreen, |a| {
            let v = !a.settings.fullscreen;
            a.set_fullscreen(v);
        });
        on!(app, ui, on_seek_by, |a, s: i32| {
            let pos = a.position_ms() as i64 + s as i64 * 1000;
            a.seek(pos.max(0) as u64);
        });
        on!(app, ui, on_volume_by, |a, d: f32| {
            let v = a.settings.volume + d;
            a.set_volume(v);
        });

        // Pointer moves and presses, before any element sees them: a panel
        // brought in by hovering goes when the pointer leaves it.
        #[cfg(not(target_os = "android"))]
        {
            use slint::winit_030::winit::event::{ElementState, WindowEvent};
            use slint::winit_030::winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
            use slint::winit_030::{EventResult, WinitWindowAccessor};
            let weak = ui.as_weak();
            let mut cursor = (0.0f32, 0.0f32);
            let mut modifiers = ModifiersState::empty();
            let mut wheel_zoom = 0.0f32;
            ui.window().on_winit_window_event(move |w, e| {
                // (Slint's scale: the system's times the zoom.)
                let scale = || w.scale_factor();
                if matches!(
                    e,
                    WindowEvent::CursorMoved { .. }
                        | WindowEvent::MouseInput { .. }
                        | WindowEvent::MouseWheel { .. }
                        | WindowEvent::KeyboardInput { .. }
                        | WindowEvent::Touch(_)
                        | WindowEvent::Resized(_)
                        | WindowEvent::Focused(_)
                ) {
                    crate::note_input();
                }
                match e {
                    WindowEvent::CursorMoved { position, .. } => {
                        let s = scale();
                        cursor = (position.x as f32 / s, position.y as f32 / s);
                        CURSOR.with(|c| c.set(cursor));
                        if let Some(ui) = weak.upgrade() {
                            ui.invoke_pointer_moved(cursor.0, cursor.1);
                        }
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        ..
                    } => {
                        if let Some(ui) = weak.upgrade() {
                            // A mouse: lists no longer scroll by dragging.
                            ui.global::<Theme>().set_finger(false);
                            ui.invoke_pointer_pressed(cursor.0, cursor.1);
                        }
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        if let Some(ui) = weak.upgrade() {
                            ui.global::<Theme>().set_finger(false);
                            if modifiers.control_key() && !modifiers.alt_key() {
                                // Ctrl+wheel zooms, a step per notch (a
                                // touchpad's pixels add up to notches).
                                use slint::winit_030::winit::event::MouseScrollDelta;
                                let y = match delta {
                                    MouseScrollDelta::LineDelta(_, y) => *y,
                                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 50.0,
                                };
                                wheel_zoom += y;
                                let steps = wheel_zoom.trunc();
                                if steps != 0.0 {
                                    wheel_zoom -= steps;
                                    ui.invoke_zoom_by(steps as i32);
                                }
                                return EventResult::PreventDefault;
                            }
                        }
                    }
                    WindowEvent::Touch(t) => {
                        // A touch screen comes as touches, not mouse
                        // presses (winit drops the mouse presses the system
                        // makes of them). While fingers are in use, lists
                        // scroll by dragging, as on a phone. Set before
                        // Slint sees the touch.
                        if let Some(ui) = weak.upgrade() {
                            ui.global::<Theme>().set_finger(true);
                            if t.phase == slint::winit_030::winit::event::TouchPhase::Started {
                                let s = scale();
                                ui.invoke_pointer_pressed(
                                    t.location.x as f32 / s,
                                    t.location.y as f32 / s,
                                );
                            }
                        }
                    }
                    WindowEvent::Moved(_) | WindowEvent::Resized(_) => window::window_changed(),
                    WindowEvent::ScaleFactorChanged { .. } => {
                        // The system's scale changed (another screen, or
                        // its settings): the zoom goes on top once Slint has
                        // taken the new scale.
                        let weak = weak.clone();
                        slint::Timer::single_shot(Duration::ZERO, move || {
                            if let Some(ui) = weak.upgrade() {
                                ui.invoke_rescaled();
                            }
                        });
                    }
                    WindowEvent::ModifiersChanged(m) => modifiers = m.state(),
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && modifiers.control_key()
                            && !modifiers.alt_key() =>
                    {
                        // Zoom: Ctrl+plus, Ctrl+minus, Ctrl+0 (here, so
                        // they work whatever has the keyboard focus).
                        let d = match (&event.logical_key, event.physical_key) {
                            (_, PhysicalKey::Code(KeyCode::NumpadAdd)) => Some(1),
                            (_, PhysicalKey::Code(KeyCode::NumpadSubtract)) => Some(-1),
                            (_, PhysicalKey::Code(KeyCode::Numpad0)) => Some(0),
                            (Key::Character(c), _) => match c.as_str() {
                                "+" | "=" => Some(1),
                                "-" | "_" => Some(-1),
                                "0" => Some(0),
                                _ => None,
                            },
                            _ => None,
                        };
                        if let Some(d) = d {
                            if let Some(ui) = weak.upgrade() {
                                ui.invoke_zoom_by(d);
                            }
                            return EventResult::PreventDefault;
                        }
                    }
                    WindowEvent::DroppedFile(path) => {
                        // Files dropped on the window play like files opened
                        // from outside.
                        DROPPED.with(|d| d.borrow_mut().push(path.clone()));
                    }
                    _ => {}
                }
                EventResult::Propagate
            });
        }
        ui.window().on_close_requested({
            let weak = Rc::downgrade(app);
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Ok(mut a) = app.try_borrow_mut() {
                        a.remember_window();
                    }
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
    }

    /// Once the window manager has taken over a move or a resize, the
    /// button's release goes to it, not to us: without one, the element
    /// pressed would keep the pointer and the window would ignore the mouse.
    #[cfg(not(target_os = "android"))]
    fn release_pointer(&self) {
        let (x, y) = CURSOR.with(|c| c.get());
        let position = slint::LogicalPosition::new(x, y);
        let weak = self.ui.clone();
        // After the press being handled now has been delivered.
        slint::Timer::single_shot(Duration::ZERO, move || {
            let Some(ui) = weak.upgrade() else { return };
            let w = ui.window();
            w.dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position,
                button: slint::platform::PointerEventButton::Left,
            });
            w.dispatch_event(slint::platform::WindowEvent::PointerExited);
        });
    }

    #[cfg(not(target_os = "android"))]
    fn with_winit(&self, f: impl FnOnce(&slint::winit_030::winit::window::Window)) {
        use slint::winit_030::WinitWindowAccessor;
        self.ui().window().with_winit_window(f);
    }

    fn init_ui(&mut self) {
        let ui = self.ui();
        ui.global::<Browser>()
            .set_items(ModelRc::from(self.files.clone()));
        let p = ui.global::<Playlist>();
        p.set_rows(ModelRc::from(self.rows.clone()));
        p.set_columns(ModelRc::from(self.columns.clone()));
        p.set_all_columns(ModelRc::from(self.all_columns.clone()));
        let n = ui.global::<Now>();
        n.set_bars(ModelRc::from(self.bars.clone()));
        n.set_peaks(ModelRc::from(self.peaks.clone()));
        n.set_volume(self.settings.volume);
        n.set_shuffle(self.settings.shuffle);
        n.set_repeat(self.settings.repeat.index());
        self.queue.set_shuffle(self.settings.shuffle);
        let theme = ui.global::<Theme>();
        let idx = theme
            .get_names()
            .iter()
            .position(|n| n == self.settings.theme.as_str())
            .unwrap_or(0);
        theme.set_index(idx as i32);
        // The window comes back as it was (its size and place, maximised or
        // fullscreen) once it is on screen: see `window_tick`. Its size is
        // set before it shows, too: otherwise Slint first sizes it to its
        // preferred size, and that resize could land after ours.
        #[cfg(not(target_os = "android"))]
        ui.window().set_size(slint::LogicalSize::new(
            self.settings.window_width as f32,
            self.settings.window_height as f32,
        ));
        ui.set_fullscreen(self.settings.fullscreen);
        ui.set_hover_opens(self.settings.hover_opens_browser);
        ui.global::<Prefs>()
            .set_version(ss(env!("CARGO_PKG_VERSION")));
        ui.global::<Prefs>()
            .set_device(ss(&self.output.device_name));
        ui.global::<Prefs>()
            .set_config_path(ss(chippy_core::settings::Settings::path().to_string_lossy()));
        self.columns_changed();
        self.sync_prefs();
        self.update_now();
    }

    fn sync_prefs(&self) {
        let ui = self.ui();
        let p = ui.global::<Prefs>();
        let s = &self.settings;
        let here = self.node.as_ref().map(Node::vpath);
        let current = here.as_deref().and_then(|h| self.music_dir_of(h));
        let dirs: Vec<MusicDir> = s
            .music_dirs
            .iter()
            .map(|d| MusicDir {
                label: ss(Some(file_name(d))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| tilde(d))),
                path: ss(tilde(d)),
                home: s.music_dir.as_ref() == Some(d),
                current: current == Some(d.as_path()),
            })
            .collect();
        let dirs = ModelRc::new(VecModel::from(dirs));
        p.set_music_dirs(dirs.clone());
        ui.global::<Browser>().set_music_dirs(dirs);
        p.set_browser_dir(ss(here.as_deref().map(tilde).unwrap_or_default()));
        p.set_show_all(s.show_all_files);
        p.set_single_tap(s.single_tap);
        p.set_hover_opens(s.hover_opens_browser);
        p.set_folder_playlist(s.open_folder_as_playlist);
        p.set_restore_session(s.restore_session);
        p.set_embedded_art(s.prefer_embedded_art);
        p.set_usf_hle(s.usf_hle);
        p.set_gme_accuracy(s.gme_accuracy);
        p.set_fullscreen(s.fullscreen);
        p.set_native_wayland(s.native_wayland);
        p.set_default_length(ss(format_ms(s.default_length_s as u64 * 1000)));
        p.set_fade(ss(format!("{} s", s.fade_s)));
        p.set_loops(ss(format!("{}", s.loops)));
        p.set_bars(ss(format!("{}", s.spectrum_bars)));
        self.sync_zoom_pref();
        p.set_soundfont_dir(ss(s
            .soundfont_dir
            .as_ref()
            .map(|d| tilde(d))
            .unwrap_or_default()));
        let names: Vec<SharedString> = self.soundfonts.iter().map(|f| ss(file_name(f))).collect();
        p.set_soundfonts(ModelRc::new(VecModel::from(names)));
        p.set_soundfont(self.soundfont_index().map(|i| i as i32).unwrap_or(-1));
        ui.global::<Now>().set_soundfont(ss(s
            .soundfont
            .as_deref()
            .map(|n| formats::split_name(n).0)
            .unwrap_or("")));
        let theme = ui.global::<Theme>();
        p.set_theme(theme.get_index());
        let b = ui.global::<Browser>();
        b.set_show_all(s.show_all_files);
        b.set_single_tap(s.single_tap);
        ui.set_hover_opens(s.hover_opens_browser);
    }

    fn mark_dirty(&mut self) {
        self.settings_dirty.get_or_insert_with(Instant::now);
    }

    fn save_settings(&mut self) {
        self.settings_dirty = None;
        if let Err(e) = self.settings.save() {
            log::warn!("could not save settings: {e}");
        }
    }

    fn mark_session(&mut self) {
        if self.settings.restore_session {
            self.session_dirty.get_or_insert_with(Instant::now);
        }
    }

    fn save_session(&mut self) {
        self.session_dirty = None;
        self.session_saved = Instant::now();
        if !self.settings.restore_session {
            return;
        }
        let session = Session {
            tracks: self.queue.items.iter().map(|i| i.track.clone()).collect(),
            current: self.queue.current,
            position_ms: match self.state {
                State::Stopped => 0,
                _ if !self.started => self.start_ms,
                _ => self.position_ms(),
            },
        };
        if let Err(e) = session.save() {
            log::warn!("could not save the session: {e}");
        }
    }

    /// Brings back the playlist and track of the last session, paused
    /// where it was. Tracks whose files are gone are left out.
    fn restore_session(&mut self) {
        let Some(session) = Session::load() else {
            return;
        };
        let mut tracks = Vec::new();
        let mut current = None;
        for (i, t) in session.tracks.into_iter().enumerate() {
            let exists = match &t.loc {
                vfs::Location::Fs(p) => p.is_file(),
                vfs::Location::Archive { archive, .. } => archive.is_file(),
            };
            if !exists {
                continue;
            }
            if session.current == Some(i) {
                current = Some(tracks.len());
            }
            tracks.push(t);
        }
        if tracks.is_empty() {
            return;
        }
        self.set_queue(tracks, current);
        if let Some(c) = current {
            self.play_index_at(c, session.position_ms, true);
        }
    }

    fn decode_options(&self) -> DecodeOptions {
        self.settings.decode_options(self.output.rate)
    }

    fn set_flag(&mut self, id: &str, v: bool) {
        let s = &mut self.settings;
        match id {
            "show-all" => {
                s.show_all_files = v;
                self.refresh();
            }
            "single-tap" => s.single_tap = v,
            "hover-opens" => s.hover_opens_browser = v,
            "folder-playlist" => s.open_folder_as_playlist = v,
            "restore-session" => {
                s.restore_session = v;
                if v {
                    self.mark_session();
                } else {
                    self.session_dirty = None;
                    Session::forget();
                }
            }
            "embedded-art" => {
                s.prefer_embedded_art = v;
                self.art_for = None;
                self.load_art();
            }
            "usf-hle" => s.usf_hle = v,
            "gme-accuracy" => s.gme_accuracy = v,
            "native-wayland" => {
                s.native_wayland = v;
                self.sync_prefs();
                self.mark_dirty();
                self.toast("Takes effect when Chippy is next started", false);
                return;
            }
            "fullscreen" => {
                self.set_fullscreen(v);
                return;
            }
            _ => {}
        }
        self.player.send(Cmd::Options(self.decode_options()));
        self.sync_prefs();
        self.mark_dirty();
    }

    fn step(&mut self, id: &str, d: i32) {
        if id == "zoom" {
            self.zoom_by(d);
            return;
        }
        let s = &mut self.settings;
        match id {
            "default-length" => {
                s.default_length_s = (s.default_length_s as i32 + d * 15).clamp(15, 3600) as u32
            }
            "fade" => s.fade_s = (s.fade_s as i32 + d).clamp(0, 30) as u32,
            "loops" => s.loops = (s.loops + d as f32).clamp(1.0, 10.0),
            "bars" => {
                s.spectrum_bars = (s.spectrum_bars as i32 + d * 4).clamp(8, 96) as u32;
                let n = s.spectrum_bars as usize;
                self.spectrum = Spectrum::new(n);
                self.bars.set_vec(vec![0.0; n]);
                self.peaks.set_vec(vec![0.0; n]);
            }
            _ => {}
        }
        self.player.send(Cmd::Options(self.decode_options()));
        self.sync_prefs();
        self.mark_dirty();
    }

    fn set_fullscreen(&mut self, fs: bool) {
        let ui = self.ui();
        if fs {
            self.remember_window();
            ui.window().set_fullscreen(true);
        } else {
            let w = ui.window();
            w.set_fullscreen(false);
            if self.settings.maximized {
                w.set_maximized(true);
                ui.set_window_maximized(true);
            } else {
                w.set_size(self.normal_size());
                #[cfg(not(target_os = "android"))]
                if let Some((x, y)) = self.saved_place() {
                    w.set_position(slint::PhysicalPosition::new(x, y));
                }
            }
        }
        self.settings.fullscreen = fs;
        ui.set_fullscreen(fs);
        self.sync_prefs();
        self.mark_dirty();
    }

    fn remember_window(&mut self) {
        let ui = self.ui();
        let w = ui.window();
        if !self.settings.fullscreen && !w.is_fullscreen() && !w.is_minimized() {
            self.settings.maximized = w.is_maximized();
            if !w.is_maximized() {
                let size = w.size().to_logical(self.system_scale());
                self.settings.window_width = size.width.round() as u32;
                self.settings.window_height = size.height.round() as u32;
                // Wayland does not tell where windows are (it reads 0, 0).
                let p = w.position();
                if !cfg!(target_os = "android") && (p.x != 0 || p.y != 0) {
                    self.settings.window_x = Some(p.x);
                    self.settings.window_y = Some(p.y);
                }
            }
        }
    }

    pub fn shutdown(&mut self) {
        self.settings.volume = self.settings.volume.clamp(0.0, 1.0);
        self.save_settings();
        self.save_session();
        self.player.send(Cmd::Stop);
    }

    fn toast(&mut self, text: &str, error: bool) {
        let ui = self.ui();
        ui.set_toast(ss(text));
        ui.set_toast_error(error);
        self.toast_until =
            Some(Instant::now() + Duration::from_millis(if error { 5000 } else { 2500 }));
    }

    // ----------------------------------------------------------------- MIDI

    fn scan_soundfonts(&mut self) {
        self.soundfonts = self
            .settings
            .soundfont_dir
            .as_ref()
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && formats::extension(&file_name(p)) == "sf2")
                    .collect()
            })
            .unwrap_or_default();
        self.soundfonts.sort_by_key(|p| file_name(p).to_lowercase());
        // The one chosen is gone (or none was): take the first.
        if self.soundfont_index().is_none() {
            self.settings.soundfont = self.soundfonts.first().map(|p| file_name(p));
        }
        self.sync_prefs();
    }

    fn soundfont_index(&self) -> Option<usize> {
        let name = self.settings.soundfont.as_deref()?;
        self.soundfonts.iter().position(|p| file_name(p) == name)
    }

    fn set_soundfont_dir(&mut self, dir: PathBuf) {
        self.settings.soundfont_dir = Some(dir);
        self.settings.soundfont = None;
        self.scan_soundfonts();
        self.load_soundfont();
        self.mark_dirty();
        if self.soundfonts.is_empty() {
            self.toast("No SoundFonts (.sf2) in that folder", true);
        }
    }

    fn pick_soundfont(&mut self, i: i32) {
        let Some(p) = self.soundfonts.get(i.max(0) as usize) else {
            return;
        };
        self.settings.soundfont = Some(file_name(p));
        self.mark_dirty();
        self.sync_prefs();
        self.load_soundfont();
    }

    fn cycle_soundfont(&mut self, d: i32) {
        let n = self.soundfonts.len() as i32;
        if n == 0 {
            self.toast(
                "Choose a folder of SoundFonts in Settings, under MIDI",
                true,
            );
            return;
        }
        let i = self
            .soundfont_index()
            .map(|i| i as i32)
            .unwrap_or(-d.signum());
        self.pick_soundfont((i + d).rem_euclid(n));
        let name = self.settings.soundfont.clone().unwrap_or_default();
        self.toast(
            &format!("SoundFont: {}", formats::split_name(&name).0),
            false,
        );
    }

    /// Loads the chosen SoundFont in the background and hands it to the
    /// MIDI player, which switches to it even in the middle of a song.
    fn load_soundfont(&mut self) {
        let id = self.font_request.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(path) = self.soundfont_index().map(|i| self.soundfonts[i].clone()) else {
            midi::set_soundfont(None);
            self.ui()
                .global::<Prefs>()
                .set_soundfont_status(SharedString::new());
            return;
        };
        if let Some(i) = self.font_cache.iter().position(|(p, _)| *p == path) {
            let entry = self.font_cache.remove(i);
            midi::set_soundfont(Some(entry.1.clone()));
            self.font_cache.push(entry);
            self.ui()
                .global::<Prefs>()
                .set_soundfont_status(SharedString::new());
            return;
        }
        midi::soundfont_loading();
        self.ui()
            .global::<Prefs>()
            .set_soundfont_status(ss(format!("Loading {}…", file_name(&path))));
        let (tx, latest) = (self.font_tx.clone(), self.font_request.clone());
        std::thread::Builder::new()
            .name("soundfont".into())
            .spawn(move || {
                crate::background_thread();
                let result = midi::load_soundfont(&path).map_err(|e| format!("{e:#}"));
                // Only the last SoundFont asked for plays.
                if latest.load(Ordering::SeqCst) == id {
                    match &result {
                        Ok(f) => midi::set_soundfont(Some(f.clone())),
                        Err(_) => midi::set_soundfont(None),
                    }
                }
                let _ = tx.send(FontLoaded { path, result });
            })
            .ok();
    }

    fn font_loaded(&mut self, f: FontLoaded) {
        let ui = self.ui();
        let p = ui.global::<Prefs>();
        match f.result {
            Ok(font) => {
                self.font_cache.retain(|(p, _)| *p != f.path);
                self.font_cache.push((f.path, font));
                // Large SoundFonts take a lot of memory: keep a few.
                if self.font_cache.len() > 3 {
                    self.font_cache.remove(0);
                }
                p.set_soundfont_status(SharedString::new());
            }
            Err(e) => {
                p.set_soundfont_status(ss(&e));
                self.toast(&e, true);
            }
        }
    }

    // -------------------------------------------------------- file browser

    fn home_dir(&self) -> PathBuf {
        self.settings
            .music_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    /// The music folder a path is in (the deepest, should they nest).
    fn music_dir_of(&self, p: &Path) -> Option<&Path> {
        self.settings
            .music_dirs
            .iter()
            .filter(|d| p.starts_with(d))
            .max_by_key(|d| d.components().count())
            .map(PathBuf::as_path)
    }

    fn add_music_dir(&mut self, p: PathBuf) {
        if !p.is_dir() {
            self.toast(&format!("{} is not a folder", tilde(&p)), true);
            return;
        }
        if self.settings.music_dirs.contains(&p) {
            self.toast(&format!("{} is already a music folder", tilde(&p)), false);
            return;
        }
        self.settings.music_dirs.push(p);
        if self.settings.music_dir.is_none() {
            self.settings.music_dir = self.settings.music_dirs.first().cloned();
        }
        self.mark_dirty();
        self.sync_prefs();
        self.toast("Music folder added", false);
    }

    fn remove_music_dir(&mut self, i: i32) {
        if i < 0 || i as usize >= self.settings.music_dirs.len() {
            return;
        }
        let d = self.settings.music_dirs.remove(i as usize);
        self.dir_places.remove(&d);
        if self.settings.music_dir.as_ref() == Some(&d) {
            self.settings.music_dir = self.settings.music_dirs.first().cloned();
        }
        self.mark_dirty();
        self.sync_prefs();
    }

    /// Switches the browser to a music folder: back to where it was in it,
    /// or to its top if it is there already.
    fn go_music_dir(&mut self, i: i32) {
        let Some(d) = self.settings.music_dirs.get(i.max(0) as usize).cloned() else {
            return;
        };
        let here = self.node.as_ref().map(Node::vpath);
        let inside = here.as_deref().and_then(|h| self.music_dir_of(h)) == Some(d.as_path());
        let target = if inside {
            d.clone()
        } else {
            self.dir_places
                .get(&d)
                .cloned()
                .unwrap_or_else(|| d.clone())
        };
        if !self.navigate(&target, true) && target != d {
            self.navigate(&d, true);
        }
    }

    /// Shows a virtual path in the browser. Returns false when it cannot.
    fn navigate(&mut self, vpath: &Path, push: bool) -> bool {
        let node = match vfs::resolve(vpath) {
            Ok(n) => n,
            Err(e) => {
                self.set_status(&format!("{e:#}"), true);
                return false;
            }
        };
        let entries = match vfs::list(&node, self.settings.show_all_files, &self.decode_options()) {
            Ok(e) => e,
            Err(e) => {
                self.set_status(&format!("Cannot open {}: {e:#}", vpath.display()), true);
                return false;
            }
        };
        let came_from = self.node.as_ref().map(Node::vpath);
        let vp = node.vpath();
        if push
            && self
                .history
                .get(self.hist_pos)
                .map(|h| h != &vp)
                .unwrap_or(true)
        {
            self.history.truncate(self.hist_pos + 1);
            self.history.push(vp.clone());
            self.hist_pos = self.history.len() - 1;
        }
        if let Some(d) = self.music_dir_of(&vp).map(Path::to_path_buf) {
            self.dir_places.insert(d, vp.clone());
        }
        self.node = Some(node);
        self.entries = entries;
        self.filter.clear();
        self.end_search();
        let ui = self.ui();
        let b = ui.global::<Browser>();
        b.set_filter(SharedString::new());
        b.set_search(SharedString::new());
        b.set_folder_serial(b.get_folder_serial() + 1);
        self.apply_filter();
        // Coming up out of a folder: select the folder we left.
        let sel = came_from
            .and_then(|from| {
                self.shown
                    .iter()
                    .position(|&i| from.starts_with(&self.entries[i].vpath))
            })
            .map(|i| i as i32)
            .unwrap_or(-1);
        b.set_selected(sel);
        self.sync_prefs();
        true
    }

    fn refresh(&mut self) {
        if self.in_results {
            self.start_search();
            return;
        }
        if let Some(n) = self.node.clone() {
            let sel = self.ui().global::<Browser>().get_selected();
            let filter = self.filter.clone();
            if self.navigate(&n.vpath(), false) {
                if !filter.is_empty() {
                    self.filter = filter.clone();
                    self.ui().global::<Browser>().set_filter(ss(&filter));
                    self.apply_filter();
                }
                self.ui()
                    .global::<Browser>()
                    .set_selected(sel.min(self.shown.len() as i32 - 1));
            }
        }
    }

    fn go_up(&mut self) {
        if let Some(p) = self.node.as_ref().and_then(Node::parent) {
            self.navigate(&p.vpath(), true);
        }
    }

    fn go_history(&mut self, d: i32) {
        let pos = self.hist_pos as i32 + d;
        if pos < 0 || pos as usize >= self.history.len() {
            return;
        }
        let p = self.history[pos as usize].clone();
        if self.navigate(&p, false) {
            self.hist_pos = pos as usize;
            self.update_nav_buttons();
        }
    }

    fn update_nav_buttons(&self) {
        let ui = self.ui();
        let b = ui.global::<Browser>();
        b.set_can_back(self.hist_pos > 0);
        b.set_can_forward(self.hist_pos + 1 < self.history.len());
        b.set_can_up(self.node.as_ref().and_then(Node::parent).is_some());
    }

    fn set_status(&self, text: &str, error: bool) {
        let ui = self.ui();
        let b = ui.global::<Browser>();
        b.set_status(ss(text));
        b.set_status_error(error);
    }

    fn apply_filter(&mut self) {
        self.relist(false);
    }

    /// Lists the entries that pass the filter; `keep_selection` while
    /// search results come in.
    fn relist(&mut self, keep_selection: bool) {
        self.shown = (0..self.entries.len())
            .filter(|&i| vfs::matches_filter(&self.entries[i].name, &self.filter))
            .collect();
        self.rebuild_files();
        let ui = self.ui();
        let b = ui.global::<Browser>();
        if keep_selection {
            b.set_selected(b.get_selected().min(self.shown.len() as i32 - 1));
        } else {
            b.set_selected(if self.filter.is_empty() || self.shown.is_empty() {
                -1
            } else {
                0
            });
        }
        if let Some(n) = &self.node {
            let vp = n.vpath();
            b.set_path(ss(tilde(&vp)));
            b.set_title(ss(n.display_name()));
            let crumbs: Vec<Crumb> = crumbs(&vp)
                .into_iter()
                .map(|(label, path, sep)| Crumb {
                    label: ss(label),
                    path: ss(path.to_string_lossy()),
                    sep,
                })
                .collect();
            b.set_crumbs(ModelRc::new(VecModel::from(crumbs)));
        }
        self.update_nav_buttons();
        let (mut dirs, mut files, mut tracks) = (0, 0, 0);
        for &i in &self.shown {
            match self.entries[i].kind {
                EntryKind::Dir | EntryKind::Archive => dirs += 1,
                EntryKind::Track => tracks += 1,
                _ => files += 1,
            }
        }
        let mut parts = Vec::new();
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        if dirs > 0 {
            parts.push(plural(dirs, "folder", "folders"));
        }
        if files > 0 {
            parts.push(plural(files, "file", "files"));
        }
        if tracks > 0 {
            parts.push(plural(tracks, "track", "tracks"));
        }
        let mut status = parts.join(" · ");
        if self.in_results {
            let shown = self.shown.len();
            status = if self.searching {
                format!("Searching… {status}")
            } else if self.entries.is_empty() {
                format!(
                    "Nothing below this folder matches \"{}\"",
                    self.search.trim()
                )
            } else if !self.filter.is_empty() {
                format!("{status} found, {shown} matching the filter")
            } else {
                format!("{status} found below this folder")
            };
            if self.entries.len() >= SEARCH_LIMIT {
                status = format!("{status} (the first {SEARCH_LIMIT})");
            }
        } else if !self.filter.is_empty() {
            status = format!(
                "{status} matching \"{}\" (of {})",
                self.filter,
                self.entries.len()
            );
        }
        self.set_status(&status, false);
    }

    // ------------------------------------------------------------ search

    fn set_search(&mut self, text: &str) {
        self.search = text.to_string();
        if text.trim().is_empty() {
            let was = self.in_results;
            self.end_search();
            if was {
                // Back to the folder, keeping the filter.
                if let Some(n) = &self.node {
                    if let Ok(e) =
                        vfs::list(n, self.settings.show_all_files, &self.decode_options())
                    {
                        self.entries = e;
                    }
                }
                self.apply_filter();
            }
        } else {
            self.search_due = Some(Instant::now() + Duration::from_millis(300));
        }
    }

    /// Stops any search; the listing is left as it is.
    fn end_search(&mut self) {
        self.search_gen.fetch_add(1, Ordering::SeqCst);
        self.search_due = None;
        self.searching = false;
        self.in_results = false;
        let ui = self.ui();
        let b = ui.global::<Browser>();
        b.set_searching(false);
        b.set_in_results(false);
    }

    fn start_search(&mut self) {
        self.search_due = None;
        let Some(node) = self.node.clone() else {
            return;
        };
        let query = self.search.trim().to_string();
        if query.is_empty() {
            return;
        }
        let gen = self.search_gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.in_results = true;
        self.searching = true;
        self.entries.clear();
        let ui = self.ui();
        let b = ui.global::<Browser>();
        b.set_in_results(true);
        b.set_searching(true);
        b.set_folder_serial(b.get_folder_serial() + 1);
        self.apply_filter();
        let (tx, current, show_all) = (
            self.search_tx.clone(),
            self.search_gen.clone(),
            self.settings.show_all_files,
        );
        std::thread::spawn(move || {
            crate::background_thread();
            let mut count = 0;
            vfs::search(&node, &query, show_all, &mut |hits| {
                if current.load(Ordering::SeqCst) != gen {
                    return false;
                }
                count += hits.len();
                let _ = tx.send((gen, Some(hits)));
                count < SEARCH_LIMIT
            });
            let _ = tx.send((gen, None));
        });
    }

    /// Takes in what the search found since the last tick.
    fn take_search_results(&mut self) {
        let gen = self.search_gen.load(Ordering::SeqCst);
        let mut changed = false;
        while let Ok((g, hits)) = self.search_rx.try_recv() {
            if g != gen || !self.in_results {
                continue;
            }
            changed = true;
            match hits {
                Some(h) => {
                    let room = SEARCH_LIMIT.saturating_sub(self.entries.len());
                    self.entries.extend(h.into_iter().take(room));
                }
                None => {
                    self.searching = false;
                    self.ui().global::<Browser>().set_searching(false);
                }
            }
        }
        if changed {
            self.relist(true);
        }
    }

    fn current_track(&self) -> Option<&TrackRef> {
        self.queue
            .current
            .and_then(|c| self.queue.items.get(c))
            .map(|i| &i.track)
    }

    fn rebuild_files(&mut self) {
        let playing = self.current_track().cloned();
        let here = self.node.as_ref().map(Node::vpath);
        let items: Vec<FileItem> = self
            .shown
            .iter()
            .map(|&i| {
                let e = &self.entries[i];
                let (stem, ext) = match e.kind {
                    EntryKind::Dir | EntryKind::Track => (e.name.as_str(), ""),
                    _ => formats::split_name(&e.name),
                };
                let kind = match e.kind {
                    EntryKind::Dir => 0,
                    EntryKind::Archive => 1,
                    EntryKind::Audio if e.may_hold_tracks => 3,
                    EntryKind::Audio => 2,
                    EntryKind::Track => 4,
                    EntryKind::Image => 5,
                    EntryKind::Other => 6,
                };
                let detail = match e.kind {
                    // Search results: the folder each was found in.
                    _ if self.in_results => found_in(here.as_deref(), &e.vpath),
                    EntryKind::Track => e.length_ms.map(format_ms).unwrap_or_default(),
                    EntryKind::Image | EntryKind::Other => human_size(e.size),
                    _ => String::new(),
                };
                let is_playing = playing.as_ref().is_some_and(|p| {
                    e.track
                        .as_ref()
                        .is_some_and(|t| t.loc == p.loc && (t.sub.is_none() || t.sub == p.sub))
                });
                FileItem {
                    stem: ss(stem),
                    ext: ss(ext),
                    kind,
                    detail: ss(detail),
                    playing: is_playing,
                }
            })
            .collect();
        self.files.set_vec(items);
    }

    fn activate(&mut self, shown_index: usize) {
        let Some(&i) = self.shown.get(shown_index) else {
            return;
        };
        let e = self.entries[i].clone();
        match e.kind {
            EntryKind::Dir | EntryKind::Archive => {
                self.navigate(&e.vpath, true);
            }
            EntryKind::Audio => {
                if e.may_hold_tracks {
                    let loc = &e.track.as_ref().unwrap().loc;
                    match decode::track_list(loc, &self.decode_options()) {
                        Ok(t) if t.len() > 1 => {
                            self.navigate(&e.vpath, true);
                            return;
                        }
                        Ok(_) => {}
                        Err(err) => {
                            self.toast(&format!("Cannot open {}: {err:#}", e.name), true);
                            return;
                        }
                    }
                }
                self.play_from_listing(i);
            }
            EntryKind::Track => self.play_from_listing(i),
            EntryKind::Image | EntryKind::Other => {
                self.toast(
                    &format!("{} is not something Chippy can play", e.name),
                    true,
                );
            }
        }
    }

    /// Plays entry `i` of the listing, with every other playable entry of
    /// the listing (filter or not) as the playlist, in listed order. A
    /// search result plays with the rest of its own folder.
    fn play_from_listing(&mut self, i: usize) {
        if self.in_results {
            let e = &self.entries[i];
            let folder = e
                .vpath
                .parent()
                .and_then(|p| vfs::resolve(p).ok())
                .and_then(|n| vfs::list(&n, false, &self.decode_options()).ok());
            if let Some(list) = folder {
                let tracks: Vec<TrackRef> = list
                    .iter()
                    .filter(|x| x.kind.is_playable())
                    .filter_map(|x| x.track.clone())
                    .collect();
                if let Some(current) = tracks.iter().position(|t| Some(t) == e.track.as_ref()) {
                    self.set_queue(tracks, Some(current));
                    self.play_index(current);
                    return;
                }
            }
        }
        let mut tracks = Vec::new();
        let mut current = 0;
        for (j, e) in self.entries.iter().enumerate() {
            if let (true, Some(t)) = (e.kind.is_playable(), &e.track) {
                if j == i {
                    current = tracks.len();
                }
                tracks.push(t.clone());
            }
        }
        self.set_queue(tracks, Some(current));
        self.play_index(current);
    }

    fn add_to_queue(&mut self, shown_index: usize) {
        let Some(&i) = self.shown.get(shown_index) else {
            return;
        };
        let e = self.entries[i].clone();
        let tracks: Vec<TrackRef> = match e.kind {
            EntryKind::Audio | EntryKind::Track => e.track.into_iter().collect(),
            EntryKind::Dir | EntryKind::Archive => vfs::resolve(&e.vpath)
                .and_then(|n| vfs::list(&n, false, &self.decode_options()))
                .map(|l| {
                    l.into_iter()
                        .filter(|x| x.kind.is_playable())
                        .filter_map(|x| x.track)
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if tracks.is_empty() {
            self.toast("Nothing to add", true);
            return;
        }
        let n = tracks.len();
        let start = self.queue.len();
        self.queue.append(tracks);
        self.rebuild_rows();
        self.load_meta_from(start);
        self.toast(
            &format!("Added {n} {}", if n == 1 { "track" } else { "tracks" }),
            false,
        );
    }

    /// Plays a file given from outside (command line, drag and drop).
    pub fn open_external(&mut self, path: &Path) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let node = match vfs::resolve(&path) {
            Ok(n) => n,
            Err(e) => {
                self.toast(&format!("{e:#}"), true);
                return;
            }
        };
        match node {
            Node::Dir(_) | Node::ArchiveDir { .. } => {
                self.navigate(&path, true);
            }
            Node::Tracks(loc) => {
                let is_multi = decode::track_list(&loc, &self.decode_options())
                    .map(|t| t.len() > 1)
                    .unwrap_or(false);
                if is_multi {
                    // A multi-track file: queue its tracks.
                    if self.navigate(&path, true) {
                        self.play_from_listing(
                            self.entries
                                .iter()
                                .position(|e| e.kind.is_playable())
                                .unwrap_or(0),
                        );
                    }
                    return;
                }
                if self.settings.open_folder_as_playlist {
                    let parent = loc.parent_node().vpath();
                    if self.navigate(&parent, true) {
                        if let Some(i) = self
                            .entries
                            .iter()
                            .position(|e| e.track.as_ref().is_some_and(|t| t.loc == loc))
                        {
                            self.play_from_listing(i);
                            return;
                        }
                    }
                }
                self.set_queue(vec![TrackRef::file(loc)], Some(0));
                self.play_index(0);
            }
        }
    }

    fn reveal(&mut self, i: i32) {
        let Some(item) = self.queue.items.get(i.max(0) as usize) else {
            return;
        };
        let loc = item.track.loc.clone();
        let sub = item.track.sub;
        let target = if sub.is_some() {
            loc.vpath()
        } else {
            loc.parent_node().vpath()
        };
        if self.navigate(&target, true) {
            if let Some(pos) = self.shown.iter().position(|&j| {
                self.entries[j]
                    .track
                    .as_ref()
                    .is_some_and(|t| t.loc == loc && (sub.is_none() || t.sub == sub))
            }) {
                self.ui().global::<Browser>().set_selected(pos as i32);
            }
            self.ui().invoke_open_left();
        }
    }

    // ------------------------------------------------------------ playlist

    fn set_queue(&mut self, tracks: Vec<TrackRef>, current: Option<usize>) {
        self.queue.replace(tracks, current);
        self.queue.set_shuffle(self.settings.shuffle);
        self.queue_gen += 1;
        self.rebuild_rows();
        self.load_meta_from(0);
    }

    /// Reads tags for the playlist from `start` on, in the background;
    /// cached tags are filled in at once.
    fn load_meta_from(&mut self, start: usize) {
        let mut todo = Vec::new();
        for i in start..self.queue.len() {
            let t = self.queue.items[i].track.clone();
            if let Some(m) = self.meta_cache.get(&t) {
                self.queue.items[i].meta = Some(m.clone());
                self.update_row(i);
            } else {
                todo.push((i, t));
            }
        }
        if todo.is_empty() {
            self.update_summary();
            return;
        }
        // The playing track first.
        if let Some(c) = self.queue.current {
            if let Some(p) = todo.iter().position(|(i, _)| *i == c) {
                let x = todo.remove(p);
                todo.insert(0, x);
            }
        }
        let tx = self.meta_tx.clone();
        let generation = self.queue_gen;
        let opts = self.decode_options();
        std::thread::Builder::new()
            .name("tags".into())
            .spawn(move || {
                crate::background_thread();
                for (i, t) in todo {
                    // A decoder bug on one file must not leave the rest of
                    // the playlist without tags.
                    let m = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        meta::read(&t, &opts)
                    }))
                    .unwrap_or_else(|_| {
                        let mut m = meta::TrackMeta::default();
                        m.fill_gaps(&t);
                        m
                    });
                    if tx.send((generation, i, t, m)).is_err() {
                        return;
                    }
                }
            })
            .ok();
    }

    fn row_for(&self, i: usize) -> PlRow {
        let item = &self.queue.items[i];
        let cells: Vec<SharedString> = self
            .settings
            .columns
            .iter()
            .filter(|c| c.visible)
            .map(|c| match &item.meta {
                Some(m) => ss(m.column(&c.id)),
                None if c.id == "title" => ss(item.track.loc.name()),
                None => SharedString::new(),
            })
            .collect();
        let (title, detail) = match &item.meta {
            Some(m) => {
                let detail: Vec<String> = [m.column("artist"), m.column("length")]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect();
                let title = Some(m.column("title"))
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| item.track.loc.name());
                (ss(title), ss(detail.join(" · ")))
            }
            None => (ss(item.track.loc.name()), SharedString::new()),
        };
        PlRow {
            cells: ModelRc::new(VecModel::from(cells)),
            playing: self.queue.current == Some(i),
            pending: item.meta.is_none(),
            title,
            detail,
        }
    }

    fn rebuild_rows(&mut self) {
        self.mark_session();
        let rows: Vec<PlRow> = (0..self.queue.len()).map(|i| self.row_for(i)).collect();
        self.rows.set_vec(rows);
        let ui = self.ui();
        let p = ui.global::<Playlist>();
        p.set_playing(self.queue.current.map(|c| c as i32).unwrap_or(-1));
        if p.get_selected() >= self.queue.len() as i32 {
            p.set_selected(-1);
        }
        self.update_summary();
        // What comes next may have changed.
        self.update_now();
    }

    fn update_row(&self, i: usize) {
        if i < self.rows.row_count() {
            self.rows.set_row_data(i, self.row_for(i));
        }
    }

    fn update_summary(&self) {
        let n = self.queue.len();
        let total: u64 = self
            .queue
            .items
            .iter()
            .filter_map(|i| i.meta.as_ref()?.length_ms)
            .sum();
        let text = if n == 0 {
            String::new()
        } else {
            format!(
                "{n} {} · {}",
                if n == 1 { "track" } else { "tracks" },
                format_ms(total)
            )
        };
        self.ui().global::<Playlist>().set_summary(ss(text));
    }

    fn remove_from_queue(&mut self, i: i32) {
        if i < 0 || i as usize >= self.queue.len() {
            return;
        }
        let was_current = self.queue.current == Some(i as usize);
        self.queue.remove(i as usize);
        if was_current {
            self.stop();
        }
        self.rebuild_rows();
        self.ui()
            .global::<Playlist>()
            .set_selected(i.min(self.queue.len() as i32 - 1));
    }

    fn clear_queue(&mut self) {
        self.stop();
        self.queue.clear();
        self.queue_gen += 1;
        self.rebuild_rows();
        self.update_now();
        self.rebuild_files();
    }

    fn columns_changed(&mut self) {
        let items = |visible_only: bool| -> Vec<ColumnItem> {
            self.settings
                .columns
                .iter()
                .filter(|c| !visible_only || c.visible)
                .map(|c| ColumnItem {
                    id: ss(&c.id),
                    title: ss(meta::COLUMNS
                        .iter()
                        .find(|x| x.0 == c.id)
                        .map(|x| x.1)
                        .unwrap_or("")),
                    width: c.width,
                    visible: c.visible,
                    right: matches!(
                        c.id.as_str(),
                        "track" | "length" | "samplerate" | "bitrate" | "channels" | "year"
                    ),
                })
                .collect()
        };
        self.columns.set_vec(items(true));
        self.all_columns.set_vec(items(false));
        self.update_others_width();
        self.rebuild_rows();
        self.mark_dirty();
    }

    fn update_others_width(&self) {
        let w: f32 = self
            .settings
            .columns
            .iter()
            .filter(|c| c.visible && c.id != "title")
            .map(|c| c.width)
            .sum();
        self.ui().global::<Playlist>().set_others_width(w);
    }

    fn toggle_column(&mut self, id: &str) {
        let visible = self.settings.columns.iter().filter(|c| c.visible).count();
        if let Some(c) = self.settings.columns.iter_mut().find(|c| c.id == id) {
            if c.visible && visible == 1 {
                return;
            }
            c.visible = !c.visible;
        }
        self.columns_changed();
    }

    fn resize_column(&mut self, id: &str, w: f32) {
        let w = w.clamp(36.0, 1200.0);
        if let Some(c) = self.settings.columns.iter_mut().find(|c| c.id == id) {
            c.width = w;
        }
        for m in [&self.columns, &self.all_columns] {
            for r in 0..m.row_count() {
                let mut item = m.row_data(r).unwrap();
                if item.id == id {
                    item.width = w;
                    m.set_row_data(r, item);
                }
            }
        }
        self.update_others_width();
        self.mark_dirty();
    }

    // ------------------------------------------------------------ playback

    fn play_index(&mut self, i: usize) {
        self.play_index_at(i, 0, false);
    }

    /// Plays track `i` from `start_ms`, or loads it there paused.
    fn play_index_at(&mut self, i: usize, start_ms: u64, paused: bool) {
        if i >= self.queue.len() {
            return;
        }
        self.resumed = paused;
        self.start_ms = start_ms;
        self.mark_session();
        let item = &self.queue.items[i];
        let track = item.track.clone();
        let old = self.queue.current;
        self.queue.current = Some(i);
        self.generation += 1;
        self.started = false;
        self.duration_ms = item.meta.as_ref().and_then(|m| m.length_ms);
        self.player.send(Cmd::Load {
            track,
            generation: self.generation,
            start_ms,
        });
        self.state = if paused {
            State::Paused
        } else {
            State::Playing
        };
        self.output.shared.lock().unwrap().paused = paused;
        if let Some(o) = old {
            if o < self.queue.len() {
                self.update_row(o);
            }
        }
        self.update_row(i);
        let ui = self.ui();
        let p = ui.global::<Playlist>();
        p.set_playing(i as i32);
        p.set_reveal_serial(p.get_reveal_serial() + 1);
        self.update_now();
        self.load_art();
        self.rebuild_files();
    }

    fn play_pause(&mut self) {
        self.mark_session();
        match self.state {
            State::Playing => {
                self.state = State::Paused;
                self.output.shared.lock().unwrap().paused = true;
            }
            State::Paused => {
                self.state = State::Playing;
                self.output.shared.lock().unwrap().paused = false;
            }
            State::Stopped => {
                let sel = self.ui().global::<Playlist>().get_selected();
                let i = if sel >= 0 {
                    Some(sel as usize)
                } else {
                    self.queue
                        .current
                        .or_else(|| self.queue.next(Repeat::Off, false))
                };
                match i {
                    Some(i) => self.play_index(i),
                    None => self.ui().invoke_open_left(),
                }
                return;
            }
        }
        self.update_now();
    }

    fn stop(&mut self) {
        self.player.send(Cmd::Stop);
        self.state = State::Stopped;
        self.started = false;
        self.generation += 1;
        self.output.shared.lock().unwrap().paused = false;
        self.update_now();
    }

    fn next(&mut self, auto: bool) {
        match self.queue.next(self.settings.repeat, auto) {
            Some(i) => self.play_index(i),
            None => {
                self.stop();
            }
        }
    }

    fn previous(&mut self) {
        if self.state != State::Stopped && self.position_ms() > 3000 {
            self.seek(0);
            return;
        }
        if let Some(i) = self.queue.previous(self.settings.repeat) {
            self.play_index(i);
        }
    }

    fn seek(&mut self, ms: u64) {
        if self.state == State::Stopped || !self.started {
            return;
        }
        let ms = self
            .duration_ms
            .map(|d| ms.min(d.saturating_sub(250)))
            .unwrap_or(ms);
        self.player.send(Cmd::Seek { ms });
        self.media.moved();
        self.mark_session();
        // Show the new place at once.
        self.output.shared.lock().unwrap().reset(ms);
    }

    fn position_ms(&self) -> u64 {
        let s = self.output.shared.lock().unwrap();
        if s.generation != self.generation {
            return 0;
        }
        s.base_ms + s.frames_played * 1000 / self.output.rate as u64
    }

    fn set_volume(&mut self, v: f32) {
        self.settings.volume = v.clamp(0.0, 1.0);
        self.muted = false;
        self.apply_volume();
        self.mark_dirty();
    }

    fn apply_volume(&mut self) {
        let v = if self.muted {
            0.0
        } else {
            self.settings.volume
        };
        self.output.shared.lock().unwrap().volume = v;
        self.media.set_volume(self.settings.volume);
        let ui = self.ui();
        let n = ui.global::<Now>();
        n.set_volume(self.settings.volume);
        n.set_muted(self.muted);
    }

    /// The bottom bar's and art panel's text, from the current track.
    fn update_now(&mut self) {
        let ui = self.ui();
        let n = ui.global::<Now>();
        let item = self.queue.current.and_then(|c| self.queue.items.get(c));
        n.set_state(match self.state {
            State::Stopped => 0,
            State::Playing => 1,
            State::Paused => 2,
        });
        n.set_has_track(item.is_some());
        match item {
            Some(item) => {
                let m = item.meta.clone().unwrap_or_else(|| {
                    let mut m = TrackMeta::default();
                    m.fill_gaps(&item.track);
                    m
                });
                n.set_title(ss(&m.title));
                n.set_artist(ss(&m.artist));
                n.set_album(ss(&m.album));
                let mut info = vec![m.format.clone()];
                if !m.system.is_empty() {
                    info.push(m.system.clone());
                }
                if let Some(r) = m.sample_rate {
                    info.push(format!("{:.1} kHz", r as f64 / 1000.0));
                }
                if let Some(b) = m.bitrate_kbps {
                    info.push(format!("{b} kbps"));
                }
                if !m.year.is_empty() {
                    info.push(m.year.clone());
                }
                n.set_info(ss(info
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ·  ")));
                if self.duration_ms.is_none() {
                    self.duration_ms = m.length_ms;
                }
                ui.set_subtitle(ss(tilde(&item.track.loc.vpath())));
                n.set_midi(matches!(
                    formats::kind_of(&item.track.loc.name()),
                    formats::FileKind::Audio(formats::Codec::Midi)
                ));
            }
            None => {
                n.set_title(SharedString::new());
                n.set_artist(SharedString::new());
                n.set_album(SharedString::new());
                n.set_info(SharedString::new());
                ui.set_subtitle(SharedString::new());
                n.set_midi(false);
            }
        }
        if self.state == State::Stopped {
            n.set_position(0.0);
            n.set_elapsed(ss("0:00"));
        }
        n.set_total(ss(self
            .duration_ms
            .map(format_ms)
            .unwrap_or_else(|| "–:––".into())));
        n.set_seekable(self.state != State::Stopped && self.duration_ms.is_some());
        // The phone's "up next" line.
        let up_next = self
            .queue
            .current
            .and_then(|c| {
                self.queue
                    .next(self.settings.repeat, true)
                    .filter(|&i| i != c)
            })
            .and_then(|i| self.queue.items.get(i))
            .map(|item| match &item.meta {
                Some(m) if !m.title.is_empty() => m.title.clone(),
                _ => item.track.loc.name(),
            })
            .unwrap_or_default();
        ui.set_up_next(ss(up_next));
        let (title, artist, album) = (n.get_title(), n.get_artist(), n.get_album());
        self.media.show(&title, &artist, &album, self.duration_ms);
        let pos = self.position_ms();
        self.media.set_playback(
            match self.state {
                State::Stopped => Playback::Stopped,
                State::Playing => Playback::Playing,
                State::Paused => Playback::Paused,
            },
            pos,
        );
    }

    /// Media keys and the desktop's player controls.
    fn media_request(&mut self, r: Request) {
        match r {
            Request::Play => {
                if self.state != State::Playing {
                    self.play_pause();
                }
            }
            Request::Pause => {
                if self.state == State::Playing {
                    self.play_pause();
                }
            }
            Request::Toggle => self.play_pause(),
            Request::Next => self.next(false),
            Request::Previous => self.previous(),
            Request::Stop => self.stop(),
            Request::SeekBy(s) => {
                let pos = self.position_ms() as i64 + s * 1000;
                self.seek(pos.max(0) as u64);
            }
            Request::SeekTo(ms) => self.seek(ms),
            Request::Volume(v) => self.set_volume(v),
            Request::Raise => {
                let ui = self.ui();
                ui.window().set_minimized(false);
                let _ = ui.show();
            }
            Request::Quit => {
                self.remember_window();
                let _ = slint::quit_event_loop();
            }
            Request::Back => {
                // Closes what is open (a panel, the playlist, a dialog);
                // with nothing open the app goes to the background, still
                // playing, as Android players do.
                self.ui().invoke_back();
            }
            Request::Refresh => self.refresh(),
            Request::Folder(which, dir) => {
                let picked = match (which.as_str(), dir) {
                    (_, None) => Picked::Nothing(None),
                    ("music", Some(d)) => Picked::MusicDir(d),
                    ("soundfont", Some(d)) => Picked::SoundfontDir(d),
                    (_, Some(d)) => Picked::ExportDir(d),
                };
                self.picked(picked);
            }
        }
    }

    fn load_art(&mut self) {
        let Some(t) = self.current_track().cloned() else {
            self.art_for = None;
            self.ui().global::<Now>().set_has_art(false);
            self.media.set_art(None);
            return;
        };
        // Tracks of one file share their art: no need to look again.
        if self.art_for.as_ref() == Some(&t.loc) {
            return;
        }
        self.art_for = Some(t.loc.clone());
        art::load(
            t.loc,
            self.settings.prefer_embedded_art,
            self.generation,
            self.art_tx.clone(),
        );
    }

    // --------------------------------------------------------------- clock

    /// Moves everything along; true while something on screen moves (the
    /// clock wants to tick quickly).
    fn tick(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;

        #[cfg(not(target_os = "android"))]
        self.window_tick(now);

        // Files dropped on the window.
        // (Each file arrives as an event of its own, all in one go.)
        let dropped: Vec<PathBuf> = DROPPED.with(|d| d.borrow_mut().drain(..).collect());
        if !dropped.is_empty() {
            self.open_paths(dropped);
        }

        while let Ok(p) = self.pick_rx.try_recv() {
            self.picked(p);
        }
        while let Ok(p) = self.export_rx.try_recv() {
            self.export_progress(p);
        }

        while let Ok(f) = self.font_rx.try_recv() {
            self.font_loaded(f);
        }

        if self.search_due.is_some_and(|t| now >= t) {
            self.start_search();
        }
        self.take_search_results();

        while let Ok(r) = self.media.requests.try_recv() {
            self.media_request(r);
        }

        while let Ok(ev) = self.player.events.try_recv() {
            match ev {
                Event::Started {
                    generation,
                    duration_ms,
                } if generation == self.generation => {
                    self.started = true;
                    self.resumed = false;
                    self.failures = 0;
                    if duration_ms.is_some() {
                        self.duration_ms = duration_ms;
                    }
                    self.update_now();
                }
                Event::Failed { generation, error } if generation == self.generation => {
                    let name = self
                        .current_track()
                        .map(|t| t.loc.name())
                        .unwrap_or_default();
                    self.toast(&format!("Cannot play {name}: {error}"), true);
                    self.failures += 1;
                    if self.resumed {
                        // Brought back from the last session: nothing
                        // starts playing on its own.
                        self.resumed = false;
                        self.failures = 0;
                        self.stop();
                    } else if self.failures < self.queue.len().max(1) {
                        self.next(false);
                    } else {
                        self.failures = 0;
                        self.stop();
                    }
                }
                _ => {}
            }
        }

        let mut meta_changed = false;
        while let Ok((generation, i, track, m)) = self.meta_rx.try_recv() {
            // Cached under the track that was read: the playlist may have
            // changed since, and row `i` may now be another track.
            self.meta_cache.insert(track.clone(), m.clone());
            if let Some(item) = self.queue.items.get_mut(i) {
                if generation == self.queue_gen && item.track == track {
                    item.meta = Some(m);
                    self.update_row(i);
                    meta_changed = true;
                    // The track playing, or the one after it ("up next").
                    let next = self.queue.next(self.settings.repeat, true);
                    if self.queue.current == Some(i) || next == Some(i) {
                        self.update_now();
                    }
                }
            }
        }
        if meta_changed {
            self.update_summary();
        }

        while let Ok((generation, pic)) = self.art_rx.try_recv() {
            let _ = generation;
            let ui = self.ui();
            let n = ui.global::<Now>();
            match pic {
                Some(p) => {
                    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        &p.rgba, p.width, p.height,
                    );
                    n.set_art(Image::from_rgba8(buf));
                    n.set_art_aspect(p.width as f32 / p.height.max(1) as f32);
                    n.set_has_art(true);
                    self.media.set_art(p.small.as_deref());
                }
                None => {
                    n.set_has_art(false);
                    self.media.set_art(None);
                }
            }
        }

        // Track end, position, spectrum.
        let (ended, pos_ms, samples) = {
            let s = self.output.shared.lock().unwrap();
            let current = s.generation == self.generation;
            let ended = current
                && self.started
                && self.state == State::Playing
                && s.producer_done
                && s.buffered_frames() == 0;
            let pos = if current {
                s.base_ms + s.frames_played * 1000 / self.output.rate as u64
            } else {
                0
            };
            let samples = (self.state == State::Playing && current).then(|| s.scope_tail(FFT_LEN));
            if s.underruns != self.underruns {
                log::warn!(
                    "the sound ran dry mid-track ({} times so far): decoding fell behind",
                    s.underruns
                );
                self.underruns = s.underruns;
            }
            (ended, pos, samples)
        };
        if ended {
            self.next(true);
            return true;
        }
        self.sheets_tick(pos_ms);
        let ui = self.ui();
        let n = ui.global::<Now>();
        if self.state != State::Stopped && self.started {
            n.set_elapsed(ss(format_ms(pos_ms)));
            if let Some(d) = self.duration_ms.filter(|&d| d > 0) {
                n.set_position((pos_ms as f64 / d as f64).min(1.0) as f32);
            }
        }
        if samples.is_some() || !self.spectrum.is_still() {
            self.spectrum
                .update(samples.as_deref(), self.output.rate, dt);
            for (i, (&b, &p)) in self
                .spectrum
                .bars
                .iter()
                .zip(&self.spectrum.peaks)
                .enumerate()
            {
                if i < self.bars.row_count() {
                    self.bars.set_row_data(i, b);
                    self.peaks.set_row_data(i, p);
                }
            }
        }

        if self.toast_until.is_some_and(|t| now >= t) {
            self.toast_until = None;
            ui.set_toast(SharedString::new());
        }
        if self
            .settings_dirty
            .is_some_and(|t| now.duration_since(t) > Duration::from_secs(2))
        {
            self.save_settings();
        }
        // The session: soon after a change, and every 15 s while playing
        // (Android may end the app without warning).
        let session_due = self
            .session_dirty
            .is_some_and(|t| now.duration_since(t) > Duration::from_secs(2))
            || (self.state == State::Playing
                && self.settings.restore_session
                && now.duration_since(self.session_saved) > Duration::from_secs(15));
        if session_due {
            self.save_session();
        }
        self.state == State::Playing
            || self.searching
            || self.search_due.is_some()
            || !self.spectrum.is_still()
    }
}

/// At most this many search results are listed.
const SEARCH_LIMIT: usize = 5000;

/// Where a search result is, below the folder searched ("." for right
/// there).
fn found_in(here: Option<&Path>, vpath: &Path) -> String {
    let parent = vpath.parent().unwrap_or(vpath);
    match here.and_then(|h| parent.strip_prefix(h).ok()) {
        Some(p) if p.as_os_str().is_empty() => String::new(),
        Some(p) => p.to_string_lossy().into_owned(),
        None => tilde(parent),
    }
}

/// How often the clock ticks while something moves (8 ms keeps up with
/// 120 Hz screens; at 16 ms the clock and spectrum moved only on every
/// other frame), and while nothing does.
const TICK_BUSY: Duration = Duration::from_millis(8);
const TICK_IDLE: Duration = Duration::from_millis(50);

thread_local! {
    /// Where the pointer is, in logical pixels.
    static CURSOR: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((0.0, 0.0)) };
    static DROPPED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Expands a leading `~` to the home folder.
fn expand(s: &str) -> PathBuf {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('~') {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest.trim_start_matches('/'));
        }
    }
    PathBuf::from(s)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.components().count() > 1)
}

/// A path with the home folder written as `~`.
fn tilde(p: &Path) -> String {
    match home().and_then(|h| p.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.to_string_lossy()),
        None => p.to_string_lossy().into_owned(),
    }
}

/// The levels of a path, for the path bar: (label, path, after a "/").
/// The home folder is one level, `~`.
fn crumbs(p: &Path) -> Vec<(String, PathBuf, bool)> {
    use std::path::Component;
    let mut out = Vec::new();
    let mut acc = PathBuf::new();
    let home = home();
    for c in p.components() {
        acc.push(c.as_os_str());
        match c {
            Component::RootDir => out.push(("/".to_string(), acc.clone(), false)),
            Component::Prefix(pre) => out.push((
                pre.as_os_str().to_string_lossy().into_owned(),
                acc.clone(),
                false,
            )),
            _ => {
                if home.as_deref() == Some(acc.as_path()) {
                    out.clear();
                    out.push(("~".to_string(), acc.clone(), false));
                    continue;
                }
                // No "/" straight after the root.
                let sep = !matches!(out.last(), Some((l, _, _)) if l == "/" || l.ends_with('\\'));
                out.push((
                    c.as_os_str().to_string_lossy().into_owned(),
                    acc.clone(),
                    sep,
                ));
            }
        }
    }
    out
}

fn human_size(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_bar() {
        std::env::set_var("HOME", "/home/me");
        assert_eq!(tilde(Path::new("/home/me/music")), "~/music");
        assert_eq!(tilde(Path::new("/home/me")), "~");
        assert_eq!(tilde(Path::new("/srv/x")), "/srv/x");
        let c = crumbs(Path::new("/home/me/music/nes.zip"));
        let labels: Vec<&str> = c.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(labels, ["~", "music", "nes.zip"]);
        assert_eq!(c[1].1, PathBuf::from("/home/me/music"));
        let c = crumbs(Path::new("/srv/x"));
        assert_eq!(
            c.iter().map(|x| (x.0.as_str(), x.2)).collect::<Vec<_>>(),
            [("/", false), ("srv", false), ("x", true)]
        );
    }
}
