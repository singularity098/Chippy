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
    /// Where the file browser starts.
    pub music_dir: Option<PathBuf>,
    /// The folder the browser showed last.
    pub last_dir: Option<PathBuf>,
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
            last_dir: None,
            show_all_files: false,
            open_folder_as_playlist: true,
            single_tap: false,
            hover_opens_browser: true,
            volume: 0.8,
            repeat: Repeat::All,
            shuffle: false,
            theme: "Iris".into(),
            fullscreen: false,
            window_width: 1360,
            window_height: 820,
            maximized: false,
            columns: default_columns(),
            prefer_embedded_art: true,
            default_length_s: 150,
            fade_s: 8,
            loops: 2.0,
            usf_hle: true,
            gme_accuracy: true,
            spectrum_bars: 32,
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("CHIPPY_HOME") {
        return PathBuf::from(h);
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

#[cfg(test)]
mod tests {
    use super::*;

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
