//! Standard MIDI files from a [`Transcription`]: one track per part (with
//! its instrument, and changes of instrument), at a fixed 120 BPM so that
//! times stay as they were played (tempo isn't worked out).

use super::Transcription;

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

pub fn write(t: &Transcription, title: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let parts: Vec<_> = t.parts.iter().filter(|p| !p.notes.is_empty()).collect();
    let mut header = Vec::new();
    header.extend_from_slice(&1u16.to_be_bytes());
    header.extend_from_slice(&(parts.len() as u16 + 1).to_be_bytes());
    header.extend_from_slice(&PPQ.to_be_bytes());
    chunk(&mut out, b"MThd", &header);

    let mut tempo = vec![
        (0, 0, text(0x03, title)),
        (0, 1, vec![0xFF, 0x51, 3, 0x07, 0xA1, 0x20]), // 500000 µs per beat
        (0, 2, vec![0xFF, 0x58, 4, 4, 2, 24, 8]),      // 4/4
    ];
    chunk(&mut out, b"MTrk", &track(&mut tempo));

    let mut next_channel = 0u8;
    for p in parts {
        let ch = if p.drums {
            DRUM_CHANNEL
        } else {
            let c = next_channel;
            next_channel = (next_channel + 1) % 15;
            if c >= DRUM_CHANNEL {
                c + 1
            } else {
                c
            }
        };
        let mut ev = vec![(0, 0, text(0x03, &p.name))];
        if !p.drums {
            ev.push((0, 1, vec![0xC0 | ch, p.program & 0x7F]));
            for &(t, prog) in &p.programs {
                let tick = (t * TICKS_PER_MS).round().max(0.0) as u32;
                ev.push((tick, 1, vec![0xC0 | ch, prog & 0x7F]));
            }
        }
        for n in &p.notes {
            let on = (n.start_ms * TICKS_PER_MS).round() as u32;
            let off = ((n.start_ms + n.dur_ms) * TICKS_PER_MS)
                .round()
                .max(on as f64 + 1.0) as u32;
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
    fn variable_length_numbers() {
        let mut v = Vec::new();
        var_len(&mut v, 0x3FFF);
        assert_eq!(v, [0xFF, 0x7F]);
        v.clear();
        var_len(&mut v, 0);
        assert_eq!(v, [0]);
    }
}
