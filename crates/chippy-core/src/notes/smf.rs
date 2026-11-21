//! Reading Standard MIDI files: their notes, as a [`Transcription`] (a part
//! per MIDI channel), and their lyrics (karaoke `.kar` files).

use anyhow::{bail, Result};
use std::collections::HashMap;

use super::{Note, Part, Transcription};

/// One event of a track, at its absolute tick.
enum Ev {
    On { ch: u8, key: u8, vel: u8 },
    Off { ch: u8, key: u8 },
    Program { ch: u8, program: u8 },
    Tempo(u32),
    /// Meta text: (kind, text). 1 text, 3 track name, 5 lyric.
    Text(u8, String),
}

struct Track {
    events: Vec<(u64, Ev)>,
}

fn be16(d: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([d[at], d[at + 1]])
}

fn var_len(d: &[u8], pos: &mut usize) -> Result<u32> {
    let mut v = 0u32;
    for _ in 0..4 {
        let Some(&b) = d.get(*pos) else {
            bail!("a MIDI track ends early")
        };
        *pos += 1;
        v = (v << 7) | (b & 0x7F) as u32;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Ok(v)
}

/// Text from a MIDI file: UTF-8 if it is, else Latin-1.
fn text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

fn parse_track(d: &[u8]) -> Result<Track> {
    let mut events = Vec::new();
    let (mut pos, mut tick, mut running) = (0usize, 0u64, 0u8);
    while pos < d.len() {
        tick += var_len(d, &mut pos)? as u64;
        let Some(&first) = d.get(pos) else { break };
        // Running status: a channel message without its status byte repeats
        // the last channel status (kept across meta events, which many
        // files rely on).
        let status = if first & 0x80 != 0 {
            pos += 1;
            if first < 0xF0 {
                running = first;
            }
            first
        } else if running == 0 {
            bail!("a MIDI track starts with running status");
        } else {
            running
        };
        let arg = |i: usize| d.get(pos + i).copied().unwrap_or(0);
        match status {
            0xFF => {
                let kind = arg(0);
                pos += 1;
                let len = var_len(d, &mut pos)? as usize;
                let body = d.get(pos..pos + len).unwrap_or(&[]);
                match kind {
                    0x2F => break,
                    0x51 if len >= 3 => events.push((
                        tick,
                        Ev::Tempo((body[0] as u32) << 16 | (body[1] as u32) << 8 | body[2] as u32),
                    )),
                    1 | 3 | 5 => events.push((tick, Ev::Text(kind, text(body)))),
                    _ => {}
                }
                pos += len;
            }
            0xF0 | 0xF7 => {
                let len = var_len(d, &mut pos)? as usize;
                pos += len;
            }
            0xF1..=0xFE => {}
            s => {
                let ch = s & 15;
                match s & 0xF0 {
                    0x90 if arg(1) > 0 => events.push((
                        tick,
                        Ev::On {
                            ch,
                            key: arg(0) & 0x7F,
                            vel: arg(1),
                        },
                    )),
                    0x80 | 0x90 => events.push((
                        tick,
                        Ev::Off {
                            ch,
                            key: arg(0) & 0x7F,
                        },
                    )),
                    0xC0 => events.push((
                        tick,
                        Ev::Program {
                            ch,
                            program: arg(0) & 0x7F,
                        },
                    )),
                    _ => {}
                }
                pos += match s & 0xF0 {
                    0xC0 | 0xD0 => 1,
                    _ => 2,
                };
            }
        }
    }
    Ok(Track { events })
}

struct File {
    tracks: Vec<Track>,
    /// Ticks per quarter note, or (negative) SMPTE ticks per second.
    division: u16,
}

fn parse(d: &[u8]) -> Result<File> {
    // RIFF MIDI (.rmi) wraps a standard file.
    let d = if d.starts_with(b"RIFF") {
        match d.windows(4).position(|w| w == b"MThd") {
            Some(i) => &d[i..],
            None => bail!("not a MIDI file"),
        }
    } else {
        d
    };
    if !d.starts_with(b"MThd") || d.len() < 14 {
        bail!("not a MIDI file");
    }
    let division = be16(d, 12);
    let mut pos = 8 + u32::from_be_bytes([d[4], d[5], d[6], d[7]]) as usize;
    let mut tracks = Vec::new();
    while pos + 8 <= d.len() {
        let len = u32::from_be_bytes([d[pos + 4], d[pos + 5], d[pos + 6], d[pos + 7]]) as usize;
        let body = &d[pos + 8..(pos + 8 + len).min(d.len())];
        if &d[pos..pos + 4] == b"MTrk" {
            tracks.push(parse_track(body)?);
        }
        pos += 8 + len;
    }
    Ok(File { tracks, division })
}

/// Turns ticks into milliseconds through the tempo changes.
struct Clock {
    /// (tick, ms at that tick, µs per quarter from there).
    map: Vec<(u64, f64, u32)>,
    ppq: f64,
    smpte_ms_per_tick: Option<f64>,
}

impl Clock {
    fn new(f: &File) -> Clock {
        let smpte_ms_per_tick = (f.division & 0x8000 != 0).then(|| {
            let fps = -((f.division >> 8) as i8) as f64;
            let per_frame = (f.division & 0xFF) as f64;
            1000.0 / (fps.max(1.0) * per_frame.max(1.0))
        });
        let mut tempos: Vec<(u64, u32)> = f
            .tracks
            .iter()
            .flat_map(|t| t.events.iter())
            .filter_map(|(tick, e)| match e {
                Ev::Tempo(t) => Some((*tick, *t)),
                _ => None,
            })
            .collect();
        tempos.sort_by_key(|t| t.0);
        let ppq = (f.division as f64).max(1.0);
        let mut map = vec![(0u64, 0.0f64, 500_000u32)];
        for (tick, tempo) in tempos {
            let &(t0, ms0, us) = map.last().unwrap();
            let ms = ms0 + (tick - t0) as f64 * us as f64 / ppq / 1000.0;
            if tick == t0 {
                map.last_mut().unwrap().2 = tempo;
            } else {
                map.push((tick, ms, tempo));
            }
        }
        Clock {
            map,
            ppq,
            smpte_ms_per_tick,
        }
    }

    fn ms(&self, tick: u64) -> f64 {
        if let Some(m) = self.smpte_ms_per_tick {
            return tick as f64 * m;
        }
        let i = self.map.partition_point(|e| e.0 <= tick).max(1) - 1;
        let (t0, ms0, us) = self.map[i];
        ms0 + (tick - t0) as f64 * us as f64 / self.ppq / 1000.0
    }
}

/// The notes of a MIDI file: a part per channel, channel 10 as drums.
pub fn read(data: &[u8]) -> Result<Transcription> {
    let f = parse(data)?;
    let clock = Clock::new(&f);
    // A channel's name: the name of the (only) track that plays it.
    let mut names: HashMap<u8, Option<String>> = HashMap::new();
    let mut events: Vec<(u64, usize, &Ev)> = Vec::new();
    for (ti, t) in f.tracks.iter().enumerate() {
        let name = t.events.iter().find_map(|(_, e)| match e {
            Ev::Text(3, s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        });
        for (tick, e) in &t.events {
            if let Ev::On { ch, .. } = e {
                let n = names.entry(*ch).or_insert_with(|| name.clone());
                if n.as_ref() != name.as_ref() {
                    *n = None;
                }
            }
            events.push((*tick, ti, e));
        }
    }
    events.sort_by_key(|e| e.0);
    let mut notes: Vec<Vec<Note>> = vec![Vec::new(); 16];
    let mut programs: Vec<Vec<(f64, u8)>> = vec![Vec::new(); 16];
    let mut program = [0u8; 16];
    let mut open: HashMap<(u8, u8), Vec<(f64, u8)>> = HashMap::new();
    let mut last = 0.0f64;
    for (tick, _, e) in events {
        let t = clock.ms(tick);
        last = last.max(t);
        match *e {
            Ev::On { ch, key, vel } => {
                open.entry((ch, key)).or_default().push((t, vel));
                programs[ch as usize].push((t, program[ch as usize]));
            }
            Ev::Off { ch, key } => {
                if let Some(v) = open.get_mut(&(ch, key)) {
                    if !v.is_empty() {
                        let (start, vel) = v.remove(0);
                        notes[ch as usize].push(Note {
                            start_ms: start,
                            dur_ms: (t - start).max(1.0),
                            key,
                            velocity: vel,
                            legato: false,
                        });
                    }
                }
            }
            Ev::Program { ch, program: p } => program[ch as usize] = p,
            _ => {}
        }
    }
    for ((ch, key), v) in open {
        for (start, vel) in v {
            notes[ch as usize].push(Note {
                start_ms: start,
                dur_ms: (last - start).max(1.0),
                key,
                velocity: vel,
                legato: false,
            });
        }
    }
    let mut parts = Vec::new();
    for (ch, mut n) in notes.into_iter().enumerate() {
        if n.is_empty() {
            continue;
        }
        n.sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms).then(a.key.cmp(&b.key)));
        let name = names
            .get(&(ch as u8))
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("Channel {}", ch + 1));
        parts.push(if ch == 9 {
            Part::drums(name, n)
        } else {
            Part::new(name, 0, n).with_programs(&programs[ch])
        });
    }
    Ok(Transcription {
        system: "MIDI".into(),
        parts,
        ..Default::default()
    })
}

/// A karaoke file's lyrics: (ms, text) per syllable or line, as stored.
/// Lyric events (type 5) if there are any, else the text events of a
/// `.kar` file (type 1, past its `@` header lines).
pub fn lyrics(data: &[u8]) -> Result<Vec<(f64, String)>> {
    let f = parse(data)?;
    let clock = Clock::new(&f);
    let pick = |kind: u8| -> Vec<(f64, String)> {
        let mut v: Vec<(f64, String)> = f
            .tracks
            .iter()
            .flat_map(|t| t.events.iter())
            .filter_map(|(tick, e)| match e {
                Ev::Text(k, s) if *k == kind && !s.starts_with('@') => Some((clock.ms(*tick), s.clone())),
                _ => None,
            })
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    };
    let lyr = pick(5);
    if lyr.iter().filter(|(_, s)| !s.trim().is_empty()).count() >= 4 {
        return Ok(lyr);
    }
    // Text events: only karaoke-style ones (with line marks) are lyrics.
    let txt = pick(1);
    let marked = txt
        .iter()
        .filter(|(_, s)| s.starts_with('/') || s.starts_with('\\'))
        .count();
    Ok(if marked >= 2 { txt } else { lyr })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smf(events: &[u8]) -> Vec<u8> {
        let mut d = b"MThd\0\0\0\x06\0\x01\0\x01\x01\xE0".to_vec(); // 480 ppq
        d.extend_from_slice(b"MTrk");
        let mut body = events.to_vec();
        body.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
        d.extend_from_slice(&(body.len() as u32).to_be_bytes());
        d.extend_from_slice(&body);
        d
    }

    #[test]
    fn reads_notes_through_tempo_changes() {
        let d = smf(&[
            0, 0xFF, 0x03, 4, b'L', b'e', b'a', b'd', // name
            0, 0xC0, 25, // steel guitar
            0, 0x90, 60, 100, // C4 on
            0x83, 0x60, 0x80, 60, 0, // 480 ticks later off (500 ms)
            0, 0xFF, 0x51, 3, 0x0F, 0x42, 0x40, // 1 s per quarter
            0, 0x90, 64, 90, // running status after meta: full status
            0x83, 0x60, 64, 0, // running status, vel 0 = off, 1000 ms
            0, 0xFF, 0x05, 3, b'H', b'e', b'y',
        ]);
        let t = read(&d).unwrap();
        assert_eq!(t.parts.len(), 1);
        let p = &t.parts[0];
        assert_eq!((p.name.as_str(), p.program), ("Lead", 25));
        let got: Vec<_> = p
            .notes
            .iter()
            .map(|n| (n.key, n.start_ms.round(), n.dur_ms.round()))
            .collect();
        assert_eq!(got, vec![(60, 0.0, 500.0), (64, 500.0, 1000.0)]);
        let l = lyrics(&d).unwrap();
        assert_eq!(l, vec![(1500.0, "Hey".to_string())]);
    }
}
