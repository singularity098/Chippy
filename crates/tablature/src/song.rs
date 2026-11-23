//! Songs read from tab files (Guitar Pro, MusicXML): their parts as notes in
//! milliseconds, each note on the string the file puts it on, and the bars
//! they fall in.
//!
//! Each reader gives what its file says in its own terms: bars with their
//! time signatures and repeats, and notes placed in quarter notes from the
//! start of their bar. [`build`] plays that through as the file asks
//! (repeats and alternate endings in order, tied notes held on, the tempo
//! as it changes) into a [`Song`].

use std::fmt;

use crate::meter::{beats_in, Bar, Meter};
use crate::{Note, Part};

/// A tab file that can't be read.
#[derive(Clone, Debug, PartialEq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error(msg.into()))
}

/// The extensions of the tab files [`read`] knows.
pub const EXTENSIONS: &[&str] = &["gp3", "gp4", "gp5", "gp", "musicxml", "mxl"];

/// A song read from a tab file.
#[derive(Clone, Debug, Default)]
pub struct Song {
    pub title: String,
    pub subtitle: String,
    pub artist: String,
    pub album: String,
    /// Who wrote the words, and the music.
    pub words: String,
    pub music: String,
    pub copyright: String,
    /// Who wrote the tab out.
    pub tab_by: String,
    /// Instructions and notes in the file.
    pub comments: Vec<String>,
    /// "Guitar Pro 5", "MusicXML"...
    pub format: String,
    /// One part per track of the file that has notes, with the strings it
    /// is written for and where each note is played.
    pub parts: Vec<Part>,
    /// The bars as played (repeats written out), from the file.
    pub meter: Meter,
}

impl Song {
    /// When the music ends: the end of the last bar, or of the last note if
    /// that rings on past it.
    pub fn end_ms(&self) -> f64 {
        self.parts
            .iter()
            .flat_map(|p| p.notes.iter())
            .map(|n| n.start_ms + n.dur_ms)
            .fold(self.meter.end_ms, f64::max)
    }
}

/// Reads the tab file called `name` (its extension says how).
pub fn read(name: &str, data: &[u8]) -> Result<Song> {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "gp3" | "gp4" | "gp5" => crate::gp::read(data),
        "gp" => crate::gpif::read_gp(data),
        "musicxml" => crate::musicxml_read::read(data),
        "mxl" => crate::musicxml_read::read_mxl(data),
        _ => err(format!("can't read .{ext} files as tab")),
    }
}

// ------------------------------------------------- what the readers give

/// A bar as written.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Measure {
    pub sig: (u8, u8),
    /// A repeat starts here.
    pub repeat_open: bool,
    /// A repeat ends here, going back this many times (0: none).
    pub repeat_close: u32,
    /// Alternate ending: the passes through the repeat this bar is played
    /// on, as bits (1 the first pass); 0 on every pass.
    pub endings: u32,
}

impl Measure {
    pub fn new(sig: (u8, u8)) -> Measure {
        Measure {
            sig,
            repeat_open: false,
            repeat_close: 0,
            endings: 0,
        }
    }

    pub fn quarters(&self) -> f64 {
        self.sig.0.max(1) as f64 * 4.0 / self.sig.1.max(1) as f64
    }
}

/// A track as written.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Track {
    pub name: String,
    pub program: u8,
    pub drums: bool,
    /// Open strings, lowest first, without the capo; empty for a staff of
    /// notes only.
    pub strings: Vec<u8>,
    pub capo: u8,
}

/// A note as written.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Written {
    pub track: usize,
    pub measure: usize,
    /// Quarter notes from the start of the bar.
    pub at: f64,
    pub quarters: f64,
    /// The key it sounds (for a tie, the key of the note it holds on).
    pub key: u8,
    pub velocity: u8,
    /// The string (0 the lowest), if the file says.
    pub string: Option<u8>,
    /// Holds on the note before it on the same string (or key), rather
    /// than playing again.
    pub tie: bool,
}

/// A change of tempo, in quarter notes a minute, at a point in a bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tempo {
    pub measure: usize,
    pub at: f64,
    pub bpm: f64,
}

/// The most bars a song can play through (repeats included), so a file
/// whose repeats never end still ends.
const MAX_PLAYED: usize = 20_000;

/// The order the bars are played in: repeats taken as often as they ask,
/// the alternate ending of each pass in its place.
pub(crate) fn play_order(measures: &[Measure]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut pass = 1u32;
    let mut i = 0;
    while i < measures.len() && out.len() < MAX_PLAYED {
        let m = &measures[i];
        if m.repeat_open && i != start {
            start = i;
            pass = 1;
        }
        if m.endings != 0 && pass <= 32 && m.endings & (1 << (pass - 1)) == 0 {
            // Another pass's ending. The last ending of a repeat played
            // through ends it, so it stops being one.
            i += 1;
            continue;
        }
        out.push(i);
        if m.repeat_close > 0 && pass <= m.repeat_close {
            pass += 1;
            i = start;
            continue;
        }
        if m.repeat_close > 0 {
            start = i + 1;
            pass = 1;
        }
        i += 1;
    }
    out
}

/// Plays through what a reader found into a song: the bars in the order
/// `play_order` gives, tempo changes in their places (`bpm` at the start),
/// tied notes held on.
pub(crate) fn build(
    mut song: Song,
    measures: &[Measure],
    tracks: &[Track],
    notes: &[Written],
    tempos: &[Tempo],
    bpm: f64,
) -> Song {
    let order = play_order(measures);
    // Where each bar played starts, in quarter notes from the start.
    let mut starts = Vec::with_capacity(order.len() + 1);
    let mut q = 0.0;
    for &m in &order {
        starts.push(q);
        q += measures[m].quarters();
    }
    let end_q = q;
    // The tempo map: (quarters, ms, bpm from there).
    let mut changes: Vec<(f64, f64)> = Vec::new();
    for (k, &m) in order.iter().enumerate() {
        for t in tempos.iter().filter(|t| t.measure == m && t.bpm > 0.0) {
            changes.push((starts[k] + t.at, t.bpm));
        }
    }
    changes.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut map: Vec<(f64, f64, f64)> = vec![(0.0, 0.0, bpm.max(1.0))];
    for (q, bpm) in changes {
        let &(q0, ms0, b0) = map.last().unwrap();
        let ms = ms0 + (q - q0) * 60_000.0 / b0;
        if q - q0 < 1e-9 {
            map.last_mut().unwrap().2 = bpm;
        } else {
            map.push((q, ms, bpm));
        }
    }
    let ms = |q: f64| {
        let i = map.partition_point(|m| m.0 <= q).max(1) - 1;
        let (q0, ms0, b) = map[i];
        ms0 + (q - q0) * 60_000.0 / b
    };

    // The bars as played.
    let bars: Vec<Bar> = order
        .iter()
        .zip(&starts)
        .map(|(&m, &q0)| {
            let sig = measures[m].sig;
            let n = beats_in(sig);
            let beat = measures[m].quarters() / n as f64;
            Bar {
                start_ms: ms(q0),
                beats: (0..n).map(|b| ms(q0 + b as f64 * beat)).collect(),
                sig,
            }
        })
        .collect();
    song.meter = Meter {
        bars,
        end_ms: ms(end_q),
        exact: true,
    };

    // Each bar's notes, in the order played.
    let mut by_measure: Vec<Vec<&Written>> = vec![Vec::new(); measures.len()];
    for n in notes {
        if let Some(v) = by_measure.get_mut(n.measure) {
            v.push(n);
        }
    }
    for v in &mut by_measure {
        v.sort_by(|a, b| a.at.total_cmp(&b.at));
    }
    let mut parts: Vec<(Vec<Note>, Vec<Option<u8>>)> = vec![Default::default(); tracks.len()];
    for (&m, &q0) in order.iter().zip(&starts) {
        for w in &by_measure[m] {
            let Some((ns, places)) = parts.get_mut(w.track) else {
                continue;
            };
            let start = ms(q0 + w.at);
            let end = ms(q0 + w.at + w.quarters);
            if w.tie {
                // The last note on this string (or of this key), held on.
                let held = ns
                    .iter()
                    .zip(places.iter())
                    .rposition(|(n, s)| match w.string {
                        Some(_) => *s == w.string,
                        None => n.key == w.key,
                    });
                if let Some(i) = held {
                    let n = &mut ns[i];
                    n.dur_ms = n.dur_ms.max(end - n.start_ms);
                    continue;
                }
            }
            ns.push(Note {
                start_ms: start,
                dur_ms: (end - start).max(1.0),
                key: w.key,
                velocity: w.velocity.clamp(1, 127),
                legato: false,
            });
            places.push(w.string);
        }
    }
    song.parts = tracks
        .iter()
        .zip(parts)
        .filter(|(_, (ns, _))| !ns.is_empty())
        .map(|(t, (ns, places))| {
            let fretted = !t.drums && !t.strings.is_empty();
            let mut p = Part {
                name: t.name.clone(),
                drums: t.drums,
                program: t.program,
                ..Default::default()
            };
            if fretted {
                p.strings = t
                    .strings
                    .iter()
                    .map(|&s| s.saturating_add(t.capo))
                    .collect();
                p.capo = t.capo;
                p.written = if places.iter().all(Option::is_some) {
                    places
                } else {
                    Vec::new()
                };
            }
            p.notes = ns;
            p
        })
        .collect();
    song
}

/// The number of a General MIDI drum kit's channel (0-based).
pub(crate) const DRUM_CHANNEL: u8 = 9;

/// A tuplet's length against the plain notes it stands for: 3 in the time
/// of 2, 5 in the time of 4...
pub(crate) fn tuplet(n: u32) -> f64 {
    if n < 2 {
        return 1.0;
    }
    let mut d = 1u32;
    while d * 2 < n {
        d *= 2;
    }
    d as f64 / n as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(open: bool, close: u32, endings: u32) -> Measure {
        Measure {
            repeat_open: open,
            repeat_close: close,
            endings,
            ..Measure::new((4, 4))
        }
    }

    #[test]
    fn plays_repeats_and_endings() {
        // 0 |: 1 2 :| 3
        let ms = [
            m(false, 0, 0),
            m(true, 0, 0),
            m(false, 1, 0),
            m(false, 0, 0),
        ];
        assert_eq!(play_order(&ms), vec![0, 1, 2, 1, 2, 3]);
        // |: 0 [1. 1 :| [2. 2 ] 3
        let ms = [
            m(true, 0, 0),
            m(false, 1, 0b01),
            m(false, 0, 0b10),
            m(false, 0, 0),
        ];
        assert_eq!(play_order(&ms), vec![0, 1, 0, 2, 3]);
        // A repeat with no start goes back to the beginning; played three
        // times.
        let ms = [m(false, 0, 0), m(false, 2, 0), m(false, 0, 0)];
        assert_eq!(play_order(&ms), vec![0, 1, 0, 1, 0, 1, 2]);
    }

    #[test]
    fn tuplets() {
        assert_eq!(tuplet(3), 2.0 / 3.0);
        assert_eq!(tuplet(5), 4.0 / 5.0);
        assert_eq!(tuplet(6), 4.0 / 6.0);
        assert_eq!(tuplet(1), 1.0);
    }

    #[test]
    fn builds_timing_ties_and_tempo() {
        let measures = [Measure::new((4, 4)), Measure::new((3, 4))];
        let tracks = [Track {
            name: "Guitar".into(),
            program: 25,
            strings: vec![40, 45, 50, 55, 59, 64],
            capo: 2,
            ..Default::default()
        }];
        let w = |measure, at, quarters, key, tie| Written {
            track: 0,
            measure,
            at,
            quarters,
            key,
            velocity: 95,
            string: Some(5),
            tie,
        };
        let notes = [
            w(0, 0.0, 1.0, 69, false),
            w(0, 3.0, 1.0, 71, false),
            // Held over the bar line.
            w(1, 0.0, 1.0, 71, true),
            w(1, 1.0, 2.0, 72, false),
        ];
        // Twice as fast from the second bar.
        let tempos = [Tempo {
            measure: 1,
            at: 0.0,
            bpm: 240.0,
        }];
        let song = build(Song::default(), &measures, &tracks, &notes, &tempos, 120.0);
        let p = &song.parts[0];
        let times: Vec<(f64, f64, u8)> = p
            .notes
            .iter()
            .map(|n| (n.start_ms, n.dur_ms, n.key))
            .collect();
        assert_eq!(
            times,
            vec![(0.0, 500.0, 69), (1500.0, 750.0, 71), (2250.0, 500.0, 72)]
        );
        assert_eq!(p.written, vec![Some(5); 3]);
        assert_eq!(p.strings, vec![42, 47, 52, 57, 61, 66]);
        assert_eq!(song.meter.bars.len(), 2);
        assert_eq!(song.meter.bars[1].start_ms, 2000.0);
        assert_eq!(song.meter.bars[1].beats, vec![2000.0, 2250.0, 2500.0]);
        assert_eq!(song.meter.end_ms, 2750.0);
        assert!(song.meter.exact);
    }
}
