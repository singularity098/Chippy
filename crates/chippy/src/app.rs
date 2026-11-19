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
use chippy_core::settings::{default_columns, Repeat, Settings};
use chippy_core::vfs::{self, Entry, EntryKind, Node, TrackRef};
use chippy_core::{decode, formats, DecodeOptions};

use crate::art::{self, Picture};
use crate::audio::Output;
use crate::media::{Media, Playback, Request};
use crate::player::{Cmd, Event, Player};
use crate::spectrum::{Spectrum, FFT_LEN};
use crate::{
    Browser, ColumnItem, Crumb, Export, FileItem, Info, MainWindow, Now, PlRow, Playlist, Prefs,
    Theme,
};

mod tools;
use tools::{ExportState, Picked};

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
    output: Output,
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
    muted: bool,
    art_tx: Sender<(u64, Option<Picture>)>,
    art_rx: Receiver<(u64, Option<Picture>)>,
    art_for: Option<vfs::Location>,
    spectrum: Spectrum,
    bars: Rc<VecModel<f32>>,
    peaks: Rc<VecModel<f32>>,
    levels: [f32; 2],
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

        let app = App {
            ui: ui.as_weak(),
            settings,
            settings_dirty: None,
            output,
            player,
            media: Media::connect(),
            node: None,
            entries: Vec::new(),
            shown: Vec::new(),
            filter: String::new(),
            history: Vec::new(),
            hist_pos: 0,
            files: Rc::new(VecModel::default()),
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
            muted: false,
            art_tx,
            art_rx,
            art_for: None,
            spectrum: Spectrum::new(bands),
            bars: Rc::new(VecModel::from(vec![0.0; bands])),
            peaks: Rc::new(VecModel::from(vec![0.0; bands])),
            levels: [0.0; 2],
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
            }
        }
        // The clock.
        let timer = slint::Timer::default();
        let weak = Rc::downgrade(&app);
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                if let Some(app) = weak.upgrade() {
                    if let Ok(mut a) = app.try_borrow_mut() {
                        a.tick();
                    }
                }
            },
        );
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
        on!(app, s, on_set_music_dir, |a, t: SharedString| {
            let p = expand(&t);
            if p.is_dir() {
                a.settings.music_dir = Some(p);
                a.mark_dirty();
                a.sync_prefs();
                a.toast("Music folder saved", false);
            } else {
                a.toast(&format!("{} is not a folder", p.display()), true);
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
                a.settings.music_dir = Some(node.vpath());
                a.mark_dirty();
                a.sync_prefs();
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
                a.toast(&format!("{} is not a folder", p.display()), true);
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
            ui.window().set_maximized(m);
            ui.set_window_maximized(m);
        });
        on!(app, ui, on_close_window, |a| {
            a.remember_window();
            let _ = slint::quit_event_loop();
        });
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
            use slint::winit_030::{EventResult, WinitWindowAccessor};
            let weak = ui.as_weak();
            let mut cursor = (0.0f32, 0.0f32);
            ui.window().on_winit_window_event(move |w, e| {
                match e {
                    WindowEvent::CursorMoved { position, .. } => {
                        let s = w.scale_factor();
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
                            ui.invoke_pointer_pressed(cursor.0, cursor.1);
                        }
                    }
                    WindowEvent::Touch(t) => {
                        if let Some(ui) = weak.upgrade() {
                            let s = w.scale_factor();
                            ui.invoke_pointer_pressed(
                                t.location.x as f32 / s,
                                t.location.y as f32 / s,
                            );
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
        let w = ui.window();
        if self.settings.fullscreen {
            w.set_fullscreen(true);
        } else {
            // Once the window exists (before, its preferred size wins).
            let (width, height, max) = (
                self.settings.window_width,
                self.settings.window_height,
                self.settings.maximized,
            );
            let weak = self.ui.clone();
            slint::Timer::single_shot(Duration::from_millis(1), move || {
                if let Some(ui) = weak.upgrade() {
                    ui.window()
                        .set_size(slint::LogicalSize::new(width as f32, height as f32));
                    if max {
                        ui.window().set_maximized(true);
                        ui.set_window_maximized(true);
                    }
                }
            });
        }
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
        p.set_music_dir(ss(s
            .music_dir
            .as_ref()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default()));
        p.set_browser_dir(ss(self
            .node
            .as_ref()
            .map(|n| n.vpath().to_string_lossy().into_owned())
            .unwrap_or_default()));
        p.set_show_all(s.show_all_files);
        p.set_single_tap(s.single_tap);
        p.set_hover_opens(s.hover_opens_browser);
        p.set_folder_playlist(s.open_folder_as_playlist);
        p.set_embedded_art(s.prefer_embedded_art);
        p.set_usf_hle(s.usf_hle);
        p.set_gme_accuracy(s.gme_accuracy);
        p.set_fullscreen(s.fullscreen);
        p.set_native_wayland(s.native_wayland);
        p.set_default_length(ss(format_ms(s.default_length_s as u64 * 1000)));
        p.set_fade(ss(format!("{} s", s.fade_s)));
        p.set_loops(ss(format!("{}", s.loops)));
        p.set_bars(ss(format!("{}", s.spectrum_bars)));
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
            ui.window().set_fullscreen(false);
            if !self.settings.maximized {
                ui.window().set_size(slint::LogicalSize::new(
                    self.settings.window_width as f32,
                    self.settings.window_height as f32,
                ));
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
                let size = w.size().to_logical(w.scale_factor());
                self.settings.window_width = size.width.round() as u32;
                self.settings.window_height = size.height.round() as u32;
            }
        }
    }

    pub fn shutdown(&mut self) {
        self.settings.volume = self.settings.volume.clamp(0.0, 1.0);
        self.save_settings();
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
        self.node = Some(node);
        self.entries = entries;
        self.filter.clear();
        let ui = self.ui();
        let b = ui.global::<Browser>();
        b.set_filter(SharedString::new());
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
        self.shown = (0..self.entries.len())
            .filter(|&i| vfs::matches_filter(&self.entries[i].name, &self.filter))
            .collect();
        self.rebuild_files();
        let ui = self.ui();
        let b = ui.global::<Browser>();
        b.set_selected(if self.filter.is_empty() || self.shown.is_empty() {
            -1
        } else {
            0
        });
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
        if !self.filter.is_empty() {
            status = format!(
                "{status} matching \"{}\" (of {})",
                self.filter,
                self.entries.len()
            );
        }
        self.set_status(&status, false);
    }

    fn current_track(&self) -> Option<&TrackRef> {
        self.queue
            .current
            .and_then(|c| self.queue.items.get(c))
            .map(|i| &i.track)
    }

    fn rebuild_files(&mut self) {
        let playing = self.current_track().cloned();
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
    /// the listing (filter or not) as the playlist, in listed order.
    fn play_from_listing(&mut self, i: usize) {
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
        let Some(item) = self.queue.items.get(i) else {
            return;
        };
        let track = item.track.clone();
        let old = self.queue.current;
        self.queue.current = Some(i);
        self.generation += 1;
        self.started = false;
        self.duration_ms = item.meta.as_ref().and_then(|m| m.length_ms);
        self.player.send(Cmd::Load {
            track,
            generation: self.generation,
            start_ms: 0,
        });
        self.state = State::Playing;
        self.output.shared.lock().unwrap().paused = false;
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
        // Show the new place at once.
        let mut s = self.output.shared.lock().unwrap();
        s.buf.clear();
        s.base_ms = ms;
        s.frames_played = 0;
        s.producer_done = false;
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
                ui.set_subtitle(ss(item.track.loc.vpath().to_string_lossy()));
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
                if !self.ui().invoke_back() {
                    #[cfg(target_os = "android")]
                    crate::android::move_to_back();
                }
            }
            Request::Refresh => self.refresh(),
        }
    }

    fn load_art(&mut self) {
        let Some(t) = self.current_track().cloned() else {
            self.art_for = None;
            self.ui().global::<Now>().set_has_art(false);
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

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;

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
                    if self.failures < self.queue.len().max(1) {
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
                }
                None => n.set_has_art(false),
            }
        }

        // Track end, position, spectrum, meter.
        let (ended, pos_ms, samples, peak) = {
            let mut s = self.output.shared.lock().unwrap();
            let current = s.generation == self.generation;
            let ended = current
                && self.started
                && self.state == State::Playing
                && s.producer_done
                && s.buf.is_empty();
            let pos = if current {
                s.base_ms + s.frames_played * 1000 / self.output.rate as u64
            } else {
                0
            };
            let samples = (self.state == State::Playing && current).then(|| s.scope_tail(FFT_LEN));
            let peak = std::mem::take(&mut s.peak);
            (ended, pos, samples, peak)
        };
        if ended {
            self.next(true);
            return;
        }
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
        let decay = (1.0 - dt * 3.0).max(0.0);
        for (level, p) in self.levels.iter_mut().zip(peak) {
            // Shown on a dB-ish scale, so quiet music still moves the meter.
            let v = if p > 0.0 {
                ((20.0 * p.log10() + 48.0) / 48.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            *level = v.max(*level * decay);
        }
        n.set_level_l(self.levels[0]);
        n.set_level_r(self.levels[1]);

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
    }
}

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
