//! NES (NSF, NSFE): the 2A03's two pulse channels, triangle, noise and
//! sample (DPCM) channel, read from the APU writes the tune's driver makes.
//! Drivers update the APU once per frame, so the channels' state is looked
//! at after each call of the play routine; the envelope, length and linear
//! counters the hardware runs between calls are modelled too.
//!
//! Cartridge sound chips are read too: the MMC5's two pulses here (they are
//! the 2A03's), the others in [`super::nesx`].

use anyhow::Result;

use super::nesx::{self, Expansion};
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
    /// The volume has gone down since the note started: a rise now is a
    /// new note, not the note's own envelope swelling (drivers often start
    /// a note soft for a few frames, then raise it).
    fell: [bool; 4],
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
        if vol < self.last_vol[c] {
            self.fell[c] = true;
        }
        let attack = rising && !self.rising[c] && self.fell[c];
        self.rising[c] = rising;
        self.last_vol[c] = vol;
        attack
    }

    /// A new note started on channel `c`.
    fn started(&mut self, c: usize) {
        self.fell[c] = false;
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

/// The 2A03 and any cartridge chips, turned into notes from their writes
/// and the driver's ticks (when it would update them, once a frame).
pub(crate) struct Nes {
    flags: u8,
    apu: Apu,
    voices: [Voice; 3],
    drums: Drums,
    /// DPCM sample addresses, in order of first use.
    samples: Vec<u8>,
    mmc5: Apu,
    mmc5_voices: [Voice; 2],
    expansion: Expansion,
}

impl Nes {
    /// With the cartridge chips in `flags` ([`nesx`]'s bits).
    pub fn new(flags: u8) -> Nes {
        Nes {
            flags,
            apu: Apu::default(),
            voices: Default::default(),
            drums: Drums::default(),
            samples: Vec::new(),
            mmc5: Apu::default(),
            mmc5_voices: Default::default(),
            expansion: Expansion::new(flags),
        }
    }

    /// A write to `addr` (a CPU address: $4000-$4017 for the 2A03).
    pub fn write(&mut self, t: f64, addr: u16, data: u8) {
        let mmc5 = self.flags & nesx::MMC5 != 0;
        match addr {
            0x4000..=0x4017 => {
                let a = (addr - 0x4000) as usize;
                self.apu.write(a, data);
                if a == 0x15 && data & 0x10 != 0 {
                    let at = self.apu.regs[0x12];
                    let samples = &mut self.samples;
                    let i = samples.iter().position(|&a| a == at).unwrap_or_else(|| {
                        samples.push(at);
                        samples.len() - 1
                    });
                    self.drums.hit(t, SAMPLE_KEYS[i % SAMPLE_KEYS.len()], 110);
                }
            }
            0x5000..=0x5007 if mmc5 => self.mmc5.write((addr - 0x5000) as usize, data),
            0x5015 if mmc5 => self.mmc5.write(0x15, data & 3),
            _ => self.expansion.write(t, addr, data),
        }
    }

    /// A frame's end: the channels' state as the driver left it.
    pub fn tick(&mut self, t: f64) {
        let apu = &mut self.apu;
        apu.clock_frame();
        pulses(apu, &mut self.voices[..2], t);
        if self.flags & nesx::MMC5 != 0 {
            self.mmc5.clock_frame();
            pulses(&mut self.mmc5, &mut self.mmc5_voices, t);
            self.mmc5.restarted = [false; 4];
        }
        let timer = apu.timer(2);
        let v = &mut self.voices[2];
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
            self.drums.hit(t, noise_key(apu.regs[0xE]), velocity(vol));
            apu.started(3);
        }
        apu.noise_on = on;
        apu.restarted = [false; 4];
    }

    pub fn finish(self, end: f64) -> Vec<Part> {
        let [p1, p2, tri] = self.voices;
        let mut parts = vec![
            part("Pulse 1", 80, p1.finish(end)),
            part("Pulse 2", 80, p2.finish(end)),
            part("Triangle", 33, tri.finish(end)),
        ];
        if self.flags & nesx::MMC5 != 0 {
            let [m1, m2] = self.mmc5_voices;
            parts.push(part("MMC5 Pulse 1", 80, m1.finish(end)));
            parts.push(part("MMC5 Pulse 2", 80, m2.finish(end)));
        }
        parts.extend(self.expansion.finish(end));
        parts.push(Part::drums("Drums (noise, samples)", self.drums.finish()));
        parts
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

    let flags = nesx::flags(&loc.read()?);
    let mut nes = Nes::new(flags);
    let mut tick = 0usize;
    for w in &writes {
        let t = tick as f64 * tick_ms;
        match w.kind {
            0 => nes.write(t, 0x4000 + w.addr as u16, w.data as u8),
            3 => nes.write(t, w.addr as u16, w.data as u8),
            1 => {
                nes.tick(t);
                tick += 1;
            }
            _ => {}
        }
    }
    let end = tick as f64 * tick_ms;
    let mut t = Transcription {
        system: cap.system,
        parts: nes.finish(end),
        ..Default::default()
    };
    if flags & nesx::FDS != 0 {
        t.notes
            .push("The FDS channel's pitch modulation (its vibrato) isn't followed.".into());
    }
    Ok(t)
}

/// The pulse channels' state after a tick of the play routine.
fn pulses(apu: &mut Apu, voices: &mut [Voice], t: f64) {
    for (c, v) in voices.iter_mut().enumerate().take(2) {
        let vol = apu.volume(c);
        let timer = apu.timer(c);
        let attack = apu.attack_of(c, vol);
        let restarted = apu.restarted[c] && apu.regs[c * 4] & 0x10 == 0;
        if apu.len[c] > 0 && vol > 0 && timer >= 8 {
            let pitch = hz_to_key(CPU_HZ / (16.0 * (timer + 1) as f64));
            if !v.sounding() || attack || restarted {
                v.attack(t, pitch, velocity(vol));
                apu.started(c);
            } else {
                v.pitch(t, pitch);
                // Swelling: as loud as it gets.
                v.velocity(velocity(vol));
            }
        } else {
            v.release(t);
        }
    }
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

    /// An NSF whose play routine (60 Hz) writes each frame's (address,
    /// value) pairs, using the cartridge chips `chips`.
    fn synth_nsf(chips: u8, frames: &[&[(u16, u8)]]) -> Vec<u8> {
        #[rustfmt::skip]
        let play = [
            0xA2, 0x00, 0xA0, 0x00,       // LDX #0; LDY #0
            0xB1, 0x00, 0x85, 0x02, 0xC8, // loop: LDA ($00),Y; STA $02; INY
            0xB1, 0x00, 0xF0, 0x0B,       // LDA ($00),Y; BEQ done
            0x85, 0x03, 0xC8, 0xB1, 0x00, // STA $03; INY; LDA ($00),Y
            0xC8, 0x81, 0x02,             // INY; STA ($02,X)
            0x4C, 0x04, 0x80,             // JMP loop
            0xC8, 0x98, 0x18, 0x65, 0x00, // done: INY; TYA; CLC; ADC $00
            0x85, 0x00, 0x90, 0x02,       // STA $00; BCC +2
            0xE6, 0x01, 0x60,             // INC $01; RTS
        ];
        let mut rom = vec![0u8; 0x100];
        rom[..play.len()].copy_from_slice(&play);
        // init: point $00 at the frames.
        rom[0x30..0x39].copy_from_slice(&[0xA9, 0x00, 0x85, 0x00, 0xA9, 0x81, 0x85, 0x01, 0x60]);
        for f in frames {
            for &(a, v) in *f {
                rom.extend([a as u8, (a >> 8) as u8, v]);
            }
            rom.extend([0, 0]);
        }
        rom.extend(std::iter::repeat_n(0, 2000));
        let mut h = vec![0u8; 0x80];
        h[..5].copy_from_slice(b"NESM\x1a");
        h[5] = 1;
        h[6] = 1;
        h[7] = 1;
        h[8..10].copy_from_slice(&0x8000u16.to_le_bytes());
        h[10..12].copy_from_slice(&0x8030u16.to_le_bytes());
        h[12..14].copy_from_slice(&0x8000u16.to_le_bytes());
        h[0x6E..0x70].copy_from_slice(&16_639u16.to_le_bytes());
        h[0x7B] = chips;
        h.extend(rom);
        h
    }

    #[test]
    fn cartridge_chips_through_libgme() {
        use crate::notes::nesx;
        let rest: &[(u16, u8)] = &[];
        let mut frames: Vec<&[(u16, u8)]> = vec![&[
            // VRC6 pulse 1: 440 Hz.
            (0x9000, 0x0F),
            (0x9001, 253),
            (0x9002, 0x80),
            // MMC5 pulse 1: 440 Hz, constant volume.
            (0x5015, 0x01),
            (0x5000, 0x3F),
            (0x5002, 253),
            (0x5003, 0x08),
        ]];
        frames.extend([rest; 29]);
        frames.push(&[(0x9002, 0x00), (0x5015, 0x00)]);
        frames.extend([rest; 10]);
        let nsf = synth_nsf(nesx::VRC6 | nesx::MMC5, &frames);
        let t = crate::notes::transcribe_bytes("t.nsf", &nsf, None, 1000);
        for name in ["VRC6 Pulse 1", "MMC5 Pulse 1"] {
            let p = crate::notes::part(&t, name);
            // The first play call is a frame in.
            assert_eq!(crate::notes::keys_at(p), vec![(69, 2)], "{name}");
            assert!(
                (p.notes[0].dur_ms - 500.0).abs() < 20.0,
                "{name}: {:?}",
                p.notes
            );
        }
    }
}
