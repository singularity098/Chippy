//! Chippy for Android: the engine the Jetpack Compose app drives, exposed
//! to Kotlin through UniFFI (`io.github.singularity098.chippy.core`).
//!
//! The UI calls [`Engine`]'s methods (from any thread); the engine tells it
//! what changed through its [`Listener`], one whole record per part of the
//! screen. A thread of the engine's own (the clock) takes in what finishes
//! in the background — tags, album art, search results, the end of a track
//! — so playback carries on while the app is in the background. What moves
//! on every display frame (the position, the spectrum) the UI asks for
//! itself, with [`Engine::frame`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

#[cfg(target_os = "android")]
mod android_context;
mod core;
mod paths;
mod types;

pub use types::*;

uniffi::setup_scaffolding!();

/// How the engine tells the UI what changed. Called from the engine's
/// threads, never at the same time; an implementation must not call back
/// into the engine (it should only hand the record to the UI thread).
#[uniffi::export(callback_interface)]
pub trait Listener: Send + Sync {
    fn browser(&self, state: BrowserState);
    fn playlist(&self, state: PlaylistState);
    fn now(&self, state: NowState);
    fn settings(&self, state: SettingsState);
    fn export(&self, state: ExportState);
    fn tabs(&self, state: TabsState);
    /// The current track's picture as stored (JPEG, PNG...); None when it
    /// has none.
    fn art(&self, data: Option<Vec<u8>>);
    fn event(&self, event: UiEvent);
}

#[derive(uniffi::Object)]
pub struct Engine {
    core: Mutex<core::Core>,
    /// Held while delivering, so records arrive in the order they were
    /// made, whichever thread made them.
    listener: Mutex<Box<dyn Listener>>,
    /// The last frame, for when the engine is busy (reading a folder).
    last_frame: Mutex<Option<Frame>>,
}

/// How often the clock ticks while something moves, and while nothing
/// does.
const TICK_BUSY: Duration = Duration::from_millis(20);
const TICK_IDLE: Duration = Duration::from_millis(80);

#[uniffi::export]
impl Engine {
    /// Starts the engine: the sound output, the player, the settings and
    /// the last session. `data_dir` is the app's private folder (settings,
    /// the session); `storage_dir` the shared storage the browser starts in
    /// (`/storage/emulated/0`). One engine per process.
    #[uniffi::constructor]
    pub fn new(data_dir: String, storage_dir: String, listener: Box<dyn Listener>) -> Arc<Engine> {
        init_logging();
        let data = PathBuf::from(&data_dir);
        std::env::set_var("CHIPPY_HOME", &data);
        std::env::set_var("TMPDIR", data.join("tmp"));
        let _ = std::fs::create_dir_all(data.join("tmp"));
        std::env::set_var("HOME", &storage_dir);

        let engine = Arc::new(Engine {
            core: Mutex::new(core::Core::new()),
            listener: Mutex::new(listener),
            last_frame: Mutex::new(None),
        });
        engine.act(|_| ());
        let weak: Weak<Engine> = Arc::downgrade(&engine);
        std::thread::Builder::new()
            .name("clock".into())
            .spawn(move || clock(weak))
            .expect("thread");
        engine
    }

    // ------------------------------------------------------------ browser

    /// A tap on a listed entry: opens folders, plays music.
    pub fn activate(&self, index: u32) {
        self.act(|c| c.activate(index as usize));
    }
    /// Adds a listed entry (a folder: its music) to the playlist.
    pub fn add_to_playlist(&self, index: u32) {
        self.act(|c| c.add_to_queue(index as usize));
    }
    pub fn go_up(&self) {
        self.act(|c| c.go_up());
    }
    pub fn go_back(&self) {
        self.act(|c| c.go_history(-1));
    }
    pub fn go_forward(&self) {
        self.act(|c| c.go_history(1));
    }
    pub fn go_home(&self) {
        self.act(|c| c.go_home());
    }
    pub fn go_path(&self, path: String) {
        self.act(|c| c.go_path(&path));
    }
    pub fn go_music_dir(&self, index: u32) {
        self.act(|c| c.go_music_dir(index as i32));
    }
    pub fn refresh(&self) {
        self.act(|c| c.refresh());
    }
    pub fn set_filter(&self, text: String) {
        self.act(|c| c.set_filter(&text));
    }
    /// The search below the folder; it starts shortly after typing stops.
    pub fn set_search(&self, text: String) {
        self.act(|c| c.set_search(&text));
    }
    /// Starts the search at once.
    pub fn run_search(&self) {
        self.act(|c| c.run_search());
    }

    // ----------------------------------------------------------- playlist

    pub fn play_row(&self, index: u32) {
        self.act(|c| c.play_index(index as usize));
    }
    pub fn remove_row(&self, index: u32) {
        self.act(|c| c.remove_from_queue(index as usize));
    }
    pub fn clear_playlist(&self) {
        self.act(|c| c.clear_queue());
    }
    /// Shows a row's file in the browser (and asks the UI to open it).
    pub fn reveal_row(&self, index: u32) {
        self.act(|c| c.reveal(index as usize));
    }
    pub fn track_info(&self, index: u32) -> Option<TrackInfo> {
        self.lock().track_info(index as usize)
    }

    // ----------------------------------------------------------- playback

    pub fn play_pause(&self) {
        self.act(|c| c.play_pause());
    }
    pub fn play(&self) {
        self.act(|c| c.play());
    }
    pub fn pause(&self) {
        self.act(|c| c.pause());
    }
    pub fn stop(&self) {
        self.act(|c| c.stop());
    }
    pub fn next(&self) {
        self.act(|c| c.next(false));
    }
    pub fn previous(&self) {
        self.act(|c| c.previous());
    }
    pub fn seek_to(&self, ms: u64) {
        self.act(|c| c.seek(ms));
    }
    /// 0..1 of the track.
    pub fn seek_fraction(&self, fraction: f32) {
        self.act(|c| c.seek_fraction(fraction));
    }
    pub fn seek_by(&self, seconds: i64) {
        self.act(|c| c.seek_by(seconds));
    }
    pub fn set_volume(&self, volume: f32) {
        self.act(|c| c.set_volume(volume));
    }
    pub fn toggle_mute(&self) {
        self.act(|c| c.toggle_mute());
    }
    pub fn toggle_shuffle(&self) {
        self.act(|c| c.toggle_shuffle());
    }
    pub fn cycle_repeat(&self) {
        self.act(|c| c.cycle_repeat());
    }
    /// The next (+1) or previous (-1) SoundFont, for MIDI.
    pub fn cycle_soundfont(&self, delta: i32) {
        self.act(|c| c.cycle_soundfont(delta));
    }

    /// The position and the spectrum, for this display frame. Called on
    /// the UI thread: it never waits for the engine (busy reading a large
    /// folder, say), and gives the last frame again instead.
    pub fn frame(&self) -> Frame {
        let mut last = self.last_frame.lock().unwrap_or_else(|e| e.into_inner());
        let core = match self.core.try_lock() {
            Ok(c) => Some(c),
            Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
        match (core, last.as_ref()) {
            (Some(mut c), _) => {
                let f = c.frame();
                *last = Some(f.clone());
                f
            }
            (None, Some(f)) => f.clone(),
            (None, None) => self.lock().frame(),
        }
    }

    // ----------------------------------------------------------- settings

    /// A switch in Settings: "show-all", "folder-playlist",
    /// "browser-playlist", "restore-session", "embedded-art", "usf-hle",
    /// "gme-accuracy".
    pub fn set_flag(&self, id: String, on: bool) {
        self.act(|c| c.set_flag(&id, on));
    }
    /// A stepper in Settings: "default-length", "fade", "loops", "bars".
    pub fn step(&self, id: String, delta: i32) {
        self.act(|c| c.step(&id, delta));
    }
    pub fn set_theme(&self, name: String) {
        self.act(|c| c.set_theme(&name));
    }
    pub fn add_music_dir(&self, path: String) {
        self.act(|c| c.add_music_dir(PathBuf::from(path)));
    }
    /// Adds the browser's folder as a music folder.
    pub fn add_browser_dir(&self) {
        self.act(|c| c.use_browser_dir());
    }
    pub fn remove_music_dir(&self, index: u32) {
        self.act(|c| c.remove_music_dir(index as i32));
    }
    pub fn set_home_dir(&self, index: u32) {
        self.act(|c| c.set_home_dir(index as i32));
    }
    pub fn set_soundfont_dir(&self, path: String) {
        self.act(|c| c.set_soundfont_dir(PathBuf::from(path)));
    }
    pub fn soundfont_browser_dir(&self) {
        self.act(|c| c.soundfont_use_browser_dir());
    }
    pub fn pick_soundfont(&self, index: u32) {
        self.act(|c| c.pick_soundfont(index as i32));
    }
    /// MIDI mode on or off (the playing track crossfades).
    pub fn toggle_midi_mode(&self) {
        self.act(|c| c.toggle_midi_mode());
    }
    /// MIDI mode on, with the instruments of style `index` (of
    /// `NowState.midi_styles`).
    pub fn set_midi_style(&self, index: u32) {
        self.act(|c| c.set_midi_style(index));
    }
    /// Where the folder picker for a setting ("music", "soundfont",
    /// "export") should open.
    pub fn picker_start(&self, which: String) -> Option<String> {
        self.lock().picker_start(&which)
    }

    // ------------------------------------------------------------- export

    /// Gets the export dialog ready for a playlist row, or the whole
    /// playlist (None). False when there is nothing to export.
    pub fn prepare_export(&self, row: Option<u32>) -> bool {
        self.act(|c| c.prepare_export(row.map(|r| r as usize)))
    }
    pub fn set_export_format(&self, format: ExportFormat) {
        self.act(|c| c.set_export_format(format));
    }
    pub fn step_export_bitrate(&self, delta: i32) {
        self.act(|c| c.step_export_bitrate(delta));
    }
    pub fn set_export_dir(&self, path: String) {
        self.act(|c| c.set_export_dir(PathBuf::from(path)));
    }
    pub fn start_export(&self) {
        self.act(|c| c.start_export());
    }
    pub fn cancel_export(&self) {
        self.act(|c| c.cancel_export());
    }

    // ---------------------------------------------------- lyrics and tabs

    /// The playing track's lyrics (None: nothing is playing).
    pub fn lyrics(&self) -> Option<LyricsInfo> {
        self.act(|c| c.lyrics())
    }

    /// Shows the guitar tab of the playing track (and of each track after
    /// it), sent through [`Listener::tabs`] as it is worked out.
    pub fn open_tabs(&self) {
        self.act(|c| c.open_tabs());
    }

    pub fn close_tabs(&self) {
        self.act(|c| c.close_tabs());
    }

    pub fn set_tab_part(&self, index: u32) {
        self.act(|c| c.set_tab_part(index as usize));
    }

    /// Item `index` of [`TabsState::merge_items`]: the shown part merged
    /// with another into a new part, or (the last item, for a merge) taken
    /// apart.
    pub fn merge_tab_part(&self, index: u32) {
        self.act(|c| c.merge_tab_part(index as usize));
    }

    pub fn set_tab_tuning(&self, index: u32) {
        self.act(|c| c.set_tab_tuning(index as usize));
    }

    /// Shows the tab `d` octaves higher (lower if negative), for this
    /// track and part until the app closes.
    pub fn shift_tab_octave(&self, d: i32) {
        self.act(|c| c.shift_tab_octave(d));
    }

    /// Moves the tab's fingering `d` steps up the neck (down if negative):
    /// the lowest-fretted notes onto another string, played higher up.
    pub fn nudge_tab(&self, d: i32) {
        self.act(|c| c.nudge_tab(d));
    }

    /// The tab back as it fits best (the chosen bars, if any are).
    pub fn reset_tab_adjust(&self) {
        self.act(|c| c.reset_tab_adjust());
    }

    /// Chooses bar `index` to nudge along the neck on its own (again:
    /// none); with `extend`, the bars from the one chosen before to it.
    pub fn select_tab_bar(&self, index: u32, extend: bool) {
        self.act(|c| c.select_tab_bar(index as usize, extend));
    }

    pub fn clear_tab_selection(&self) {
        self.act(|c| c.clear_tab_selection());
    }

    /// Plays from the start of bar `index`.
    pub fn seek_tab_bar(&self, index: u32) {
        self.act(|c| c.seek_tab_bar(index as usize));
    }

    /// The time signature, as one of [`TabsState::sigs`] (0: as found).
    pub fn set_tab_sig(&self, index: u32) {
        self.act(|c| c.set_tab_sig(index as usize));
    }

    /// The beat twice as fast (1) or half as fast (-1).
    pub fn step_tab_speed(&self, d: i32) {
        self.act(|c| c.step_tab_speed(d));
    }

    /// The bar lines a beat later (1) or earlier (-1).
    pub fn shift_tab_bars(&self, d: i32) {
        self.act(|c| c.shift_tab_bars(d));
    }

    /// The bars back as found.
    pub fn reset_tab_meter(&self) {
        self.act(|c| c.reset_tab_meter());
    }

    /// Saves every part as tab in bars, in the export folder: Guitar Pro
    /// 5, or MusicXML.
    pub fn save_tab(&self, guitar_pro: bool) {
        self.act(|c| c.save_tab(guitar_pro));
    }

    /// How many fret numbers (a digit and its padding) fit across a line
    /// of the tab as shown: the lines are laid out to that width.
    pub fn set_tab_width(&self, digits: f32) {
        self.act(|c| c.set_tab_width(digits));
    }

    // ---------------------------------------------------------- lifecycle

    /// Saves the settings and the session now (the app is leaving the
    /// screen, and Android may end it without warning).
    pub fn save(&self) {
        self.act(|c| {
            c.save_settings();
            c.save_session();
        });
    }

    /// Saves everything and stops playback, for good.
    pub fn shutdown(&self) {
        self.act(|c| c.shutdown());
    }
}

impl Engine {
    fn lock(&self) -> MutexGuard<'_, core::Core> {
        // A panic elsewhere must not leave the app unable to do anything.
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on the engine's state, then sends the UI what it changed.
    fn act<R>(&self, f: impl FnOnce(&mut core::Core) -> R) -> R {
        let mut core = self.lock();
        let r = f(&mut core);
        let out = core.take_outbox();
        // Take the listener before letting go of the state, so whatever
        // changes next is delivered after this.
        let listener = self.listener.lock().unwrap_or_else(|e| e.into_inner());
        drop(core);
        deliver(&**listener, out);
        r
    }
}

fn deliver(l: &dyn Listener, out: core::Outbox) {
    if let Some(s) = out.browser {
        l.browser(s);
    }
    if let Some(s) = out.playlist {
        l.playlist(s);
    }
    if let Some(s) = out.now {
        l.now(s);
    }
    if let Some(s) = out.settings {
        l.settings(s);
    }
    if let Some(s) = out.export {
        l.export(s);
    }
    if let Some(s) = out.tabs {
        l.tabs(s);
    }
    if let Some(a) = out.art {
        l.art(a);
    }
    for e in out.events {
        l.event(e);
    }
}

/// The engine's clock: runs until the engine is dropped.
fn clock(engine: Weak<Engine>) {
    let mut interval = TICK_BUSY;
    loop {
        std::thread::sleep(interval);
        let Some(e) = engine.upgrade() else { return };
        let busy = e.act(|c| c.tick());
        interval = if busy { TICK_BUSY } else { TICK_IDLE };
    }
}

fn init_logging() {
    #[cfg(target_os = "android")]
    {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("chippy"),
        );
        std::panic::set_hook(Box::new(|info| {
            log::error!("panic: {info}");
        }));
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = env_logger::try_init();
    }
}
