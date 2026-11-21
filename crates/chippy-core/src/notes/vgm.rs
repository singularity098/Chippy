//! Sega Genesis / Mega Drive and Master System (VGM, VGZ, GYM): these files
//! are logs of the writes the game made to its sound chips, so they are
//! read directly, with no emulation. The YM2612's six FM channels give
//! exact notes (frequency and key-on registers); so do the SN76489's three
//! tone channels, whose notes start on a rise in volume. Its noise channel
//! becomes drums. FM channel 6 in DAC mode plays samples and is skipped.

use anyhow::{bail, Result};

use super::{hz_to_key, Drums, Part, Transcription, Voice};
use crate::vfs::Location;

const VGM_RATE: f64 = 44_100.0;

#[derive(Default)]
struct Ym2612 {
    clock: f64,
    /// Frequency number and block per channel.
    fnum: [u16; 6],
    block: [u8; 6],
    /// The high byte ($A4-$A6) waiting for its low byte to take effect.
    latch: [u8; 2],
    /// Operators keyed on, per channel.
    keyed: [u8; 6],
    dac: bool,
    voices: [Voice; 6],
}

impl Ym2612 {
    fn pitch(&self, ch: usize) -> Option<f64> {
        let f = self.fnum[ch] as f64;
        if f == 0.0 {
            return None;
        }
        let hz = f * self.clock / (144.0 * (1u32 << (21 - self.block[ch] as u32)) as f64);
        Some(hz_to_key(hz))
    }

    fn write(&mut self, t: f64, port: usize, reg: u8, data: u8) {
        match (port, reg) {
            (0, 0x28) => {
                let c = (data & 3) as usize;
                if c == 3 {
                    return;
                }
                let ch = c + if data & 4 != 0 { 3 } else { 0 };
                let ops = data >> 4;
                let was = self.keyed[ch];
                self.keyed[ch] = ops;
                if ch == 5 && self.dac {
                    return;
                }
                if ops == 0 {
                    self.voices[ch].release(t);
                } else if was == 0 {
                    match self.pitch(ch) {
                        Some(p) => self.voices[ch].attack(t, p, 100),
                        None => self.voices[ch].release(t),
                    }
                }
            }
            (0, 0x2B) => {
                self.dac = data & 0x80 != 0;
                if self.dac {
                    self.voices[5].release(t);
                }
            }
            (_, 0xA4..=0xA6) => self.latch[port] = data,
            (_, 0xA0..=0xA2) => {
                let ch = port * 3 + (reg - 0xA0) as usize;
                let hi = self.latch[port];
                self.fnum[ch] = ((hi as u16 & 7) << 8) | data as u16;
                self.block[ch] = (hi >> 3) & 7;
                if self.keyed[ch] != 0 && !(ch == 5 && self.dac) {
                    if let Some(p) = self.pitch(ch) {
                        self.voices[ch].pitch(t, p);
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct Sn76489 {
    clock: f64,
    period: [u16; 3],
    /// Attenuation per channel (15 is silent); the 4th is noise.
    atten: [u8; 4],
    rising: [bool; 4],
    noise: u8,
    latched: usize,
    latched_volume: bool,
    voices: [Voice; 3],
}

impl Sn76489 {
    fn new(clock: f64) -> Self {
        Sn76489 {
            clock,
            atten: [15; 4],
            ..Default::default()
        }
    }

    fn pitch(&self, ch: usize) -> Option<f64> {
        let n = self.period[ch];
        // Periods this short are above hearing: drivers use them to mute.
        (n >= 4).then(|| hz_to_key(self.clock / (32.0 * n as f64)))
    }

    fn volume_of(atten: u8) -> u8 {
        (40 + (15 - atten.min(15)) as u32 * 87 / 15) as u8
    }

    fn write(&mut self, t: f64, data: u8, drums: &mut Drums) {
        let ch;
        if data & 0x80 != 0 {
            self.latched = ((data >> 5) & 3) as usize;
            self.latched_volume = data & 0x10 != 0;
            ch = self.latched;
            if self.latched_volume {
                return self.volume(t, ch, data & 15, drums);
            }
            if ch == 3 {
                self.noise = data & 7;
                return;
            }
            self.period[ch] = (self.period[ch] & 0x3F0) | (data & 15) as u16;
        } else {
            ch = self.latched;
            if self.latched_volume {
                return self.volume(t, ch, data & 15, drums);
            }
            if ch == 3 {
                self.noise = data & 7;
                return;
            }
            self.period[ch] = (self.period[ch] & 15) | ((data & 0x3F) as u16) << 4;
        }
        if self.atten[ch] < 15 {
            match self.pitch(ch) {
                Some(p) if self.voices[ch].sounding() => self.voices[ch].pitch(t, p),
                Some(p) => self.voices[ch].attack(t, p, Self::volume_of(self.atten[ch])),
                None => self.voices[ch].release(t),
            }
        }
    }

    fn volume(&mut self, t: f64, ch: usize, atten: u8, drums: &mut Drums) {
        let rising = atten < self.atten[ch];
        let attack = rising && !self.rising[ch];
        self.rising[ch] = rising;
        self.atten[ch] = atten;
        let vel = Self::volume_of(atten);
        if ch == 3 {
            if attack {
                drums.hit(
                    t,
                    if self.noise & 3 == 3 {
                        36
                    } else if self.noise & 4 != 0 {
                        38
                    } else {
                        42
                    },
                    vel,
                );
            }
            return;
        }
        let pitch = self.pitch(ch);
        let v = &mut self.voices[ch];
        if atten >= 15 {
            v.release(t);
        } else if let Some(p) = pitch {
            if attack || !v.sounding() {
                v.attack(t, p, vel);
            } else {
                v.velocity(vel);
            }
        }
    }
}

struct Chips {
    ym: Ym2612,
    psg: Sn76489,
    drums: Drums,
}

fn u32_at(d: &[u8], at: usize) -> u32 {
    d.get(at..at + 4)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Runs a VGM's commands until its end (without looping) or `max_ms`;
/// returns the time it ended at.
fn run_vgm(d: &[u8], chips: &mut Chips, max_ms: f64) -> Result<f64> {
    if !d.starts_with(b"Vgm ") {
        bail!("not a VGM file");
    }
    let version = u32_at(d, 8);
    let psg_clock = u32_at(d, 0x0C) & 0x3FFF_FFFF;
    let ym_clock = if version >= 0x110 {
        u32_at(d, 0x2C)
    } else {
        u32_at(d, 0x10)
    } & 0x3FFF_FFFF;
    chips.ym.clock = if ym_clock > 0 {
        ym_clock as f64
    } else {
        7_670_453.0
    };
    chips.psg = Sn76489::new(if psg_clock > 0 {
        psg_clock as f64
    } else {
        3_579_545.0
    });
    let mut pos = if version >= 0x150 && u32_at(d, 0x34) != 0 {
        0x34 + u32_at(d, 0x34) as usize
    } else {
        0x40
    };
    let mut samples = 0u64;
    let ms = |s: u64| s as f64 * 1000.0 / VGM_RATE;
    while pos < d.len() && ms(samples) < max_ms {
        let t = ms(samples);
        let op = d[pos];
        let arg = |i: usize| d.get(pos + i).copied().unwrap_or(0);
        let len = match op {
            0x50 => {
                chips.psg.write(t, arg(1), &mut chips.drums);
                2
            }
            0x52 | 0x53 => {
                chips.ym.write(t, (op - 0x52) as usize, arg(1), arg(2));
                3
            }
            0x61 => {
                samples += u16::from_le_bytes([arg(1), arg(2)]) as u64;
                3
            }
            0x62 => {
                samples += 735;
                1
            }
            0x63 => {
                samples += 882;
                1
            }
            0x66 => break,
            0x67 => 7 + (u32_at(d, pos + 3) & 0x7FFF_FFFF) as usize,
            0x68 => 12,
            0x70..=0x7F => {
                samples += (op & 15) as u64 + 1;
                1
            }
            0x80..=0x8F => {
                samples += (op & 15) as u64;
                1
            }
            0x90 | 0x91 | 0x95 => 5,
            0x92 => 6,
            0x93 => 11,
            0x94 => 2,
            0x30..=0x3F | 0x4F => 2,
            0x40..=0x4E | 0x51 | 0x54..=0x5F | 0xA0..=0xBF => 3,
            0xC0..=0xDF => 4,
            0xE0..=0xFF => 5,
            _ => 1,
        };
        pos += len;
    }
    Ok(ms(samples))
}

/// GYM: YM2612 and PSG writes, with a wait for each 60 Hz frame.
fn run_gym(d: &[u8], chips: &mut Chips, max_ms: f64) -> Result<f64> {
    let mut pos = 0;
    if d.starts_with(b"GYMX") {
        // The header's last fields: loop start, and the unpacked size of a
        // packed file.
        if u32_at(d, 0x1A8) != 0 {
            bail!("packed GYM files aren't supported");
        }
        pos = 0x1AC;
    }
    chips.ym.clock = 7_670_453.0;
    chips.psg = Sn76489::new(3_579_545.0);
    let mut frames = 0u64;
    let ms = |f: u64| f as f64 * 1000.0 / 60.0;
    while pos < d.len() && ms(frames) < max_ms {
        let t = ms(frames);
        let arg = |i: usize| d.get(pos + i).copied().unwrap_or(0);
        pos += match d[pos] {
            0 => {
                frames += 1;
                1
            }
            1 | 2 => {
                chips.ym.write(t, (d[pos] - 1) as usize, arg(1), arg(2));
                3
            }
            3 => {
                chips.psg.write(t, arg(1), &mut chips.drums);
                2
            }
            _ => 1,
        };
    }
    Ok(ms(frames))
}

pub fn transcribe(loc: &Location, max_ms: Option<u64>) -> Result<Transcription> {
    let data = crate::decode::gme::load(loc)?;
    let mut chips = Chips {
        ym: Ym2612::default(),
        psg: Sn76489::new(3_579_545.0),
        drums: Drums::default(),
    };
    let max = max_ms.map_or(f64::INFINITY, |m| m as f64);
    let gym = crate::formats::extension(&loc.name()) == "gym";
    let end = if gym {
        run_gym(&data, &mut chips, max)?
    } else {
        run_vgm(&data, &mut chips, max)?
    };
    let Chips { ym, psg, drums } = chips;
    let fm_used = ym.clock > 0.0;
    let mut parts = Vec::new();
    for (i, v) in ym.voices.into_iter().enumerate() {
        parts.push(Part {
            name: format!("FM {}", i + 1),
            drums: false,
            program: 81,
            notes: v.finish(end),
        });
    }
    for (i, v) in psg.voices.into_iter().enumerate() {
        parts.push(Part {
            name: format!("PSG {}", i + 1),
            drums: false,
            program: 80,
            notes: v.finish(end),
        });
    }
    parts.push(Part {
        name: "Drums (PSG noise)".into(),
        drums: true,
        program: 0,
        notes: drums.finish(),
    });
    let mut t = Transcription {
        system: if fm_used {
            "Sega Genesis / Mega Drive"
        } else {
            "Sega"
        }
        .into(),
        parts,
        ..Default::default()
    };
    if ym.dac {
        t.notes
            .push("FM 6 plays samples (DAC) at times: those aren't read as notes.".into());
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vgm(cmds: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x40];
        d[..4].copy_from_slice(b"Vgm ");
        d[8..12].copy_from_slice(&0x150u32.to_le_bytes());
        d[0x0C..0x10].copy_from_slice(&3_579_545u32.to_le_bytes());
        d[0x2C..0x30].copy_from_slice(&7_670_453u32.to_le_bytes());
        d[0x34..0x38].copy_from_slice(&0x0Cu32.to_le_bytes());
        d.extend_from_slice(cmds);
        d.push(0x66);
        d
    }

    fn chips() -> Chips {
        Chips {
            ym: Ym2612::default(),
            psg: Sn76489::new(3_579_545.0),
            drums: Drums::default(),
        }
    }

    #[test]
    fn reads_fm_notes() {
        // Channel 1: A4 (block 4, fnum 1084), keyed on for 22050 samples
        // (500 ms), then C5 (fnum 1289) legato-free after a key-off.
        let d = vgm(&[
            0x52, 0xA4, 0x24, 0x52, 0xA0, 0x3C, // fnum 0x43C = 1084, block 4
            0x52, 0x28, 0xF0, // key on, all operators
            0x61, 0x22, 0x56, // wait 22050
            0x52, 0x28, 0x00, // key off
            0x52, 0xA4, 0x25, 0x52, 0xA0, 0x09, // fnum 0x509 = 1289
            0x52, 0x28, 0xF0, 0x62, 0x52, 0x28, 0x00,
        ]);
        let mut c = chips();
        let end = run_vgm(&d, &mut c, f64::INFINITY).unwrap();
        let notes = std::mem::take(&mut c.ym.voices[0]).finish(end);
        let got: Vec<_> = notes.iter().map(|n| (n.key, n.start_ms.round())).collect();
        assert_eq!(got, vec![(69, 0.0), (72, 500.0)]);
        assert!((notes[1].dur_ms - 735.0 * 1000.0 / 44100.0).abs() < 0.01);
    }

    #[test]
    fn reads_psg_notes() {
        // Tone 0 at period 254 (440.4 Hz), volume up, then down, then up.
        let d = vgm(&[
            0x50, 0x8E, 0x50, 0x0F, // period 0xFE
            0x50, 0x90, 0x62, // volume 0 (loudest)
            0x50, 0x94, 0x62, // softer: same note
            0x50, 0x90, 0x62, // a new attack
            0x50, 0x9F, 0x62, // silent
        ]);
        let mut c = chips();
        let end = run_vgm(&d, &mut c, f64::INFINITY).unwrap();
        let notes = std::mem::take(&mut c.psg.voices[0]).finish(end);
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().all(|n| n.key == 69));
    }
}
