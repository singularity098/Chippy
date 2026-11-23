//! The sound chips NES cartridges added (NSF expansion audio): Konami's
//! VRC6 and VRC7, the Famicom Disk System's wavetable, Namco's 163 and
//! Sunsoft's 5B, read from the writes libgme's hook passes on by address.
//! (The MMC5's pulses work like the 2A03's, so [`super::nes`] reads them.)

use super::chips::{part, sound, velocity, Ay, Chip, Edge, Opll};
use super::{Part, Voice};

const CPU_HZ: f64 = 1_789_773.0;

/// The header's expansion bits.
pub const VRC6: u8 = 1;
pub const VRC7: u8 = 2;
pub const FDS: u8 = 4;
pub const MMC5: u8 = 8;
pub const N163: u8 = 16;
pub const S5B: u8 = 32;

/// Which cartridge chips an NSF or NSFE says it uses.
pub fn flags(data: &[u8]) -> u8 {
    if data.starts_with(b"NESM\x1a") {
        return data.get(0x7B).copied().unwrap_or(0);
    }
    if data.starts_with(b"NSFE") {
        // Chunks: length (LE), id, data. INFO's 8th byte is the chips.
        let mut pos = 4;
        while pos + 8 <= data.len() {
            let len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize;
            if &data[pos + 4..pos + 8] == b"INFO" {
                return data.get(pos + 8 + 7).copied().unwrap_or(0);
            }
            pos += 8 + len;
        }
    }
    0
}

// ------------------------------------------------------------------ VRC6

/// Two pulse channels and a sawtooth, at $9000, $A000 and $B000.
#[derive(Default)]
struct Vrc6 {
    regs: [[u8; 3]; 3],
    on: [bool; 3],
    edge: [Edge; 3],
    voices: [Voice; 3],
}

impl Vrc6 {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        let osc = (addr.wrapping_sub(0x9000) >> 12) as usize;
        let reg = (addr & 0xFFF) as usize;
        if osc >= 3 || reg >= 3 {
            return;
        }
        self.regs[osc][reg] = data;
        let r = self.regs[osc];
        let level = if osc < 2 { r[0] & 15 } else { r[0] & 0x3F };
        // A pulse in "digitized" mode outputs its volume as it is: samples.
        let digitized = osc < 2 && r[0] & 0x80 != 0;
        let on = r[2] & 0x80 != 0 && level > 0 && !digitized;
        let attack = self.edge[osc].attack(if on { level } else { 0 }) || (on && !self.on[osc]);
        self.on[osc] = on;
        let v = &mut self.voices[osc];
        let period = r[1] as f64 + ((r[2] & 15) as u32 * 256) as f64;
        if !on {
            return v.release(t);
        }
        let steps = if osc < 2 { 16.0 } else { 14.0 };
        let max = if osc < 2 { 15 } else { 42 };
        sound(
            v,
            t,
            CPU_HZ / (steps * (period + 1.0)),
            attack,
            velocity(level, max),
        );
    }

    fn finish(self, end: f64) -> Vec<Part> {
        let [p1, p2, saw] = self.voices;
        vec![
            part("VRC6 Pulse 1".into(), 80, p1.finish(end)),
            part("VRC6 Pulse 2".into(), 80, p2.finish(end)),
            part("VRC6 Saw".into(), 81, saw.finish(end)),
        ]
    }
}

// ------------------------------------------------------------------- FDS

/// The Famicom Disk System's wavetable channel ($4080-$408A).
#[derive(Default)]
struct Fds {
    regs: [u8; 11],
    level: u8,
    edge: Edge,
    voice: Voice,
}

impl Fds {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        if !(0x4080..=0x408A).contains(&addr) {
            return;
        }
        let r = (addr - 0x4080) as usize;
        self.regs[r] = data;
        let mut attack = false;
        if r == 0 {
            if data & 0x80 != 0 {
                // Direct gain.
                self.level = (data & 0x3F).min(32);
                attack = self.edge.attack(self.level);
            } else {
                // The envelope starts again: a new note.
                self.level = 32;
                attack = true;
                self.edge.attack(0);
                self.edge.attack(32);
            }
        }
        let f = self.regs[2] as u32 | ((self.regs[3] & 15) as u32) << 8;
        let halted = self.regs[3] & 0x80 != 0 || self.regs[9] & 0x80 != 0;
        if halted || f == 0 || self.level == 0 {
            if r == 3 || r == 9 || (r == 0 && self.level == 0) {
                self.edge.attack(0);
            }
            return self.voice.release(t);
        }
        // A 64-step wave, stepped by a 16-bit accumulator.
        let hz = CPU_HZ * f as f64 / (65536.0 * 64.0);
        sound(&mut self.voice, t, hz, attack, velocity(self.level, 32));
    }

    fn finish(self, end: f64) -> Vec<Part> {
        vec![part("FDS Wave".into(), 80, self.voice.finish(end))]
    }
}

// ------------------------------------------------------------------ N163

/// Namco's 163: up to eight wavetable channels, whose registers are in the
/// chip's 128 bytes of RAM, written through an address port ($F800) and a
/// data port ($4800).
struct N163 {
    ram: [u8; 128],
    addr: u8,
    edge: [Edge; 8],
    voices: [Voice; 8],
}

impl N163 {
    fn new() -> N163 {
        N163 {
            ram: [0; 128],
            addr: 0,
            edge: Default::default(),
            voices: Default::default(),
        }
    }

    fn write(&mut self, t: f64, addr: u16, data: u8) {
        match addr {
            0xF800 => self.addr = data,
            0x4800 => {
                let i = (self.addr & 0x7F) as usize;
                self.ram[i] = data;
                if self.addr & 0x80 != 0 {
                    self.addr = 0x80 | ((self.addr + 1) & 0x7F);
                }
                if i == 0x7F {
                    (0..8).for_each(|c| self.update(t, c));
                } else if i >= 0x40 {
                    self.update(t, (i - 0x40) / 8);
                }
            }
            _ => {}
        }
    }

    fn update(&mut self, t: f64, c: usize) {
        let count = ((self.ram[0x7F] >> 4) & 7) as usize + 1;
        let b = 0x40 + c * 8;
        let r = &self.ram[b..b + 8];
        let f = r[0] as u32 | (r[2] as u32) << 8 | ((r[4] & 3) as u32) << 16;
        let len = 256 - (r[4] & 0xFC) as u32;
        let active = c >= 8 - count;
        let vol = if active && f > 0 { r[7] & 15 } else { 0 };
        let attack = self.edge[c].attack(vol);
        let v = &mut self.voices[c];
        if vol == 0 {
            return v.release(t);
        }
        // The active channels take turns, 15 CPU clocks each.
        let hz = f as f64 * CPU_HZ / (15.0 * 65536.0 * count as f64 * len as f64);
        sound(v, t, hz, attack, velocity(vol, 15));
    }

    fn finish(self, end: f64) -> Vec<Part> {
        self.voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| part(format!("N163 {}", i + 1), 80, v.finish(end)))
            .collect()
    }
}

// ------------------------------------------------------------- the lot

/// The cartridge chips a tune says it uses.
pub struct Expansion {
    vrc6: Option<Vrc6>,
    vrc7: Option<(u8, Box<Opll>)>,
    fds: Option<Fds>,
    n163: Option<N163>,
    s5b: Option<(u8, Box<Ay>)>,
}

impl Expansion {
    pub fn new(flags: u8) -> Expansion {
        Expansion {
            vrc6: (flags & VRC6 != 0).then(Vrc6::default),
            vrc7: (flags & VRC7 != 0).then(|| (0, Box::new(Opll::vrc7()))),
            fds: (flags & FDS != 0).then(Fds::default),
            n163: (flags & N163 != 0).then(N163::new),
            // An AY: half the CPU's clock, so CPU / (32 * period).
            s5b: (flags & S5B != 0).then(|| (0, Box::new(Ay::new("5B", CPU_HZ / 2.0)))),
        }
    }

    pub fn write(&mut self, t: f64, addr: u16, data: u8) {
        if let Some(c) = &mut self.fds {
            if (0x4040..=0x4092).contains(&addr) {
                return c.write(t, addr, data);
            }
        }
        if let Some(c) = &mut self.n163 {
            if addr == 0x4800 || addr == 0xF800 {
                return c.write(t, addr, data);
            }
        }
        if let Some((latch, c)) = &mut self.s5b {
            if addr >= 0xC000 {
                match addr & 0xE000 {
                    0xC000 => *latch = data & 15,
                    _ => c.write(t, *latch as u16, data),
                }
                return;
            }
        }
        if let Some(c) = &mut self.vrc6 {
            if (0x9000..=0xB002).contains(&addr) && addr & 0xFFF < 3 {
                return c.write(t, addr, data);
            }
        }
        if let Some((latch, c)) = &mut self.vrc7 {
            match addr {
                0x9010 => *latch = data,
                0x9028..=0x9030 => c.write(t, *latch as u16, data),
                _ => {}
            }
        }
    }

    pub fn finish(self, end: f64) -> Vec<Part> {
        let mut parts = Vec::new();
        if let Some(c) = self.vrc6 {
            parts.extend(c.finish(end));
        }
        if let Some((_, c)) = self.vrc7 {
            parts.extend(c.finish(end));
        }
        if let Some(c) = self.fds {
            parts.extend(c.finish(end));
        }
        if let Some(c) = self.n163 {
            parts.extend(c.finish(end));
        }
        if let Some((_, c)) = self.s5b {
            parts.extend(c.finish(end));
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(p: &Part) -> Vec<u8> {
        p.notes.iter().map(|n| n.key).collect()
    }

    #[test]
    fn vrc6_pulse_and_saw() {
        let mut x = Expansion::new(VRC6);
        // Pulse: 1789773 / (16 * 254) = 440.4 Hz.
        x.write(0.0, 0x9000, 0x0F);
        x.write(0.0, 0x9001, 253);
        x.write(0.0, 0x9002, 0x80);
        x.write(100.0, 0x9002, 0x00);
        // Saw: 1789773 / (14 * 581) = 220.0 Hz.
        x.write(0.0, 0xB000, 20);
        x.write(0.0, 0xB001, (580 & 0xFF) as u8);
        x.write(0.0, 0xB002, 0x80 | (580 >> 8) as u8);
        let parts = x.finish(200.0);
        assert_eq!(keys(&parts[0]), vec![69]);
        assert_eq!(parts[0].notes[0].dur_ms, 100.0);
        assert_eq!(keys(&parts[2]), vec![57]);
    }

    #[test]
    fn n163_channels() {
        let mut x = Expansion::new(N163);
        let mut w = |t: f64, reg: u8, v: u8| {
            x.write(t, 0xF800, reg);
            x.write(t, 0x4800, v);
        };
        // One channel (7), a 32-sample wave: 440 Hz needs
        // f = 440 * 15 * 65536 * 32 / 1789773 = 7733.
        w(0.0, 0x7F, 0x0F);
        w(0.0, 0x78, (7733 & 0xFF) as u8);
        w(0.0, 0x7A, (7733 >> 8) as u8);
        w(0.0, 0x7C, (256 - 32) as u8);
        w(100.0, 0x7F, 0x00);
        let parts = x.finish(200.0);
        assert_eq!(keys(&parts[7]), vec![69]);
    }

    #[test]
    fn fds_and_5b_and_vrc7() {
        let mut x = Expansion::new(FDS | S5B | VRC7);
        // FDS: f = 440 * 4194304 / 1789773 = 1031.
        x.write(0.0, 0x4080, 0x80 | 32);
        x.write(0.0, 0x4082, (1031 & 0xFF) as u8);
        x.write(0.0, 0x4083, (1031 >> 8) as u8);
        x.write(100.0, 0x4083, 0x80);
        // 5B: 1789773 / (32 * 127) = 440.4 Hz.
        for (r, v) in [(0, 127), (7, 0x3E), (8, 15)] {
            x.write(0.0, 0xC000, r);
            x.write(0.0, 0xE000, v);
        }
        // VRC7: fnum 289, block 4, key on.
        for (r, v) in [(0x10, 289 & 0xFF), (0x20, 0x10 | 4 << 1 | 1)] {
            x.write(0.0, 0x9010, r as u8);
            x.write(0.0, 0x9030, v as u8);
        }
        let parts = x.finish(200.0);
        let named = |n: &str| keys(parts.iter().find(|p| p.name == n).unwrap());
        assert_eq!(named("FDS Wave"), vec![69]);
        assert_eq!(named("5B A"), vec![69]);
        assert_eq!(named("VRC7 1"), vec![69]);
    }

    #[test]
    fn reads_the_chip_flags() {
        let mut nsf = vec![0u8; 0x80];
        nsf[..5].copy_from_slice(b"NESM\x1a");
        nsf[0x7B] = VRC6 | N163;
        assert_eq!(flags(&nsf), VRC6 | N163);
        let mut nsfe = b"NSFE".to_vec();
        nsfe.extend(9u32.to_le_bytes());
        nsfe.extend(b"INFO");
        nsfe.extend([0, 0x80, 0, 0x80, 0, 0x80, 0, FDS, 1]);
        assert_eq!(flags(&nsfe), FDS);
    }
}
