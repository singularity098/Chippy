//! SGC: Sega Master System and Game Gear music. The file holds the game's
//! Z80 sound code and data; it runs on an emulated Z80 with the Sega
//! mapper, its init routine once and its play routine every frame, and the
//! SN76489 PSG it writes to makes the sound (with the Game Gear's stereo).
//! The Master System's FM unit (YM2413) is not emulated, so music written
//! only for FM plays silent; ColecoVision SGCs need a BIOS and are refused.

use anyhow::{bail, Result};
use iz80::{Cpu, Machine, Reg16};

use super::timed::{Engine, Timed};
use super::{DecodeOptions, SubTrack};
use crate::meta::TrackMeta;
use crate::vfs::Location;

const HEADER: usize = 0xa0;
const NTSC_CLOCK: u32 = 3_579_545;
const PAL_CLOCK: u32 = 3_546_893;
/// Where routines return to: a HALT the player watches for.
const IDLE: u16 = 0x0000;

struct Header {
    pal: bool,
    load: u16,
    init: u16,
    play: u16,
    sp: u16,
    rst: [u16; 7],
    mapping: [u8; 4],
    first_song: u8,
    songs: u8,
    system: u8,
    game: String,
    author: String,
    copyright: String,
}

fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end]
        .iter()
        .map(|&c| c as char)
        .collect::<String>()
        .trim()
        .to_string()
}

fn parse(d: &[u8]) -> Result<Header> {
    if d.len() < HEADER || &d[..4] != b"SGC\x1a" {
        bail!("not an SGC file");
    }
    let w = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
    let mut rst = [0u16; 7];
    for (i, r) in rst.iter_mut().enumerate() {
        *r = w(0x10 + i * 2);
    }
    Ok(Header {
        pal: d[0x05] == 1,
        load: w(0x08),
        init: w(0x0a),
        play: w(0x0c),
        sp: w(0x0e),
        rst,
        mapping: [d[0x1e], d[0x1f], d[0x20], d[0x21]],
        first_song: d[0x22],
        songs: d[0x23].max(1),
        system: d[0x26],
        game: text(&d[0x40..0x60]),
        author: text(&d[0x60..0x80]),
        copyright: text(&d[0x80..0xa0]),
    })
}

// ------------------------------------------------------------------- PSG

/// Texas Instruments SN76489 as in the Master System and Game Gear.
struct Psg {
    tone: [u16; 3],
    counter: [u16; 4],
    out: [bool; 4],
    vol: [u8; 4],
    noise: u8,
    lfsr: u16,
    latch: usize,
    /// Game Gear stereo: bit n right, bit n+4 left, per channel.
    stereo: u8,
}

/// 2 dB per step; 15 is silence.
const VOLUME: [f32; 16] = [
    1.0, 0.794, 0.631, 0.501, 0.398, 0.316, 0.251, 0.200, 0.158, 0.126, 0.100, 0.079, 0.063, 0.050,
    0.040, 0.0,
];

impl Psg {
    fn new() -> Psg {
        Psg {
            tone: [0; 3],
            counter: [1; 4],
            out: [true; 4],
            vol: [15; 4],
            noise: 0,
            lfsr: 0x8000,
            latch: 0,
            stereo: 0xff,
        }
    }

    fn write(&mut self, v: u8) {
        if v & 0x80 != 0 {
            self.latch = ((v >> 4) & 7) as usize;
            self.set(v & 0x0f, false);
        } else {
            self.set(v & 0x3f, true);
        }
    }

    fn set(&mut self, data: u8, high: bool) {
        let ch = self.latch / 2;
        if self.latch & 1 == 1 {
            self.vol[ch] = data & 0x0f;
        } else if ch < 3 {
            self.tone[ch] = if high {
                (self.tone[ch] & 0x0f) | ((data as u16) << 4)
            } else {
                (self.tone[ch] & 0x3f0) | (data as u16 & 0x0f)
            };
        } else {
            self.noise = data & 7;
            self.lfsr = 0x8000;
        }
    }

    /// One step at clock/16; returns (left, right).
    fn step(&mut self) -> (f32, f32) {
        for ch in 0..3 {
            self.counter[ch] = self.counter[ch].saturating_sub(1);
            if self.counter[ch] == 0 {
                self.counter[ch] = self.tone[ch].max(1);
                // A period of 0 or 1 holds the output high (used for
                // sampled sound through the volume register).
                self.out[ch] = if self.tone[ch] <= 1 {
                    true
                } else {
                    !self.out[ch]
                };
            }
        }
        self.counter[3] = self.counter[3].saturating_sub(1);
        if self.counter[3] == 0 {
            self.counter[3] = match self.noise & 3 {
                0 => 0x10,
                1 => 0x20,
                2 => 0x40,
                _ => self.tone[2].max(1),
            };
            self.out[3] = !self.out[3];
            if self.out[3] {
                // White noise taps bits 0 and 3; periodic noise only bit 0.
                let fb = if self.noise & 4 != 0 {
                    (self.lfsr ^ (self.lfsr >> 3)) & 1
                } else {
                    self.lfsr & 1
                };
                self.lfsr = (self.lfsr >> 1) | (fb << 15);
            }
        }
        let (mut l, mut r) = (0f32, 0f32);
        for ch in 0..4 {
            let level = if ch == 3 {
                self.lfsr & 1 == 1
            } else {
                self.out[ch]
            };
            let v = if level {
                VOLUME[self.vol[ch] as usize]
            } else {
                -VOLUME[self.vol[ch] as usize]
            };
            if self.stereo & (0x10 << ch) != 0 {
                l += v;
            }
            if self.stereo & (1 << ch) != 0 {
                r += v;
            }
        }
        (l * 0.25, r * 0.25)
    }
}

// --------------------------------------------------------------- machine

struct Bus {
    rom: Vec<u8>,
    ram: Box<[u8; 0x2000]>,
    cart_ram: Box<[u8; 0x8000]>,
    mapper: [u8; 4],
    vectors: [u8; 0x40],
    /// PSG writes during the frame: (cycle, port, value).
    writes: Vec<(u64, u8, u8)>,
    now: u64,
}

impl Bus {
    fn rom_byte(&self, page: u8, offset: u16) -> u8 {
        let pages = (self.rom.len() / 0x4000).max(1);
        let p = page as usize % pages;
        *self.rom.get(p * 0x4000 + offset as usize).unwrap_or(&0xff)
    }
}

impl Machine for Bus {
    fn peek(&mut self, a: u16) -> u8 {
        match a {
            0x0000..=0x003f => self.vectors[a as usize],
            // The first 1 KB is always page 0.
            0x0040..=0x03ff => self.rom_byte(0, a),
            0x0400..=0x3fff => self.rom_byte(self.mapper[1], a),
            0x4000..=0x7fff => self.rom_byte(self.mapper[2], a - 0x4000),
            0x8000..=0xbfff => {
                if self.mapper[0] & 0x08 != 0 {
                    let bank = ((self.mapper[0] >> 2) & 1) as usize * 0x4000;
                    self.cart_ram[bank + (a - 0x8000) as usize]
                } else {
                    self.rom_byte(self.mapper[3], a - 0x8000)
                }
            }
            _ => self.ram[(a & 0x1fff) as usize],
        }
    }

    fn poke(&mut self, a: u16, v: u8) {
        match a {
            0x8000..=0xbfff if self.mapper[0] & 0x08 != 0 => {
                let bank = ((self.mapper[0] >> 2) & 1) as usize * 0x4000;
                self.cart_ram[bank + (a - 0x8000) as usize] = v;
            }
            0xc000..=0xffff => {
                self.ram[(a & 0x1fff) as usize] = v;
                if a >= 0xfffc {
                    self.mapper[(a - 0xfffc) as usize] = v;
                }
            }
            _ => {}
        }
    }

    fn port_in(&mut self, a: u16) -> u8 {
        match a as u8 {
            // VDP status: say a frame has just ended, so waiting loops end.
            0xbd | 0xbf => 0x80,
            // V counter: moves on with time.
            0x7e => (self.now / 228) as u8,
            0xdc | 0xdd | 0xc0 | 0xc1 => 0xff,
            _ => 0xff,
        }
    }

    fn port_out(&mut self, a: u16, v: u8) {
        let port = a as u8;
        // The PSG, the Game Gear's stereo, and the FM unit's address and
        // data (not played, but its notes can be found).
        if (0x40..=0x7f).contains(&port) || port == 0x06 || port == 0xf0 || port == 0xf1 {
            self.writes.push((self.now, port, v));
        }
    }
}

pub struct SgcEngine {
    h: Header,
    data: Vec<u8>,
    song: u8,
    cpu: Cpu,
    bus: Bus,
    psg: Psg,
    rate: u32,
    clock: u32,
    frame_cycles: u64,
    /// PSG steps (clock/16) per output frame, as a fraction.
    steps_per_frame: f64,
    step_acc: f64,
    /// The frame's audio not handed out yet.
    pending: Vec<i16>,
    pending_pos: usize,
    dc: [f32; 2],
    /// The init routine's PSG writes.
    init_writes: Vec<(u64, u8, u8)>,
}

// Plain data, owned by the decoder.
unsafe impl Send for SgcEngine {}

impl SgcEngine {
    fn new(data: Vec<u8>, song: u8, rate: u32) -> Result<SgcEngine> {
        let h = parse(&data)?;
        if h.system == 2 {
            bail!("ColecoVision SGC files need the ColecoVision BIOS, which Chippy does not have");
        }
        let clock = if h.pal { PAL_CLOCK } else { NTSC_CLOCK };
        let mut e = SgcEngine {
            frame_cycles: clock as u64 / if h.pal { 50 } else { 60 },
            steps_per_frame: clock as f64 / 16.0 / rate as f64,
            h,
            data,
            song,
            cpu: Cpu::new_z80(),
            bus: Bus {
                rom: Vec::new(),
                ram: Box::new([0; 0x2000]),
                cart_ram: Box::new([0; 0x8000]),
                mapper: [0; 4],
                vectors: [0xff; 0x40],
                writes: Vec::new(),
                now: 0,
            },
            psg: Psg::new(),
            rate,
            clock,
            step_acc: 0.0,
            pending: Vec::new(),
            pending_pos: 0,
            dc: [0.0; 2],
            init_writes: Vec::new(),
        };
        e.reset()?;
        Ok(e)
    }

    fn reset(&mut self) -> Result<()> {
        let h = &self.h;
        let mut rom = vec![0xffu8; h.load as usize];
        rom.extend_from_slice(&self.data[HEADER..]);
        rom.resize(rom.len().div_ceil(0x4000) * 0x4000, 0xff);
        self.bus.rom = rom;
        self.bus.ram.fill(0);
        self.bus.cart_ram.fill(0);
        self.bus.vectors = [0xff; 0x40];
        self.bus.vectors[0] = 0x76; // HALT: where routines return to
        for (i, &addr) in h.rst.iter().enumerate() {
            let o = (i + 1) * 8;
            self.bus.vectors[o] = 0xc3; // JP addr
            self.bus.vectors[o + 1..o + 3].copy_from_slice(&addr.to_le_bytes());
        }
        for (i, &m) in h.mapping.iter().enumerate() {
            self.bus.poke(0xfffc + i as u16, m);
        }
        self.psg = Psg::new();
        self.cpu = Cpu::new_z80();
        self.pending.clear();
        self.pending_pos = 0;
        self.step_acc = 0.0;
        self.dc = [0.0; 2];
        // Init gets a few frames' time; its PSG writes apply at once.
        self.cpu.registers().set_a(self.song);
        self.call(self.h.init, self.frame_cycles * 20);
        self.init_writes = std::mem::take(&mut self.bus.writes);
        for (_, port, v) in self.init_writes.clone() {
            self.apply(port, v);
        }
        Ok(())
    }

    fn apply(&mut self, port: u8, v: u8) {
        match port {
            0x06 => self.psg.stereo = v,
            0xf0 | 0xf1 => {}
            _ => self.psg.write(v),
        }
    }

    /// Calls `addr` and runs until it returns or `budget` cycles pass.
    fn call(&mut self, addr: u16, budget: u64) {
        let sp = self.h.sp.wrapping_sub(2);
        self.bus.poke(sp, (IDLE & 0xff) as u8);
        self.bus.poke(sp.wrapping_add(1), (IDLE >> 8) as u8);
        let r = self.cpu.registers();
        r.set16(Reg16::SP, sp);
        r.set_pc(addr);
        let start = self.cpu.cycle_count();
        loop {
            self.bus.now = self.cpu.cycle_count() - start;
            if self.cpu.immutable_registers().pc() == IDLE || self.bus.now >= budget {
                break;
            }
            self.cpu.execute_instruction(&mut self.bus);
        }
    }

    /// Plays one frame: the play routine, then the PSG through the frame.
    fn frame(&mut self) {
        self.call(self.h.play, self.frame_cycles);
        let writes = std::mem::take(&mut self.bus.writes);
        let mut wi = 0;
        let mut cycle = 0f64;
        let cycles_per_out = self.clock as f64 / self.rate as f64;
        self.pending.clear();
        self.pending_pos = 0;
        while cycle < self.frame_cycles as f64 {
            while wi < writes.len() && (writes[wi].0 as f64) <= cycle {
                self.apply(writes[wi].1, writes[wi].2);
                wi += 1;
            }
            self.step_acc += self.steps_per_frame;
            let n = self.step_acc as u32;
            self.step_acc -= n as f64;
            let (mut l, mut r) = (0f32, 0f32);
            for _ in 0..n {
                let (a, b) = self.psg.step();
                l += a;
                r += b;
            }
            let k = n.max(1) as f32;
            let (mut l, mut r) = (l / k, r / k);
            // Remove the DC the unipolar-looking output carries.
            self.dc[0] += (l - self.dc[0]) * 0.0015;
            self.dc[1] += (r - self.dc[1]) * 0.0015;
            l -= self.dc[0];
            r -= self.dc[1];
            self.pending.push((l.clamp(-1.0, 1.0) * 32767.0) as i16);
            self.pending.push((r.clamp(-1.0, 1.0) * 32767.0) as i16);
            cycle += cycles_per_out;
        }
        for &(_, port, v) in &writes[wi..] {
            self.apply(port, v);
        }
    }
}

impl Engine for SgcEngine {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn render(&mut self, out: &mut [i16]) -> Result<()> {
        let mut done = 0;
        while done < out.len() {
            if self.pending_pos >= self.pending.len() {
                self.frame();
            }
            let n = (self.pending.len() - self.pending_pos).min(out.len() - done);
            out[done..done + n]
                .copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + n]);
            self.pending_pos += n;
            done += n;
        }
        Ok(())
    }
    fn restart(&mut self) -> Result<()> {
        self.reset()
    }
}

pub fn open(loc: &Location, sub: Option<u32>, opts: &DecodeOptions) -> Result<Timed<SgcEngine>> {
    let data = loc.read()?.to_vec();
    let h = parse(&data)?;
    let song = sub.map(|s| s as u8).unwrap_or(h.first_song);
    let engine = SgcEngine::new(data, song, opts.sample_rate)?;
    Ok(Timed::new(engine, opts.default_length_ms, opts.fade_ms))
}

/// Runs song `sub` for `ms` without making sound, handing `f` each PSG
/// and FM unit write: (ms into the song, port, value). Returns the
/// system's name.
pub(crate) fn capture(
    loc: &Location,
    sub: Option<u32>,
    ms: u64,
    mut f: impl FnMut(f64, u8, u8),
) -> Result<&'static str> {
    let data = loc.read()?.to_vec();
    let h = parse(&data)?;
    let song = sub.map(|s| s as u8).unwrap_or(h.first_song);
    let mut e = SgcEngine::new(data, song, 44_100)?;
    let system = if e.h.system == 1 {
        "Sega Game Gear"
    } else {
        "Sega Master System"
    };
    let frame_ms = 1000.0 / if e.h.pal { 50.0 } else { 60.0 };
    for (_, port, v) in std::mem::take(&mut e.init_writes) {
        f(0.0, port, v);
    }
    let frames = (ms as f64 / frame_ms).ceil() as u64;
    for i in 0..frames {
        e.call(e.h.play, e.frame_cycles);
        let t0 = i as f64 * frame_ms;
        for (cycle, port, v) in std::mem::take(&mut e.bus.writes) {
            f(t0 + cycle as f64 * 1000.0 / e.clock as f64, port, v);
        }
    }
    Ok(system)
}

pub fn track_list(loc: &Location, opts: &DecodeOptions) -> Result<Vec<SubTrack>> {
    let h = parse(&loc.read()?)?;
    Ok((0..h.songs as u32)
        .map(|i| SubTrack {
            index: i,
            title: String::new(),
            length_ms: Some(opts.default_length_ms + opts.fade_ms),
        })
        .collect())
}

pub fn meta(loc: &Location, sub: Option<u32>, opts: &DecodeOptions) -> Result<TrackMeta> {
    let h = parse(&loc.read()?)?;
    Ok(TrackMeta {
        album: h.game,
        artist: h.author,
        year: super::gme::year_in(&h.copyright).unwrap_or_default(),
        copyright: h.copyright,
        track_no: (h.songs > 1).then(|| sub.unwrap_or(h.first_song as u32) + 1),
        length_ms: Some(opts.default_length_ms + opts.fade_ms),
        system: match h.system {
            1 => "Sega Game Gear",
            2 => "ColecoVision",
            _ => "Sega Master System",
        }
        .into(),
        format: "SGC".into(),
        channels: Some(2),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An SGC whose play routine sets a tone on channel 0 at full volume.
    fn tone_sgc() -> Vec<u8> {
        let mut h = vec![0u8; HEADER];
        h[..4].copy_from_slice(b"SGC\x1a");
        h[4] = 1;
        h[0x08..0x0a].copy_from_slice(&0x0400u16.to_le_bytes()); // load
        h[0x0a..0x0c].copy_from_slice(&0x0400u16.to_le_bytes()); // init
        h[0x0c..0x0e].copy_from_slice(&0x0410u16.to_le_bytes()); // play
        h[0x0e..0x10].copy_from_slice(&0xdff0u16.to_le_bytes()); // stack
        h[0x1f] = 0;
        h[0x20] = 1;
        h[0x21] = 2;
        h[0x23] = 3; // songs
        h[0x26] = 1; // Game Gear
        h[0x40..0x44].copy_from_slice(b"Game");
        // init: ret
        let mut code = vec![0xc9];
        code.resize(0x10, 0);
        // play: ld a,0x8e / out (0x7f),a / ld a,0x0f / out (0x7f),a /
        //       ld a,0x90 / out (0x7f),a / ret   (tone 0xfe, volume 0)
        code.extend([
            0x3e, 0x8e, 0xd3, 0x7f, 0x3e, 0x0f, 0xd3, 0x7f, 0x3e, 0x90, 0xd3, 0x7f, 0xc9,
        ]);
        h.extend(code);
        h
    }

    #[test]
    fn finds_its_notes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.sgc");
        std::fs::write(&p, tone_sgc()).unwrap();
        let t = crate::notes::transcribe(&Location::Fs(p), None, Some(2000)).unwrap();
        assert_eq!(t.system, "Sega Game Gear");
        // One held A4 (the play routine rewrites it every frame).
        assert_eq!(t.parts.len(), 1);
        let n = &t.parts[0].notes;
        assert_eq!(n.len(), 1, "{n:?}");
        assert_eq!(n[0].key, 69);
    }

    #[test]
    fn plays_a_tone() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.sgc");
        std::fs::write(&p, tone_sgc()).unwrap();
        let loc = Location::Fs(p);
        let opts = DecodeOptions::default();
        assert_eq!(track_list(&loc, &opts).unwrap().len(), 3);
        let m = meta(&loc, Some(1), &opts).unwrap();
        assert_eq!(
            (m.album.as_str(), m.system.as_str()),
            ("Game", "Sega Game Gear")
        );
        let mut e =
            SgcEngine::new(std::fs::read(dir.path().join("t.sgc")).unwrap(), 0, 48_000).unwrap();
        let mut buf = vec![0i16; 48_000 * 2 / 4];
        e.render(&mut buf).unwrap();
        // A square wave of clock / (32 × 0xfe) ≈ 440 Hz: count its cycles.
        let tail = &buf[buf.len() / 2..];
        let crossings = tail
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| w[0] < 0 && w[1] >= 0)
            .count();
        let hz = crossings as f64 / (tail.len() / 2) as f64 * 48_000.0;
        assert!((hz - 440.0).abs() < 25.0, "{hz} Hz");
        let peak = tail.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 4000, "too quiet: {peak}");
    }
}
