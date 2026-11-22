//! The app's logic on Android: the file browser's state, the playlist,
//! playback, the settings and exporting, the same as the desktop's
//! (`crates/chippy/src/app.rs`) but with no UI toolkit. Each change marks
//! the parts of the screen it touched (`Dirty`); the engine (`lib.rs`) then
//! sends those parts, whole, to the Compose UI.

use crossbeam_channel::{Receiver, Sender};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chippy_core::decode::midi;
use chippy_core::meta::{self, format_ms, TrackMeta};
use chippy_core::notes::style::Style;
use chippy_core::playlist::Playlist as Queue;
use chippy_core::settings::{Repeat, Session, Settings};
use chippy_core::vfs::{self, Entry, EntryKind, Node, TrackRef};
use chippy_core::{decode, formats, DecodeOptions};
use chippy_engine::audio::Output;
use chippy_engine::export::{self, Format, Progress};
use chippy_engine::midimode;
use chippy_engine::player::{Cmd, Event, Player};
use chippy_engine::spectrum::{Spectrum, FFT_LEN};
use chippy_engine::tabs::{Available, TabView};

use crate::paths::{self, file_name, found_in, human_size, pretty};
use crate::types::*;

/// At most this many search results are listed.
const SEARCH_LIMIT: usize = 5000;
/// While tags stream in, the playlist is sent at most this often.
const PLAYLIST_EVERY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Stopped,
    Playing,
    Paused,
}

/// The parts of the screen that changed since they were last sent.
#[derive(Default)]
pub struct Dirty {
    pub browser: bool,
    pub playlist: bool,
    /// The playlist changed by the user's hand: sent at once, not
    /// throttled.
    pub playlist_now: bool,
    pub now: bool,
    pub settings: bool,
    pub export: bool,
    pub tabs: bool,
}

/// What is waiting to be sent to the UI.
#[derive(Default)]
pub struct Outbox {
    pub browser: Option<BrowserState>,
    pub playlist: Option<PlaylistState>,
    pub now: Option<NowState>,
    pub settings: Option<SettingsState>,
    pub export: Option<ExportState>,
    pub tabs: Option<TabsState>,
    /// Some(None): the track has no art.
    pub art: Option<Option<Vec<u8>>>,
    pub events: Vec<UiEvent>,
}

/// A SoundFont finished loading (or failed to).
struct FontLoaded {
    path: PathBuf,
    result: Result<Arc<rustysynth::SoundFont>, String>,
}

#[derive(Default)]
struct ExportJob {
    /// Playlist rows to export.
    rows: Vec<usize>,
    what: String,
    cancel: Option<Arc<AtomicBool>>,
    total: usize,
    index: usize,
    name: String,
    progress: f32,
    status: String,
}

pub struct Core {
    settings: Settings,
    settings_dirty: Option<Instant>,
    output: Output,
    underruns: u64,
    player: Player,

    // ---- file browser
    node: Option<Node>,
    entries: Vec<Entry>,
    /// Indices into `entries` that pass the filter, as listed.
    shown: Vec<usize>,
    filter: String,
    history: Vec<PathBuf>,
    hist_pos: usize,
    /// Where the browser last was inside each music folder.
    dir_places: HashMap<PathBuf, PathBuf>,
    selected: i32,
    browser_serial: u64,
    status: String,
    status_error: bool,
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
    reveal_serial: u64,
    playlist_sent: Instant,

    // ---- playback
    state: State,
    generation: u64,
    started: bool,
    duration_ms: Option<u64>,
    failures: usize,
    /// The track was brought back from the last session, paused: if it
    /// cannot be played, nothing else starts in its place.
    resumed: bool,
    start_ms: u64,
    session_dirty: Option<Instant>,
    session_saved: Instant,
    muted: bool,
    seek_serial: u64,
    art_tx: Sender<(u64, Option<Vec<u8>>)>,
    art_rx: Receiver<(u64, Option<Vec<u8>>)>,
    art_for: Option<vfs::Location>,
    art_gen: u64,
    spectrum: Spectrum,
    last_frame: Instant,

    // ---- MIDI
    soundfonts: Vec<PathBuf>,
    font_cache: Vec<(PathBuf, Arc<rustysynth::SoundFont>)>,
    font_request: Arc<AtomicU64>,
    font_tx: Sender<FontLoaded>,
    font_rx: Receiver<FontLoaded>,
    soundfont_status: String,

    // ---- guitar tab
    tabs_shown: bool,
    tabs: TabView,
    /// Whether the playing track has lyrics and notes to show.
    avail: Available,

    // ---- export
    export: ExportJob,
    export_tx: Sender<Progress>,
    export_rx: Receiver<Progress>,

    dirty: Dirty,
    art_out: Option<Option<Vec<u8>>>,
    events: Vec<UiEvent>,
    /// What MIDI mode last said about the playing track.
    midi_status: midimode::Status,
}

impl Core {
    pub fn new() -> Core {
        let settings = Settings::load();
        let output = Output::open();
        output.shared.lock().unwrap().volume = settings.volume;
        let player = Player::spawn(
            output.shared.clone(),
            output.rate,
            settings.decode_options(output.rate),
        );
        let (meta_tx, meta_rx) = crossbeam_channel::unbounded();
        let (art_tx, art_rx) = crossbeam_channel::unbounded();
        let (font_tx, font_rx) = crossbeam_channel::unbounded();
        let (export_tx, export_rx) = crossbeam_channel::unbounded();
        let (search_tx, search_rx) = crossbeam_channel::unbounded();
        let bands = settings.spectrum_bars as usize;
        let mut core = Core {
            settings,
            settings_dirty: None,
            output,
            underruns: 0,
            player,
            node: None,
            entries: Vec::new(),
            shown: Vec::new(),
            filter: String::new(),
            history: Vec::new(),
            hist_pos: 0,
            dir_places: HashMap::new(),
            selected: -1,
            browser_serial: 0,
            status: String::new(),
            status_error: false,
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
            reveal_serial: 0,
            playlist_sent: Instant::now(),
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
            seek_serial: 0,
            art_tx,
            art_rx,
            art_for: None,
            art_gen: 0,
            spectrum: Spectrum::new(bands),
            last_frame: Instant::now(),
            soundfonts: Vec::new(),
            font_cache: Vec::new(),
            font_request: Arc::new(AtomicU64::new(0)),
            font_tx,
            font_rx,
            soundfont_status: String::new(),
            tabs_shown: false,
            tabs: TabView::default(),
            avail: Available::default(),
            export: ExportJob::default(),
            export_tx,
            export_rx,
            dirty: Dirty {
                browser: true,
                playlist: true,
                playlist_now: true,
                now: true,
                settings: true,
                export: true,
                tabs: false,
            },
            art_out: Some(None),
            events: Vec::new(),
            midi_status: midimode::Status::Off,
        };
        core.queue.set_shuffle(core.settings.shuffle);
        core.scan_soundfonts();
        core.load_soundfont();
        midimode::set(core.settings.midi_mode.then(|| core.midi_style()));
        let start = core.home_dir();
        core.navigate(&start, true);
        if core.settings.restore_session {
            core.restore_session();
        }
        core
    }

    // ------------------------------------------------------------ outbox

    /// Builds what changed for the UI. The playlist waits while tags stream
    /// in, so a long one is not sent for every row.
    pub fn take_outbox(&mut self) -> Outbox {
        let mut out = Outbox {
            art: self.art_out.take(),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        if std::mem::take(&mut self.dirty.browser) {
            out.browser = Some(self.browser_state());
        }
        if self.dirty.playlist
            && (self.dirty.playlist_now || self.playlist_sent.elapsed() >= PLAYLIST_EVERY)
        {
            self.dirty.playlist = false;
            self.dirty.playlist_now = false;
            self.playlist_sent = Instant::now();
            out.playlist = Some(self.playlist_state());
        }
        if std::mem::take(&mut self.dirty.now) {
            out.now = Some(self.now_state());
        }
        if std::mem::take(&mut self.dirty.settings) {
            out.settings = Some(self.settings_state());
        }
        if std::mem::take(&mut self.dirty.tabs) {
            out.tabs = Some(self.tabs_state());
        }
        if std::mem::take(&mut self.dirty.export) {
            out.export = Some(self.export_state());
        }
        out
    }

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.events.push(UiEvent::Toast {
            text: text.into(),
            error,
        });
    }

    fn playlist_changed(&mut self) {
        self.dirty.playlist = true;
        self.dirty.playlist_now = true;
    }

    fn browser_state(&self) -> BrowserState {
        let vp = self.node.as_ref().map(Node::vpath);
        BrowserState {
            title: self
                .node
                .as_ref()
                .map(|n| {
                    let name = n.display_name();
                    match paths::crumbs(&n.vpath()).last() {
                        Some((label, _)) if label != &name => label.clone(),
                        _ => name,
                    }
                })
                .unwrap_or_default(),
            crumbs: vp
                .as_deref()
                .map(paths::crumbs)
                .unwrap_or_default()
                .into_iter()
                .map(|(label, path)| Crumb {
                    label,
                    path: path.to_string_lossy().into_owned(),
                })
                .collect(),
            items: self.file_items(),
            status: self.status.clone(),
            status_error: self.status_error,
            can_up: self.node.as_ref().and_then(Node::parent).is_some(),
            can_back: self.hist_pos > 0,
            can_forward: self.hist_pos + 1 < self.history.len(),
            filter: self.filter.clone(),
            search: self.search.clone(),
            searching: self.searching,
            in_results: self.in_results,
            music_dirs: self.music_dirs(),
            selected: self.selected,
            serial: self.browser_serial,
        }
    }

    fn file_items(&self) -> Vec<FileItem> {
        let playing = self.current_track();
        let here = self.node.as_ref().map(Node::vpath);
        self.shown
            .iter()
            .map(|&i| {
                let e = &self.entries[i];
                let (stem, ext) = match e.kind {
                    EntryKind::Dir | EntryKind::Track => (e.name.as_str(), ""),
                    _ => formats::split_name(&e.name),
                };
                let kind = match e.kind {
                    EntryKind::Dir => FileKind::Folder,
                    EntryKind::Archive => FileKind::Archive,
                    EntryKind::Audio if e.may_hold_tracks => FileKind::MultiTrack,
                    EntryKind::Audio => FileKind::Audio,
                    EntryKind::Track => FileKind::Track,
                    EntryKind::Image => FileKind::Image,
                    EntryKind::Other => FileKind::Other,
                };
                let detail = match e.kind {
                    _ if self.in_results => found_in(here.as_deref(), &e.vpath),
                    EntryKind::Track => e.length_ms.map(format_ms).unwrap_or_default(),
                    EntryKind::Image | EntryKind::Other => human_size(e.size),
                    _ => String::new(),
                };
                let is_playing = playing.is_some_and(|p| {
                    e.track
                        .as_ref()
                        .is_some_and(|t| t.loc == p.loc && (t.sub.is_none() || t.sub == p.sub))
                });
                FileItem {
                    stem: stem.to_string(),
                    ext: ext.trim_start_matches('.').to_lowercase(),
                    kind,
                    detail,
                    playing: is_playing,
                }
            })
            .collect()
    }

    fn music_dirs(&self) -> Vec<MusicDir> {
        let here = self.node.as_ref().map(Node::vpath);
        let current = here.as_deref().and_then(|h| self.music_dir_of(h));
        self.settings
            .music_dirs
            .iter()
            .map(|d| MusicDir {
                label: Some(file_name(d))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| pretty(d)),
                path: pretty(d),
                home: self.settings.music_dir.as_ref() == Some(d),
                current: current == Some(d.as_path()),
            })
            .collect()
    }

    fn playlist_state(&self) -> PlaylistState {
        let rows = self
            .queue
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let (title, detail) = match &item.meta {
                    Some(m) => {
                        let detail: Vec<String> = [m.column("artist"), m.column("length")]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect();
                        let title = Some(m.column("title"))
                            .filter(|t| !t.is_empty())
                            .unwrap_or_else(|| item.track.loc.name());
                        (title, detail.join(" · "))
                    }
                    None => (item.track.loc.name(), String::new()),
                };
                PlRow {
                    title,
                    detail,
                    playing: self.queue.current == Some(i),
                    pending: item.meta.is_none(),
                }
            })
            .collect();
        let n = self.queue.len();
        let total: u64 = self
            .queue
            .items
            .iter()
            .filter_map(|i| i.meta.as_ref()?.length_ms)
            .sum();
        PlaylistState {
            rows,
            playing: self.queue.current.map(|c| c as i32).unwrap_or(-1),
            summary: if n == 0 {
                String::new()
            } else {
                format!(
                    "{n} {} · {}",
                    if n == 1 { "track" } else { "tracks" },
                    format_ms(total)
                )
            },
            reveal_serial: self.reveal_serial,
        }
    }

    fn now_state(&self) -> NowState {
        let item = self.queue.current.and_then(|c| self.queue.items.get(c));
        let m = item.map(|item| {
            item.meta.clone().unwrap_or_else(|| {
                let mut m = TrackMeta::default();
                m.fill_gaps(&item.track);
                m
            })
        });
        let duration_ms = self.duration_ms.or(m.as_ref().and_then(|m| m.length_ms));
        let info = m
            .as_ref()
            .map(|m| {
                let mut info = vec![m.format.clone(), m.system.clone()];
                if let Some(r) = m.sample_rate {
                    info.push(format!("{:.1} kHz", r as f64 / 1000.0));
                }
                if let Some(b) = m.bitrate_kbps {
                    info.push(format!("{b} kbps"));
                }
                info.push(m.year.clone());
                info.into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ·  ")
            })
            .unwrap_or_default();
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
        NowState {
            state: match self.state {
                State::Stopped => PlayState::Stopped,
                State::Playing => PlayState::Playing,
                State::Paused => PlayState::Paused,
            },
            has_track: item.is_some(),
            title: m.as_ref().map(|m| m.title.clone()).unwrap_or_default(),
            artist: m.as_ref().map(|m| m.artist.clone()).unwrap_or_default(),
            album: m.as_ref().map(|m| m.album.clone()).unwrap_or_default(),
            info,
            location: item
                .map(|i| pretty(&i.track.loc.vpath()))
                .unwrap_or_default(),
            total: duration_ms.map(format_ms).unwrap_or_else(|| "–:––".into()),
            duration_ms,
            seekable: self.state != State::Stopped && self.started && duration_ms.is_some(),
            shuffle: self.settings.shuffle,
            repeat: match self.settings.repeat {
                Repeat::Off => RepeatMode::Off,
                Repeat::All => RepeatMode::All,
                Repeat::One => RepeatMode::One,
            },
            up_next,
            midi: item.is_some_and(|i| {
                matches!(
                    formats::kind_of(&i.track.loc.name()),
                    formats::FileKind::Audio(formats::Codec::Midi)
                )
            }),
            soundfont: self
                .settings
                .soundfont
                .as_deref()
                .map(|n| formats::split_name(n).0.to_string())
                .unwrap_or_default(),
            has_lyrics: self.avail.lyrics(),
            has_notes: self.avail.notes(&self.tabs),
            midi_mode: self.settings.midi_mode,
            midi_style: self.midi_style().index() as u32,
            midi_styles: Style::ALL.iter().map(|s| s.name().to_string()).collect(),
            midi_state: match self.midi_status {
                midimode::Status::Off => MidiState::Off,
                midimode::Status::Unsupported => MidiState::Unsupported,
                midimode::Status::Preparing => MidiState::Preparing,
                midimode::Status::Playing => MidiState::Playing,
                midimode::Status::Failed => MidiState::Failed,
            },
            volume: self.settings.volume,
            muted: self.muted,
            position_ms: if self.state == State::Stopped {
                0
            } else if !self.started {
                self.start_ms
            } else {
                self.position_ms()
            },
            seek_serial: self.seek_serial,
        }
    }

    fn settings_state(&self) -> SettingsState {
        let s = &self.settings;
        SettingsState {
            music_dirs: self.music_dirs(),
            browser_dir: self
                .node
                .as_ref()
                .map(|n| pretty(&n.vpath()))
                .unwrap_or_default(),
            show_all: s.show_all_files,
            folder_playlist: s.open_folder_as_playlist,
            restore_session: s.restore_session,
            embedded_art: s.prefer_embedded_art,
            usf_hle: s.usf_hle,
            gme_accuracy: s.gme_accuracy,
            default_length: format_ms(s.default_length_s as u64 * 1000),
            fade: format!("{} s", s.fade_s),
            loops: format!("{}", s.loops),
            bars: format!("{}", s.spectrum_bars),
            soundfont_dir: s.soundfont_dir.as_deref().map(pretty).unwrap_or_default(),
            soundfonts: self.soundfonts.iter().map(|f| file_name(f)).collect(),
            soundfont: self.soundfont_index().map(|i| i as i32).unwrap_or(-1),
            soundfont_status: self.soundfont_status.clone(),
            theme: s.theme.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            device: self.output.device_name.clone(),
            config_path: Settings::path().to_string_lossy().into_owned(),
        }
    }

    fn export_state(&self) -> ExportState {
        let s = &self.settings;
        let format = self.export_format();
        let (ok, total) = self.midi_count();
        ExportState {
            midi_ok: ok > 0,
            midi_note: if ok > 0 && ok < total {
                format!("Only {ok} of the {total} tracks can be: the others are left out.")
            } else {
                String::new()
            },
            what: self.export.what.clone(),
            format: match format {
                Format::Wav => ExportFormat::Wav,
                Format::Flac => ExportFormat::Flac,
                Format::Mp3 => ExportFormat::Mp3,
                Format::Opus => ExportFormat::Opus,
                Format::Midi => ExportFormat::Midi,
                Format::MusicXml => ExportFormat::MusicXml,
                Format::GuitarPro => ExportFormat::GuitarPro,
            },
            format_detail: format.detail().into(),
            bitrate: match format {
                Format::Opus => s.export_opus_kbps,
                _ => s.export_mp3_kbps,
            },
            folder: s.export_dir.as_deref().map(pretty).unwrap_or_default(),
            running: self.export.cancel.is_some(),
            progress: self.export.progress,
            status: self.export.status.clone(),
        }
    }

    // ---------------------------------------------------- lyrics and tabs

    /// The playing track, its title and its length.
    fn sheet_track(&self) -> Option<(TrackRef, String, Option<u64>)> {
        let i = self.queue.current?;
        let item = self.queue.items.get(i)?;
        let title = item
            .meta
            .as_ref()
            .map(|m| m.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| item.track.loc.name());
        let length = item.meta.as_ref().and_then(|m| m.length_ms);
        Some((item.track.clone(), title, length))
    }

    pub fn lyrics(&mut self) -> Option<LyricsInfo> {
        let (track, title, _) = self.sheet_track()?;
        Some(match chippy_core::lyrics::find(&track) {
            Some(l) => LyricsInfo {
                title,
                source: format!(
                    "From {}{}",
                    l.source,
                    if l.synced { " · timed" } else { "" }
                ),
                synced: l.synced,
                lines: l
                    .lines
                    .into_iter()
                    .map(|x| LyricLine {
                        ms: x.ms.map_or(-1, |m| m as i64),
                        text: x.text,
                    })
                    .collect(),
                message: String::new(),
            },
            None => {
                let name = track.loc.name();
                let (stem, _) = formats::split_name(&name);
                LyricsInfo {
                    title,
                    source: String::new(),
                    synced: false,
                    lines: Vec::new(),
                    message: format!(
                        "No lyrics found for this track. Put them in a file beside it, \
                         “{stem}.lrc” (timed, so they follow the song) or “{stem}.txt”; \
                         lyrics in the file's own tags are found too."
                    ),
                }
            }
        })
    }

    pub fn open_tabs(&mut self) {
        if self.sheet_track().is_none() {
            self.toast("Play something first", true);
            return;
        }
        self.tabs_shown = true;
        self.load_tabs();
    }

    fn load_tabs(&mut self) {
        if let Some((track, _, length)) = self.sheet_track() {
            self.tabs.load(&track, length);
        }
        self.dirty.tabs = true;
    }

    /// Puts the tab away; what was worked out is kept, to come back to.
    pub fn close_tabs(&mut self) {
        self.tabs_shown = false;
        self.dirty.tabs = true;
    }

    /// How many fret numbers fit across a line of the tab as shown.
    pub fn set_tab_width(&mut self, digits: f32) {
        if self.tabs.set_width(digits) {
            self.dirty.tabs = true;
        }
    }

    pub fn set_tab_part(&mut self, i: usize) {
        self.tabs.set_part(i);
        self.dirty.tabs = true;
    }

    /// Item `i` of [`TabsState::merge_items`]: the shown part merged with
    /// another, or taken apart.
    pub fn merge_tab_part(&mut self, i: usize) {
        let choices = self.tabs.merge_choices();
        match choices.get(i) {
            Some(&(other, _)) => self.tabs.merge_with(other),
            None if i == choices.len() => self.tabs.unmerge(),
            None => return,
        }
        self.dirty.tabs = true;
    }

    pub fn set_tab_tuning(&mut self, i: usize) {
        self.tabs.set_tuning(i);
        self.dirty.tabs = true;
    }

    pub fn shift_tab_octave(&mut self, d: i32) {
        self.tabs.shift_octave(d);
        self.dirty.tabs = true;
    }

    pub fn nudge_tab(&mut self, d: i32) {
        self.tabs.nudge(d);
        self.dirty.tabs = true;
    }

    pub fn reset_tab_adjust(&mut self) {
        self.tabs.reset_adjust();
        self.dirty.tabs = true;
    }

    pub fn select_tab_bar(&mut self, i: usize, extend: bool) {
        self.tabs.select_bar(i, extend);
        self.dirty.tabs = true;
    }

    pub fn clear_tab_selection(&mut self) {
        self.tabs.clear_selection();
        self.dirty.tabs = true;
    }

    /// Plays from the start of bar `i`.
    pub fn seek_tab_bar(&mut self, i: usize) {
        if let Some(ms) = self.tabs.bar_start(i) {
            self.seek(ms);
        }
    }

    pub fn set_tab_sig(&mut self, i: usize) {
        self.tabs.set_sig(i);
        self.dirty.tabs = true;
    }

    pub fn step_tab_speed(&mut self, d: i32) {
        self.tabs.step_speed(d);
        self.dirty.tabs = true;
    }

    pub fn shift_tab_bars(&mut self, d: i32) {
        self.tabs.shift_bars(d);
        self.dirty.tabs = true;
    }

    pub fn reset_tab_meter(&mut self) {
        self.tabs.reset_meter();
        self.dirty.tabs = true;
    }

    /// Saves the tab, every part, in the export folder: as Guitar Pro, or
    /// MusicXML.
    pub fn save_tab(&mut self, guitar_pro: bool) {
        let Some(folder) = self.settings.export_dir.clone() else {
            self.toast(
                "Choose an export folder first (Export, in the playlist's menu)",
                true,
            );
            return;
        };
        let title = self.sheet_track().map(|t| t.1).unwrap_or_default();
        let Some(score) = self.tabs.score(&title) else {
            return;
        };
        let format = if guitar_pro {
            Format::GuitarPro
        } else {
            Format::MusicXml
        };
        let mut name = export::safe_name(&title);
        if name.is_empty() {
            name = "Tab".into();
        }
        let path = folder.join(format!("{name}.{}", format.extension()));
        let done = std::fs::create_dir_all(&folder)
            .map_err(anyhow::Error::from)
            .and_then(|_| export::write_notes(&score, format, &path));
        match done {
            Ok(()) => self.toast(format!("Saved {}", pretty(&path)), false),
            Err(e) => self.toast(format!("Couldn't save the tab: {e:#}"), true),
        }
    }

    /// Takes in a finished tab, and follows the playlist to the next
    /// track; true while one is being worked out.
    fn tabs_tick(&mut self) -> bool {
        let playing = self
            .queue
            .current
            .and_then(|i| self.queue.items.get(i))
            .map(|i| i.track.clone());
        // Whether there are lyrics and notes, for the buttons.
        let notes = self.avail.notes(&self.tabs);
        if self.avail.check(playing.as_ref()) | self.avail.poll()
            || notes != self.avail.notes(&self.tabs)
        {
            self.dirty.now = true;
        }
        let status = midimode::status();
        if status != self.midi_status {
            self.midi_status = status;
            self.dirty.now = true;
            if status == midimode::Status::Failed {
                self.toast(
                    format!("Can't play this track as MIDI: {}", midimode::error()),
                    true,
                );
            }
        }
        let pending = self.avail.pending();
        if !self.tabs_shown {
            return pending;
        }
        if playing.is_some() && playing != self.tabs.track {
            self.load_tabs();
        }
        if self.tabs.poll() {
            self.dirty.tabs = true;
            // The notes may have turned out not to be there.
            self.dirty.now = true;
        }
        self.tabs.busy() || pending
    }

    fn tabs_state(&self) -> TabsState {
        let v = &self.tabs;
        let title = self.sheet_track().map(|t| t.1).unwrap_or_default();
        let selected = v.selection();
        let nudged = v.nudged_bars();
        TabsState {
            shown: self.tabs_shown,
            busy: v.busy(),
            message: v.message(),
            parts: v.part_names(),
            part: v.part_index() as u32,
            merge_items: v
                .merge_choices()
                .into_iter()
                .map(|(_, name)| format!("With {name}"))
                .chain(v.is_merged().then(|| "Take this merge apart".to_string()))
                .collect(),
            tunings: TabView::tuning_names(),
            tuning: v.tuning_index() as u32,
            strings: v.strings(),
            lines: v
                .lines()
                .iter()
                .map(|l| TabLine {
                    start_ms: l.start_ms as u64,
                    end_ms: l.end_ms as u64,
                    label: l.label.clone(),
                    notes: l
                        .cells
                        .iter()
                        .map(|&(x, string, fret, soft)| TabNote {
                            x,
                            string,
                            fret,
                            soft,
                        })
                        .collect(),
                    marks: l
                        .marks
                        .iter()
                        .map(|&(ms, x)| TabMark {
                            ms: ms.max(0.0) as u64,
                            x,
                        })
                        .collect(),
                    bars: l
                        .segments
                        .iter()
                        .map(|&(x0, x1, i)| TabBar {
                            x0,
                            x1,
                            index: i as u32,
                            line: l.bars.iter().any(|b| b.1 == i),
                            label: (i + 1).to_string(),
                            selected: selected.is_some_and(|(a, b)| (a..=b).contains(&i)),
                            nudged: nudged.contains(&i),
                        })
                        .collect(),
                })
                .collect(),
            detail: v.detail(),
            text: v.text(&title),
            title,
            octave: v.adjust().octave,
            position: v.steps(),
            can_up: v.can_nudge(true),
            can_down: v.can_nudge(false),
            adjusted: v.adjusted(),
            selection: match selected {
                Some((a, b)) if a == b => format!("Bar {}", a + 1),
                Some((a, b)) => format!("Bars {}–{}", a + 1, b + 1),
                None => String::new(),
            },
            has_bars: v.meter().is_some(),
            meter_text: v.meter_text(),
            sigs: TabView::sig_names(),
            sig: v.sig_index() as u32,
            speed: v.speed(),
            meter_adjusted: v.meter_adjusted(),
        }
    }

    // ---------------------------------------------------------- settings

    fn mark_dirty(&mut self) {
        self.settings_dirty.get_or_insert_with(Instant::now);
        self.dirty.settings = true;
    }

    pub fn save_settings(&mut self) {
        self.settings_dirty = None;
        self.settings.volume = self.settings.volume.clamp(0.0, 1.0);
        if let Err(e) = self.settings.save() {
            log::warn!("could not save settings: {e}");
        }
    }

    fn mark_session(&mut self) {
        if self.settings.restore_session {
            self.session_dirty.get_or_insert_with(Instant::now);
        }
    }

    pub fn save_session(&mut self) {
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

    pub fn set_flag(&mut self, id: &str, v: bool) {
        let s = &mut self.settings;
        match id {
            "show-all" => {
                s.show_all_files = v;
                self.refresh();
            }
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
            _ => return,
        }
        self.player.send(Cmd::Options(self.decode_options()));
        self.mark_dirty();
    }

    pub fn step(&mut self, id: &str, d: i32) {
        let s = &mut self.settings;
        match id {
            "default-length" => {
                s.default_length_s = (s.default_length_s as i32 + d * 15).clamp(15, 3600) as u32
            }
            "fade" => s.fade_s = (s.fade_s as i32 + d).clamp(0, 30) as u32,
            "loops" => s.loops = (s.loops + d as f32).clamp(1.0, 10.0),
            "bars" => {
                s.spectrum_bars = (s.spectrum_bars as i32 + d * 4).clamp(8, 96) as u32;
                self.spectrum = Spectrum::new(s.spectrum_bars as usize);
            }
            _ => return,
        }
        self.player.send(Cmd::Options(self.decode_options()));
        self.mark_dirty();
    }

    pub fn set_theme(&mut self, name: &str) {
        self.settings.theme = name.to_string();
        self.mark_dirty();
    }

    // -------------------------------------------------------------- MIDI

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
        if self.soundfont_index().is_none() {
            self.settings.soundfont = self.soundfonts.first().map(|p| file_name(p));
        }
        self.dirty.settings = true;
        self.dirty.now = true;
    }

    fn soundfont_index(&self) -> Option<usize> {
        let name = self.settings.soundfont.as_deref()?;
        self.soundfonts.iter().position(|p| file_name(p) == name)
    }

    pub fn set_soundfont_dir(&mut self, dir: PathBuf) {
        if !dir.is_dir() {
            self.toast(format!("{} is not a folder", pretty(&dir)), true);
            return;
        }
        self.settings.soundfont_dir = Some(dir);
        self.settings.soundfont = None;
        self.scan_soundfonts();
        self.load_soundfont();
        self.mark_dirty();
        if self.soundfonts.is_empty() {
            self.toast("No SoundFonts (.sf2) in that folder", true);
        }
    }

    pub fn soundfont_use_browser_dir(&mut self) {
        match &self.node {
            Some(Node::Dir(d)) => {
                let d = d.clone();
                self.set_soundfont_dir(d);
            }
            _ => self.toast("SoundFonts have to be in a folder on disk", true),
        }
    }

    // ----------------------------------------------------------- MIDI mode

    fn midi_style(&self) -> Style {
        Style::from_id(&self.settings.midi_style).unwrap_or(Style::Original)
    }

    pub fn toggle_midi_mode(&mut self) {
        self.settings.midi_mode = !self.settings.midi_mode;
        self.apply_midi_mode();
    }

    pub fn set_midi_style(&mut self, i: u32) {
        let style = Style::from_index(i as usize).unwrap_or(Style::Original);
        self.settings.midi_style = style.id().into();
        self.settings.midi_mode = true;
        self.apply_midi_mode();
    }

    fn apply_midi_mode(&mut self) {
        midimode::set(self.settings.midi_mode.then(|| self.midi_style()));
        self.mark_dirty();
        self.dirty.now = true;
        if self.settings.midi_mode && self.settings.soundfont.is_none() {
            self.toast(
                "MIDI mode needs a SoundFont: choose a folder of SF2 files in Settings, under MIDI",
                true,
            );
        }
    }

    pub fn pick_soundfont(&mut self, i: i32) {
        let Some(p) = self.soundfonts.get(i.max(0) as usize) else {
            return;
        };
        self.settings.soundfont = Some(file_name(p));
        self.mark_dirty();
        self.dirty.now = true;
        self.load_soundfont();
    }

    pub fn cycle_soundfont(&mut self, d: i32) {
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
            format!("SoundFont: {}", formats::split_name(&name).0),
            false,
        );
    }

    /// Loads the chosen SoundFont in the background and hands it to the
    /// MIDI player, which switches to it even in the middle of a song.
    fn load_soundfont(&mut self) {
        let id = self.font_request.fetch_add(1, Ordering::SeqCst) + 1;
        self.dirty.settings = true;
        let Some(path) = self.soundfont_index().map(|i| self.soundfonts[i].clone()) else {
            midi::set_soundfont(None);
            self.soundfont_status.clear();
            return;
        };
        if let Some(i) = self.font_cache.iter().position(|(p, _)| *p == path) {
            let entry = self.font_cache.remove(i);
            midi::set_soundfont(Some(entry.1.clone()));
            self.font_cache.push(entry);
            self.soundfont_status.clear();
            return;
        }
        midi::soundfont_loading();
        self.soundfont_status = format!("Loading {}…", file_name(&path));
        let (tx, latest) = (self.font_tx.clone(), self.font_request.clone());
        std::thread::Builder::new()
            .name("soundfont".into())
            .spawn(move || {
                chippy_engine::background_thread();
                let result = midi::load_soundfont(&path).map_err(|e| format!("{e:#}"));
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
        self.dirty.settings = true;
        match f.result {
            Ok(font) => {
                self.font_cache.retain(|(p, _)| *p != f.path);
                self.font_cache.push((f.path, font));
                // Large SoundFonts take a lot of memory: keep a few.
                if self.font_cache.len() > 3 {
                    self.font_cache.remove(0);
                }
                self.soundfont_status.clear();
            }
            Err(e) => {
                self.soundfont_status = e.clone();
                self.toast(e, true);
            }
        }
    }

    // ------------------------------------------------------ file browser

    fn home_dir(&self) -> PathBuf {
        self.settings
            .music_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(paths::home)
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

    pub fn add_music_dir(&mut self, p: PathBuf) {
        if !p.is_dir() {
            self.toast(format!("{} is not a folder", pretty(&p)), true);
            return;
        }
        if self.settings.music_dirs.contains(&p) {
            self.toast(format!("{} is already a music folder", pretty(&p)), false);
            return;
        }
        self.settings.music_dirs.push(p);
        if self.settings.music_dir.is_none() {
            self.settings.music_dir = self.settings.music_dirs.first().cloned();
        }
        self.mark_dirty();
        self.dirty.browser = true;
        self.toast("Music folder added", false);
    }

    /// Adds the browser's folder (an archive's folder, when in one).
    pub fn use_browser_dir(&mut self) {
        let Some(mut node) = self.node.clone() else {
            return;
        };
        while !matches!(node, Node::Dir(_)) {
            match node.parent() {
                Some(p) => node = p,
                None => break,
            }
        }
        self.add_music_dir(node.vpath());
    }

    pub fn remove_music_dir(&mut self, i: i32) {
        if i < 0 || i as usize >= self.settings.music_dirs.len() {
            return;
        }
        let d = self.settings.music_dirs.remove(i as usize);
        self.dir_places.remove(&d);
        if self.settings.music_dir.as_ref() == Some(&d) {
            self.settings.music_dir = self.settings.music_dirs.first().cloned();
        }
        self.mark_dirty();
        self.dirty.browser = true;
    }

    pub fn set_home_dir(&mut self, i: i32) {
        if let Some(d) = self.settings.music_dirs.get(i.max(0) as usize) {
            self.settings.music_dir = Some(d.clone());
            self.mark_dirty();
            self.dirty.browser = true;
        }
    }

    /// Switches the browser to a music folder: back to where it was in it,
    /// or to its top if it is there already.
    pub fn go_music_dir(&mut self, i: i32) {
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

    pub fn go_home(&mut self) {
        let h = self.home_dir();
        self.navigate(&h, true);
    }

    /// Shows a virtual path in the browser. Returns false when it cannot.
    pub fn navigate(&mut self, vpath: &Path, push: bool) -> bool {
        let node = match vfs::resolve(vpath) {
            Ok(n) => n,
            Err(e) => {
                self.set_status(format!("{e:#}"), true);
                return false;
            }
        };
        let entries = match vfs::list(&node, self.settings.show_all_files, &self.decode_options()) {
            Ok(e) => e,
            Err(e) => {
                self.set_status(format!("Cannot open {}: {e:#}", pretty(vpath)), true);
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
        self.search.clear();
        self.end_search();
        self.browser_serial += 1;
        self.relist(false);
        // Coming up out of a folder: mark the folder we left.
        self.selected = came_from
            .and_then(|from| {
                self.shown
                    .iter()
                    .position(|&i| from.starts_with(&self.entries[i].vpath))
            })
            .map(|i| i as i32)
            .unwrap_or(-1);
        self.dirty.settings = true;
        true
    }

    pub fn go_path(&mut self, path: &str) {
        let p = PathBuf::from(path.trim());
        self.navigate(&p, true);
    }

    pub fn refresh(&mut self) {
        if self.in_results {
            self.start_search();
            return;
        }
        if let Some(n) = self.node.clone() {
            let (sel, filter, serial) = (self.selected, self.filter.clone(), self.browser_serial);
            if self.navigate(&n.vpath(), false) {
                // The same folder: keep the place in it.
                self.browser_serial = serial;
                if !filter.is_empty() {
                    self.filter = filter;
                    self.relist(false);
                }
                self.selected = sel.min(self.shown.len() as i32 - 1);
            }
        }
    }

    pub fn go_up(&mut self) {
        if let Some(p) = self.node.as_ref().and_then(Node::parent) {
            self.navigate(&p.vpath(), true);
        }
    }

    pub fn go_history(&mut self, d: i32) {
        let pos = self.hist_pos as i32 + d;
        if pos < 0 || pos as usize >= self.history.len() {
            return;
        }
        let p = self.history[pos as usize].clone();
        if self.navigate(&p, false) {
            self.hist_pos = pos as usize;
        }
    }

    fn set_status(&mut self, text: String, error: bool) {
        self.status = text;
        self.status_error = error;
        self.dirty.browser = true;
    }

    pub fn set_filter(&mut self, t: &str) {
        self.filter = t.to_string();
        self.relist(false);
    }

    /// Lists the entries that pass the filter; `keep_selection` while
    /// search results come in.
    fn relist(&mut self, keep_selection: bool) {
        self.shown = (0..self.entries.len())
            .filter(|&i| vfs::matches_filter(&self.entries[i].name, &self.filter))
            .collect();
        if keep_selection {
            self.selected = self.selected.min(self.shown.len() as i32 - 1);
        } else {
            self.selected = -1;
        }
        let (mut dirs, mut files, mut tracks) = (0, 0, 0);
        for &i in &self.shown {
            match self.entries[i].kind {
                EntryKind::Dir | EntryKind::Archive => dirs += 1,
                EntryKind::Track => tracks += 1,
                _ => files += 1,
            }
        }
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = Vec::new();
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
        self.set_status(status, false);
    }

    // ------------------------------------------------------------ search

    pub fn set_search(&mut self, text: &str) {
        self.search = text.to_string();
        self.dirty.browser = true;
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
                self.browser_serial += 1;
                self.relist(false);
            }
        } else {
            self.search_due = Some(Instant::now() + Duration::from_millis(300));
        }
    }

    /// Starts the search typed at once (the keyboard's search key).
    pub fn run_search(&mut self) {
        if self.search_due.is_some() {
            self.start_search();
        }
    }

    /// Stops any search; the listing is left as it is.
    fn end_search(&mut self) {
        self.search_gen.fetch_add(1, Ordering::SeqCst);
        self.search_due = None;
        self.searching = false;
        self.in_results = false;
        self.dirty.browser = true;
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
        self.browser_serial += 1;
        self.relist(false);
        let (tx, current, show_all) = (
            self.search_tx.clone(),
            self.search_gen.clone(),
            self.settings.show_all_files,
        );
        std::thread::spawn(move || {
            chippy_engine::background_thread();
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
                None => self.searching = false,
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

    /// A tap on a listed entry: opens folders, plays music.
    pub fn activate(&mut self, shown_index: usize) {
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
                    if let Some(t) = &e.track {
                        match decode::track_list(&t.loc, &self.decode_options()) {
                            Ok(t) if t.len() > 1 => {
                                self.navigate(&e.vpath, true);
                                return;
                            }
                            Ok(_) => {}
                            Err(err) => {
                                self.toast(format!("Cannot open {}: {err:#}", e.name), true);
                                return;
                            }
                        }
                    }
                }
                self.play_from_listing(i);
            }
            EntryKind::Track => self.play_from_listing(i),
            EntryKind::Image | EntryKind::Other => {
                self.toast(format!("{} is not something Chippy can play", e.name), true);
            }
        }
    }

    /// Plays entry `i` of the listing, with every other playable entry of
    /// the listing as the playlist, in listed order. A search result plays
    /// with the rest of its own folder.
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

    /// Adds a listed entry (a folder: its music) to the playlist.
    pub fn add_to_queue(&mut self, shown_index: usize) {
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
        self.queue_rebuilt();
        self.load_meta_from(start);
        self.toast(
            format!("Added {n} {}", if n == 1 { "track" } else { "tracks" }),
            false,
        );
    }

    /// Shows a playlist row's file in the browser.
    pub fn reveal(&mut self, i: usize) {
        let Some(item) = self.queue.items.get(i) else {
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
                self.selected = pos as i32;
            }
            self.events.push(UiEvent::OpenBrowser);
        }
    }

    // ---------------------------------------------------------- playlist

    fn set_queue(&mut self, tracks: Vec<TrackRef>, current: Option<usize>) {
        self.queue.replace(tracks, current);
        self.queue.set_shuffle(self.settings.shuffle);
        self.queue_gen += 1;
        self.queue_rebuilt();
        self.load_meta_from(0);
    }

    /// The playlist changed: what to send, what comes next, the session.
    fn queue_rebuilt(&mut self) {
        self.mark_session();
        self.playlist_changed();
        self.dirty.now = true;
    }

    /// Reads tags for the playlist from `start` on, in the background;
    /// cached tags are filled in at once.
    fn load_meta_from(&mut self, start: usize) {
        let mut todo = Vec::new();
        for i in start..self.queue.len() {
            let t = self.queue.items[i].track.clone();
            if let Some(m) = self.meta_cache.get(&t) {
                self.queue.items[i].meta = Some(m.clone());
            } else {
                todo.push((i, t));
            }
        }
        self.playlist_changed();
        if todo.is_empty() {
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
                chippy_engine::background_thread();
                for (i, t) in todo {
                    // A decoder bug on one file must not leave the rest of
                    // the playlist without tags.
                    let m = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        meta::read(&t, &opts)
                    }))
                    .unwrap_or_else(|_| {
                        let mut m = TrackMeta::default();
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

    pub fn remove_from_queue(&mut self, i: usize) {
        if i >= self.queue.len() {
            return;
        }
        let was_current = self.queue.current == Some(i);
        self.queue.remove(i);
        if was_current {
            self.stop();
        }
        self.queue_rebuilt();
        self.dirty.browser = true;
    }

    pub fn clear_queue(&mut self) {
        self.stop();
        self.queue.clear();
        self.queue_gen += 1;
        self.queue_rebuilt();
        self.dirty.browser = true;
        self.load_art();
    }

    // ---------------------------------------------------------- playback

    pub fn play_index(&mut self, i: usize) {
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
        self.reveal_serial += 1;
        self.playlist_changed();
        self.dirty.now = true;
        self.dirty.browser = true;
        self.load_art();
    }

    pub fn play_pause(&mut self) {
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
                match self
                    .queue
                    .current
                    .or_else(|| self.queue.next(Repeat::Off, false))
                {
                    Some(i) => self.play_index(i),
                    None => self.events.push(UiEvent::OpenBrowser),
                }
                return;
            }
        }
        self.dirty.now = true;
    }

    pub fn play(&mut self) {
        if self.state != State::Playing {
            self.play_pause();
        }
    }

    pub fn pause(&mut self) {
        if self.state == State::Playing {
            self.play_pause();
        }
    }

    pub fn stop(&mut self) {
        self.player.send(Cmd::Stop);
        self.state = State::Stopped;
        self.started = false;
        self.generation += 1;
        self.output.shared.lock().unwrap().paused = false;
        self.mark_session();
        self.dirty.now = true;
    }

    pub fn next(&mut self, auto: bool) {
        match self.queue.next(self.settings.repeat, auto) {
            Some(i) => self.play_index(i),
            None => self.stop(),
        }
    }

    pub fn previous(&mut self) {
        if self.state != State::Stopped && self.position_ms() > 3000 {
            self.seek(0);
            return;
        }
        if let Some(i) = self.queue.previous(self.settings.repeat) {
            self.play_index(i);
        }
    }

    pub fn seek(&mut self, ms: u64) {
        if self.state == State::Stopped || !self.started {
            return;
        }
        let ms = self
            .duration_ms
            .map(|d| ms.min(d.saturating_sub(250)))
            .unwrap_or(ms);
        self.player.send(Cmd::Seek { ms });
        self.mark_session();
        // Show the new place at once.
        self.output.shared.lock().unwrap().reset(ms);
        self.seek_serial += 1;
        self.dirty.now = true;
    }

    pub fn seek_fraction(&mut self, f: f32) {
        if let Some(d) = self.duration_ms {
            self.seek((d as f64 * f.clamp(0.0, 1.0) as f64) as u64);
        }
    }

    pub fn seek_by(&mut self, seconds: i64) {
        let pos = self.position_ms() as i64 + seconds * 1000;
        self.seek(pos.max(0) as u64);
    }

    fn position_ms(&self) -> u64 {
        let s = self.output.shared.lock().unwrap();
        if s.generation != self.generation {
            return 0;
        }
        s.base_ms + s.frames_played * 1000 / self.output.rate as u64
    }

    pub fn set_volume(&mut self, v: f32) {
        self.settings.volume = v.clamp(0.0, 1.0);
        self.muted = false;
        self.apply_volume();
        self.mark_dirty();
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.apply_volume();
    }

    fn apply_volume(&mut self) {
        let v = if self.muted {
            0.0
        } else {
            self.settings.volume
        };
        self.output.shared.lock().unwrap().volume = v;
        self.dirty.now = true;
    }

    pub fn toggle_shuffle(&mut self) {
        self.settings.shuffle = !self.settings.shuffle;
        self.queue.set_shuffle(self.settings.shuffle);
        self.dirty.now = true;
        self.mark_dirty();
    }

    pub fn cycle_repeat(&mut self) {
        self.settings.repeat = self.settings.repeat.next();
        self.dirty.now = true;
        self.mark_dirty();
    }

    fn load_art(&mut self) {
        let Some(t) = self.current_track().cloned() else {
            if self.art_for.take().is_some() {
                self.art_gen += 1;
                self.art_out = Some(None);
            }
            return;
        };
        // Tracks of one file share their art: no need to look again.
        if self.art_for.as_ref() == Some(&t.loc) {
            return;
        }
        self.art_for = Some(t.loc.clone());
        self.art_gen += 1;
        let (gen, tx, prefer) = (
            self.art_gen,
            self.art_tx.clone(),
            self.settings.prefer_embedded_art,
        );
        // The picture is sent as it is stored (JPEG, PNG...): Android
        // decodes it, at the size it is shown.
        std::thread::Builder::new()
            .name("art".into())
            .spawn(move || {
                chippy_engine::background_thread();
                let art = chippy_core::art::find(&t.loc, prefer).map(|a| a.data);
                let _ = tx.send((gen, art));
            })
            .ok();
    }

    // ------------------------------------------------------------ export

    /// Gets the export dialog ready for one playlist row, or all (None).
    pub fn prepare_export(&mut self, row: Option<usize>) -> bool {
        if self.export.cancel.is_some() {
            return true;
        }
        let rows: Vec<usize> = match row {
            None => (0..self.queue.len()).collect(),
            Some(r) if r < self.queue.len() => vec![r],
            Some(_) => Vec::new(),
        };
        if rows.is_empty() {
            self.toast("The playlist is empty", true);
            return false;
        }
        self.export.what = if rows.len() == 1 {
            let item = &self.queue.items[rows[0]];
            format!(
                "“{}”",
                item.meta
                    .as_ref()
                    .map(|m| m.title.clone())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| item.track.loc.name())
            )
        } else {
            format!("{} tracks", rows.len())
        };
        self.export.rows = rows;
        self.export.status.clear();
        self.export.progress = 0.0;
        self.dirty.export = true;
        true
    }

    pub fn set_export_dir(&mut self, d: PathBuf) {
        self.settings.export_dir = Some(d);
        self.mark_dirty();
        self.dirty.export = true;
    }

    /// How many of the rows to export can be MIDI, of how many.
    fn midi_count(&self) -> (usize, usize) {
        let rows = &self.export.rows;
        let ok = rows
            .iter()
            .filter_map(|&i| self.queue.items.get(i))
            .filter(|item| export::midi_possible(&item.track))
            .count();
        (ok, rows.len())
    }

    /// The format to export to: the one chosen, unless that is the notes
    /// and none of the tracks' notes can be found.
    fn export_format(&self) -> Format {
        match Format::from_index(self.settings.export_format) {
            f if f.is_notes() && self.midi_count().0 == 0 => Format::Flac,
            f => f,
        }
    }

    pub fn set_export_format(&mut self, f: ExportFormat) {
        if Format::from_index(f as u32).is_notes() && self.midi_count().0 == 0 {
            return;
        }
        self.settings.export_format = f as u32;
        self.mark_dirty();
        self.dirty.export = true;
    }

    pub fn step_export_bitrate(&mut self, d: i32) {
        const MP3: &[u32] = &[96, 128, 160, 192, 224, 256, 320];
        const OPUS: &[u32] = &[64, 96, 128, 160, 192, 256];
        let (steps, v) = match Format::from_index(self.settings.export_format) {
            Format::Opus => (OPUS, &mut self.settings.export_opus_kbps),
            _ => (MP3, &mut self.settings.export_mp3_kbps),
        };
        let pos = steps
            .iter()
            .position(|&s| s >= *v)
            .unwrap_or(steps.len() - 1) as i32;
        *v = steps[(pos + d).clamp(0, steps.len() as i32 - 1) as usize];
        self.mark_dirty();
        self.dirty.export = true;
    }

    pub fn start_export(&mut self) {
        if self.export.cancel.is_some() {
            return;
        }
        let Some(folder) = self.settings.export_dir.clone() else {
            self.toast("Choose a folder first", true);
            return;
        };
        if let Err(e) = std::fs::create_dir_all(&folder) {
            self.toast(format!("Cannot use {}: {e}", pretty(&folder)), true);
            return;
        }
        let opts = self.decode_options();
        let format = self.export_format();
        let tracks: Vec<(TrackRef, TrackMeta)> = self
            .export
            .rows
            .iter()
            .filter_map(|&i| self.queue.items.get(i))
            .filter(|item| !format.is_notes() || export::midi_possible(&item.track))
            .map(|item| {
                let m = item
                    .meta
                    .clone()
                    .unwrap_or_else(|| meta::read(&item.track, &opts));
                (item.track.clone(), m)
            })
            .collect();
        if tracks.is_empty() {
            return;
        }
        // The WonderSwan core plays one song at a time.
        let is_wsr = |t: &TrackRef| t.loc.extension() == "wsr";
        if tracks.iter().any(|(t, _)| is_wsr(t)) && self.current_track().is_some_and(is_wsr) {
            self.stop();
            self.toast(
                "Stopped playback: WonderSwan songs export one at a time",
                false,
            );
        }
        let kbps = match format {
            Format::Opus => self.settings.export_opus_kbps,
            _ => self.settings.export_mp3_kbps,
        };
        let shown = self.tabs.shown_for_export(&tracks, format);
        let cancel = Arc::new(AtomicBool::new(false));
        self.export.cancel = Some(cancel.clone());
        self.export.total = tracks.len();
        self.export.index = 0;
        self.export.progress = 0.0;
        self.export.status = "Starting…".into();
        export::spawn(
            export::Job {
                tracks,
                folder,
                format,
                kbps,
                opts,
                shown,
            },
            cancel,
            self.export_tx.clone(),
        );
        self.dirty.export = true;
    }

    pub fn cancel_export(&mut self) {
        if let Some(c) = &self.export.cancel {
            c.store(true, Ordering::Relaxed);
            self.export.status = "Cancelling…".into();
            self.dirty.export = true;
        }
    }

    fn export_progress(&mut self, p: Progress) {
        self.dirty.export = true;
        match p {
            Progress::Track { index, total, name } => {
                self.export.index = index;
                self.export.total = total;
                self.export.status = format!("{} of {}: {}", index + 1, total, name);
                self.export.name = name;
                self.export.progress = index as f32 / total.max(1) as f32;
            }
            Progress::Fraction(f) => {
                let total = self.export.total.max(1) as f32;
                self.export.progress = (self.export.index as f32 + f) / total;
            }
            Progress::Finished {
                written,
                failed,
                cancelled,
            } => {
                self.export.cancel = None;
                self.export.progress = if cancelled { 0.0 } else { 1.0 };
                let files = |n: usize| if n == 1 { "file" } else { "files" };
                let msg = if cancelled {
                    format!("Cancelled after {written} {}", files(written))
                } else if failed.is_empty() {
                    format!("Exported {written} {}", files(written))
                } else {
                    format!(
                        "Exported {written} {}; {} failed ({})",
                        files(written),
                        failed.len(),
                        failed[0]
                    )
                };
                self.export.status = msg.clone();
                self.toast(msg, !failed.is_empty());
            }
        }
    }

    // -------------------------------------------------------- properties

    pub fn track_info(&self, i: usize) -> Option<TrackInfo> {
        let item = self.queue.items.get(i)?;
        let t = item.track.clone();
        let m = item
            .meta
            .clone()
            .unwrap_or_else(|| meta::read(&t, &self.decode_options()));
        Some(TrackInfo {
            title: if m.title.is_empty() {
                t.loc.name()
            } else {
                m.title.clone()
            },
            fields: info_fields(&t, &m)
                .into_iter()
                .map(|(label, value)| InfoField { label, value })
                .collect(),
        })
    }

    /// Where a folder picker should open for a setting.
    pub fn picker_start(&self, which: &str) -> Option<String> {
        let current = match which {
            "soundfont" => self.settings.soundfont_dir.clone(),
            "export" => self.settings.export_dir.clone(),
            _ => None,
        };
        current
            .filter(|d| d.is_dir())
            .or_else(|| {
                let mut node = self.node.clone();
                while let Some(n) = node {
                    if let Node::Dir(d) = &n {
                        return Some(d.clone());
                    }
                    node = n.parent();
                }
                None
            })
            .map(|p| p.to_string_lossy().into_owned())
    }

    // ------------------------------------------------------------- clock

    /// What moves on every frame: the position and the spectrum.
    pub fn frame(&mut self) -> Frame {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let (pos, samples) = {
            let s = self.output.shared.lock().unwrap();
            let current = s.generation == self.generation;
            let pos = if !current {
                0
            } else {
                s.base_ms + s.frames_played * 1000 / self.output.rate as u64
            };
            let samples = (self.state == State::Playing && current).then(|| s.scope_tail(FFT_LEN));
            (pos, samples)
        };
        let pos = match self.state {
            State::Stopped => 0,
            _ if !self.started => self.start_ms,
            _ => pos,
        };
        if samples.is_some() || !self.spectrum.is_still() {
            self.spectrum
                .update(samples.as_deref(), self.output.rate, dt);
        }
        Frame {
            position_ms: pos,
            fraction: self
                .duration_ms
                .filter(|&d| d > 0)
                .map(|d| (pos as f64 / d as f64).min(1.0) as f32)
                .unwrap_or(0.0),
            elapsed: format_ms(pos),
            bars: self.spectrum.bars.clone(),
            peaks: self.spectrum.peaks.clone(),
        }
    }

    /// Takes in what finished in the background and moves playback along;
    /// true while something moves (the clock should tick quickly).
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
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
                    self.dirty.now = true;
                }
                Event::Failed { generation, error } if generation == self.generation => {
                    let name = self
                        .current_track()
                        .map(|t| t.loc.name())
                        .unwrap_or_default();
                    self.toast(format!("Cannot play {name}: {error}"), true);
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

        while let Ok((generation, i, track, m)) = self.meta_rx.try_recv() {
            // Cached under the track that was read: the playlist may have
            // changed since, and row `i` may now be another track.
            self.meta_cache.insert(track.clone(), m.clone());
            if let Some(item) = self.queue.items.get_mut(i) {
                if generation == self.queue_gen && item.track == track {
                    item.meta = Some(m);
                    self.dirty.playlist = true;
                    // The track playing, or the one after it ("up next").
                    let next = self.queue.next(self.settings.repeat, true);
                    if self.queue.current == Some(i) || next == Some(i) {
                        self.dirty.now = true;
                    }
                }
            }
        }

        while let Ok((gen, data)) = self.art_rx.try_recv() {
            if gen == self.art_gen {
                self.art_out = Some(data);
            }
        }

        // The end of the track.
        let ended = {
            let s = self.output.shared.lock().unwrap();
            if s.underruns != self.underruns {
                log::warn!(
                    "the sound ran dry mid-track ({} times so far): decoding fell behind",
                    s.underruns
                );
                self.underruns = s.underruns;
            }
            s.generation == self.generation
                && self.started
                && self.state == State::Playing
                && s.producer_done
                && s.buffered_frames() == 0
        };
        if ended {
            self.next(true);
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
        let tabs_busy = self.tabs_tick();
        self.state == State::Playing
            || tabs_busy
            || self.searching
            || self.search_due.is_some()
            || self.export.cancel.is_some()
    }

    pub fn shutdown(&mut self) {
        self.save_settings();
        self.save_session();
        self.player.send(Cmd::Stop);
        self.state = State::Stopped;
    }
}

/// A track's properties, as (label, value) pairs; empty values left out.
fn info_fields(t: &TrackRef, m: &TrackMeta) -> Vec<(String, String)> {
    let mut f: Vec<(String, String)> = Vec::new();
    let mut add = |k: &str, v: String| {
        if !v.trim().is_empty() {
            f.push((k.to_string(), v));
        }
    };
    add("Title", m.title.clone());
    add("Artist", m.artist.clone());
    add("Album / game", m.album.clone());
    add(
        "Track",
        m.track_no.map(|n| n.to_string()).unwrap_or_default(),
    );
    add("Length", m.length_ms.map(format_ms).unwrap_or_default());
    add("Year", m.year.clone());
    add("Genre", m.genre.clone());
    add("System", m.system.clone());
    add("Copyright", m.copyright.clone());
    add("Ripped by", m.dumper.clone());
    add("Comment", m.comment.clone());
    add("Format", m.format.clone());
    add(
        "Sample rate",
        m.sample_rate.map(|r| format!("{r} Hz")).unwrap_or_default(),
    );
    add(
        "Channels",
        m.channels.map(|c| c.to_string()).unwrap_or_default(),
    );
    add(
        "Bitrate",
        m.bitrate_kbps
            .map(|b| format!("{b} kbps"))
            .unwrap_or_default(),
    );
    add(
        "Track in file",
        t.sub.map(|s| format!("{}", s + 1)).unwrap_or_default(),
    );
    add("File name", t.loc.name());
    add("Location", pretty(&t.loc.vpath()));
    let size = match &t.loc {
        vfs::Location::Fs(p) => std::fs::metadata(p).ok().map(|m| m.len()),
        vfs::Location::Archive { .. } => t.loc.read().ok().map(|d| d.len() as u64),
    };
    if let Some(size) = size {
        add("File size", human_size(size));
    }
    // Every tag of PSF-family files, including the ones not shown above.
    if let Ok(data) = t.loc.read() {
        if let Ok(p) = chippy_core::psf::parse(&data) {
            for (k, v) in &p.tags {
                add(&format!("Tag: {k}"), v.clone());
            }
        }
    }
    f
}
