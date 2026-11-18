//! A track's tags and properties, as shown in the playlist's columns.

use serde::{Deserialize, Serialize};

use crate::decode::{self, DecodeOptions};
use crate::vfs::TrackRef;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_no: Option<u32>,
    pub length_ms: Option<u64>,
    pub genre: String,
    pub year: String,
    pub comment: String,
    /// The machine a chiptune was written for ("Nintendo NES").
    pub system: String,
    pub copyright: String,
    /// Who ripped the chiptune.
    pub dumper: String,
    /// The format or codec ("FLAC", "NSFE", "ADX").
    pub format: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub filename: String,
    /// Where the file is (its virtual path).
    pub path: String,
}

/// The playlist's columns: (id, header). Ids are stored in settings.json:
/// never rename one.
pub const COLUMNS: &[(&str, &str)] = &[
    ("track", "#"),
    ("title", "Title"),
    ("artist", "Artist"),
    ("album", "Album"),
    ("length", "Length"),
    ("genre", "Genre"),
    ("year", "Year"),
    ("system", "System"),
    ("format", "Format"),
    ("samplerate", "Sample rate"),
    ("bitrate", "Bitrate"),
    ("channels", "Channels"),
    ("copyright", "Copyright"),
    ("dumper", "Ripped by"),
    ("comment", "Comment"),
    ("filename", "File name"),
    ("path", "Path"),
];

pub fn format_ms(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl TrackMeta {
    /// The text of one column.
    pub fn column(&self, id: &str) -> String {
        let opt = |v: Option<u32>, unit: &str| v.map(|v| format!("{v}{unit}")).unwrap_or_default();
        match id {
            "track" => opt(self.track_no, ""),
            "title" => self.title.clone(),
            "artist" => self.artist.clone(),
            "album" => self.album.clone(),
            "length" => self.length_ms.map(format_ms).unwrap_or_default(),
            "genre" => self.genre.clone(),
            "year" => self.year.clone(),
            "system" => self.system.clone(),
            "format" => self.format.clone(),
            "samplerate" => self
                .sample_rate
                .map(|r| format!("{:.1} kHz", r as f64 / 1000.0))
                .unwrap_or_default(),
            "bitrate" => opt(self.bitrate_kbps, " kbps"),
            "channels" => opt(self.channels, ""),
            "copyright" => self.copyright.clone(),
            "dumper" => self.dumper.clone(),
            "comment" => self.comment.lines().next().unwrap_or("").to_string(),
            "filename" => self.filename.clone(),
            "path" => self.path.clone(),
            _ => String::new(),
        }
    }

    /// Fills what the file's tags left empty from its name.
    pub fn fill_gaps(&mut self, t: &TrackRef) {
        let name = t.loc.name();
        self.filename = name.clone();
        self.path = t.loc.vpath().to_string_lossy().into_owned();
        if let Some(s) = t.sub {
            if self.track_no.is_none() {
                self.track_no = Some(s + 1);
            }
        }
        let (stem, _) = crate::formats::split_name(&name);
        if self.track_no.is_none() {
            self.track_no = leading_number(stem);
        }
        if self.title.trim().is_empty() {
            self.title = match t.sub {
                Some(s) => format!("{stem} #{}", s + 1),
                None => strip_leading_number(stem).to_string(),
            };
        }
        if self.format.is_empty() {
            self.format = t.loc.extension().to_uppercase();
        }
    }
}

/// "07 - Song" -> 7.
fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() || digits.len() > 3 {
        return None;
    }
    digits.parse().ok()
}

/// "07 - Song" -> "Song".
fn strip_leading_number(s: &str) -> &str {
    let t = s.trim_start();
    let rest = t.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == t.len() || t.len() - rest.len() > 3 {
        return s;
    }
    let rest = rest.trim_start_matches([' ', '-', '.', '_', ')']);
    if rest.is_empty() {
        s
    } else {
        rest
    }
}

/// Reads a track's metadata; never fails (a file that cannot be read still
/// gets its name as title).
pub fn read(t: &TrackRef, opts: &DecodeOptions) -> TrackMeta {
    let mut m = decode::read_meta(t, opts).unwrap_or_else(|e| {
        log::debug!("no metadata for {}: {e:#}", t.loc.vpath().display());
        TrackMeta::default()
    });
    m.fill_gaps(t);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::Location;

    #[test]
    fn names_fill_gaps() {
        let t = TrackRef::file(Location::Fs("/m/07 - Hello World.flac".into()));
        let mut m = TrackMeta::default();
        m.fill_gaps(&t);
        assert_eq!(m.title, "Hello World");
        assert_eq!(m.track_no, Some(7));
        assert_eq!(m.format, "FLAC");
        let t = TrackRef {
            loc: Location::Fs("/m/game.nsf".into()),
            sub: Some(4),
        };
        let mut m = TrackMeta::default();
        m.fill_gaps(&t);
        assert_eq!(m.title, "game #5");
        assert_eq!(m.track_no, Some(5));
        assert_eq!(strip_leading_number("1999"), "1999");
    }

    #[test]
    fn times() {
        assert_eq!(format_ms(61_000), "1:01");
        assert_eq!(format_ms(3_725_000), "1:02:05");
    }
}
