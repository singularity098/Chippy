//! Sega Genesis / Mega Drive and Master System (VGM, VGZ, GYM): these files
//! are logs of the writes the game made to its sound chips, so they are
//! read directly, with no emulation. The YM2612's six FM channels give
//! exact notes (frequency and key-on registers); so do the SN76489's three
//! tone channels, whose notes start on a rise in volume. Its noise channel
//! becomes drums. FM channel 6 in DAC mode plays samples and is skipped.

use anyhow::{bail, Result};

use super::chips::{self, Chip};
use super::nes::Nes;
use super::nesx;
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
    fn new(clock: f64) -> Ym2612 {
        Ym2612 {
            clock,
            ..Default::default()
        }
    }

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
pub(crate) struct Sn76489 {
    clock: f64,
    period: [u16; 3],
    /// Attenuation per channel (15 is silent); the 4th is noise.
    atten: [u8; 4],
    rising: [bool; 4],
    noise: u8,
    latched: usize,
    latched_volume: bool,
    pub voices: [Voice; 3],
}

impl Sn76489 {
    pub fn new(clock: f64) -> Self {
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

    pub fn write(&mut self, t: f64, data: u8, drums: &mut Drums) {
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
    /// The other chips a VGM can log, made when first written to, by
    /// command (with bit 8 set for the second of a pair).
    others: Vec<(u16, Box<dyn Chip>)>,
}

/// The chips other than the first YM2612 and SN76489 a VGM command writes
/// to: (command, header offset of the chip's clock, default clock, maker,
/// name). The maker gets the clock and the header's whole clock field
/// (whose top bits flag variants).
type Maker = fn(f64, u32) -> Box<dyn Chip>;
const OTHERS: &[(u8, usize, f64, Maker, &str)] = &[
    // Second chips only (the first are `Chips`' own).
    (
        0x50,
        0x0C,
        3_579_545.0,
        |c, _| Box::new(chips::Psg::new(c)),
        "SN76489",
    ),
    (
        0x52,
        0x2C,
        7_670_453.0,
        |c, _| Box::new(Ym2612Chip(Ym2612::new(c))),
        "YM2612",
    ),
    (
        0x51,
        0x10,
        3_579_545.0,
        |c, _| Box::new(chips::Opll::new(c)),
        "YM2413",
    ),
    (
        0x54,
        0x30,
        3_579_545.0,
        |c, _| Box::new(chips::Opm::new(c)),
        "YM2151",
    ),
    (
        0x55,
        0x44,
        3_993_600.0,
        |c, _| Box::new(chips::Opn::ym2203(c)),
        "YM2203",
    ),
    (
        0x56,
        0x48,
        7_987_200.0,
        |c, _| Box::new(chips::Opn::ym2608(c)),
        "YM2608",
    ),
    (
        0x58,
        0x4C,
        8_000_000.0,
        |c, _| Box::new(chips::Opn::ym2610(c)),
        "YM2610",
    ),
    (
        0x5A,
        0x50,
        3_579_545.0,
        |c, _| Box::new(chips::Opl::new(c / 72.0, 1)),
        "YM3812",
    ),
    (
        0x5B,
        0x54,
        3_579_545.0,
        |c, _| Box::new(chips::Opl::new(c / 72.0, 1)),
        "YM3526",
    ),
    (
        0x5C,
        0x58,
        3_579_545.0,
        |c, _| Box::new(chips::Opl::named("Y8950", c / 72.0, 1)),
        "Y8950",
    ),
    (
        0x5E,
        0x5C,
        14_318_180.0,
        |c, _| Box::new(chips::Opl::new(c / 288.0, 2)),
        "YMF262",
    ),
    (
        0xA0,
        0x74,
        1_789_772.5,
        |c, _| Box::new(chips::Ay::new("AY", c)),
        "AY-3-8910",
    ),
    (
        0xB3,
        0x80,
        4_194_304.0,
        |_, _| Box::new(chips::Dmg::new()),
        "Game Boy",
    ),
    // Bit 31 of the clock: with the FDS.
    (
        0xB4,
        0x84,
        1_789_772.0,
        |_, raw| {
            Box::new(NesChip::new(if raw & 0x8000_0000 != 0 {
                nesx::FDS
            } else {
                0
            }))
        },
        "NES APU",
    ),
    (
        0xB9,
        0xA4,
        3_579_545.0,
        |c, _| Box::new(chips::Huc::new(c)),
        "HuC6280",
    ),
    (
        0xBB,
        0xB0,
        1_789_772.0,
        |c, _| Box::new(chips::Pokey::new("POKEY", c)),
        "POKEY",
    ),
    // VGM clocks the SCC at half the MSX's 3.58 MHz.
    (
        0xD2,
        0x9C,
        1_789_772.0,
        |c, _| Box::new(chips::Scc::new(c * 2.0)),
        "K051649",
    ),
];

/// The YM2612 as a [`Chip`] (for a second one): port 1 in bit 8.
struct Ym2612Chip(Ym2612);

impl Chip for Ym2612Chip {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        self.0.write(t, (addr >> 8) as usize & 1, addr as u8, data);
    }

    fn finish(self: Box<Self>, end: f64) -> Vec<Part> {
        self.0
            .voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| Part::new(format!("FM {}", i + 1), 81, v.finish(end)))
            .collect()
    }
}

/// The NES APU (and FDS), whose writes are timed, ticked at 60 Hz for the
/// counters [`Nes`] models a frame at a time.
struct NesChip {
    nes: Nes,
    next: f64,
}

const NES_FRAME_MS: f64 = 1000.0 / 60.0988;

impl NesChip {
    fn new(flags: u8) -> NesChip {
        NesChip {
            nes: Nes::new(flags),
            next: NES_FRAME_MS,
        }
    }

    fn run_to(&mut self, t: f64) {
        while self.next <= t {
            self.nes.tick(self.next);
            self.next += NES_FRAME_MS;
        }
    }
}

impl Chip for NesChip {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        self.run_to(t);
        self.nes.write(t, addr, data);
    }

    fn finish(mut self: Box<Self>, end: f64) -> Vec<Part> {
        self.run_to(end);
        self.nes.finish(end)
    }
}

impl Chips {
    /// A write to the chip of command `cmd` (its second, with `second`).
    fn other(&mut self, d: &[u8], cmd: u8, second: bool, t: f64, addr: u16, data: u8) {
        let key = cmd as u16 | if second { 0x100 } else { 0 };
        let i = match self.others.iter().position(|(c, _)| *c == key) {
            Some(i) => i,
            None => {
                let Some(&(_, at, default, make, _)) = OTHERS.iter().find(|o| o.0 == cmd) else {
                    return;
                };
                // Clocks past the end of an older, shorter header read as 0.
                let header_end = if u32_at(d, 8) >= 0x150 && u32_at(d, 0x34) != 0 {
                    0x34 + u32_at(d, 0x34) as usize
                } else {
                    0x40
                };
                let raw = if at + 4 <= header_end {
                    u32_at(d, at)
                } else {
                    0
                };
                let clock = (raw & 0x3FFF_FFFF) as f64;
                self.others
                    .push((key, make(if clock > 0.0 { clock } else { default }, raw)));
                self.others.len() - 1
            }
        };
        self.others[i].1.write(t, addr, data);
    }
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
            // The second SN76489.
            0x30 => {
                chips.other(d, 0x50, true, t, 0, arg(1));
                2
            }
            0x52 | 0x53 => {
                chips.ym.write(t, (op - 0x52) as usize, arg(1), arg(2));
                3
            }
            // A chip of one register bank, or two (ports 0 and 1: two
            // commands); the second chip of a pair by command $A1-$AF.
            0x51..=0x5F | 0xA1..=0xAF => {
                let second = op >= 0xA0;
                let base = op & 0x5F | 0x50;
                let (cmd, port) = match base {
                    0x52 | 0x53 => (0x52, base - 0x52),
                    0x56 | 0x57 => (0x56, base - 0x56),
                    0x58 | 0x59 => (0x58, base - 0x58),
                    0x5E | 0x5F => (0x5E, base - 0x5E),
                    b => (b, 0),
                };
                if second || cmd != 0x52 {
                    chips.other(
                        d,
                        cmd,
                        second,
                        t,
                        (port as u16) << 8 | arg(1) as u16,
                        arg(2),
                    );
                }
                3
            }
            // Register, data: bit 7 of the register picks the second chip.
            0xA0 | 0xB3 | 0xB9 | 0xBB => {
                let reg = arg(1);
                chips.other(d, op, reg & 0x80 != 0, t, (reg & 0x7F) as u16, arg(2));
                3
            }
            // NES APU: $4000-$401F, then the FDS's $4080-$409E, and $4023.
            0xB4 => {
                let reg = arg(1) & 0x7F;
                let addr = match reg {
                    0x00..=0x1F => 0x4000 + reg as u16,
                    0x20..=0x3E => 0x4080 + (reg - 0x20) as u16,
                    _ => 0x4023,
                };
                chips.other(d, op, arg(1) & 0x80 != 0, t, addr, arg(2));
                3
            }
            // K051649 (SCC): port (wave, period, volume, enable), register.
            0xD2 => {
                let (port, reg) = (arg(1) & 0x7F, arg(2) as u16);
                let addr = match port {
                    0 => Some(reg & 0x7F),
                    1 => Some(0x80 + reg.min(9)),
                    2 => Some(0x8A + reg.min(4)),
                    3 => Some(0x8F),
                    _ => None,
                };
                if let Some(a) = addr {
                    chips.other(d, op, arg(1) & 0x80 != 0, t, a, arg(3));
                }
                4
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
            0x31..=0x3F | 0x4F => 2,
            0x40..=0x4E | 0xB0..=0xB2 | 0xB5..=0xB8 | 0xBA | 0xBC..=0xBF => 3,
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
        others: Vec::new(),
    };
    let max = max_ms.map_or(f64::INFINITY, |m| m as f64);
    let gym = crate::formats::extension(&loc.name()) == "gym";
    let end = if gym {
        run_gym(&data, &mut chips, max)?
    } else {
        run_vgm(&data, &mut chips, max)?
    };
    let Chips {
        ym,
        psg,
        drums,
        others,
    } = chips;
    let mut parts = Vec::new();
    for (i, v) in ym.voices.into_iter().enumerate() {
        parts.push(Part::new(format!("FM {}", i + 1), 81, v.finish(end)));
    }
    let fm_used = parts.iter().any(|p| !p.notes.is_empty());
    for (i, v) in psg.voices.into_iter().enumerate() {
        parts.push(Part::new(format!("PSG {}", i + 1), 80, v.finish(end)));
    }
    parts.push(Part::drums("Drums (PSG noise)", drums.finish()));
    let sega = parts.iter().any(|p| !p.notes.is_empty());
    let mut names: Vec<&str> = Vec::new();
    for (key, c) in others {
        let name = OTHERS
            .iter()
            .find(|o| o.0 as u16 == key & 0xFF)
            .map_or("?", |o| o.4);
        if !names.contains(&name) {
            names.push(name);
        }
        let mut more = c.finish(end);
        if key & 0x100 != 0 {
            for p in &mut more {
                p.name += " #2";
            }
        }
        parts.extend(more);
    }
    let mut t = Transcription {
        system: if !names.is_empty() && !sega {
            format!("VGM ({})", names.join(", "))
        } else if fm_used || !names.is_empty() {
            "Sega Genesis / Mega Drive".into()
        } else {
            "Sega".into()
        },
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
            others: Vec::new(),
        }
    }

    #[test]
    fn reads_other_chips() {
        // A YM2151 note: A4 for 22050 samples.
        let d = vgm(&[
            0x54, 0x28, 0x4A, 0x54, 0x08, 0x78, 0x61, 0x22, 0x56, 0x54, 0x08, 0x00,
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.vgm");
        std::fs::write(&path, d).unwrap();
        let t = transcribe(&crate::vfs::Location::Fs(path), None).unwrap();
        assert_eq!(t.system, "VGM (YM2151)");
        let notes = &t.parts.iter().find(|p| !p.notes.is_empty()).unwrap().notes;
        assert_eq!((notes[0].key, notes[0].dur_ms.round()), (69, 500.0));
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

    fn run(cmds: &[u8]) -> Transcription {
        crate::notes::transcribe_bytes("t.vgm", &vgm(cmds), None, 10_000)
    }

    fn keys(t: &Transcription, name: &str) -> Vec<u8> {
        crate::notes::part(t, name)
            .notes
            .iter()
            .map(|n| n.key)
            .collect()
    }

    #[test]
    fn reads_the_ym2610_fm_ssg_and_samples() {
        let t = run(&[
            // FM channel 2: block 4, fnum 1038 (440 Hz at 8 MHz / 144).
            0x58, 0xA5, 0x24, 0x58, 0xA1, 0x0E, 0x58, 0x28, 0xF1,
            // SSG A: period 142 at 1 MHz.
            0x58, 0x00, 142, 0x58, 0x07, 0x3E, 0x58, 0x08, 15,
            // An ADPCM-A sample on channel 1.
            0x59, 0x10, 5, 0x59, 0x18, 0, 0x59, 0x08, 0x1F, 0x59, 0x00, 0x01, 0x61, 0x22,
            0x56, // 500 ms
            0x58, 0x28, 0x01, 0x58, 0x08, 0,
        ]);
        assert_eq!(t.system, "VGM (YM2610)");
        assert_eq!(keys(&t, "FM 2"), vec![69]);
        assert_eq!(keys(&t, "SSG A"), vec![69]);
        assert_eq!(crate::notes::part(&t, "Drums (ADPCM-A)").notes.len(), 1);
    }

    #[test]
    fn reads_second_chips() {
        let t = run(&[
            0x54, 0x28, 0x4A, 0x54, 0x08, 0x78, // YM2151 1: A4
            0xA4, 0x28, 0x4E, 0xA4, 0x08, 0x78, // YM2151 2: C5
            0x61, 0x22, 0x56,
        ]);
        assert_eq!(keys(&t, "FM 1"), vec![69]);
        assert_eq!(keys(&t, "FM 1 #2"), vec![72]);
    }

    #[test]
    fn reads_the_nes_apu_and_scc() {
        let t = run(&[
            // NES pulse 1: timer 253 (440 Hz), constant volume 15.
            0xB4, 0x15, 0x01, 0xB4, 0x00, 0x3F, 0xB4, 0x02, 253, 0xB4, 0x03, 0x08,
            // SCC channel 1: period 253.
            0xD2, 1, 0, 253, 0xD2, 1, 1, 0, 0xD2, 2, 0, 15, 0xD2, 3, 0, 1, 0x61, 0x22, 0x56, 0xB4,
            0x15, 0x00, 0xD2, 3, 0, 0, 0x61, 0x22, 0x56,
        ]);
        let pulse = crate::notes::part(&t, "Pulse 1");
        assert_eq!(
            pulse.notes.iter().map(|n| n.key).collect::<Vec<_>>(),
            vec![69]
        );
        // Seen at the first 60 Hz tick, ended at the one after the write.
        assert!(
            (pulse.notes[0].dur_ms - 500.0).abs() < 17.0,
            "{:?}",
            pulse.notes
        );
        let scc = crate::notes::part(&t, "SCC 1");
        assert_eq!(
            scc.notes.iter().map(|n| n.key).collect::<Vec<_>>(),
            vec![69]
        );
        assert_eq!(scc.notes[0].dur_ms.round(), 500.0);
    }
}
