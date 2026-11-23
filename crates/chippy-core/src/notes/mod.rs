//! Notes from chiptunes: what each channel of the sound chip plays, worked
//! out from the writes the tune makes to the chip's registers.
//!
//! Most chiptune formats hold no notes, only the game's sound driver (NSF,
//! SPC) or a log of its register writes (VGM, GYM). Watching those writes
//! gives each channel's pitch and when it starts and stops sounding, which
//! [`Voice`] turns into notes. How exact the result is depends on the chip:
//!
//! * NES, Genesis (YM2612 and SN76489): the registers hold the frequency,
//!   so the notes are exact.
//! * SNES: the DSP plays samples, and its pitch register only says how fast.
//!   The note also depends on the pitch the sample was recorded at, which
//!   [`spc`] measures from the sample itself.

use anyhow::{bail, Result};

use crate::vfs::Location;

mod brr;
mod chips;
pub mod gm;
pub mod gp5;
pub mod instrument;
pub mod meter;
pub mod midi;
mod module;
pub mod musicxml;
mod nes;
mod nesx;
pub mod score;
mod sgc;
mod sid;
pub mod smf;
mod spc;
pub mod style;
pub mod tab;
mod vgm;
mod wsr;

pub use spc::SampleInfo;

/// One note, in milliseconds from the start of the track.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub start_ms: f64,
    pub dur_ms: f64,
    /// MIDI note number (60 is middle C).
    pub key: u8,
    pub velocity: u8,
    /// Reached by a pitch change without a new attack (a slide or legato).
    pub legato: bool,
}

/// One line of music: a chip channel, or the drums.
#[derive(Clone, Debug, Default)]
pub struct Part {
    pub name: String,
    /// Unpitched: keys are General MIDI percussion.
    pub drums: bool,
    /// A General MIDI program to play the part with.
    pub program: u8,
    /// Changes of program later on (ms, program), in time order: a channel
    /// that switches samples switches instrument.
    pub programs: Vec<(f64, u8)>,
    pub notes: Vec<Note>,
}

impl Part {
    pub fn new(name: impl Into<String>, program: u8, notes: Vec<Note>) -> Part {
        Part {
            name: name.into(),
            program,
            notes,
            ..Default::default()
        }
    }

    pub fn drums(name: impl Into<String>, notes: Vec<Note>) -> Part {
        Part {
            name: name.into(),
            drums: true,
            notes,
            ..Default::default()
        }
    }

    /// Sets the programs from the instrument each note started with
    /// ((ms, program) in time order): the first is the part's own, and each
    /// change after it a program change.
    pub fn with_programs(mut self, uses: &[(f64, u8)]) -> Part {
        self.programs.clear();
        let mut cur = None;
        for &(t, p) in uses {
            match cur {
                None => self.program = p,
                Some(c) if c != p => self.programs.push((t, p)),
                _ => {}
            }
            cur = Some(p);
        }
        self
    }

    /// The program sounding at `ms`.
    pub fn program_at(&self, ms: f64) -> u8 {
        self.programs
            .iter()
            .take_while(|(t, _)| *t <= ms)
            .last()
            .map_or(self.program, |&(_, p)| p)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Transcription {
    pub system: String,
    pub parts: Vec<Part>,
    /// SNES only: each instrument sample the tune uses, with its measured pitch.
    pub samples: Vec<SampleInfo>,
    /// Things worth knowing about how good the result is.
    pub notes: Vec<String>,
    /// Where the bars fall, when the file says (MIDI files, and tracker
    /// modules' rows and patterns); see [`meter::find`].
    pub meter: Option<meter::Meter>,
}

/// Finds the notes in `loc` (subtrack `sub` of a multi-track file), over its
/// first `max_ms` milliseconds, or its own length.
pub fn transcribe(loc: &Location, sub: Option<u32>, max_ms: Option<u64>) -> Result<Transcription> {
    let name = loc.name();
    let ext = crate::formats::extension(&name);
    let track = sub.unwrap_or(0);
    let mut t = match ext.as_str() {
        "nsf" | "nsfe" => nes::transcribe(loc, track, max_ms)?,
        "spc" => spc::transcribe(loc, max_ms)?,
        "vgm" | "vgz" | "gym" => vgm::transcribe(loc, max_ms)?,
        "ay" | "gbs" | "hes" | "kss" | "sap" => chips::transcribe_gme(loc, &ext, track, max_ms)?,
        "sgc" => sgc::transcribe(loc, sub, max_ms)?,
        "wsr" => wsr::transcribe(loc, sub, max_ms)?,
        e if crate::formats::SID.contains(&e) => sid::transcribe(loc, sub, max_ms)?,
        e if crate::formats::MODULES.contains(&e) => module::transcribe(loc, max_ms)?,
        e if crate::formats::MIDI.contains(&e) => {
            let mut t = smf::read(&loc.read()?)?;
            if let Some(max) = max_ms {
                t.cut(max as f64);
            }
            t
        }
        _ => bail!("can't find the notes in .{ext} files"),
    };
    t.parts.retain(|p| !p.notes.is_empty());
    Ok(t)
}

/// Whether [`transcribe`] can find the notes of a file with this name.
pub fn supported(name: &str) -> bool {
    let ext = crate::formats::extension(name);
    let e = ext.as_str();
    matches!(
        e,
        "nsf"
            | "nsfe"
            | "spc"
            | "vgm"
            | "vgz"
            | "gym"
            | "ay"
            | "gbs"
            | "hes"
            | "kss"
            | "sap"
            | "sgc"
            | "wsr"
    ) || crate::formats::SID.contains(&e)
        || crate::formats::MODULES.contains(&e)
        || crate::formats::MIDI.contains(&e)
}

impl Transcription {
    /// Drops what starts after `ms`, and ends what is still sounding there.
    pub fn cut(&mut self, ms: f64) {
        for p in &mut self.parts {
            p.notes.retain(|n| n.start_ms < ms);
            for n in &mut p.notes {
                n.dur_ms = n.dur_ms.min(ms - n.start_ms);
            }
            p.programs.retain(|(t, _)| *t < ms);
        }
    }

    /// When the last note ends, ms.
    pub fn end_ms(&self) -> f64 {
        self.parts
            .iter()
            .flat_map(|p| p.notes.iter())
            .map(|n| n.start_ms + n.dur_ms)
            .fold(0.0, f64::max)
    }
}

/// The note nearest a frequency, as a fractional MIDI note number.
pub(crate) fn hz_to_key(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

pub fn key_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[key as usize % 12], key as i32 / 12 - 1)
}

/// How far a held note's pitch must move (in semitones) before it counts as
/// a new note rather than vibrato.
const SPLIT: f64 = 0.75;
/// A pitch change this soon after an attack retunes that note: drivers often
/// key a voice on and set its pitch a moment apart.
const SETTLE_MS: f64 = 4.0;
/// Notes reached by sliding that are shorter than this are folded into the
/// note before them (vibrato and slides passing through), as are slides
/// back to the same note.
const MIN_LEGATO_MS: f64 = 30.0;

/// Turns one channel's pitch and on/off changes into notes. Each channel
/// plays one note at a time.
#[derive(Default)]
pub(crate) struct Voice {
    cur: Option<Note>,
    notes: Vec<Note>,
}

impl Voice {
    fn key_of(pitch: f64) -> Option<u8> {
        let k = pitch.round();
        (0.0..=127.0).contains(&k).then_some(k as u8)
    }

    /// A new note starts (an attack), at `pitch` (a fractional MIDI key).
    pub fn attack(&mut self, t: f64, pitch: f64, velocity: u8) {
        self.release(t);
        if let Some(key) = Self::key_of(pitch) {
            self.cur = Some(Note {
                start_ms: t,
                dur_ms: 0.0,
                key,
                velocity: velocity.clamp(1, 127),
                legato: false,
            });
        }
    }

    /// The sounding note's pitch is now `pitch`.
    pub fn pitch(&mut self, t: f64, pitch: f64) {
        let Some(cur) = &mut self.cur else { return };
        if (pitch - cur.key as f64).abs() < SPLIT {
            return;
        }
        let Some(key) = Self::key_of(pitch) else {
            return self.release(t);
        };
        if t - cur.start_ms <= SETTLE_MS && !cur.legato {
            cur.key = key;
            return;
        }
        let velocity = cur.velocity;
        self.release(t);
        self.cur = Some(Note {
            start_ms: t,
            dur_ms: 0.0,
            key,
            velocity,
            legato: true,
        });
    }

    pub fn velocity(&mut self, velocity: u8) {
        if let Some(cur) = &mut self.cur {
            cur.velocity = cur.velocity.max(velocity.clamp(1, 127));
        }
    }

    pub fn sounding(&self) -> bool {
        self.cur.is_some()
    }

    /// The sounding note (if any) stops.
    pub fn release(&mut self, t: f64) {
        if let Some(mut n) = self.cur.take() {
            n.dur_ms = (t - n.start_ms).max(0.0);
            if n.dur_ms > 0.0 {
                self.notes.push(n);
            }
        }
    }

    pub fn finish(mut self, t: f64) -> Vec<Note> {
        self.release(t);
        let mut out: Vec<Note> = Vec::with_capacity(self.notes.len());
        for n in self.notes {
            if let Some(prev) = out.last_mut() {
                let joined = (prev.start_ms + prev.dur_ms - n.start_ms).abs() < 0.01;
                if n.legato && joined && (n.dur_ms < MIN_LEGATO_MS || n.key == prev.key) {
                    prev.dur_ms += n.dur_ms;
                    continue;
                }
            }
            out.push(n);
        }
        out
    }
}

/// Drum hits, from noise channels and samples.
#[derive(Default)]
pub(crate) struct Drums {
    notes: Vec<Note>,
}

impl Drums {
    pub fn hit(&mut self, t: f64, key: u8, velocity: u8) {
        self.notes.push(Note {
            start_ms: t,
            dur_ms: 60.0,
            key,
            velocity: velocity.clamp(1, 127),
            legato: false,
        });
    }

    pub fn finish(mut self) -> Vec<Note> {
        self.notes
            .sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms).then(a.key.cmp(&b.key)));
        self.notes
    }
}

/// Writes `data` to a file called `name` and finds its notes (tests).
#[cfg(test)]
pub(crate) fn transcribe_bytes(
    name: &str,
    data: &[u8],
    sub: Option<u32>,
    ms: u64,
) -> Transcription {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, data).unwrap();
    let loc = match crate::vfs::resolve(&path).unwrap() {
        crate::vfs::Node::Tracks(l) => l,
        n => panic!("{n:?}"),
    };
    transcribe(&loc, sub, Some(ms)).unwrap()
}

/// The part called `name` (tests).
#[cfg(test)]
pub(crate) fn part<'a>(t: &'a Transcription, name: &str) -> &'a Part {
    t.parts.iter().find(|p| p.name == name).unwrap_or_else(|| {
        panic!(
            "no part {name}: {:?}",
            t.parts.iter().map(|p| &p.name).collect::<Vec<_>>()
        )
    })
}

/// A part's notes as (key, start in 10 ms steps) (tests).
#[cfg(test)]
pub(crate) fn keys_at(p: &Part) -> Vec<(u8, i64)> {
    p.notes
        .iter()
        .map(|n| (n.key, (n.start_ms / 10.0).round() as i64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_turns_changes_into_notes() {
        let mut v = Voice::default();
        v.attack(0.0, 60.1, 100);
        v.pitch(10.0, 60.4); // vibrato: same note
        v.pitch(100.0, 62.0); // a slide to a new note
        v.release(200.0);
        v.attack(250.0, 64.0, 90);
        v.pitch(251.0, 67.0); // pitch set just after the key-on
        let notes = v.finish(300.0);
        let keys: Vec<_> = notes.iter().map(|n| (n.key, n.legato)).collect();
        assert_eq!(keys, vec![(60, false), (62, true), (67, false)]);
        assert_eq!(notes[0].dur_ms, 100.0);
        assert_eq!(notes[1].dur_ms, 100.0);
    }

    #[test]
    fn short_slides_fold_into_the_note_before() {
        let mut v = Voice::default();
        v.attack(0.0, 60.0, 100);
        v.pitch(100.0, 61.0);
        v.pitch(110.0, 60.0);
        let notes = v.finish(200.0);
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].key, notes[0].dur_ms), (60, 200.0));
    }

    #[test]
    fn names_keys() {
        assert_eq!(key_name(60), "C4");
        assert_eq!(key_name(69), "A4");
        assert!((hz_to_key(440.0) - 69.0).abs() < 1e-9);
    }
}
