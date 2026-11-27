//! MIDI files and chiptunes imported as guitar tablature.
//!
//! Chippy works out what each channel of a tune plays (from what it does to
//! its sound chips, a tracker module's patterns, or a MIDI file's events)
//! and where the bars fall; the [`tablature`] crate then fingers each part
//! on a guitar or bass and writes it out in those bars as a
//! [`Document`] to edit.
//!
//! These formats can only be imported, never saved back to: they hold a
//! game's sound driver or a log of its chip writes, not notes.

use std::path::Path;

use anyhow::{bail, Result};
use chippy_core::decode::{self, DecodeOptions};
use chippy_core::formats;
use chippy_core::notes;
use chippy_core::{Location, TrackRef};

pub use tablature;
use tablature::doc::Document;
use tablature::meter::Meter;
use tablature::Part;

/// Chiptune formats whose notes can be found.
const CHIPTUNES: &[&str] = &[
    "nsf", "nsfe", "spc", "vgm", "vgz", "gym", "ay", "gbs", "hes", "kss", "sap", "sgc", "wsr",
    "2sf", "mini2sf", "gsf", "minigsf", "psf", "minipsf", "psf1", "minipsf1", "psf2", "minipsf2",
    "spu",
];

/// Every extension [`import`] takes (lowercase, no dot): MIDI files,
/// tracker modules and chiptunes.
pub fn extensions() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = formats::MIDI.to_vec();
    v.extend_from_slice(CHIPTUNES);
    v.extend_from_slice(formats::SID);
    v.extend_from_slice(formats::MODULES);
    v.sort_unstable();
    v.dedup();
    v
}

/// Whether a file with this name can be imported.
pub fn supported(name: &str) -> bool {
    let ext = formats::extension(name);
    extensions().contains(&ext.as_str())
}

/// Whether it is a MIDI file (rather than a chiptune or module).
pub fn is_midi(name: &str) -> bool {
    formats::MIDI.contains(&formats::extension(name).as_str())
}

/// One of the tunes in a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Tune {
    /// Which tune (for files holding several), as [`import`] takes it.
    pub sub: Option<u32>,
    pub title: String,
    pub length_ms: Option<u64>,
}

/// The tunes in a file: several for an NSF, a SID with subtunes, a
/// multi-song WSR...; one for anything else.
pub fn tunes(path: &Path) -> Result<Vec<Tune>> {
    let loc = Location::Fs(path.to_path_buf());
    let name = loc.name();
    if !supported(&name) {
        bail!("{name} can't be imported");
    }
    if !formats::may_hold_tracks(&name) {
        return Ok(vec![Tune {
            sub: None,
            title: stem(&name),
            length_ms: None,
        }]);
    }
    let list = decode::track_list(&loc, &DecodeOptions::default())?;
    Ok(list
        .into_iter()
        .map(|t| Tune {
            sub: Some(t.index),
            title: t.title,
            length_ms: t.length_ms,
        })
        .collect())
}

fn stem(name: &str) -> String {
    name.rsplit_once('.').map_or(name, |(s, _)| s).to_string()
}

/// What an import found.
#[derive(Clone, Debug)]
pub struct Imported {
    pub title: String,
    pub artist: String,
    /// The machine or format the notes came from ("NES", "MIDI"...).
    pub system: String,
    /// Each channel's (or MIDI channel's) notes.
    pub parts: Vec<Part>,
    /// Where the bars fall: from the file (MIDI, modules) or worked out
    /// from the notes.
    pub meter: Meter,
    /// Remarks on how good the result is.
    pub remarks: Vec<String>,
}

/// The longest a tune that loops forever is followed for.
pub const DEFAULT_LENGTH_MS: u64 = 150_000;

/// Finds the notes of tune `sub` of the file at `path`, over at most
/// `max_ms` (else its own length, or [`DEFAULT_LENGTH_MS`] for a tune that
/// loops forever).
pub fn import(path: &Path, sub: Option<u32>, max_ms: Option<u64>) -> Result<Imported> {
    let loc = Location::Fs(path.to_path_buf());
    let name = loc.name();
    if !supported(&name) {
        bail!("{name} can't be imported");
    }
    let opts = DecodeOptions::default();
    let track = TrackRef {
        loc: loc.clone(),
        sub,
    };
    let meta = chippy_core::meta::read(&track, &opts);
    let max = max_ms.or(if is_midi(&name) {
        None
    } else {
        Some(meta.length_ms.unwrap_or(DEFAULT_LENGTH_MS))
    });
    let t = notes::transcribe(&loc, sub, max)?;
    if t.parts.is_empty() {
        bail!("no notes were found in {name}");
    }
    let end = t.end_ms();
    let Some(meter) = notes::meter::find(&t, end) else {
        bail!("the bars of {name} couldn't be worked out");
    };
    let title = if meta.title.trim().is_empty() {
        stem(&name)
    } else {
        meta.title.clone()
    };
    Ok(Imported {
        title,
        artist: meta.artist.clone(),
        system: if t.system.is_empty() {
            meta.system.clone()
        } else {
            t.system.clone()
        },
        parts: t.parts,
        meter,
        remarks: t.notes,
    })
}

/// [`import`], written out as tab to edit.
pub fn import_document(path: &Path, sub: Option<u32>, max_ms: Option<u64>) -> Result<Document> {
    let i = import(path, sub, max_ms)?;
    let mut doc = tablature::doc::from_parts(&i.title, &i.parts, &i.meter);
    doc.info.artist = i.artist;
    doc.format = i.system;
    if !i.remarks.is_empty() {
        doc.info.notice = i.remarks.join("\n");
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A MIDI file: 120 bpm, 4/4, C E G on channel 1 a beat apart.
    fn midi() -> Vec<u8> {
        let mut trk = vec![
            0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, 0x00, 0xFF, 0x58, 0x04, 4, 2, 24, 8,
        ];
        trk.extend([0x00, 0xC0, 29]);
        for k in [60u8, 64, 67, 72].iter() {
            trk.extend([0x00, 0x90, *k, 100]);
            trk.extend([0x83, 0x60, 0x80, *k, 0]);
        }
        trk.extend([0x00, 0xFF, 0x2F, 0x00]);
        let mut f = b"MThd".to_vec();
        f.extend(6u32.to_be_bytes());
        f.extend([0, 0, 0, 1, 0x01, 0xE0]);
        f.extend(b"MTrk");
        f.extend((trk.len() as u32).to_be_bytes());
        f.extend(trk);
        f
    }

    #[test]
    fn imports_a_midi_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tune.mid");
        std::fs::write(&p, midi()).unwrap();
        assert!(supported("x.NSF") && supported("a.mid") && !supported("a.gp5"));
        assert_eq!(tunes(&p).unwrap().len(), 1);
        let d = import_document(&p, None, None).unwrap();
        assert_eq!(d.info.title, "tune");
        let keys: Vec<u8> = d.tracks[0].bars[0].voices[0]
            .iter()
            .flat_map(|b| b.notes.iter().map(|n| d.tracks[0].key_of(n)))
            .collect();
        assert_eq!(keys, vec![60, 64, 67, 72]);
    }
}
