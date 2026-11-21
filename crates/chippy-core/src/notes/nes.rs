//! NES (NSF, NSFE): the 2A03's two pulse channels, triangle, noise and
//! sample (DPCM) channel, read from the APU writes the tune's driver makes.
//! Drivers update the APU once per frame, so the channels' state is looked
//! at after each call of the play routine; the envelope, length and linear
//! counters the hardware runs between calls are modelled too.

use anyhow::Result;

use super::{hz_to_key, Drums, Part, Transcription, Voice};
use crate::decode::gme::{capture, ChipWrite};
use crate::vfs::Location;

const CPU_HZ: f64 = 1_789_773.0;

const LENGTHS: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];

/// Keys for DPCM samples, in order of first use (General MIDI drums).
const SAMPLE_KEYS: [u8; 10] = [36, 38, 40, 41, 43, 45, 47, 48, 50, 39];

#[derive(Default)]
struct Apu {
    regs: [u8; 0x18],
    /// Length counters: pulse 1, pulse 2, triangle, noise.
    len: [u32; 4],
    /// Envelope levels (pulse 1, pulse 2, -, noise).
    env: [f64; 4],
    linear: u32,
    linear_reload: bool,
    /// Which of the length registers ($4003, $4007, $400B, $400F) were
    /// written since the last tick (they restart the envelope).
    restarted: [bool; 4],
    last_vol: [u8; 4],
    rising: [bool; 4],
    noise_on: bool,
}

impl Apu {
    fn write(&mut self, addr: usize, data: u8) {
        if addr >= self.regs.len() {
            return;
        }
        self.regs[addr] = data;
        match addr {
            3 | 7 | 0xB | 0xF => {
                let c = addr / 4;
                if self.regs[0x15] & (1 << c) != 0 {
                    self.len[c] = LENGTHS[data as usize >> 3] as u32;
                }
                self.restarted[c] = true;
                if c == 2 {
                    self.linear_reload = true;
                } else {
                    self.env[c] = 15.0;
                }
            }
            0x15 => {
                for (c, len) in self.len.iter_mut().enumerate().take(4) {
                    if data & (1 << c) == 0 {
                        *len = 0;
                    }
                }
            }
            _ => {}
        }
    }

    fn halted(&self, c: usize) -> bool {
        let flag = if c == 2 { 0x80 } else { 0x20 };
        self.regs[c * 4] & flag != 0
    }

    /// The hardware's own counters over one frame: four quarter frames,
    /// two half frames.
    fn clock_frame(&mut self) {
        for c in [0, 1, 3] {
            let r = self.regs[c * 4];
            if r & 0x10 == 0 {
                self.env[c] -= 4.0 / ((r & 15) as f64 + 1.0);
                if self.env[c] < 0.0 {
                    self.env[c] = if self.halted(c) {
                        self.env[c] + 16.0
                    } else {
                        0.0
                    };
                }
            }
        }
        for c in 0..4 {
            if !self.halted(c) {
                self.len[c] = self.len[c].saturating_sub(2);
            }
        }
        let r = self.regs[8];
        for _ in 0..4 {
            if self.linear_reload {
                self.linear = (r & 0x7F) as u32;
            } else {
                self.linear = self.linear.saturating_sub(1);
            }
            if r & 0x80 == 0 {
                self.linear_reload = false;
            }
        }
    }

    fn volume(&self, c: usize) -> u8 {
        let r = self.regs[c * 4];
        if r & 0x10 != 0 {
            r & 15
        } else {
            self.env[c].round() as u8
        }
    }

    fn timer(&self, c: usize) -> u32 {
        self.regs[c * 4 + 2] as u32 | ((self.regs[c * 4 + 3] & 7) as u32) << 8
    }

    /// Whether channel `c`'s volume went up just now, and not as part of a
    /// rise already under way (a new note's attack, not a swell).
    fn attack_of(&mut self, c: usize, vol: u8) -> bool {
        let rising = vol > self.last_vol[c];
        let attack = rising && !self.rising[c];
        self.rising[c] = rising;
        self.last_vol[c] = vol;
        attack
    }
}

fn velocity(vol: u8) -> u8 {
    (40 + vol.min(15) as u32 * 87 / 15) as u8
}

fn noise_key(period: u8) -> u8 {
    match period & 15 {
        0..=4 => 42,
        5..=9 => 38,
        _ => 36,
    }
}

pub fn transcribe(loc: &Location, track: u32, max_ms: Option<u64>) -> Result<Transcription> {
    const RATE: u32 = 44_100;
    let mut writes: Vec<ChipWrite> = Vec::new();
    let cap = capture(loc, track, RATE, max_ms, 2048, |_, w| {
        writes.extend_from_slice(w)
    })?;
    let ticks = writes.iter().filter(|w| w.kind == 1).count().max(1);
    let played_ms = cap.frames as f64 * 1000.0 / RATE as f64;
    // The play routine's rate, as it ran (NTSC, PAL or the tune's own).
    let tick_ms = (played_ms / ticks as f64).clamp(1.0, 100.0);

    let mut apu = Apu::default();
    let mut voices: [Voice; 3] = Default::default();
    let mut drums = Drums::default();
    let mut samples: Vec<u8> = Vec::new();
    let mut tick = 0usize;
    for w in &writes {
        let t = tick as f64 * tick_ms;
        match w.kind {
            0 => {
                let addr = w.addr as usize;
                let data = w.data as u8;
                apu.write(addr, data);
                if addr == 0x15 && data & 0x10 != 0 {
                    let at = apu.regs[0x12];
                    let i = samples.iter().position(|&a| a == at).unwrap_or_else(|| {
                        samples.push(at);
                        samples.len() - 1
                    });
                    drums.hit(t, SAMPLE_KEYS[i % SAMPLE_KEYS.len()], 110);
                }
            }
            1 => {
                apu.clock_frame();
                for (c, v) in voices.iter_mut().enumerate().take(2) {
                    let vol = apu.volume(c);
                    let timer = apu.timer(c);
                    let attack = apu.attack_of(c, vol);
                    let restarted = apu.restarted[c] && apu.regs[c * 4] & 0x10 == 0;
                    if apu.len[c] > 0 && vol > 0 && timer >= 8 {
                        let pitch = hz_to_key(CPU_HZ / (16.0 * (timer + 1) as f64));
                        if !v.sounding() || attack || restarted {
                            v.attack(t, pitch, velocity(vol));
                        } else {
                            v.pitch(t, pitch);
                        }
                    } else {
                        v.release(t);
                    }
                }
                let timer = apu.timer(2);
                let v = &mut voices[2];
                if apu.len[2] > 0 && apu.linear > 0 && timer >= 2 {
                    let pitch = hz_to_key(CPU_HZ / (32.0 * (timer + 1) as f64));
                    if v.sounding() {
                        v.pitch(t, pitch);
                    } else {
                        v.attack(t, pitch, 100);
                    }
                } else {
                    v.release(t);
                }
                let vol = apu.volume(3);
                let attack = apu.attack_of(3, vol);
                let on = apu.len[3] > 0 && vol > 0;
                if on && (!apu.noise_on || attack || apu.restarted[3]) {
                    drums.hit(t, noise_key(apu.regs[0xE]), velocity(vol));
                }
                apu.noise_on = on;
                apu.restarted = [false; 4];
                tick += 1;
            }
            _ => {}
        }
    }
    let end = tick as f64 * tick_ms;
    let [p1, p2, tri] = voices;
    let mut t = Transcription {
        system: cap.system,
        parts: vec![
            part("Pulse 1", 80, p1.finish(end)),
            part("Pulse 2", 80, p2.finish(end)),
            part("Triangle", 33, tri.finish(end)),
            Part::drums("Drums (noise, samples)", drums.finish()),
        ],
        ..Default::default()
    };
    let data = loc.read()?;
    if data.starts_with(b"NESM\x1a") && data.get(0x7B).is_some_and(|&x| x != 0) {
        t.notes.push(
            "This tune also uses a cartridge sound chip (VRC6, VRC7, FDS, MMC5, N163 or \
             5B), whose channels aren't read yet."
                .into(),
        );
    }
    Ok(t)
}

fn part(name: &str, program: u8, notes: Vec<super::Note>) -> Part {
    Part::new(name, program, notes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_bundled_test_nsf() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/game-music-emu/test.nsf");
        let loc = match crate::vfs::resolve(&path).unwrap() {
            crate::vfs::Node::Tracks(l) => l,
            n => panic!("{n:?}"),
        };
        let t = super::transcribe(&loc, 0, Some(10_000)).unwrap();
        let p1 = &t.parts[0].notes;
        // Checked against the spectrum of the rendered audio.
        let first: Vec<u8> = p1.iter().take(6).map(|n| n.key).collect();
        assert_eq!(first, [69, 73, 78, 73, 68, 73]);
        assert!(p1.len() > 30);
        assert!(!t.parts[1].notes.is_empty());
    }
}
