//! What the engine tells the Kotlin side: plain records, one per part of
//! the screen, sent whole whenever that part changes (see `Listener`).

/// What a listed entry is, for its icon and what a tap does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FileKind {
    Folder,
    Archive,
    /// A music file.
    Audio,
    /// A music file that may hold several tracks (opens as a folder).
    MultiTrack,
    /// One track of a multi-track file.
    Track,
    Image,
    Other,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FileItem {
    /// The name without its extension (tracks and folders: all of it).
    pub stem: String,
    /// The extension, lower case, without the dot ("" if none shown).
    pub ext: String,
    pub kind: FileKind,
    /// The length (tracks), the size (other files), or where a search
    /// result was found.
    pub detail: String,
    /// It is the track playing.
    pub playing: bool,
}

/// One level of the path above the listing.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Crumb {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MusicDir {
    /// The folder's name.
    pub label: String,
    /// Where it is, written for people ("Internal storage/Music").
    pub path: String,
    /// The browser starts here (its home button).
    pub home: bool,
    /// The browser is in it now.
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BrowserState {
    /// The folder (or archive, or multi-track file) listed.
    pub title: String,
    pub crumbs: Vec<Crumb>,
    pub items: Vec<FileItem>,
    /// "12 folders · 3 tracks", or what went wrong.
    pub status: String,
    pub status_error: bool,
    pub can_up: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub filter: String,
    pub search: String,
    /// A search is still looking.
    pub searching: bool,
    /// The listing shows search results, not the folder.
    pub in_results: bool,
    pub music_dirs: Vec<MusicDir>,
    /// The entry to bring into view and mark (the folder just left, a track
    /// revealed from the playlist); -1 for none.
    pub selected: i32,
    /// Changes whenever another folder is listed (scroll back to the top,
    /// or to `selected`).
    pub serial: u64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PlRow {
    pub title: String,
    /// "Artist · 2:31".
    pub detail: String,
    pub playing: bool,
    /// Its tags are still being read.
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PlaylistState {
    pub rows: Vec<PlRow>,
    /// The row playing; -1 for none.
    pub playing: i32,
    /// "24 tracks · 1:02:13".
    pub summary: String,
    /// Changes whenever another track starts (bring it into view).
    pub reveal_serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NowState {
    pub state: PlayState,
    pub has_track: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// "NSF  ·  NES  ·  44.1 kHz".
    pub info: String,
    /// Where the track is, written for people.
    pub location: String,
    /// "2:31", or "–:––" when not known.
    pub total: String,
    pub duration_ms: Option<u64>,
    pub seekable: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// The title of the track after this one ("" if none).
    pub up_next: String,
    /// The track is MIDI: the SoundFont can be switched beside its title.
    pub midi: bool,
    /// The SoundFont in use (MIDI), without its extension.
    pub soundfont: String,
    pub volume: f32,
    pub muted: bool,
    /// Where playback is, as this state was sent (for the media session).
    pub position_ms: u64,
    /// Changes with every seek (the media session is told afresh).
    pub seek_serial: u64,
}

/// What moves on every display frame.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Frame {
    pub position_ms: u64,
    /// 0..1 of the track; 0 when its length is not known.
    pub fraction: f32,
    pub elapsed: String,
    /// The spectrum: bar heights and their peak caps, 0..1.
    pub bars: Vec<f32>,
    pub peaks: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SettingsState {
    pub music_dirs: Vec<MusicDir>,
    /// The browser's folder, written for people ("" if none).
    pub browser_dir: String,
    pub show_all: bool,
    pub folder_playlist: bool,
    pub restore_session: bool,
    pub embedded_art: bool,
    pub usf_hle: bool,
    pub gme_accuracy: bool,
    /// The steppers' values, as shown ("2:30", "8 s", "2", "32").
    pub default_length: String,
    pub fade: String,
    pub loops: String,
    pub bars: String,
    pub soundfont_dir: String,
    pub soundfonts: Vec<String>,
    /// Index into `soundfonts`; -1 for none.
    pub soundfont: i32,
    /// "Loading GeneralUser.sf2…", an error, or "".
    pub soundfont_status: String,
    /// The theme's name (the looks themselves are in Kotlin).
    pub theme: String,
    pub version: String,
    pub device: String,
    pub config_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ExportFormat {
    Wav,
    Flac,
    Mp3,
    Opus,
    Midi,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ExportState {
    /// "“Title”" or "12 tracks": what the dialog would export.
    pub what: String,
    pub format: ExportFormat,
    /// "Lossless and compressed, 16-bit."
    pub format_detail: String,
    /// kbps, for MP3 and Opus.
    pub bitrate: u32,
    pub folder: String,
    pub running: bool,
    /// 0..1.
    pub progress: f32,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct InfoField {
    pub label: String,
    pub value: String,
}

/// A track's properties.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TrackInfo {
    pub title: String,
    pub fields: Vec<InfoField>,
}

/// Something to show once.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum UiEvent {
    Toast {
        text: String,
        error: bool,
    },
    /// Show the file browser (a track revealed, or play with nothing to
    /// play).
    OpenBrowser,
}
