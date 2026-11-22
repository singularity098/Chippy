//! Chippy's settings, kept as JSON in the user's config folder
//! (`CHIPPY_HOME` overrides where).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::decode::DecodeOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
    pub fn index(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub id: String,
    pub visible: bool,
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where the file browser starts (its home button); one of
    /// `music_dirs`.
    pub music_dir: Option<PathBuf>,
    /// Every music folder, switched between in the file browser.
    pub music_dirs: Vec<PathBuf>,
    /// Bring back the last playlist, track and place at start-up.
    pub restore_session: bool,
    /// List every file, not only folders, archives and music.
    pub show_all_files: bool,
    /// A file opened from outside (command line, file manager) brings the
    /// other music of its folder along as the playlist.
    pub open_folder_as_playlist: bool,
    /// Open folders and play files with one tap (touch screens).
    pub single_tap: bool,
    /// The file browser comes in when the mouse rests on the left edge.
    pub hover_opens_browser: bool,
    pub volume: f32,
    pub repeat: Repeat,
    pub shuffle: bool,
    pub theme: String,
    pub fullscreen: bool,
    pub window_width: u32,
    pub window_height: u32,
    pub maximized: bool,
    /// Where the window was (its outer top-left corner, in physical
    /// pixels), when the window system says.
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    /// How much bigger than the system's size text and controls are drawn
    /// (1.0 is the system's size; Ctrl+plus and Ctrl+minus change it).
    pub zoom: f32,
    pub columns: Vec<Column>,
    pub prefer_embedded_art: bool,
    /// Seconds: tracks that do not say how long they are play this long.
    pub default_length_s: u32,
    pub fade_s: u32,
    pub loops: f32,
    pub usf_hle: bool,
    pub gme_accuracy: bool,
    /// Spectrum bar count.
    pub spectrum_bars: u32,
    /// A folder of SF2 SoundFonts for MIDI, and the file name of the one
    /// in use.
    pub soundfont_dir: Option<PathBuf>,
    pub soundfont: Option<String>,
    /// MIDI mode: chiptunes play their notes as MIDI, with the instruments
    /// of a style (its id, see `notes::style::Style`).
    pub midi_mode: bool,
    pub midi_style: String,
    /// Linux: run as a native Wayland window. Off, a Wayland desktop runs
    /// Chippy through XWayland, where dropping files on the window and the
    /// window's icon work (the toolkit supports neither on Wayland yet).
    pub native_wayland: bool,
    /// Export: 0 WAV, 1 FLAC, 2 MP3, 3 Opus; bitrates in kbps; the folder.
    pub export_format: u32,
    pub export_mp3_kbps: u32,
    pub export_opus_kbps: u32,
    pub export_dir: Option<PathBuf>,
}

pub fn default_columns() -> Vec<Column> {
    crate::meta::COLUMNS
        .iter()
        .map(|(id, _)| {
            let (visible, width) = match *id {
                "track" => (true, 50.0),
                "title" => (true, 200.0),
                "artist" => (true, 190.0),
                "album" => (true, 210.0),
                "length" => (true, 76.0),
                "path" => (false, 360.0),
                "comment" => (false, 260.0),
                "year" | "channels" => (false, 70.0),
                _ => (false, 130.0),
            };
            Column {
                id: id.to_string(),
                visible,
                width,
            }
        })
        .collect()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            music_dir: directories::UserDirs::new().and_then(|u| u.audio_dir().map(PathBuf::from)),
            music_dirs: Vec::new(),
            restore_session: true,
            show_all_files: false,
            open_folder_as_playlist: true,
            // Touch first on Android: a tap plays, and nothing hovers.
            single_tap: cfg!(target_os = "android"),
            hover_opens_browser: !cfg!(target_os = "android"),
            volume: 0.8,
            repeat: Repeat::All,
            shuffle: false,
            theme: "Iris".into(),
            fullscreen: false,
            window_width: 1360,
            window_height: 820,
            maximized: false,
            window_x: None,
            window_y: None,
            zoom: 1.0,
            columns: default_columns(),
            prefer_embedded_art: true,
            default_length_s: 150,
            fade_s: 8,
            loops: 2.0,
            usf_hle: true,
            native_wayland: false,
            export_format: 1,
            export_mp3_kbps: 320,
            export_opus_kbps: 160,
            export_dir: None,
            gme_accuracy: true,
            spectrum_bars: 32,
            soundfont_dir: None,
            soundfont: None,
            midi_mode: false,
            midi_style: "original".into(),
        }
    }
}

/// The folder that, beside the program, makes Chippy portable: settings
/// are kept in it instead of the user's profile (the Windows zip has one).
pub const PORTABLE_DIR: &str = "chippy-settings";

/// Where settings and the session are kept: `CHIPPY_HOME`; else a
/// `chippy-settings` folder beside the program, if there is one; else the
/// system's place (`~/.config/chippy`, `%APPDATA%\chippy\config`).
pub fn config_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("CHIPPY_HOME") {
        return PathBuf::from(h);
    }
    #[cfg(not(target_os = "android"))]
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(PORTABLE_DIR)))
        .filter(|d| d.is_dir())
    {
        return dir;
    }
    directories::ProjectDirs::from("", "", "chippy")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".chippy"))
}

impl Settings {
    pub fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    pub fn load() -> Settings {
        let mut s: Settings = std::fs::read(Self::path())
            .ok()
            .and_then(|d| {
                serde_json::from_slice(&d)
                    .map_err(|e| log::warn!("settings.json unreadable: {e}"))
                    .ok()
            })
            .unwrap_or_default();
        s.normalise();
        s
    }

    /// Keeps known columns (adding new ones) in the stored order.
    pub fn normalise(&mut self) {
        let known: Vec<&str> = crate::meta::COLUMNS.iter().map(|c| c.0).collect();
        self.columns.retain(|c| known.contains(&c.id.as_str()));
        for d in default_columns() {
            if !self.columns.iter().any(|c| c.id == d.id) {
                self.columns.push(d);
            }
        }
        if !self.columns.iter().any(|c| c.visible) {
            self.columns = default_columns();
        }
        for c in &mut self.columns {
            c.width = c.width.clamp(36.0, 1200.0);
        }
        self.volume = self.volume.clamp(0.0, 1.0);
        self.loops = self.loops.clamp(0.0, 20.0);
        self.spectrum_bars = self.spectrum_bars.clamp(8, 96);
        // Settings from before there were several music folders: the one
        // folder is the first. The home folder is always one of them.
        let mut seen = std::collections::HashSet::new();
        self.music_dirs.retain(|d| seen.insert(d.clone()));
        if let Some(home) = &self.music_dir {
            if !self.music_dirs.contains(home) {
                self.music_dirs.insert(0, home.clone());
            }
        } else {
            self.music_dir = self.music_dirs.first().cloned();
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, p)
    }

    pub fn decode_options(&self, sample_rate: u32) -> DecodeOptions {
        DecodeOptions {
            sample_rate,
            default_length_ms: self.default_length_s as u64 * 1000,
            fade_ms: self.fade_s as u64 * 1000,
            loops: self.loops as f64,
            usf_hle: self.usf_hle,
            gme_accuracy: self.gme_accuracy,
        }
    }
}

/// What was playing when Chippy last ran, kept beside the settings in
/// `session.json` (when [`Settings::restore_session`] is on).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub tracks: Vec<crate::vfs::TrackRef>,
    pub current: Option<usize>,
    pub position_ms: u64,
}

impl Session {
    pub fn path() -> PathBuf {
        config_dir().join("session.json")
    }

    pub fn load() -> Option<Session> {
        let d = std::fs::read(Self::path()).ok()?;
        serde_json::from_slice(&d)
            .map_err(|e| log::warn!("session.json unreadable: {e}"))
            .ok()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default())?;
        std::fs::rename(tmp, p)
    }

    pub fn forget() {
        let _ = std::fs::remove_file(Self::path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn music_dirs_include_home() {
        let mut s = Settings {
            music_dir: Some("/m/a".into()),
            music_dirs: vec!["/m/b".into(), "/m/b".into()],
            ..Default::default()
        };
        s.normalise();
        assert_eq!(s.music_dirs, [PathBuf::from("/m/a"), PathBuf::from("/m/b")]);
        let mut s = Settings {
            music_dir: None,
            music_dirs: vec!["/m/b".into()],
            ..Default::default()
        };
        s.normalise();
        assert_eq!(s.music_dir, Some(PathBuf::from("/m/b")));
    }

    #[test]
    fn round_trip_and_unknown_fields() {
        let s = Settings::default();
        let j = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
        let partial: Settings = serde_json::from_str(r#"{"volume": 0.3, "bogus": 1}"#).unwrap();
        assert_eq!(partial.volume, 0.3);
        assert!(!partial.columns.is_empty());
    }

    #[test]
    fn normalise_adds_columns() {
        let mut s = Settings {
            columns: vec![Column {
                id: "title".into(),
                visible: true,
                width: 5.0,
            }],
            ..Default::default()
        };
        s.normalise();
        assert_eq!(s.columns[0].id, "title");
        assert_eq!(s.columns[0].width, 36.0);
        assert_eq!(s.columns.len(), crate::meta::COLUMNS.len());
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(Repeat::Off.next().next().next(), Repeat::Off);
    }
}
