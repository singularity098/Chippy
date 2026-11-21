//! SNES (SPC): the S-DSP's eight voices, read from the writes the sound
//! driver makes to the DSP's registers.
//!
//! Key-on and key-off give exact note timing. The pitch register only gives
//! a playback rate (a ratio), so a note's pitch also needs the pitch its
//! sample was recorded at. That is measured from the sample itself: decoded
//! from the snapshot's RAM through the sample directory, and run through a
//! pitch detector. Samples with no clear pitch are taken as drums.

use anyhow::{bail, Result};
use std::collections::HashMap;

use super::{brr, Drums, Part, Transcription, Voice};
use crate::decode::gme::capture;
use crate::vfs::Location;

const RATE: u32 = 32_000;
/// SPC clocks per millisecond (1.024 MHz).
const CLOCKS_PER_MS: f64 = 1024.0;
const CLOCKS_PER_FRAME: i64 = 32;

const KON: usize = 0x4C;
const KOFF: usize = 0x5C;
const NON: usize = 0x3D;
const DIR: usize = 0x5D;

/// Keys for unpitched samples, in order of first use (General MIDI drums).
const DRUM_KEYS: [u8; 12] = [36, 38, 42, 46, 49, 45, 48, 51, 39, 37, 41, 56];

/// One of the tune's instrument samples.
#[derive(Clone, Debug)]
pub struct SampleInfo {
    /// Its number in the sample directory (the voices' SRCN).
    pub srcn: u8,
    pub start: u16,
    /// Length in samples, up to the end or the loop.
    pub len: usize,
    pub looped: bool,
    /// The measured pitch at 32 kHz, `None` if unpitched (played as drums).
    pub hz: Option<f64>,
    /// How sure that measure is, 0 to 1.
    pub clarity: f64,
    /// The drum key it was given, if unpitched.
    pub drum_key: Option<u8>,
    /// How many notes play it.
    pub uses: usize,
}

struct Spc {
    ram: Vec<u8>,
    dsp: [u8; 128],
}

fn parse(data: &[u8]) -> Result<Spc> {
    if !data.starts_with(b"SNES-SPC700 Sound File Data") || data.len() < 0x10180 {
        bail!("not an SPC file");
    }
    let mut dsp = [0u8; 128];
    dsp.copy_from_slice(&data[0x10100..0x10180]);
    Ok(Spc {
        ram: data[0x100..0x10100].to_vec(),
        dsp,
    })
}

/// The samples the tune plays, measured once each.
struct Samples<'a> {
    ram: &'a [u8],
    by_entry: HashMap<(u8, u8), usize>,
    list: Vec<SampleInfo>,
    drums: usize,
}

impl Samples<'_> {
    /// The sample `srcn` in the directory at page `dir`.
    fn get(&mut self, dir: u8, srcn: u8) -> &mut SampleInfo {
        let i = *self.by_entry.entry((dir, srcn)).or_insert_with(|| {
            let e = (dir as usize) * 0x100 + srcn as usize * 4;
            let word =
                |a: usize| u16::from_le_bytes([self.ram[a & 0xFFFF], self.ram[(a + 1) & 0xFFFF]]);
            let (start, loop_addr) = (word(e), word(e + 2));
            let s = brr::decode(self.ram, start, loop_addr);
            let p = brr::pitch(&s);
            let drum_key = p.is_none().then(|| {
                self.drums += 1;
                DRUM_KEYS[(self.drums - 1) % DRUM_KEYS.len()]
            });
            self.list.push(SampleInfo {
                srcn,
                start,
                len: s.pcm.len(),
                looped: s.loop_start.is_some(),
                hz: p.as_ref().map(|p| p.hz),
                clarity: p.as_ref().map_or(0.0, |p| p.clarity),
                drum_key,
                uses: 0,
            });
            self.list.len() - 1
        });
        &mut self.list[i]
    }
}

pub fn transcribe(loc: &Location, max_ms: Option<u64>) -> Result<Transcription> {
    let data = crate::decode::gme::load(loc)?;
    let spc = parse(&data)?;
    let mut regs = spc.dsp;
    let mut samples = Samples {
        ram: &spc.ram,
        by_entry: HashMap::new(),
        list: Vec::new(),
        drums: 0,
    };
    let mut voices: [Voice; 8] = Default::default();
    // The sample each voice was keyed on with, while it sounds.
    let mut playing: [Option<f64>; 8] = [None; 8];
    let mut drums = Drums::default();
    let mut last_t = 0.0;

    let cap = capture(loc, 0, RATE, max_ms, 512, |frame, writes| {
        for w in writes.iter().filter(|w| w.kind == 2) {
            let addr = w.addr as usize;
            if addr >= 0x80 {
                continue;
            }
            let data = w.data as u8;
            regs[addr] = data;
            let t = (frame as i64 * CLOCKS_PER_FRAME + w.time).max(0) as f64 / CLOCKS_PER_MS;
            last_t = t;
            let rate_of = |regs: &[u8; 128], v: usize| {
                (regs[v * 16 + 2] as u32 | ((regs[v * 16 + 3] & 0x3F) as u32) << 8) as f64
            };
            let key_of = |base: f64, rate: f64| base + 12.0 * (rate / 4096.0).log2();
            match addr {
                KON => {
                    for v in (0..8).filter(|v| data & (1 << v) != 0) {
                        let s = samples.get(regs[DIR], regs[v * 16 + 4]);
                        s.uses += 1;
                        let rate = rate_of(&regs, v);
                        let noise = regs[NON] & (1 << v) != 0;
                        match (s.hz, s.drum_key) {
                            (Some(hz), _) if !noise && rate > 0.0 => {
                                let base = super::hz_to_key(hz);
                                voices[v].attack(t, key_of(base, rate), 100);
                                playing[v] = Some(base);
                            }
                            (_, key) => {
                                voices[v].release(t);
                                playing[v] = None;
                                drums.hit(t, if noise { 42 } else { key.unwrap_or(38) }, 100);
                            }
                        }
                    }
                }
                KOFF => {
                    for v in (0..8).filter(|v| data & (1 << v) != 0) {
                        voices[v].release(t);
                        playing[v] = None;
                    }
                }
                a if a & 0x0F == 2 || a & 0x0F == 3 => {
                    let v = a >> 4;
                    if let Some(base) = playing[v] {
                        let rate = rate_of(&regs, v);
                        if rate > 0.0 {
                            voices[v].pitch(t, key_of(base, rate));
                        }
                    }
                }
                _ => {}
            }
        }
    })?;
    let end = (cap.frames as f64 * 1000.0 / RATE as f64).max(last_t);
    let mut parts: Vec<Part> = voices
        .into_iter()
        .enumerate()
        .map(|(i, v)| Part {
            name: format!("Voice {}", i + 1),
            drums: false,
            program: 0,
            notes: v.finish(end),
        })
        .collect();
    parts.push(Part {
        name: "Drums (unpitched samples)".into(),
        drums: true,
        program: 0,
        notes: drums.finish(),
    });
    let mut list = samples.list;
    list.sort_by_key(|s| s.srcn);
    Ok(Transcription {
        system: cap.system,
        parts,
        notes: vec![
            "SNES pitches depend on each sample's measured root note: a part \
             playing an octave (or a fifth) off means its sample was misjudged."
                .into(),
        ],
        samples: list,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny SPC: an SPC700 program playing a looped sine instrument
    /// (root 500 Hz, so B4 +21 cents) on voice 0, and a noise drum on
    /// voice 1. Each step is (keys to key on, voice 0's note, 10 ms units to
    /// wait, whether to key off after).
    fn synth_spc(steps: &[(u8, Option<f64>, u8, bool)]) -> Vec<u8> {
        let mut code: Vec<u8> = vec![0x8F, 80, 0xFA, 0x8F, 0x01, 0xF1]; // timer 0: 10 ms
        let w =
            |code: &mut Vec<u8>, reg: u8, val: u8| code.extend([0x8F, reg, 0xF2, 0x8F, val, 0xF3]);
        for (r, v) in [(0x6C, 0x20), (0x0C, 0x7F), (0x1C, 0x7F), (0x5D, 0x10)] {
            w(&mut code, r, v);
        }
        for v in 0..2u8 {
            for (r, x) in [(0, 0x60), (1, 0x60), (4, v), (5, 0x8F), (6, 0xE0)] {
                w(&mut code, v * 16 + r, x);
            }
        }
        w(&mut code, 0x13, 0x10);
        let root = crate::notes::hz_to_key(500.0);
        for &(kon, key, wait, koff) in steps {
            if let Some(key) = key {
                let p = (4096.0 * 2f64.powf((key - root) / 12.0)).round() as u16;
                w(&mut code, 0x02, p as u8);
                w(&mut code, 0x03, (p >> 8) as u8);
            }
            if kon != 0 {
                w(&mut code, KOFF as u8, 0);
                w(&mut code, KON as u8, kon);
            }
            // MOV Y,#wait; loop: MOV A,$FD; BEQ loop; DBNZ Y,loop
            code.extend([0x8D, wait, 0xE4, 0xFD, 0xF0, 0xFC, 0xFE, 0xFA]);
            if koff {
                w(&mut code, KOFF as u8, 0x03);
            }
        }
        code.extend([0x2F, 0xFE]); // BRA $
        let brr = |pcm: &[i32], looped: bool| {
            let mut out = Vec::new();
            let blocks = pcm.len() / 16;
            for (b, ch) in pcm.chunks(16).enumerate() {
                let peak = ch.iter().map(|s| s.abs()).max().unwrap();
                let mut shift = 0;
                while shift < 12 && peak * 2 > 7 << shift {
                    shift += 1;
                }
                let end = if b + 1 == blocks {
                    1 | if looped { 2 } else { 0 }
                } else {
                    0
                };
                out.push((shift << 4) as u8 | end);
                let nib = |s: i32| (((s * 2) >> shift).clamp(-8, 7) & 15) as u8;
                out.extend(ch.chunks(2).map(|p| nib(p[0]) << 4 | nib(p[1])));
            }
            out
        };
        let sine: Vec<i32> = (0..64)
            .map(|i| ((i as f64 * std::f64::consts::TAU / 64.0).sin() * 12000.0) as i32)
            .collect();
        let mut x = 1u32;
        let noise: Vec<i32> = (0..4000)
            .map(|i| {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                ((x >> 16) as i32 - 32768) * (4000 - i) / 4000 / 3
            })
            .collect();
        let mut f = vec![0u8; 0x10200];
        f[..33].copy_from_slice(b"SNES-SPC700 Sound File Data v0.30");
        f[0x21..0x25].copy_from_slice(&[26, 26, 27, 30]);
        f[0x25..0x27].copy_from_slice(&0x200u16.to_le_bytes());
        f[0x2B] = 0xEF;
        let ram = &mut f[0x100..0x10100];
        ram[0x200..0x200 + code.len()].copy_from_slice(&code);
        ram[0x1000..0x1008].copy_from_slice(&[0x00, 0x11, 0x00, 0x11, 0x00, 0x12, 0x00, 0x12]);
        let s = brr(&sine, true);
        ram[0x1100..0x1100 + s.len()].copy_from_slice(&s);
        let n = brr(&noise, false);
        ram[0x1200..0x1200 + n.len()].copy_from_slice(&n);
        f[0x10100 + 0x6C] = 0x60;
        f
    }

    #[test]
    fn finds_the_notes_and_the_samples_pitch() {
        let spc = synth_spc(&[
            (3, Some(72.0), 30, true),
            (0, None, 5, false),
            (3, Some(76.0), 30, true),
            (0, None, 5, false),
            (1, Some(79.0), 30, false),
            (0, Some(81.0), 30, true), // a slide up, no new key-on
            (0, None, 5, false),
            (3, Some(84.0), 60, true),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synth.spc");
        std::fs::write(&path, spc).unwrap();
        let loc = match crate::vfs::resolve(&path).unwrap() {
            crate::vfs::Node::Tracks(l) => l,
            n => panic!("{n:?}"),
        };
        let t = transcribe(&loc, Some(3000)).unwrap();

        let sine = &t.samples[0];
        assert!((sine.hz.unwrap() - 500.0).abs() < 1.0, "{:?}", sine.hz);
        assert_eq!(t.samples[1].drum_key, Some(36));

        let v = &t.parts[0].notes;
        let got: Vec<_> = v
            .iter()
            .map(|n| (n.key, n.legato, (n.start_ms / 10.0).round() as i32 * 10))
            .collect();
        assert_eq!(
            got,
            vec![
                (72, false, 0),
                (76, false, 350),
                (79, false, 700),
                (81, true, 1000),
                (84, false, 1350)
            ]
        );
        assert!((v[4].dur_ms - 600.0).abs() < 15.0, "{}", v[4].dur_ms);
        let drums = &t.parts.last().unwrap().notes;
        assert_eq!(drums.len(), 3);
    }
}
