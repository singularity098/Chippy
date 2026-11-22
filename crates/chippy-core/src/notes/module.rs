//! Tracker modules (MOD, XM, S3M, IT...): the notes are written in the
//! patterns, so they are read as libxmp plays them, frame by frame, with
//! slides and vibrato as they sound.
//!
//! A tracker note says how fast to play a sample, not what pitch comes
//! out: that also depends on the pitch the sample was recorded at, which
//! is measured (as for the SNES). Unpitched samples become drums, and each
//! sample gets an instrument from its name or, failing that, its sound.

use anyhow::Result;

use super::instrument::{self, Kind};
use super::{Drums, Part, SampleInfo, Transcription, Voice};
use crate::decode::xmp::{scan, ModSample};
use crate::vfs::Location;

/// libxmp's period for C-0, and the rate a sample plays at for note 60.
const PERIOD_BASE: f64 = 13_696.0;
const C4_RATE: f64 = 8_363.0;
const KEY_OFF: i32 = 0x81;
/// Tone portamento (slide to the note, no new attack).
const FX_TONEPORTA: i32 = 0x03;
const FX_TONE_VSLIDE: i32 = 0x05;

/// What happened on a channel, in time order.
enum Ev {
    Attack {
        t: f64,
        smp: usize,
        note: f64,
        vol: i32,
    },
    Pitch {
        t: f64,
        note: f64,
    },
    Release {
        t: f64,
    },
}

/// A sample, as decided after hearing how it is used.
struct Decided {
    /// Semitones between what the patterns say and what sounds.
    offset: f64,
    kind: Kind,
    /// How long it plays at note 60, ms (`None`: it loops).
    length_ms: Option<f64>,
}

fn velocity(vol: i32) -> u8 {
    (40 + vol.clamp(0, 64) as u32 * 87 / 64) as u8
}

/// A channel's period as a fractional note number (60 plays at 8363 Hz).
fn note_of(period: i32) -> Option<f64> {
    (period > 0).then(|| 12.0 * (PERIOD_BASE * 4096.0 / period as f64).log2())
}

fn decide(s: &ModSample, name: &str, notes: &mut [f64]) -> Decided {
    let pitch = instrument::pitch(&s.pcm, s.loop_start, C4_RATE);
    // Measured where it is mostly played, so a drum's brightness is right.
    notes.sort_by(f64::total_cmp);
    let typical = notes.get(notes.len() / 2).copied().unwrap_or(60.0);
    let rate = C4_RATE * 2f64.powf((typical - 60.0) / 12.0);
    let hz = pitch.as_ref().map(|p| p.hz * rate / C4_RATE);
    let features = instrument::measure(&s.pcm, s.loop_start, rate, hz);
    let offset = pitch
        .as_ref()
        .map_or(0.0, |p| super::hz_to_key(p.hz) - 60.0);
    let median = (typical + offset).round().clamp(0.0, 127.0) as u8;
    Decided {
        offset,
        kind: instrument::classify(name, &features, pitch.is_some(), Some(median)),
        length_ms: s
            .loop_start
            .is_none()
            .then(|| s.pcm.len() as f64 * 1000.0 / C4_RATE),
    }
}

pub fn transcribe(loc: &Location, max_ms: Option<u64>) -> Result<Transcription> {
    let mut events: Vec<Vec<Ev>> = Vec::new();
    // Each channel: its last row, and whether a note sounds.
    let mut last_row: Option<(i32, i32)> = None;
    let mut sounding: Vec<bool> = Vec::new();
    let mut uses: Vec<Vec<f64>> = Vec::new();
    // When each row starts: (ms, order position, row), for the bars.
    let mut rows: Vec<(f64, i32, i32)> = Vec::new();
    let scanned = scan(loc, max_ms, |t, row, chans| {
        if events.len() < chans.len() {
            events.resize_with(chans.len(), Vec::new);
            sounding.resize(chans.len(), false);
        }
        let new_row = last_row != Some(row);
        last_row = Some(row);
        if new_row {
            rows.push((t, row.0, row.1));
        }
        for (c, ch) in chans.iter().enumerate() {
            let ev = &mut events[c];
            let note = note_of(ch.period);
            if new_row {
                let n = ch.ev_note;
                if n >= KEY_OFF {
                    if sounding[c] {
                        ev.push(Ev::Release { t });
                        sounding[c] = false;
                    }
                    continue;
                }
                let porta = [ch.ev_fxt, ch.ev_f2t]
                    .iter()
                    .any(|&f| f == FX_TONEPORTA || f == FX_TONE_VSLIDE);
                if n > 0 && !(porta && sounding[c]) && ch.smp >= 0 && ch.vol > 0 {
                    if let Some(note) = note {
                        let smp = ch.smp as usize;
                        if uses.len() <= smp {
                            uses.resize_with(smp + 1, Vec::new);
                        }
                        uses[smp].push(note);
                        ev.push(Ev::Attack {
                            t,
                            smp,
                            note,
                            vol: ch.vol,
                        });
                        sounding[c] = true;
                        continue;
                    }
                }
            }
            if sounding[c] {
                match note {
                    Some(n) if ch.vol > 0 => ev.push(Ev::Pitch { t, note: n }),
                    _ => {
                        ev.push(Ev::Release { t });
                        sounding[c] = false;
                    }
                }
            }
        }
    })?;
    let end = scanned.end_ms;

    // Decide each sample, by its name (the instrument's and its own) and
    // its sound.
    let decided: Vec<Option<Decided>> = scanned
        .samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let s = s.as_ref()?;
            let notes = uses.get_mut(i)?;
            if notes.is_empty() {
                return None;
            }
            // Instruments and samples are numbered alike in most formats.
            let ins = scanned
                .instrument_names
                .get(i)
                .map(String::as_str)
                .unwrap_or("");
            Some(decide(s, &format!("{ins} {}", s.name), notes))
        })
        .collect();

    let mut drums = Drums::default();
    let mut parts = Vec::new();
    for (c, evs) in events.into_iter().enumerate() {
        let mut v = Voice::default();
        let mut progs = Vec::new();
        let mut offset = 0.0;
        let mut ends: Option<f64> = None;
        let mut last_t = 0.0;
        for e in evs {
            let t = match e {
                Ev::Attack { t, .. } | Ev::Pitch { t, .. } | Ev::Release { t } => t,
            };
            if let Some(e) = ends.filter(|&e| e <= t) {
                v.release(e);
                ends = None;
            }
            last_t = t;
            match e {
                Ev::Attack { t, smp, note, vol } => {
                    let Some(d) = decided.get(smp).and_then(Option::as_ref) else {
                        v.release(t);
                        continue;
                    };
                    match d.kind {
                        Kind::Drum(k) => {
                            v.release(t);
                            drums.hit(t, k, velocity(vol));
                        }
                        Kind::Melodic(p) => {
                            offset = d.offset;
                            v.attack(t, note + offset, velocity(vol));
                            progs.push((t, p));
                            ends = d.length_ms.map(|l| t + l / 2f64.powf((note - 60.0) / 12.0));
                        }
                    }
                }
                Ev::Pitch { t, note } => v.pitch(t, note + offset),
                Ev::Release { t } => v.release(t),
            }
        }
        if let Some(e) = ends {
            v.release(e.min(end.max(last_t)));
        }
        let notes = v.finish(end.max(last_t));
        if notes.is_empty() {
            continue;
        }
        let mut p = Part::new(format!("Channel {}", c + 1), 0, notes).with_programs(&progs);
        p.name = format!("Channel {} · {}", c + 1, super::gm::program_name(p.program));
        parts.push(p);
    }
    parts.push(Part::drums("Drums (unpitched samples)", drums.finish()));

    let samples = scanned
        .samples
        .iter()
        .zip(&decided)
        .enumerate()
        .filter_map(|(i, (s, d))| {
            let (s, d) = (s.as_ref()?, d.as_ref()?);
            Some(SampleInfo {
                srcn: i as u8,
                name: s.name.clone(),
                len: s.pcm.len(),
                looped: s.loop_start.is_some(),
                hz: matches!(d.kind, Kind::Melodic(_))
                    .then(|| 440.0 * 2f64.powf((60.0 + d.offset - 69.0) / 12.0)),
                program: match d.kind {
                    Kind::Melodic(p) => Some(p),
                    _ => None,
                },
                drum_key: match d.kind {
                    Kind::Drum(k) => Some(k),
                    _ => None,
                },
                uses: uses.get(i).map_or(0, Vec::len),
                ..Default::default()
            })
        })
        .collect();
    Ok(Transcription {
        system: "Tracker module".into(),
        parts,
        samples,
        notes: vec![
            "Each sample's pitch is measured to find the notes that sound; \
             instruments are guessed from samples' names and sounds."
                .into(),
        ],
        meter: super::meter::from_rows(&rows, end),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_module() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/libxmp/test-dev/data/Gaffeltruck.mod");
        if !p.exists() {
            return;
        }
        let t = super::transcribe(&crate::vfs::Location::Fs(p), Some(20_000)).unwrap();
        let notes: usize = t.parts.iter().map(|p| p.notes.len()).sum();
        assert!(notes > 50, "{notes}");
        assert!(!t.samples.is_empty());
        // Not every instrument a piano.
        let programs: std::collections::HashSet<u8> = t
            .parts
            .iter()
            .filter(|p| !p.drums)
            .map(|p| p.program)
            .collect();
        assert!(
            programs.len() > 1 || programs.iter().all(|&p| p != 0),
            "{programs:?}"
        );
    }
}
