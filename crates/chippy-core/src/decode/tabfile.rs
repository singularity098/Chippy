//! Tab files (Guitar Pro 3 to 5 and 7 to 8, MusicXML), read by the
//! `tablature` crate and played through the SoundFont as MIDI files are:
//! every track with its instrument, the repeats taken, the tempo as it
//! changes. The guitar tab view shows them on the strings and frets the
//! file gives.

use anyhow::Result;

use super::midi::MidiDecoder;
use super::DecodeOptions;
use crate::meta::TrackMeta;
use crate::vfs::Location;

/// What a decoder plays after the last note ends, for it to ring out (as
/// the MIDI decoder does).
const TAIL_MS: f64 = 1500.0;

/// Reads the tab file at `loc`.
pub fn read(loc: &Location) -> Result<tablature::Song> {
    Ok(tablature::read(&loc.name(), &loc.read()?)?)
}

pub fn open(loc: &Location, opts: &DecodeOptions) -> Result<MidiDecoder> {
    let t = crate::notes::from_song(read(loc)?);
    MidiDecoder::open_notes(&t, opts.sample_rate)
}

pub fn meta(loc: &Location) -> Result<TrackMeta> {
    let song = read(loc)?;
    let last = song
        .parts
        .iter()
        .flat_map(|p| p.notes.iter())
        .map(|n| n.start_ms + n.dur_ms)
        .fold(0.0, f64::max);
    let mut comment = song.comments.clone();
    if !song.subtitle.is_empty() {
        comment.insert(0, song.subtitle.clone());
    }
    let artist = [&song.artist, &song.music, &song.words]
        .into_iter()
        .find(|a| !a.is_empty())
        .cloned()
        .unwrap_or_default();
    Ok(TrackMeta {
        title: song.title,
        artist,
        album: song.album,
        length_ms: Some((last + TAIL_MS) as u64),
        comment: comment.join("\n"),
        copyright: song.copyright,
        dumper: song.tab_by,
        format: song.format,
        channels: Some(2),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::meter::{Bar, Meter};
    use crate::notes::{score, tab, Note};

    /// A Guitar Pro 5 file: two bars of a riff in drop D.
    fn riff() -> Vec<u8> {
        let notes: Vec<Note> = [38, 45, 50, 52, 38, 45, 50, 57]
            .iter()
            .enumerate()
            .map(|(i, &key)| Note {
                start_ms: i as f64 * 500.0,
                dur_ms: 450.0,
                key,
                velocity: 100,
                legato: false,
            })
            .collect();
        let drop_d = &tab::TUNINGS[tab::tuning_named("Guitar (drop D)").unwrap()];
        let t = tab::arrange(&notes, drop_d);
        let meter = Meter {
            bars: (0..2)
                .map(|b| Bar {
                    start_ms: b as f64 * 2000.0,
                    beats: (0..4)
                        .map(|k| b as f64 * 2000.0 + k as f64 * 500.0)
                        .collect(),
                    sig: (4, 4),
                })
                .collect(),
            end_ms: 4000.0,
            exact: true,
        };
        let s = score::build(
            "Riff",
            &meter,
            &[score::PartTab {
                name: "Rhythm".into(),
                program: 30,
                tuning: drop_d,
                tab: &t,
            }],
        );
        crate::notes::gp5::write(&s)
    }

    #[test]
    fn reads_tab_files_as_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Riff.gp5");
        std::fs::write(&path, riff()).unwrap();
        let loc = Location::Fs(path);
        assert!(crate::notes::supported("Riff.gp5"));

        let m = meta(&loc).unwrap();
        assert_eq!(
            (m.title.as_str(), m.format.as_str()),
            ("Riff", "Guitar Pro 5")
        );
        // The last note starts at 3.5 s and ends by the bar's end; then it
        // rings out.
        let len = m.length_ms.unwrap();
        assert!((3500 + 1500..=4000 + 1500).contains(&len), "{len}");

        let t = crate::notes::transcribe(&loc, None, None).unwrap();
        assert_eq!(t.system, "Guitar Pro 5");
        assert_eq!(t.parts.len(), 1);
        let p = &t.parts[0];
        assert_eq!((p.name.as_str(), p.program), ("Rhythm", 30));
        assert_eq!(p.strings, vec![38, 45, 50, 55, 59, 64]);
        assert_eq!(p.written.len(), 8);
        let m = t.meter.as_ref().unwrap();
        assert!(m.exact);
        assert_eq!(m.bars.len(), 2);

        // Cut short, the strings stay with their notes.
        let t = crate::notes::transcribe(&loc, None, Some(1200)).unwrap();
        let p = &t.parts[0];
        assert_eq!(p.notes.len(), 3);
        assert_eq!(p.written.len(), 3);
    }
}
