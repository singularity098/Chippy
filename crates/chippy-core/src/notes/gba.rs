//! Game Boy Advance (GSF). Most GBA games play their music with Nintendo's
//! "m4a" sound engine (known as Sappy), which mixes samples in software:
//! the hardware only sees the finished mix. So the notes are read from the
//! engine itself, once a frame: its SoundInfo (pointed to from $3007FFF0)
//! lists the channels it plays, each with its MIDI key, velocity, sample
//! and the track (the song's part) it belongs to.
//!
//! Games with other engines are read from the writes to the Game Boy sound
//! channels the GBA kept (square, wave and noise), which is all the
//! hardware can tell.

use anyhow::Result;
use std::collections::HashMap;

use super::chips::{Chip, Dmg};
use super::{instrument, Note, Part, SampleInfo, Transcription};
use crate::decode::gsf::{Memory, Watcher};
use crate::vfs::Location;

/// Where m4a keeps the pointer to its SoundInfo, and the SoundInfo's tag.
const SOUND_INFO_PTR: u32 = 0x0300_7FF0;
const IDENT: u32 = 0x6873_6D53;
/// m4a's DirectSound channels at most, and its Game Boy channels.
const DS_CHANNELS: usize = 12;
const SLOTS: usize = DS_CHANNELS + 4;
/// A channel's status flags.
const SF_START: u8 = 0x80;
const SF_STOP: u8 = 0x40;
const SF_IEC: u8 = 0x04;
const SF_ON: u8 = 0xC7;
const ENV_ATTACK: u8 = 3;
/// How far a held note's pitch must move to be a new note.
const SPLIT: f64 = 0.75;

fn in_ram(p: u32) -> bool {
    (0x0200_0000..0x0204_0000).contains(&p) || (0x0300_0000..0x0300_8000).contains(&p)
}

fn in_memory(p: u32) -> bool {
    in_ram(p) || (0x0800_0000..0x0E00_0000).contains(&p)
}

/// What one of m4a's channels is doing.
#[derive(Clone, Copy, Default, PartialEq)]
struct Slot {
    sounding: bool,
    start: bool,
    phase: u8,
    kind: u8,
    key: u8,
    midi: u8,
    velocity: u8,
    freq: u32,
    wav: u32,
    track: u32,
}

impl Slot {
    /// The SoundChannel (or CgbChannel: the same fields at the same
    /// places) at `c`.
    fn read(mem: &dyn Memory, c: u32) -> Slot {
        let status = mem.read8(c);
        let start = status & SF_START != 0;
        let active = status & SF_ON != 0;
        Slot {
            sounding: active && (start || status & (SF_STOP | SF_IEC) == 0),
            start,
            phase: status & 3,
            kind: mem.read8(c + 1),
            key: mem.read8(c + 8),
            midi: mem.read8(c + 0x11),
            velocity: mem.read8(c + 0x12),
            freq: mem.read32(c + 0x20),
            wav: mem.read32(c + 0x24),
            track: mem.read32(c + 0x2C),
        }
    }
}

/// A note being played on a channel.
struct Open {
    note: Note,
    track: u32,
    /// The sample (DirectSound) it plays, or None (a Game Boy channel).
    sample: Option<usize>,
    /// The drum it is, if one.
    drum: Option<u8>,
}

struct Sample {
    info: SampleInfo,
    features: instrument::Features,
    keys: Vec<u8>,
}

#[derive(Default)]
struct M4a {
    seen: bool,
    prev: [Slot; SLOTS],
    open: [Option<Open>; SLOTS],
    /// Tracks in order of first note, with their notes (and samples).
    tracks: Vec<(u32, Vec<(Note, Option<usize>)>)>,
    drums: Vec<Note>,
    by_wav: HashMap<u32, usize>,
    samples: Vec<Sample>,
}

impl M4a {
    fn frame(&mut self, t: f64, mem: &dyn Memory) {
        let info = mem.read32(SOUND_INFO_PTR);
        if !in_ram(info) {
            return;
        }
        // The engine adds 1 to the tag while it works.
        let ident = mem.read32(info);
        if ident != IDENT && ident != IDENT + 1 {
            return;
        }
        self.seen = true;
        let max = (mem.read8(info + 6) as usize).min(DS_CHANNELS);
        let cgb = mem.read32(info + 0x1C);
        for i in 0..SLOTS {
            let slot = if i < max {
                Slot::read(mem, info + 0x50 + i as u32 * 0x40)
            } else if i >= DS_CHANNELS && in_ram(cgb) {
                Slot::read(mem, cgb + (i - DS_CHANNELS) as u32 * 0x40)
            } else {
                Slot::default()
            };
            self.step(t, i, slot, mem);
        }
    }

    fn step(&mut self, t: f64, i: usize, s: Slot, mem: &dyn Memory) {
        let p = self.prev[i];
        let new = s.sounding
            && (!p.sounding
                || s.wav != p.wav
                || s.track != p.track
                || s.midi != p.midi
                || (s.start && !p.start)
                // Back to its attack, not from a start pending.
                || (s.phase == ENV_ATTACK && p.phase != ENV_ATTACK && !p.start));
        if p.sounding && (!s.sounding || new) {
            self.close(t, i);
        }
        if new {
            self.start(t, i, &s, mem, false);
        } else if s.sounding {
            // A bend far enough to be another note.
            let moved = self.open[i].as_ref().is_some_and(|o| {
                o.drum.is_none() && (self.key(i, &s, mem) - o.note.key as f64).abs() >= SPLIT
            });
            if moved {
                self.close(t, i);
                self.start(t, i, &s, mem, true);
            }
        }
        self.prev[i] = s;
    }

    /// The pitch a channel plays, as a fractional MIDI key.
    fn key(&self, i: usize, s: &Slot, mem: &dyn Memory) -> f64 {
        let key = s.key as f64;
        // DirectSound: its rate against the sample's rate at middle C
        // (both in 1024ths of a Hz) gives bends too.
        if i < DS_CHANNELS && s.freq > 0 && in_memory(s.wav) {
            let base = mem.read32(s.wav + 4);
            if base > 0 {
                let k = 60.0 + 12.0 * (s.freq as f64 / base as f64).log2();
                if (k - key).abs() <= 2.0 {
                    return k;
                }
            }
        }
        key
    }

    fn start(&mut self, t: f64, i: usize, s: &Slot, mem: &dyn Memory, legato: bool) {
        let kit = in_ram(s.track) && mem.read8(s.track + 0x24) & 0x80 != 0;
        let ds = i < DS_CHANNELS;
        let sample = (ds && in_memory(s.wav)).then(|| self.sample(s.wav, mem));
        // Drum kits, fixed-rate samples (not tuned to the key) and noise.
        let fixed = ds && s.kind & 0x08 != 0;
        let noise = !ds && s.kind & 7 == 4;
        let drum = if kit && (27..=87).contains(&s.midi) {
            Some(s.midi)
        } else if kit || fixed || noise {
            Some(match sample {
                Some(x) => instrument::drum_for(&self.samples[x].features),
                None => 42,
            })
        } else {
            None
        };
        let key = self.key(i, s, mem).round().clamp(0.0, 127.0) as u8;
        if let (Some(x), None) = (sample, drum) {
            self.samples[x].keys.push(key);
            self.samples[x].info.uses += 1;
        }
        self.open[i] = Some(Open {
            note: Note {
                start_ms: t,
                dur_ms: 0.0,
                key: drum.unwrap_or(key),
                velocity: s.velocity.clamp(1, 127),
                legato,
            },
            track: s.track,
            sample,
            drum,
        });
    }

    fn close(&mut self, t: f64, i: usize) {
        let Some(mut o) = self.open[i].take() else {
            return;
        };
        o.note.dur_ms = (t - o.note.start_ms).max(1.0);
        if o.drum.is_some() {
            o.note.dur_ms = o.note.dur_ms.min(60.0);
            return self.drums.push(o.note);
        }
        let n = match self.tracks.iter().position(|(k, _)| *k == o.track) {
            Some(n) => n,
            None => {
                self.tracks.push((o.track, Vec::new()));
                self.tracks.len() - 1
            }
        };
        self.tracks[n].1.push((o.note, o.sample));
    }

    /// The sample (WaveData) at `wav`, measured once.
    fn sample(&mut self, wav: u32, mem: &dyn Memory) -> usize {
        if let Some(&i) = self.by_wav.get(&wav) {
            return i;
        }
        // WaveData: type, status (loops: $4000), rate at middle C in 1024ths
        // of a Hz, loop start, size, then signed 8-bit samples.
        let looped = mem.read32(wav) >> 16 & 0xC000 != 0;
        let rate = (mem.read32(wav + 4) as f64 / 1024.0).max(1.0);
        let loop_start = mem.read32(wav + 8) as usize;
        let size = (mem.read32(wav + 12) as usize).min(65_536);
        let mut pcm = Vec::with_capacity(size);
        let mut a = wav + 16;
        while pcm.len() < size {
            let w = mem.read32(a).to_le_bytes();
            pcm.extend(w.iter().map(|&b| b as i8 as f32 / 128.0));
            a += 4;
        }
        pcm.truncate(size);
        let loop_start = (looped && loop_start < size).then_some(loop_start);
        let p = instrument::pitch(&pcm, loop_start, rate);
        let hz = p.as_ref().map(|p| p.hz);
        let features = instrument::measure(&pcm, loop_start, rate, hz);
        self.samples.push(Sample {
            info: SampleInfo {
                srcn: self.samples.len().min(255) as u8,
                start: wav as u16,
                len: size,
                looped: loop_start.is_some(),
                hz,
                clarity: p.as_ref().map_or(0.0, |p| p.clarity),
                ..Default::default()
            },
            features,
            keys: Vec::new(),
        });
        let i = self.samples.len() - 1;
        self.by_wav.insert(wav, i);
        i
    }

    fn finish(mut self, end: f64) -> (Vec<Part>, Vec<SampleInfo>) {
        for i in 0..SLOTS {
            self.close(end, i);
        }
        for s in &mut self.samples {
            let median = instrument::median(&mut s.keys);
            if let instrument::Kind::Melodic(p) =
                instrument::classify("", &s.features, true, median)
            {
                s.info.program = Some(p);
            }
        }
        let mut parts = Vec::new();
        for (n, (_, mut notes)) in self.tracks.into_iter().enumerate() {
            notes.sort_by(|a, b| a.0.start_ms.total_cmp(&b.0.start_ms));
            let progs: Vec<(f64, u8)> = notes
                .iter()
                .map(|(note, s)| {
                    let p = match s {
                        Some(s) => self.samples[*s].info.program.unwrap_or(0),
                        // The Game Boy's channels: synth leads.
                        None => 80,
                    };
                    (note.start_ms, p)
                })
                .collect();
            let notes: Vec<Note> = notes.into_iter().map(|(n, _)| n).collect();
            let mut p = Part::new("", 0, notes).with_programs(&progs);
            p.name = format!("Track {} · {}", n + 1, super::gm::program_name(p.program));
            parts.push(p);
        }
        self.drums
            .sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms).then(a.key.cmp(&b.key)));
        parts.push(Part::drums("Drums", self.drums));
        (parts, self.samples.into_iter().map(|s| s.info).collect())
    }
}

/// The I/O byte offsets of the Game Boy sound registers, as the Game Boy's
/// own ($FF10 on): NR10 at $60, NR11-NR14 at $62-$65, and so on.
fn nr_of(io: u32) -> Option<u16> {
    Some(match io {
        0x60 => 0x00,
        0x62..=0x65 => io as u16 - 0x62 + 0x01,
        0x68 | 0x69 => io as u16 - 0x68 + 0x06,
        0x6C | 0x6D => io as u16 - 0x6C + 0x08,
        0x70 => 0x0A,
        0x72..=0x75 => io as u16 - 0x72 + 0x0B,
        0x78 | 0x79 => io as u16 - 0x78 + 0x10,
        0x7C | 0x7D => io as u16 - 0x7C + 0x12,
        0x80 | 0x81 => io as u16 - 0x80 + 0x14,
        0x84 => 0x16,
        _ => return None,
    })
}

struct Gba {
    m4a: M4a,
    dmg: Box<Dmg>,
}

impl Watcher for Gba {
    fn write(&mut self, ms: f64, address: u32, value: u32, bytes: i32) {
        for b in 0..bytes.clamp(1, 2) as u32 {
            if let Some(nr) = nr_of(address + b) {
                self.dmg.write(ms, nr, (value >> (8 * b)) as u8);
            }
        }
    }

    fn frame(&mut self, ms: f64, mem: &dyn Memory) {
        self.m4a.frame(ms, mem);
    }
}

pub fn transcribe(loc: &Location, max_ms: Option<u64>) -> Result<Transcription> {
    let mut gba = Gba {
        m4a: M4a::default(),
        dmg: Box::new(Dmg::new()),
    };
    let end = crate::decode::gsf::capture(loc, max_ms, &mut gba)?;
    let Gba { m4a, dmg } = gba;
    if m4a.seen {
        let (parts, samples) = m4a.finish(end);
        return Ok(Transcription {
            system: "Game Boy Advance".into(),
            parts,
            samples,
            notes: vec![
                "Read from the m4a (Sappy) sound engine: instruments are guessed from how \
                 each sample sounds."
                    .into(),
            ],
            meter: None,
        });
    }
    Ok(Transcription {
        system: "Game Boy Advance".into(),
        parts: dmg.finish(end),
        notes: vec![
            "Only the Game Boy sound channels are read: music this engine mixes from \
             samples isn't."
                .into(),
        ],
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A GBA memory map of bytes, as a test sets it.
    #[derive(Default)]
    struct Fake(RefCell<HashMap<u32, u8>>);

    impl Fake {
        fn set8(&self, a: u32, v: u8) {
            self.0.borrow_mut().insert(a, v);
        }
        fn set32(&self, a: u32, v: u32) {
            for (i, b) in v.to_le_bytes().into_iter().enumerate() {
                self.set8(a + i as u32, b);
            }
        }
    }

    impl Memory for Fake {
        fn read8(&self, a: u32) -> u8 {
            self.0.borrow().get(&a).copied().unwrap_or(0)
        }
        fn read32(&self, a: u32) -> u32 {
            u32::from_le_bytes([0, 1, 2, 3].map(|i| self.read8(a + i)))
        }
    }

    const INFO: u32 = 0x0300_1000;
    const TRACK_A: u32 = 0x0300_2000;
    const KIT: u32 = 0x0300_2100;
    const WAV: u32 = 0x0800_1000;

    fn chan(m: &Fake, i: u32, status: u8, key: u8, midi: u8, track: u32) {
        let c = INFO + 0x50 + i * 0x40;
        m.set8(c, status);
        m.set8(c + 8, key);
        m.set8(c + 0x11, midi);
        m.set8(c + 0x12, 100);
        m.set32(c + 0x24, WAV);
        m.set32(c + 0x2C, track);
        // The rate for the key: the sample's at middle C, shifted.
        let base = 16_000u32 * 1024;
        m.set32(
            c + 0x20,
            (base as f64 * 2f64.powf((key as f64 - 60.0) / 12.0)) as u32,
        );
    }

    #[test]
    fn reads_m4a_channels() {
        let m = Fake::default();
        m.set32(SOUND_INFO_PTR, INFO);
        m.set32(INFO, IDENT);
        m.set8(INFO + 6, 4);
        m.set8(KIT + 0x24, 0x80);
        // A looped square-ish sample, 16 kHz at middle C.
        m.set32(WAV, 0x4000 << 16);
        m.set32(WAV + 4, 16_000 * 1024);
        m.set32(WAV + 8, 0);
        m.set32(WAV + 12, 64);
        for i in 0..64 {
            m.set8(WAV + 16 + i, if (i / 8) % 2 == 0 { 0x60 } else { 0xA0 });
        }
        let mut a = M4a::default();
        let frame = 1000.0 / 60.0;
        // Frame 0: C4 on channel 0 starts (pending), a kick drum on 1.
        chan(&m, 0, SF_START, 60, 60, TRACK_A);
        chan(&m, 1, SF_START, 60, 36, KIT);
        a.frame(0.0, &m);
        // Frame 1: both under way.
        chan(&m, 0, ENV_ATTACK, 60, 60, TRACK_A);
        chan(&m, 1, 2, 60, 36, KIT);
        a.frame(frame, &m);
        // Frame 3: channel 0 released; then E4 on it at frame 5.
        a.frame(2.0 * frame, &m);
        chan(&m, 0, SF_STOP | 2, 60, 60, TRACK_A);
        chan(&m, 1, 0, 60, 36, KIT);
        a.frame(3.0 * frame, &m);
        a.frame(4.0 * frame, &m);
        chan(&m, 0, SF_START, 64, 64, TRACK_A);
        a.frame(5.0 * frame, &m);
        let (parts, samples) = a.finish(10.0 * frame);
        let keys: Vec<_> = parts[0]
            .notes
            .iter()
            .map(|n| {
                (
                    n.key,
                    (n.start_ms / frame).round() as i32,
                    (n.dur_ms / frame).round() as i32,
                )
            })
            .collect();
        assert_eq!(keys, vec![(60, 0, 3), (64, 5, 5)]);
        let drums = &parts.last().unwrap().notes;
        assert_eq!(drums.iter().map(|n| n.key).collect::<Vec<_>>(), vec![36]);
        assert_eq!(samples.len(), 1);
    }

    #[test]
    fn maps_the_game_boy_registers() {
        assert_eq!(nr_of(0x60), Some(0x00));
        assert_eq!(nr_of(0x65), Some(0x04));
        assert_eq!(nr_of(0x6D), Some(0x09));
        assert_eq!(nr_of(0x75), Some(0x0E));
        assert_eq!(nr_of(0x7D), Some(0x13));
        assert_eq!(nr_of(0x61), None);
    }
}
