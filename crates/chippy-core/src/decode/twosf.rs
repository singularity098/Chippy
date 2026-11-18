//! Nintendo DS 2SF / mini2SF through vio2sf. Program sections are
//! [offset, size, data] laid into a ROM image; the reserved area may hold
//! "SAVE" blocks (compressed [offset, size, data]) making a save state.

use anyhow::{bail, Result};
use std::io::Read;

use super::timed::{Engine, Timed};
use super::DecodeOptions;
use crate::meta::TrackMeta;
use crate::psf;
use crate::vfs::Location;
use twosf_sys as ffi;

const VERSION: u8 = 0x24;

struct Image {
    rom: Vec<u8>,
    state: Vec<u8>,
    tags: psf::Psf,
}

fn le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// Lays an [offset, size, data] block into `image`.
fn map(image: &mut Vec<u8>, block: &[u8], pow2: bool) {
    if block.len() < 8 {
        return;
    }
    let (offset, size) = (le(block, 0) as usize, le(block, 4) as usize);
    let size = size.min(block.len() - 8);
    psf::place(image, offset, &block[8..8 + size], pow2);
}

fn image(loc: &Location) -> Result<Image> {
    let (mut rom, mut state) = (Vec::new(), Vec::new());
    let tags = psf::load_chain(loc, VERSION, &mut |p| {
        map(&mut rom, &p.program_data()?, true);
        let r = &p.reserved;
        let mut pos = 0;
        while pos + 12 <= r.len() {
            let size = le(r, pos + 4) as usize;
            if pos + 12 + size > r.len() {
                break;
            }
            if &r[pos..pos + 4] == b"SAVE" {
                let mut block = Vec::new();
                flate2::read::ZlibDecoder::new(&r[pos + 12..pos + 12 + size])
                    .read_to_end(&mut block)?;
                map(&mut state, &block, false);
            }
            pos += 12 + size;
        }
        Ok(())
    })?;
    if rom.is_empty() && state.is_empty() {
        bail!("the 2SF holds no program");
    }
    Ok(Image { rom, state, tags })
}

fn tag_num(p: &psf::Psf, key: &str) -> i32 {
    p.tag(key).and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

pub struct TwosfEngine {
    s: *mut ffi::NDS_state,
    img: Image,
}

// The emulator is owned by this value alone.
unsafe impl Send for TwosfEngine {}

impl TwosfEngine {
    fn start(img: &Image) -> Result<*mut ffi::NDS_state> {
        let t = &img.tags;
        let clockdown = tag_num(t, "_clockdown");
        let or_clockdown = |v: i32| if v == 0 { clockdown } else { v };
        let s = unsafe {
            ffi::twosf_open(
                img.rom.as_ptr(),
                img.rom.len() as u32,
                img.state.as_ptr(),
                img.state.len() as u32,
                tag_num(t, "_frames"),
                tag_num(t, "_vio2sf_sync_type"),
                or_clockdown(tag_num(t, "_vio2sf_arm7_clockdown_level")),
                or_clockdown(tag_num(t, "_vio2sf_arm9_clockdown_level")),
                // Sinc interpolation.
                4,
            )
        };
        if s.is_null() {
            bail!("vio2sf could not start this 2SF");
        }
        Ok(s)
    }
}

impl Drop for TwosfEngine {
    fn drop(&mut self) {
        unsafe { ffi::twosf_close(self.s) }
    }
}

impl Engine for TwosfEngine {
    fn rate(&self) -> u32 {
        ffi::RATE
    }
    fn render(&mut self, out: &mut [i16]) -> Result<()> {
        unsafe { ffi::twosf_render(self.s, out.as_mut_ptr(), (out.len() / 2) as u32) };
        Ok(())
    }
    fn restart(&mut self) -> Result<()> {
        let s = Self::start(&self.img)?;
        unsafe { ffi::twosf_close(self.s) };
        self.s = s;
        Ok(())
    }
}

pub fn open(loc: &Location, opts: &DecodeOptions) -> Result<Timed<TwosfEngine>> {
    let img = image(loc)?;
    let (length, fade) = psf::timing(&img.tags, opts);
    let s = TwosfEngine::start(&img)?;
    Ok(Timed::new(TwosfEngine { s, img }, length, fade))
}

pub fn meta(loc: &Location, opts: &DecodeOptions) -> Result<TrackMeta> {
    let p = psf::parse(&loc.read()?)?;
    let format = if loc.extension() == "mini2sf" {
        "mini2SF"
    } else {
        "2SF"
    };
    Ok(psf::meta(&p, opts, "Nintendo DS", format))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::testing::{hex, pitch, psf};
    use crate::decode::Decoder;

    /// An NDS ROM: the ARM9 loops; the ARM7 starts PSG channel 8 at about
    /// 440 Hz and loops.
    fn rom() -> Vec<u8> {
        let (a9, a7) = (
            hex("feffffea"),
            hex(
                "0103a0e3050c80e30219a0e37f1081e3b010c0e10103a0e3120d80e3ed1ca0e3\
                 681081e3b810c0e1e314a0e3011581e37f1081e3001080e5feffffea",
            ),
        );
        let mut r = vec![0u8; 0x1000];
        let mut put = |o: usize, v: u32| r[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(0x20, 0x200);
        put(0x24, 0x0200_0000);
        put(0x28, 0x0200_0000);
        put(0x2c, a9.len() as u32);
        put(0x30, 0x400);
        put(0x34, 0x0238_0000);
        put(0x38, 0x0238_0000);
        put(0x3c, a7.len() as u32);
        r[0x200..0x200 + a9.len()].copy_from_slice(&a9);
        r[0x400..0x400 + a7.len()].copy_from_slice(&a7);
        r
    }

    #[test]
    fn plays_a_tone() {
        let dir = tempfile::tempdir().unwrap();
        let rom = rom();
        let mut prog = 0u32.to_le_bytes().to_vec();
        prog.extend((rom.len() as u32).to_le_bytes());
        prog.extend(&rom);
        let p = dir.path().join("beep.2sf");
        std::fs::write(&p, psf(VERSION, &prog, "title=DS\nlength=0:01\nfade=0\n")).unwrap();
        let loc = Location::Fs(p);
        let opts = DecodeOptions::default();
        assert_eq!(meta(&loc, &opts).unwrap().system, "Nintendo DS");
        let mut d = open(&loc, &opts).unwrap();
        let mut buf = vec![0f32; 44_100];
        let n = d.read(&mut buf).unwrap();
        let hz = pitch(&buf[n..n * 2], ffi::RATE);
        assert!((hz - 440.0).abs() < 15.0, "{hz} Hz");
    }
}
