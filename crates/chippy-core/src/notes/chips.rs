//! More sound chips, read from their register writes the same way as the
//! NES's and the Genesis's: the General Instrument AY-3-8910 (MSX, ZX
//! Spectrum, arcade), the Game Boy's APU, the PC Engine's HuC6280, Atari's
//! POKEY, and Yamaha's YM2413 (OPLL), YM2151 (OPM) and YM3812/YMF262 (OPL2,
//! OPL3) FM chips. Each takes timed writes and gives back its parts.
//!
//! They serve both the GME formats that hook those chips (GBS, HES, KSS,
//! SAP) and VGM logs, which can hold any of them.

use anyhow::{bail, Result};

use super::vgm::Sn76489;
use super::{hz_to_key, Drums, Part, Transcription, Voice};
use crate::decode::gme::capture;
use crate::vfs::Location;

/// A sound chip that turns register writes into notes.
pub(crate) trait Chip {
    /// A write of `data` to register `addr`, `t` ms into the tune.
    fn write(&mut self, t: f64, addr: u16, data: u8);
    /// The parts it played, up to `end` ms.
    fn finish(self: Box<Self>, end: f64) -> Vec<Part>;
}

pub(crate) fn velocity(vol: u8, max: u8) -> u8 {
    (40 + vol.min(max) as u32 * 87 / max.max(1) as u32) as u8
}

/// A part's notes, given a bass instrument when it plays low.
pub(crate) fn part(name: String, program: u8, notes: Vec<super::Note>) -> Part {
    let mut keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
    let low = super::instrument::median(&mut keys).is_some_and(|k| k < 48);
    Part::new(name, if low { 38 } else { program }, notes)
}

/// Rising-edge detection of a channel's loudness: a new note's attack is a
/// jump up in volume, not each step of a swell.
#[derive(Default, Clone, Copy)]
pub(crate) struct Edge {
    last: u8,
    rising: bool,
}

impl Edge {
    pub fn attack(&mut self, level: u8) -> bool {
        let rising = level > self.last;
        let attack = rising && !self.rising;
        self.rising = rising;
        self.last = level;
        attack
    }
}

/// Plays a note at `hz` on `voice`: a new attack, or a change of pitch.
pub(crate) fn sound(voice: &mut Voice, t: f64, hz: f64, attack: bool, vel: u8) {
    if !(20.0..=12_000.0).contains(&hz) {
        return voice.release(t);
    }
    let pitch = hz_to_key(hz);
    if attack || !voice.sounding() {
        voice.attack(t, pitch, vel);
    } else {
        voice.pitch(t, pitch);
        voice.velocity(vel);
    }
}

// ------------------------------------------------------------ AY-3-8910

/// General Instrument AY-3-8910 / Yamaha YM2149: three square-wave tones
/// and a noise source mixed into them.
pub(crate) struct Ay {
    name: &'static str,
    clock: f64,
    /// The clock of writes with bit 8 of the address set (AY files: the
    /// Amstrad CPC's AY, where the Spectrum's is the usual one).
    alt_clock: f64,
    regs: [u8; 16],
    edge: [Edge; 3],
    voices: [Voice; 3],
    drums: Drums,
}

impl Ay {
    pub fn new(name: &'static str, clock: f64) -> Ay {
        Ay {
            name,
            clock,
            alt_clock: clock,
            regs: [0; 16],
            edge: Default::default(),
            voices: Default::default(),
            drums: Drums::default(),
        }
    }

    pub fn with_alt_clock(mut self, clock: f64) -> Ay {
        self.alt_clock = clock;
        self
    }

    /// The pitch of a channel playing the envelope alone, repeating fast
    /// enough to be a tone (the "buzzer" bass): a sawtooth (shapes 8, 12)
    /// or a triangle (10, 14) of the envelope period.
    fn envelope_hz(&self) -> Option<f64> {
        let period = self.regs[11] as u32 | (self.regs[12] as u32) << 8;
        let steps = match self.regs[13] & 15 {
            8 | 12 => 256.0,
            10 | 14 => 512.0,
            _ => return None,
        };
        (period > 0).then(|| self.clock / (steps * period as f64))
    }

    fn update(&mut self, t: f64, c: usize, retrigger: bool) {
        let vol = self.regs[8 + c];
        let level = if vol & 0x10 != 0 { 15 } else { vol & 15 };
        let mixer = self.regs[7];
        let tone = mixer & (1 << c) == 0;
        let noise = mixer & (8 << c) == 0;
        let attack = self.edge[c].attack(level) || (retrigger && vol & 0x10 != 0);
        let buzz = (vol & 0x10 != 0 && !tone)
            .then(|| self.envelope_hz())
            .flatten();
        let v = &mut self.voices[c];
        if let Some(hz) = buzz {
            // The envelope's own restarts aren't new notes when it repeats.
            return sound(v, t, hz, attack && !retrigger, velocity(15, 15));
        }
        if level == 0 || (!tone && !noise) {
            return v.release(t);
        }
        let period = self.regs[c * 2] as u32 | ((self.regs[c * 2 + 1] & 15) as u32) << 8;
        if tone && period > 0 {
            sound(
                v,
                t,
                self.clock / (16.0 * period as f64),
                attack,
                velocity(level, 15),
            );
        } else {
            v.release(t);
            if attack {
                let np = self.regs[6] & 31;
                let key = if np < 8 {
                    42
                } else if np < 20 {
                    38
                } else {
                    36
                };
                self.drums.hit(t, key, velocity(level, 15));
            }
        }
    }
}

impl Chip for Ay {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        if addr & 0x100 != 0 && self.clock != self.alt_clock {
            self.clock = self.alt_clock;
        }
        let r = (addr & 0xFF) as usize;
        if r >= 16 {
            return;
        }
        self.regs[r] = data;
        match r {
            0..=5 => self.update(t, r / 2, false),
            7 | 11 | 12 => (0..3).for_each(|c| self.update(t, c, false)),
            8..=10 => self.update(t, r - 8, false),
            // Writing the envelope shape restarts the envelope.
            13 => (0..3).for_each(|c| self.update(t, c, true)),
            _ => {}
        }
    }

    fn finish(self: Box<Self>, end: f64) -> Vec<Part> {
        let mut parts: Vec<Part> = self
            .voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                part(
                    format!("{} {}", self.name, (b'A' + i as u8) as char),
                    80,
                    v.finish(end),
                )
            })
            .collect();
        parts.push(Part::drums(
            format!("Drums ({} noise)", self.name),
            self.drums.finish(),
        ));
        parts
    }
}

// ------------------------------------------------------------- Game Boy

/// The Game Boy's APU (registers from $FF10): two pulse channels, a wave
/// channel and noise. Notes start on a trigger, and end by their length
/// counter or volume envelope, or a write that silences them.
pub(crate) struct Dmg {
    regs: [u8; 0x30],
    voices: [Voice; 3],
    /// When each channel's note runs out by itself.
    ends: [Option<f64>; 3],
    drums: Drums,
}

/// Each tone channel's first register.
const DMG_BASE: [usize; 3] = [0x00, 0x05, 0x0A];

impl Dmg {
    pub fn new() -> Dmg {
        Dmg {
            regs: [0; 0x30],
            voices: Default::default(),
            ends: [None; 3],
            drums: Drums::default(),
        }
    }

    fn expire(&mut self, t: f64) {
        for c in 0..3 {
            if let Some(e) = self.ends[c] {
                if e <= t {
                    self.voices[c].release(e);
                    self.ends[c] = None;
                }
            }
        }
    }

    fn hz(&self, c: usize) -> f64 {
        let b = DMG_BASE[c];
        let x = self.regs[b + 3] as u32 | ((self.regs[b + 4] & 7) as u32) << 8;
        let rate = if c == 2 { 65_536.0 } else { 131_072.0 };
        rate / (2048 - x) as f64
    }

    fn trigger(&mut self, t: f64, c: usize) {
        let b = DMG_BASE[c];
        let (vol, dac) = if c == 2 {
            let code = (self.regs[0x0C] >> 5) & 3;
            ([0, 15, 8, 4][code as usize], self.regs[0x0A] & 0x80 != 0)
        } else {
            let env = self.regs[b + 2];
            // Rising from silence: give it a middling loudness.
            let v = env >> 4;
            (if v == 0 && env & 8 != 0 { 8 } else { v }, env & 0xF8 != 0)
        };
        if !dac || vol == 0 {
            self.ends[c] = None;
            return self.voices[c].release(t);
        }
        let hz = self.hz(c);
        sound(&mut self.voices[c], t, hz, true, velocity(vol, 15));
        let mut end = f64::INFINITY;
        if c < 2 {
            let env = self.regs[b + 2];
            let period = (env & 7) as f64;
            if env & 8 == 0 && period > 0.0 {
                end = t + vol as f64 * period * 1000.0 / 64.0;
            }
        }
        if self.regs[b + 4] & 0x40 != 0 {
            let len = if c == 2 {
                256 - self.regs[0x0B] as u32
            } else {
                64 - (self.regs[b + 1] & 63) as u32
            };
            end = end.min(t + len as f64 * 1000.0 / 256.0);
        }
        self.ends[c] = end.is_finite().then_some(end);
    }
}

impl Chip for Dmg {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        let a = addr as usize;
        if a >= 0x30 {
            return;
        }
        self.expire(t);
        self.regs[a] = data;
        match a {
            0x04 | 0x09 | 0x0E => {
                let c = (a - 4) / 5;
                if data & 0x80 != 0 {
                    self.trigger(t, c);
                } else if self.voices[c].sounding() {
                    let hz = self.hz(c);
                    sound(&mut self.voices[c], t, hz, false, 100);
                }
            }
            0x03 | 0x08 | 0x0D => {
                let c = (a - 3) / 5;
                if self.voices[c].sounding() {
                    let hz = self.hz(c);
                    self.voices[c].pitch(t, hz_to_key(hz));
                }
            }
            0x02 | 0x07 if data & 0xF8 == 0 => {
                let c = a / 5;
                self.voices[c].release(t);
                self.ends[c] = None;
            }
            0x0A if data & 0x80 == 0 => {
                self.voices[2].release(t);
                self.ends[2] = None;
            }
            0x0C if (data >> 5) & 3 == 0 => {
                self.voices[2].release(t);
                self.ends[2] = None;
            }
            0x13 if data & 0x80 != 0 => {
                let env = self.regs[0x11];
                if env & 0xF8 != 0 {
                    let poly = self.regs[0x12];
                    let r = match poly & 7 {
                        0 => 0.5,
                        d => d as f64,
                    };
                    let hz = 524_288.0 / r / (1u32 << ((poly >> 4) + 1)) as f64;
                    let long = env & 7 == 0 || env & 8 != 0 || (env & 7) >= 5;
                    let key = if hz > 8000.0 {
                        if long {
                            46
                        } else {
                            42
                        }
                    } else if hz > 1500.0 {
                        38
                    } else {
                        36
                    };
                    self.drums.hit(t, key, velocity(env >> 4, 15));
                }
            }
            0x16 if data & 0x80 == 0 => {
                for c in 0..3 {
                    self.voices[c].release(t);
                    self.ends[c] = None;
                }
            }
            _ => {}
        }
    }

    fn finish(mut self: Box<Self>, end: f64) -> Vec<Part> {
        self.expire(end);
        let [p1, p2, w] = self.voices;
        vec![
            part("Pulse 1".into(), 80, p1.finish(end)),
            part("Pulse 2".into(), 80, p2.finish(end)),
            part("Wave".into(), 81, w.finish(end)),
            Part::drums("Drums (noise)", self.drums.finish()),
        ]
    }
}

// --------------------------------------------------------------- HuC6280

/// The PC Engine's HuC6280 PSG: six wavetable channels, the last two of
/// which can play noise. Channels in DDA mode play samples, skipped.
pub(crate) struct Huc {
    clock: f64,
    sel: usize,
    freq: [u16; 6],
    ctrl: [u8; 6],
    noise: [u8; 6],
    edge: [Edge; 6],
    voices: [Voice; 6],
    drums: Drums,
}

impl Huc {
    pub fn new(clock: f64) -> Huc {
        Huc {
            clock,
            sel: 0,
            freq: [0; 6],
            ctrl: [0; 6],
            noise: [0; 6],
            edge: Default::default(),
            voices: Default::default(),
            drums: Drums::default(),
        }
    }

    fn update(&mut self, t: f64, c: usize) {
        let ctrl = self.ctrl[c];
        let on = ctrl & 0x80 != 0 && ctrl & 0x40 == 0;
        let vol = if on { ctrl & 31 } else { 0 };
        let attack = self.edge[c].attack(vol);
        let v = &mut self.voices[c];
        if vol == 0 {
            return v.release(t);
        }
        if c >= 4 && self.noise[c] & 0x80 != 0 {
            v.release(t);
            if attack {
                let n = self.noise[c] & 31;
                let key = if n >= 28 {
                    42
                } else if n >= 20 {
                    38
                } else {
                    36
                };
                self.drums.hit(t, key, velocity(vol, 31));
            }
            return;
        }
        let period = self.freq[c];
        if period == 0 {
            return v.release(t);
        }
        sound(
            v,
            t,
            self.clock / (32.0 * period as f64),
            attack,
            velocity(vol, 31),
        );
    }
}

impl Chip for Huc {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        match addr {
            0 => {
                self.sel = (data & 7) as usize;
                return;
            }
            2 if self.sel < 6 => self.freq[self.sel] = (self.freq[self.sel] & 0xF00) | data as u16,
            3 if self.sel < 6 => {
                self.freq[self.sel] = (self.freq[self.sel] & 0xFF) | ((data & 15) as u16) << 8
            }
            4 if self.sel < 6 => self.ctrl[self.sel] = data,
            7 if self.sel < 6 => self.noise[self.sel] = data,
            _ => return,
        }
        self.update(t, self.sel);
    }

    fn finish(self: Box<Self>, end: f64) -> Vec<Part> {
        let mut parts: Vec<Part> = self
            .voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| part(format!("Wave {}", i + 1), 80, v.finish(end)))
            .collect();
        parts.push(Part::drums("Drums (noise)", self.drums.finish()));
        parts
    }
}

// ----------------------------------------------------------------- POKEY

/// Atari's POKEY: four channels of divided clocks, pure tones or noise
/// (by their distortion setting), pairable into 16-bit channels.
pub(crate) struct Pokey {
    name: &'static str,
    clock: f64,
    regs: [u8; 9],
    edge: [Edge; 4],
    voices: [Voice; 4],
    drums: Drums,
}

impl Pokey {
    pub fn new(name: &'static str, clock: f64) -> Pokey {
        Pokey {
            name,
            clock,
            regs: [0; 9],
            edge: Default::default(),
            voices: Default::default(),
            drums: Drums::default(),
        }
    }

    fn hz(&self, c: usize) -> f64 {
        let ctl = self.regs[8];
        let base = if ctl & 1 != 0 {
            self.clock / 114.0
        } else {
            self.clock / 28.0
        };
        let fast = |ch: usize| (ch == 0 && ctl & 0x40 != 0) || (ch == 2 && ctl & 0x20 != 0);
        let joined = (c == 1 && ctl & 0x10 != 0) || (c == 3 && ctl & 0x08 != 0);
        if joined {
            let div = self.regs[(c - 1) * 2] as f64 + self.regs[c * 2] as f64 * 256.0;
            if fast(c - 1) {
                self.clock / (2.0 * (div + 7.0))
            } else {
                base / (2.0 * (div + 1.0))
            }
        } else {
            let div = self.regs[c * 2] as f64;
            if fast(c) {
                self.clock / (2.0 * (div + 4.0))
            } else {
                base / (2.0 * (div + 1.0))
            }
        }
    }

    fn update(&mut self, t: f64, c: usize) {
        let ctl = self.regs[8];
        let audc = self.regs[c * 2 + 1];
        // The low half of a joined pair, and volume-only (sample) output.
        let low_half = (c == 0 && ctl & 0x10 != 0) || (c == 2 && ctl & 0x08 != 0);
        let level = if low_half || audc & 0x10 != 0 {
            0
        } else {
            audc & 15
        };
        let attack = self.edge[c].attack(level);
        let hz = self.hz(c);
        let v = &mut self.voices[c];
        if level == 0 {
            return v.release(t);
        }
        match audc >> 5 {
            // Pure tones.
            5 | 7 => sound(v, t, hz, attack, velocity(level, 15)),
            // The 4-bit polynomial alone: a buzzy tone, 15 steps long.
            6 => sound(v, t, hz * 2.0 / 15.0, attack, velocity(level, 15)),
            _ => {
                v.release(t);
                if attack {
                    let key = if hz < 200.0 {
                        36
                    } else if hz < 2500.0 {
                        38
                    } else {
                        42
                    };
                    self.drums.hit(t, key, velocity(level, 15));
                }
            }
        }
    }
}

impl Chip for Pokey {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        let a = addr as usize;
        if a > 8 {
            return;
        }
        self.regs[a] = data;
        for c in 0..4 {
            self.update(t, c);
        }
    }

    fn finish(self: Box<Self>, end: f64) -> Vec<Part> {
        let mut parts: Vec<Part> = self
            .voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| part(format!("{} {}", self.name, i + 1), 80, v.finish(end)))
            .collect();
        parts.push(Part::drums(
            format!("Drums ({} noise)", self.name),
            self.drums.finish(),
        ));
        parts
    }
}

// ------------------------------------------------------- SN76489 wrapper

pub(crate) struct Psg {
    sn: Sn76489,
    drums: Drums,
}

impl Psg {
    pub fn new(clock: f64) -> Psg {
        Psg {
            sn: Sn76489::new(clock),
            drums: Drums::default(),
        }
    }
}

impl Chip for Psg {
    fn write(&mut self, t: f64, _addr: u16, data: u8) {
        self.sn.write(t, data, &mut self.drums);
    }

    fn finish(self: Box<Self>, end: f64) -> Vec<Part> {
        let mut parts: Vec<Part> = self
            .sn
            .voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| part(format!("PSG {}", i + 1), 80, v.finish(end)))
            .collect();
        parts.push(Part::drums("Drums (PSG noise)", self.drums.finish()));
        parts
    }
}

// ------------------------------------------------------------------ SCC

/// Konami's SCC (K051649): five wavetable channels, with registers
/// numbered as on the MSX cartridge: waves at $00-$7F (the fifth channel
/// shares the fourth's), periods at $80-$89, volumes $8A-$8E, and the
/// channels' enable bits at $8F.
pub(crate) struct Scc {
    clock: f64,
    regs: [u8; 0x90],
    edge: [Edge; 5],
    voices: [Voice; 5],
}

impl Scc {
    pub fn new(clock: f64) -> Scc {
        Scc {
            clock,
            regs: [0; 0x90],
            edge: Default::default(),
            voices: Default::default(),
        }
    }

    fn update(&mut self, t: f64, c: usize) {
        let on = self.regs[0x8F] & (1 << c) != 0;
        let vol = if on { self.regs[0x8A + c] & 15 } else { 0 };
        let period = self.regs[0x80 + c * 2] as u32 | ((self.regs[0x81 + c * 2] & 15) as u32) << 8;
        // Periods this short are above hearing: a way to mute.
        let vol = if period < 8 { 0 } else { vol };
        let attack = self.edge[c].attack(vol);
        let v = &mut self.voices[c];
        if vol == 0 {
            return v.release(t);
        }
        // A 32-step wave.
        let hz = self.clock / (32.0 * (period + 1) as f64);
        sound(v, t, hz, attack, velocity(vol, 15));
    }
}

impl Chip for Scc {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        let a = addr as usize;
        if !(0x80..0x90).contains(&a) {
            return;
        }
        self.regs[a] = data;
        match a {
            0x80..=0x89 => self.update(t, (a - 0x80) / 2),
            0x8A..=0x8E => self.update(t, a - 0x8A),
            _ => (0..5).for_each(|c| self.update(t, c)),
        }
    }

    fn finish(self: Box<Self>, end: f64) -> Vec<Part> {
        self.voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| part(format!("SCC {}", i + 1), 80, v.finish(end)))
            .collect()
    }
}

// ---------------------------------------------------------------- YM2413

/// The YM2413's built-in instruments, as General MIDI programs (0 is the
/// tune's own instrument).
const OPLL_PROGRAMS: [u8; 16] = [81, 40, 24, 0, 73, 71, 68, 56, 16, 60, 81, 6, 11, 38, 32, 27];
/// The VRC7's: bell, guitar, piano, flute, clarinet, rattling bell,
/// trumpet, reed organ, soft bell, xylophone, vibraphone, brass, bass
/// guitar, synthesizer, chorus.
const VRC7_PROGRAMS: [u8; 16] = [
    81, 14, 24, 0, 73, 71, 14, 56, 20, 10, 13, 11, 61, 33, 81, 52,
];

/// Yamaha YM2413 (OPLL): nine FM channels with preset instruments, the
/// last three of which can become five drums.
pub(crate) struct Opll {
    clock: f64,
    name: &'static str,
    programs: &'static [u8; 16],
    channels: usize,
    regs: [u8; 0x40],
    voices: [Voice; 9],
    uses: [Vec<(f64, u8)>; 9],
    drums: Drums,
}

impl Opll {
    pub fn new(clock: f64) -> Opll {
        Opll {
            clock,
            name: "FM",
            programs: &OPLL_PROGRAMS,
            channels: 9,
            regs: [0; 0x40],
            voices: Default::default(),
            uses: Default::default(),
            drums: Drums::default(),
        }
    }

    /// Konami's VRC7 (NES cartridges): six channels of an OPLL with
    /// instruments of its own, and no rhythm.
    pub fn vrc7() -> Opll {
        Opll {
            name: "VRC7",
            programs: &VRC7_PROGRAMS,
            channels: 6,
            ..Opll::new(3_579_545.0)
        }
    }

    fn hz(&self, c: usize) -> f64 {
        let fnum = self.regs[0x10 + c] as u32 | ((self.regs[0x20 + c] & 1) as u32) << 8;
        let block = (self.regs[0x20 + c] >> 1) & 7;
        fnum as f64 * self.clock / 72.0 / (1u32 << (19 - block)) as f64
    }
}

impl Chip for Opll {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        let r = (addr & 0x3F) as usize;
        let old = self.regs[r];
        self.regs[r] = data;
        let rhythm = self.regs[0x0E] & 0x20 != 0;
        match r {
            0x0E if rhythm && self.channels == 9 => {
                let on = data & !old & 0x1F;
                for (bit, key) in [(4, 36), (3, 38), (2, 45), (1, 49), (0, 42)] {
                    if on & (1 << bit) != 0 {
                        self.drums.hit(t, key, 100);
                    }
                }
            }
            0x10..=0x18 | 0x20..=0x28 => {
                let c = r & 15;
                if c >= self.channels || (rhythm && c >= 6) {
                    return;
                }
                let key = self.regs[0x20 + c] & 0x10 != 0;
                let was = if r >= 0x20 { old & 0x10 != 0 } else { key };
                let v = &mut self.voices[c];
                if !key {
                    v.release(t);
                } else if !was {
                    let hz = self.hz(c);
                    sound(&mut self.voices[c], t, hz, true, 100);
                    let inst = self.regs[0x30 + c] >> 4;
                    self.uses[c].push((t, self.programs[inst as usize]));
                } else if v.sounding() {
                    let hz = self.hz(c);
                    self.voices[c].pitch(t, hz_to_key(hz));
                }
            }
            _ => {}
        }
    }

    fn finish(self: Box<Self>, end: f64) -> Vec<Part> {
        let name = self.name;
        let mut parts: Vec<Part> = self
            .voices
            .into_iter()
            .zip(self.uses)
            .take(self.channels)
            .enumerate()
            .map(|(i, (v, uses))| {
                Part::new(format!("{name} {}", i + 1), 81, v.finish(end)).with_programs(&uses)
            })
            .collect();
        if self.channels == 9 {
            parts.push(Part::drums("Drums (FM rhythm)", self.drums.finish()));
        }
        parts
    }
}

// ---------------------------------------------------------------- YM2151

/// Yamaha YM2151 (OPM): eight FM channels, pitched by key code (octave and
/// note) and a key fraction.
pub(crate) struct Opm {
    clock: f64,
    regs: [u8; 0x100],
    keyed: [u8; 8],
    voices: [Voice; 8],
}

impl Opm {
    pub fn new(clock: f64) -> Opm {
        Opm {
            clock,
            regs: [0; 0x100],
            keyed: [0; 8],
            voices: Default::default(),
        }
    }

    fn key(&self, c: usize) -> f64 {
        // Note codes: C# D D# (D#) E F F# (F#) G G# A (A) A# B C (C).
        const SEMI: [u8; 16] = [1, 2, 3, 3, 4, 5, 6, 6, 7, 8, 9, 9, 10, 11, 12, 12];
        let kc = self.regs[0x28 + c];
        let oct = ((kc >> 4) & 7) as f64;
        let semi = SEMI[(kc & 15) as usize] as f64;
        let frac = (self.regs[0x30 + c] >> 2) as f64 / 64.0;
        12.0 * (oct + 1.0) + semi + frac + 12.0 * (self.clock / 3_579_545.0).log2()
    }
}

impl Chip for Opm {
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        let r = addr as usize & 0xFF;
        self.regs[r] = data;
        match r {
            0x08 => {
                let c = (data & 7) as usize;
                let ops = (data >> 3) & 15;
                let was = self.keyed[c];
                self.keyed[c] = ops;
                if ops == 0 {
                    self.voices[c].release(t);
                } else if was == 0 {
                    let k = self.key(c);
                    self.voices[c].attack(t, k, 100);
                }
            }
            0x28..=0x37 => {
                let c = r & 7;
                if self.keyed[c] != 0 {
                    let k = self.key(c);
                    self.voices[c].pitch(t, k);
                }
            }
            _ => {}
        }
    }

    fn finish(self: Box<Self>, end: f64) -> Vec<Part> {
        self.voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| part(format!("FM {}", i + 1), 81, v.finish(end)))
            .collect()
    }
}

// ------------------------------------------------------------ OPL2/OPL3

/// Yamaha YM3812 (OPL2) and YMF262 (OPL3): nine FM channels per register
/// bank (two banks on OPL3), and five rhythm drums.
pub(crate) struct Opl {
    name: &'static str,
    /// The chip's sample rate: clock / 72 (OPL2) or / 288 (OPL3).
    rate: f64,
    regs: [[u8; 0x100]; 2],
    voices: Vec<Voice>,
    drums: Drums,
}

impl Opl {
    pub fn new(rate: f64, banks: usize) -> Opl {
        Opl::named("FM", rate, banks)
    }

    pub fn named(name: &'static str, rate: f64, banks: usize) -> Opl {
        Opl {
            name,
            rate,
            regs: [[0; 0x100]; 2],
            voices: (0..9 * banks).map(|_| Voice::default()).collect(),
            drums: Drums::default(),
        }
    }

    fn hz(&self, bank: usize, c: usize) -> f64 {
        let r = &self.regs[bank];
        let fnum = r[0xA0 + c] as u32 | ((r[0xB0 + c] & 3) as u32) << 8;
        let block = (r[0xB0 + c] >> 2) & 7;
        fnum as f64 * self.rate / (1u32 << (20 - block)) as f64
    }
}

impl Chip for Opl {
    /// `addr` holds the bank in bit 8.
    fn write(&mut self, t: f64, addr: u16, data: u8) {
        let bank = ((addr >> 8) & 1) as usize;
        if bank * 9 >= self.voices.len() {
            return;
        }
        let r = (addr & 0xFF) as usize;
        let old = self.regs[bank][r];
        self.regs[bank][r] = data;
        let rhythm = self.regs[0][0xBD] & 0x20 != 0;
        match r {
            0xBD if bank == 0 && rhythm => {
                let on = data & !old & 0x1F;
                for (bit, key) in [(4, 36), (3, 38), (2, 45), (1, 49), (0, 42)] {
                    if on & (1 << bit) != 0 {
                        self.drums.hit(t, key, 100);
                    }
                }
            }
            0xA0..=0xA8 | 0xB0..=0xB8 => {
                let c = r & 15;
                if bank == 0 && rhythm && c >= 6 {
                    return;
                }
                let key = self.regs[bank][0xB0 + c] & 0x20 != 0;
                let was = if r >= 0xB0 { old & 0x20 != 0 } else { key };
                let hz = self.hz(bank, c);
                let v = &mut self.voices[bank * 9 + c];
                if !key {
                    v.release(t);
                } else if !was {
                    sound(v, t, hz, true, 100);
                } else if v.sounding() && hz > 0.0 {
                    v.pitch(t, hz_to_key(hz));
                }
            }
            _ => {}
        }
    }

    fn finish(self: Box<Self>, end: f64) -> Vec<Part> {
        let mut parts: Vec<Part> = self
            .voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| part(format!("{} {}", self.name, i + 1), 81, v.finish(end)))
            .collect();
        parts.push(Part::drums(
            format!("Drums ({} rhythm)", self.name),
            self.drums.finish(),
        ));
        parts
    }
}

// ------------------------------------------------------ GME formats

/// ZX Spectrum and Amstrad CPC (AY), Game Boy (GBS), PC Engine (HES), MSX
/// and Sega (KSS) and Atari (SAP) tunes: their chips' writes, seen as the emulator plays.
pub fn transcribe_gme(
    loc: &Location,
    ext: &str,
    track: u32,
    max_ms: Option<u64>,
) -> Result<Transcription> {
    const RATE: u32 = 44_100;
    const CHUNK: usize = 1024;
    let mut chips: Vec<Box<dyn Chip>> = match ext {
        // The Spectrum's AY (half its 3.5469 MHz CPU clock), or the CPC's.
        "ay" => vec![Box::new(
            Ay::new("AY", 1_773_450.0).with_alt_clock(1_000_000.0),
        )],
        "gbs" => vec![Box::new(Dmg::new())],
        "hes" => vec![Box::new(Huc::new(3_579_545.0))],
        // The PSG and Sega PSG (by APU write), then the SCC and the FM
        // chips (MSX-MUSIC or the Master System's FM unit, and
        // MSX-AUDIO), by OUT port.
        "kss" => vec![
            Box::new(Ay::new("AY", 1_789_772.5)),
            Box::new(Psg::new(3_579_545.0)),
            Box::new(Scc::new(3_579_545.0)),
            Box::new(Opll::new(3_579_545.0)),
            Box::new(Opl::named("MSX-AUDIO", 3_579_545.0 / 72.0, 1)),
        ],
        "sap" => vec![
            Box::new(Pokey::new("POKEY", 1_789_790.0)),
            Box::new(Pokey::new("POKEY 2", 1_789_790.0)),
        ],
        _ => bail!("can't find the notes in .{ext} files"),
    };
    let mut writes = 0usize;
    let mut latch = [0u8; 2];
    let cap = capture(loc, track, RATE, max_ms, CHUNK, |frame, w| {
        let block = frame as f64 * 1000.0 / RATE as f64;
        for w in w.iter().filter(|w| w.kind == 0 || w.kind == 3) {
            let t = if w.ms >= 0.0 { w.ms } else { block };
            let data = w.data as u8;
            let (chip, addr) = match (ext, w.kind, w.addr) {
                (_, 0, _) => {
                    writes += 1;
                    match ext {
                        "kss" if w.addr == 16 => (1, 0),
                        "sap" if w.addr >= 10 => (1, w.addr - 10),
                        _ => (0, w.addr),
                    }
                }
                ("kss", _, 0x100..) => (2, w.addr - 0x100),
                // FM address ports: MSX-MUSIC, the Sega FM unit, MSX-AUDIO.
                ("kss", _, 0x7C | 0xF0) => {
                    latch[0] = data;
                    continue;
                }
                ("kss", _, 0xC0) => {
                    latch[1] = data;
                    continue;
                }
                ("kss", _, 0x7D | 0xF1) => (3, latch[0] as i32),
                ("kss", _, 0xC1) => (4, latch[1] as i32),
                _ => continue,
            };
            if w.kind == 3 {
                writes += 1;
            }
            chips[chip].write(t, addr as u16, data);
        }
    })?;
    let end = cap.frames as f64 * 1000.0 / RATE as f64;
    let mut t = Transcription {
        system: cap.system,
        parts: chips.into_iter().flat_map(|c| c.finish(end)).collect(),
        ..Default::default()
    };
    if writes == 0 {
        t.notes
            .push("The tune wrote nothing to its sound chip.".into());
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(p: &Part) -> Vec<u8> {
        p.notes.iter().map(|n| n.key).collect()
    }

    #[test]
    fn ay_tones_and_volume_attacks() {
        let mut ay = Box::new(Ay::new("AY", 1_789_772.5));
        // Channel A, period 254 (440 Hz), tone on, volume up, down, up.
        ay.write(0.0, 0, 254);
        ay.write(0.0, 7, 0b0011_1110);
        ay.write(0.0, 8, 15);
        ay.write(100.0, 8, 10);
        ay.write(200.0, 8, 15);
        ay.write(300.0, 0, 127); // an octave up, no new attack
        ay.write(400.0, 8, 0);
        let parts = ay.finish(500.0);
        assert_eq!(keys(&parts[0]), vec![69, 69, 81]);
        assert!(parts[0].notes[2].legato);
    }

    #[test]
    fn game_boy_triggers_and_envelopes() {
        let mut gb = Box::new(Dmg::new());
        // Pulse 1: volume 15, decreasing every 1/64 s: silent after 234 ms.
        gb.write(0.0, 0x02, 0xF1);
        // x = 1750: 131072 / 298 = 439.8 Hz.
        gb.write(0.0, 0x03, (1750 & 0xFF) as u8);
        gb.write(0.0, 0x04, 0x80 | (1750 >> 8) as u8);
        gb.write(1000.0, 0x13, 0x80); // noise trigger, but its DAC is off
        let parts = gb.finish(2000.0);
        assert_eq!(keys(&parts[0]), vec![69]);
        assert!(
            (parts[0].notes[0].dur_ms - 234.4).abs() < 1.0,
            "{}",
            parts[0].notes[0].dur_ms
        );
        assert!(parts[3].notes.is_empty());
    }

    #[test]
    fn opm_key_codes() {
        let mut opm = Box::new(Opm::new(3_579_545.0));
        opm.write(0.0, 0x28, 0x4A); // octave 4, A
        opm.write(0.0, 0x08, 0x78); // key on channel 0
        opm.write(500.0, 0x08, 0x00);
        let parts = opm.finish(1000.0);
        assert_eq!(keys(&parts[0]), vec![69]);
    }

    #[test]
    fn opl_and_opll_pitches() {
        // OPL2: fnum 577, block 4 = 440 Hz at 49716 Hz.
        let mut opl = Box::new(Opl::new(3_579_545.0 / 72.0, 1));
        opl.write(0.0, 0xA0, (577 & 0xFF) as u8);
        opl.write(0.0, 0xB0, 0x20 | 4 << 2 | (577 >> 8) as u8);
        opl.write(500.0, 0xB0, 4 << 2 | (577 >> 8) as u8);
        assert_eq!(keys(&opl.finish(1000.0)[0]), vec![69]);
        // OPLL: fnum 289, block 4.
        let mut opll = Box::new(Opll::new(3_579_545.0));
        opll.write(0.0, 0x30, 0x30); // piano
        opll.write(0.0, 0x10, (289 & 0xFF) as u8);
        opll.write(0.0, 0x20, 0x10 | 4 << 1 | (289 >> 8) as u8);
        let parts = opll.finish(500.0);
        assert_eq!(keys(&parts[0]), vec![69]);
        assert_eq!(parts[0].program, 0);
    }

    #[test]
    fn pokey_pure_tones() {
        let mut p = Box::new(Pokey::new("POKEY", 1_789_790.0));
        // 64 kHz base: 63921 / (2 * (72 + 1)) = 437.8 Hz.
        p.write(0.0, 0, 72);
        p.write(0.0, 1, 0xA8);
        p.write(300.0, 1, 0xA0);
        assert_eq!(keys(&p.finish(500.0)[0]), vec![69]);
    }

    /// A ZX Spectrum AY file whose play routine (called at 50 Hz) writes
    /// each frame's (register, value) pairs to the AY.
    pub(crate) fn synth_ay(frames: &[&[(u8, u8)]]) -> Vec<u8> {
        const CODE: u16 = 0x8000;
        const PTR: u16 = 0x8100;
        const DATA: u16 = 0x8200;
        let mut mem = vec![0u8; (DATA - CODE) as usize];
        #[rustfmt::skip]
        let play = [
            0x2A, PTR as u8, (PTR >> 8) as u8, // LD HL,(PTR)
            0x7E, 0xFE, 0xFF, 0x28, 0x0E,      // loop: LD A,(HL); CP $FF; JR Z,done
            0x01, 0xFD, 0xFF, 0xED, 0x79,      // LD BC,$FFFD; OUT (C),A
            0x23, 0x7E, 0x06, 0xBF, 0xED, 0x79, // INC HL; LD A,(HL); LD B,$BF; OUT (C),A
            0x23, 0x18, 0xED,                  // INC HL; JR loop
            0x23, 0x22, PTR as u8, (PTR >> 8) as u8, 0xC9, // done: INC HL; LD (PTR),HL; RET
        ];
        mem[..play.len()].copy_from_slice(&play);
        mem[0x20] = 0xC9; // init: RET
        mem[0x100..0x102].copy_from_slice(&DATA.to_le_bytes());
        for f in frames {
            for &(r, v) in *f {
                mem.extend([r, v]);
            }
            mem.push(0xFF);
        }
        mem.extend(std::iter::repeat_n(0xFF, 3000));

        let mut f = vec![0u8; 54];
        let rel = |f: &mut Vec<u8>, at: usize, to: usize| {
            f[at..at + 2].copy_from_slice(&((to - at) as u16).to_be_bytes())
        };
        f[..8].copy_from_slice(b"ZXAYEMUL");
        rel(&mut f, 12, 52);
        rel(&mut f, 14, 52);
        rel(&mut f, 18, 20);
        rel(&mut f, 20, 52);
        rel(&mut f, 22, 24);
        f[24..28].copy_from_slice(&[0, 1, 2, 3]);
        f[28..30].copy_from_slice(&3000u16.to_be_bytes());
        rel(&mut f, 34, 38);
        rel(&mut f, 36, 44);
        f[38..40].copy_from_slice(&0xF000u16.to_be_bytes());
        f[40..42].copy_from_slice(&(CODE + 0x20).to_be_bytes());
        f[42..44].copy_from_slice(&CODE.to_be_bytes());
        f[44..46].copy_from_slice(&CODE.to_be_bytes());
        f[46..48].copy_from_slice(&(mem.len() as u16).to_be_bytes());
        rel(&mut f, 48, 54);
        f.extend(mem);
        f
    }

    #[test]
    fn ay_file_tones_and_buzzer_bass() {
        let rest: &[(u8, u8)] = &[];
        let mut frames: Vec<&[(u8, u8)]> = Vec::new();
        // A4 on channel A (period 252), and on B the envelope alone as a
        // sawtooth: 1773450 / (256 * 63) = 110 Hz, A2.
        frames.push(&[
            (7, 0b0011_1110),
            (0, 252),
            (1, 0),
            (8, 15),
            (11, 63),
            (12, 0),
            (13, 8),
            (9, 0x10),
        ]);
        frames.extend([rest; 9]);
        frames.push(&[(0, 126)]); // A5, slid to
        frames.extend([rest; 9]);
        frames.push(&[(8, 0), (9, 0)]);
        frames.extend([rest; 4]);
        frames.push(&[(0, 168), (8, 12)]); // 659.8 Hz: E5
        frames.extend([rest; 10]);
        let t = super::super::transcribe_bytes("t.ay", &synth_ay(&frames), None, 1500);
        use super::super::{keys_at, part};
        let a = part(&t, "AY A");
        // The first play call is the first interrupt, 20 ms in.
        assert_eq!(keys_at(a), vec![(69, 2), (81, 22), (76, 52)]);
        assert!(a.notes[1].legato);
        assert_eq!(keys_at(part(&t, "AY B")), vec![(45, 2)]);
    }

    /// An MSX KSS whose play routine (60 Hz) makes each frame's writes:
    /// `Out(port, value)`, or `Scc(register, value)` to the SCC's memory.
    enum Kw {
        Out(u8, u8),
        Scc(u8, u8),
    }

    fn synth_kss(frames: &[&[Kw]]) -> Vec<u8> {
        #[rustfmt::skip]
        let play = [
            0x2A, 0x00, 0xC0,       // LD HL,(PTR)
            0x7E, 0x23, 0xB7,       // loop: LD A,(HL); INC HL; OR A
            0x28, 0x14, 0xFE, 0x01, // JR Z,done; CP 1
            0x4E, 0x23, 0x46, 0x23, // LD C,(HL); INC HL; LD B,(HL); INC HL
            0x20, 0x04,             // JR NZ,mem
            0xED, 0x41, 0x18, 0xEF, // OUT (C),B; JR loop
            0xE5, 0x26, 0x98, 0x69, // mem: PUSH HL; LD H,$98; LD L,C
            0x70, 0xE1, 0x18, 0xE7, // LD (HL),B; POP HL; JR loop
            0x22, 0x00, 0xC0, 0xC9, // done: LD (PTR),HL; RET
        ];
        let mut code = vec![0u8; 0x80];
        code[..play.len()].copy_from_slice(&play);
        // init: PTR = the frames.
        code[0x40..0x47].copy_from_slice(&[0x21, 0x80, 0x40, 0x22, 0x00, 0xC0, 0xC9]);
        for f in frames {
            for w in *f {
                match *w {
                    Kw::Out(p, v) => code.extend([1, p, v]),
                    Kw::Scc(r, v) => code.extend([2, r, v]),
                }
            }
            code.push(0);
        }
        code.extend(std::iter::repeat_n(0, 2000));
        let mut f = b"KSCC".to_vec();
        f.extend(0x4000u16.to_le_bytes());
        f.extend((code.len() as u16).to_le_bytes());
        f.extend(0x4040u16.to_le_bytes());
        f.extend(0x4000u16.to_le_bytes());
        f.extend([0, 0, 0, 0]);
        f.extend(code);
        f
    }

    #[test]
    fn kss_psg_scc_and_fm_through_libgme() {
        use Kw::*;
        let rest: &[Kw] = &[];
        let mut frames: Vec<&[Kw]> = vec![&[
            // SCC channel 1: 3579545 / (32 * 254) = 440.4 Hz.
            Scc(0x80, 253),
            Scc(0x81, 0),
            Scc(0x8A, 15),
            Scc(0x8F, 1),
            // MSX-MUSIC channel 1: fnum 289, block 4, key on.
            Out(0x7C, 0x10),
            Out(0x7D, 0x21),
            Out(0x7C, 0x20),
            Out(0x7D, 0x19),
            // The PSG's channel A: period 254.
            Out(0xA0, 7),
            Out(0xA1, 0x3E),
            Out(0xA0, 0),
            Out(0xA1, 254),
            Out(0xA0, 8),
            Out(0xA1, 15),
        ]];
        frames.extend([rest; 29]);
        frames.push(&[
            Scc(0x8F, 0),
            Out(0x7C, 0x20),
            Out(0x7D, 0x09),
            Out(0xA0, 8),
            Out(0xA1, 0),
        ]);
        frames.extend([rest; 10]);
        let t = super::super::transcribe_bytes("t.kss", &synth_kss(&frames), None, 1000);
        for name in ["SCC 1", "FM 1", "AY A"] {
            let p = super::super::part(&t, name);
            assert_eq!(keys(p), vec![69], "{name}");
            assert!(
                (p.notes[0].dur_ms - 500.0).abs() < 5.0,
                "{name}: {:?}",
                p.notes
            );
        }
    }
}
