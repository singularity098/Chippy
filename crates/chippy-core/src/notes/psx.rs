//! PlayStation (PSF, SPU) and PlayStation 2 (PSF2): the SPU's 24 voices,
//! or the SPU2's 48, read from the register writes the sound driver makes
//! as the Audio Overload engines play the tune. Each voice plays a sample
//! (4-bit ADPCM, in the SPU's RAM) at a pitch ratio, so the notes' pitches
//! come from each sample's measured pitch ([`super::sampled`]).

use anyhow::Result;

use super::sampled::{sample_id, velocity, Pcm, Sampler};
use super::Transcription;
use crate::vfs::Location;

/// Decodes the SPU ADPCM sample starting at byte `start` of `ram`, to its
/// end or `max` samples; with where its loop starts, if it loops.
pub(crate) fn decode_adpcm(ram: &[u8], start: usize, max: usize) -> (Vec<f32>, Option<usize>) {
    const FILTERS: [(i32, i32); 5] = [(0, 0), (60, 0), (115, -52), (98, -55), (122, -60)];
    let mut out = Vec::new();
    let (mut s1, mut s2) = (0i32, 0i32);
    let mut loop_mark = None;
    let mut pos = start;
    while pos + 16 <= ram.len() && out.len() < max {
        let shift = (ram[pos] & 15).min(12) as u32;
        let (f0, f1) = FILTERS[((ram[pos] >> 4) as usize).min(4)];
        let flags = ram[pos + 1];
        if flags & 4 != 0 {
            loop_mark = Some(out.len());
        }
        for i in 0..28 {
            let b = ram[pos + 2 + i / 2];
            let nib = if i % 2 == 0 { b & 15 } else { b >> 4 };
            let s = (((nib as i32) << 28) >> 28) << (12 - shift);
            let v = (s + (s1 * f0 + s2 * f1 + 32) / 64).clamp(-32768, 32767);
            s2 = s1;
            s1 = v;
            out.push(v as f32 / 32768.0);
        }
        pos += 16;
        if flags & 1 != 0 {
            // The end: with "repeat", it loops.
            return (
                out,
                if flags & 2 != 0 {
                    Some(loop_mark.unwrap_or(0))
                } else {
                    None
                },
            );
        }
    }
    (out, None)
}

/// The longest a sample is decoded to: 3 s at 44.1 kHz.
const MAX_SAMPLES: usize = 132_300;

/// One SPU (PS1) or SPU2 (PS2), as register writes reach it.
struct Spu {
    ps2: bool,
    regs: Vec<u16>,
    sampler: Sampler,
}

impl Spu {
    fn new(ps2: bool) -> Spu {
        Spu {
            ps2,
            regs: vec![0; 0x8000],
            sampler: Sampler::new(if ps2 { 48 } else { 24 }),
        }
    }

    /// Register offsets of voice `v`'s parameters (volumes, pitch) and its
    /// start address, in halfwords.
    fn voice_regs(&self, v: usize) -> (usize, usize) {
        if self.ps2 {
            let (core, v) = (v / 24, v % 24);
            (core * 0x400 + v * 0x10, core * 0x400 + 0x1C0 + v * 12)
        } else {
            (0xC00 + v * 0x10, 0xC00 + v * 0x10 + 6)
        }
    }

    fn reg(&self, byte: usize) -> u16 {
        self.regs[(byte >> 1) & 0x7FFF]
    }

    fn ratio(&self, v: usize) -> f64 {
        self.reg(self.voice_regs(v).0 + 4) as f64 / 4096.0
    }

    fn level(&self, v: usize) -> f64 {
        let p = self.voice_regs(v).0;
        let vol = |r: u16| {
            if r & 0x8000 != 0 {
                // A sweep: say loud.
                0x3000
            } else {
                // 15 bits, signed: its size.
                let x = ((r << 1) as i16) >> 1;
                x.unsigned_abs() as i32
            }
        };
        vol(self.reg(p)).max(vol(self.reg(p + 2))) as f64
    }

    fn start(&self, v: usize) -> usize {
        let (_, a) = self.voice_regs(v);
        if self.ps2 {
            // In halfwords: 20 bits over two registers.
            (((self.reg(a) as usize & 0xF) << 16) | self.reg(a + 2) as usize) * 2
        } else {
            self.reg(a) as usize * 8
        }
    }

    /// Whether voice `v` plays noise rather than its sample.
    fn noise(&self, v: usize) -> bool {
        let (base, bit) = if self.ps2 {
            (
                (v / 24) * 0x400 + 0x184 + if v % 24 >= 16 { 2 } else { 0 },
                v % 24 % 16,
            )
        } else {
            (0xD94 + if v >= 16 { 2 } else { 0 }, v % 16)
        };
        self.reg(base) & (1 << bit) != 0
    }

    fn write(&mut self, t: f64, reg: usize, val: u16, ram: &[u8]) {
        self.regs[(reg >> 1) & 0x7FFF] = val;
        // Key on and off: 16 voices a register (two for each 24).
        let keys = if self.ps2 {
            match reg & 0x3FF {
                0x1A0..=0x1A6 => {
                    let core = (reg >> 10) & 1;
                    let half = (reg >> 1) & 1;
                    Some((reg & 4 == 0, core * 24 + half * 16))
                }
                _ => None,
            }
        } else {
            match reg {
                0xD88..=0xD8E => Some((reg < 0xD8C, if reg & 2 != 0 { 16 } else { 0 })),
                _ => None,
            }
        };
        if let Some((on, first)) = keys {
            let count = if first % 24 == 16 { 8 } else { 16 };
            for b in (0..count).filter(|b| val & (1 << b) != 0) {
                let v = first + b;
                if !on {
                    self.sampler.key_off(t, v);
                    continue;
                }
                let vel = velocity(self.level(v), 0x3FFF as f64);
                if self.noise(v) {
                    self.sampler.noise(t, v, vel);
                    continue;
                }
                let start = self.start(v);
                let rate = if self.ps2 { 48_000.0 } else { 44_100.0 };
                let s = self.sampler.sample(sample_id(start, ram), || {
                    let (pcm, loop_start) = decode_adpcm(ram, start, MAX_SAMPLES);
                    Pcm {
                        pcm,
                        loop_start,
                        rate,
                    }
                });
                let ratio = self.ratio(v);
                self.sampler.key_on(t, v, s, ratio, vel);
            }
            return;
        }
        // A pitch change.
        let voices = if self.ps2 { 48 } else { 24 };
        let v = (0..voices).find(|&v| self.voice_regs(v).0 + 4 == reg);
        if let Some(v) = v {
            if self.sampler.sounding(v) {
                let ratio = self.ratio(v);
                self.sampler.pitch(t, v, ratio);
            }
        }
    }
}

pub fn transcribe(loc: &Location, max_ms: Option<u64>) -> Result<Transcription> {
    let mut spu: Option<Spu> = None;
    let (system, end) = crate::decode::ao::capture(loc, max_ms, &mut |t, kind, reg, val, ram| {
        let s = spu.get_or_insert_with(|| Spu::new(kind == 2));
        s.write(t, reg as usize, val as u16, ram);
    })?;
    let spu = spu.unwrap_or_else(|| Spu::new(false));
    let name = if spu.ps2 { "SPU2" } else { "Voice" };
    let (parts, samples) = spu.sampler.finish(name, end);
    Ok(Transcription {
        system: system.into(),
        parts,
        samples,
        notes: vec![
            "PlayStation pitches depend on each sample's measured root note: a part \
             playing an octave (or a fifth) off means its sample was misjudged. \
             Instruments are guessed from how each sample sounds."
                .into(),
        ],
        meter: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes PCM as SPU ADPCM (filter 0 only), the last block flagged
    /// as the end (and repeat, if `looped`, from the start).
    pub(crate) fn encode(pcm: &[i16], looped: bool) -> Vec<u8> {
        let blocks = pcm.chunks(28).count();
        let mut out = Vec::new();
        for (b, ch) in pcm.chunks(28).enumerate() {
            let peak = ch.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0);
            let mut shift = 12u8;
            while shift > 0 && peak > (7 << (12 - shift)) {
                shift -= 1;
            }
            let mut flags = if b == 0 && looped { 4 } else { 0 };
            if b + 1 == blocks {
                flags |= 1 | if looped { 2 } else { 0 };
            }
            out.extend([shift, flags]);
            let nib = |s: i16| (((s as i32) >> (12 - shift as u32)).clamp(-8, 7) & 15) as u8;
            for i in 0..14 {
                let a = ch.get(i * 2).copied().unwrap_or(0);
                let b = ch.get(i * 2 + 1).copied().unwrap_or(0);
                out.push(nib(a) | nib(b) << 4);
            }
        }
        out
    }

    #[test]
    fn decodes_adpcm() {
        let tone: Vec<i16> = (0..280)
            .map(|i| ((i as f64 / 28.0 * std::f64::consts::TAU).sin() * 20000.0) as i16)
            .collect();
        let enc = encode(&tone, true);
        let (pcm, looped) = decode_adpcm(&enc, 0, 10_000);
        assert_eq!(pcm.len(), 280);
        assert_eq!(looped, Some(0));
        let err = pcm
            .iter()
            .zip(&tone)
            .map(|(a, &b)| (a * 32768.0 - b as f32).abs())
            .fold(0.0, f32::max);
        // The test encoder truncates to 4 bits: steps of 4096 at this level.
        assert!(err <= 4096.0, "{err}");
    }

    /// An old-format SPU log: RAM with a looped tone (one cycle of 28
    /// samples, repeated: 44100 / 28 = 1575 Hz at pitch $1000), then
    /// (tick, register, value) writes.
    fn synth_spu(events: &[(u32, u32, u16)]) -> Vec<u8> {
        let mut f = vec![0u8; 0x80208];
        f[..4].copy_from_slice(b"SPU\0");
        let tone: Vec<i16> = (0..28 * 40)
            .map(|i| ((i as f64 / 28.0 * std::f64::consts::TAU).sin() * 20000.0) as i16)
            .collect();
        let enc = encode(&tone, true);
        f[0x1000..0x1000 + enc.len()].copy_from_slice(&enc);
        f[0x80200..0x80204].copy_from_slice(&[0x44, 0xAC, 0, 0]);
        f[0x80204..0x80208].copy_from_slice(&(events.len() as u32).to_le_bytes());
        for &(tick, reg, val) in events {
            f.extend(tick.to_le_bytes());
            f.extend((0x1F80_1000 + reg).to_le_bytes());
            f.extend(val.to_le_bytes());
            f.extend([0, 0]);
        }
        f.extend([0xFF; 12]);
        f
    }

    #[test]
    fn finds_spu_notes() {
        // Voice 0 plays the tone at half speed (787.5 Hz: G5 +12 cents),
        // then twice as fast as that, an octave up.
        let s = 44_100;
        let spu = synth_spu(&[
            (0, 0xC00, 0x3FFF),
            (0, 0xC02, 0x3FFF),
            (0, 0xC04, 0x0800),
            (0, 0xC06, 0x1000 / 8),
            (0, 0xD88, 1),
            (s / 2, 0xD8C, 1),
            (s, 0xC04, 0x1000),
            (s, 0xD88, 1),
            (s * 3 / 2, 0xD8C, 1),
        ]);
        let t = crate::notes::transcribe_bytes("t.spu", &spu, None, 2500);
        assert!(
            (t.samples[0].hz.unwrap() - 1575.0).abs() < 5.0,
            "{:?}",
            t.samples[0].hz
        );
        let v = &t.parts[0];
        let got: Vec<_> = v
            .notes
            .iter()
            .map(|n| (n.key, (n.start_ms / 10.0).round() as i64))
            .collect();
        assert_eq!(got, vec![(79, 0), (91, 100)]);
        assert!((v.notes[0].dur_ms - 500.0).abs() < 2.0);
    }
}
