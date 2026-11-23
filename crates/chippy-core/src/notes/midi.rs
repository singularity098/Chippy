//! Standard MIDI files from a [`Transcription`]: one track per part (with
//! its instrument, and changes of instrument). Either at a fixed 120 BPM
//! ([`write`], times as they were played, for playing), or along the
//! track's bars ([`write_in`]: its tempo and time signatures, so that the
//! notes fall on the beats in a sequencer or notation program, at the
//! times they were played).

use super::meter::{compound, Meter};
use super::{Part, Transcription};

const PPQ: u16 = 480;
/// At 120 BPM a quarter note lasts 500 ms.
const TICKS_PER_MS: f64 = PPQ as f64 / 500.0;
const DRUM_CHANNEL: u8 = 9;

fn var_len(out: &mut Vec<u8>, mut v: u32) {
    let mut bytes = vec![(v & 0x7F) as u8];
    v >>= 7;
    while v > 0 {
        bytes.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    out.extend(bytes.iter().rev());
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
}

/// Events of one track: (tick, order at that tick, bytes). Note-offs sort
/// before note-ons at the same tick so repeated notes don't cut each other.
fn track(events: &mut [(u32, u8, Vec<u8>)]) -> Vec<u8> {
    events.sort_by_key(|e| (e.0, e.1));
    let mut body = Vec::new();
    let mut now = 0;
    for (tick, _, bytes) in events.iter() {
        var_len(&mut body, tick - now);
        body.extend_from_slice(bytes);
        now = *tick;
    }
    body.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
    body
}

fn text(kind: u8, s: &str) -> Vec<u8> {
    let mut v = vec![0xFF, kind];
    var_len(&mut v, s.len() as u32);
    v.extend_from_slice(s.as_bytes());
    v
}

/// At 120 BPM in 4/4, times as they were played.
pub fn write(t: &Transcription, title: &str) -> Vec<u8> {
    let tempo = vec![
        (0, 1, vec![0xFF, 0x51, 3, 0x07, 0xA1, 0x20]), // 500000 µs per beat
        (0, 2, vec![0xFF, 0x58, 4, 4, 2, 24, 8]),      // 4/4
    ];
    write_with(t, title, tempo, &|ms| {
        (ms * TICKS_PER_MS).round().max(0.0) as u32
    })
}

/// Along the bars of `meter`: each beat a beat, its tempo changing as the
/// beats do, and the time signatures as they change. Tick 0 is the start
/// of the first bar.
pub fn write_in(t: &Transcription, title: &str, meter: &Meter) -> Vec<u8> {
    if meter.bars.is_empty() {
        return write(t, title);
    }
    let ruler = meter.ruler();
    let ticks = |ms: f64| (ruler.quarters(ms) * PPQ as f64).round().max(0.0) as u32;
    let mut tempo = Vec::new();
    // A tempo for each beat, written when it changes enough that playing
    // on at the last one would put the notes more than 2 ms out.
    let (mut last_us, mut last_sig, mut drift) = (0u32, None, 0.0f64);
    for (i, bar) in meter.bars.iter().enumerate() {
        let tick = ticks(bar.start_ms);
        if last_sig != Some(bar.sig) {
            let (n, d) = bar.sig;
            let clocks = if compound(bar.sig) { 36 } else { 96 / d.max(1) };
            tempo.push((
                tick,
                2,
                vec![0xFF, 0x58, 4, n, d.max(1).trailing_zeros() as u8, clocks, 8],
            ));
            last_sig = Some(bar.sig);
        }
        let end = meter.bar_end(i);
        for (k, &b) in bar.beats.iter().enumerate() {
            let next = bar.beats.get(k + 1).copied().unwrap_or(end);
            let ms = next - b;
            let us = (ms / bar.beat_quarters() * 1000.0)
                .round()
                .clamp(1.0, 16_777_215.0) as u32;
            let at_last = last_us as f64 / 1000.0 * bar.beat_quarters();
            if last_us == 0 || (drift + at_last - ms).abs() > 2.0 {
                let [_, a, b2, c] = us.to_be_bytes();
                tempo.push((ticks(b), 1, vec![0xFF, 0x51, 3, a, b2, c]));
                last_us = us;
                drift += us as f64 / 1000.0 * bar.beat_quarters() - ms;
            } else {
                drift += at_last - ms;
            }
        }
    }
    write_with(t, title, tempo, &ticks)
}

/// The MIDI channel each of `parts` plays on: its own, if it was read
/// from a MIDI file, else the one [`write`] gives it (the drums on 10, the
/// other parts on the rest in turn; parts without notes get none).
pub fn channels(parts: &[Part]) -> Vec<u8> {
    let mut next_channel = 0u8;
    parts
        .iter()
        .map(|p| {
            if let Some(c) = p.channel {
                return c & 0x0F;
            }
            if p.drums {
                return DRUM_CHANNEL;
            }
            if p.notes.is_empty() {
                return 0;
            }
            let c = next_channel;
            next_channel = (next_channel + 1) % 15;
            if c >= DRUM_CHANNEL {
                c + 1
            } else {
                c
            }
        })
        .collect()
}

fn write_with(
    t: &Transcription,
    title: &str,
    conductor: Vec<(u32, u8, Vec<u8>)>,
    ticks: &dyn Fn(f64) -> u32,
) -> Vec<u8> {
    let mut out = Vec::new();
    let parts = t.parts.iter().filter(|p| !p.notes.is_empty()).count();
    let mut header = Vec::new();
    header.extend_from_slice(&1u16.to_be_bytes());
    header.extend_from_slice(&(parts as u16 + 1).to_be_bytes());
    header.extend_from_slice(&PPQ.to_be_bytes());
    chunk(&mut out, b"MThd", &header);

    let mut tempo = vec![(0, 0, text(0x03, title))];
    tempo.extend(conductor);
    chunk(&mut out, b"MTrk", &track(&mut tempo));

    let all = channels(&t.parts);
    for (p, &ch) in t
        .parts
        .iter()
        .zip(&all)
        .filter(|(p, _)| !p.notes.is_empty())
    {
        let mut ev = vec![(0, 0, text(0x03, &p.name))];
        if !p.drums {
            ev.push((0, 1, vec![0xC0 | ch, p.program & 0x7F]));
            for &(t, prog) in &p.programs {
                ev.push((ticks(t), 1, vec![0xC0 | ch, prog & 0x7F]));
            }
        }
        for n in &p.notes {
            let on = ticks(n.start_ms);
            let off = ticks(n.start_ms + n.dur_ms).max(on + 1);
            ev.push((
                on,
                3,
                vec![0x90 | ch, n.key & 0x7F, n.velocity.clamp(1, 127)],
            ));
            ev.push((off, 2, vec![0x80 | ch, n.key & 0x7F, 0]));
        }
        chunk(&mut out, b"MTrk", &track(&mut ev));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{Note, Part};
    use super::*;

    #[test]
    fn writes_a_readable_file() {
        let t = Transcription {
            parts: vec![Part {
                name: "Lead".into(),
                program: 80,
                programs: vec![(500.0, 81)],
                notes: vec![
                    Note {
                        start_ms: 0.0,
                        dur_ms: 500.0,
                        key: 60,
                        velocity: 100,
                        legato: false,
                    },
                    Note {
                        start_ms: 500.0,
                        dur_ms: 250.0,
                        key: 64,
                        velocity: 90,
                        legato: false,
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let bytes = write(&t, "Test");
        // RustySynth (Chippy's MIDI player) reads it back, 750 ms long.
        let mut cur = std::io::Cursor::new(bytes);
        let m = rustysynth::MidiFile::new(&mut cur).unwrap();
        assert!((m.get_length() - 0.75).abs() < 0.01, "{}", m.get_length());
        // And Chippy's own reader: the notes, and the change of instrument.
        let back = super::super::smf::read(&write(&t, "Test")).unwrap();
        let p = &back.parts[0];
        assert_eq!(p.notes.len(), 2);
        assert_eq!((p.program, p.programs.len()), (80, 1));
        assert!((p.notes[1].start_ms - 500.0).abs() < 1.0);
    }

    #[test]
    fn writes_along_the_bars() {
        use super::super::meter::{Bar, Meter};
        let note = |t: f64| Note {
            start_ms: t,
            dur_ms: 300.0,
            key: 60,
            velocity: 100,
            legato: false,
        };
        let t = Transcription {
            parts: vec![Part::new(
                "Lead",
                80,
                vec![note(0.0), note(1200.0), note(2000.0)],
            )],
            ..Default::default()
        };
        // Bars of 3/4 at 400 ms a beat, then 2/4 at 500.
        let meter = Meter {
            bars: vec![
                Bar {
                    start_ms: 0.0,
                    beats: vec![0.0, 400.0, 800.0],
                    sig: (3, 4),
                },
                Bar {
                    start_ms: 1200.0,
                    beats: vec![1200.0, 1700.0],
                    sig: (2, 4),
                },
            ],
            end_ms: 2200.0,
            exact: false,
        };
        let bytes = write_in(&t, "Test", &meter);
        let back = super::super::smf::read(&bytes).unwrap();
        let starts: Vec<f64> = back.parts[0].notes.iter().map(|n| n.start_ms).collect();
        for (a, b) in starts.iter().zip([0.0, 1200.0, 2000.0]) {
            assert!((a - b).abs() < 1.5, "{starts:?}");
        }
        let m = back.meter.unwrap();
        assert_eq!(m.bars[0].sig, (3, 4));
        assert_eq!(m.bars[1].sig, (2, 4));
        assert!((m.bars[1].start_ms - 1200.0).abs() < 1.5);
        assert!((m.bpm(0) - 150.0).abs() < 0.5);
    }

    #[test]
    fn variable_length_numbers() {
        let mut v = Vec::new();
        var_len(&mut v, 0x3FFF);
        assert_eq!(v, [0xFF, 0x7F]);
        v.clear();
        var_len(&mut v, 0);
        assert_eq!(v, [0]);
    }
}
