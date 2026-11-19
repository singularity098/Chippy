//! Game Boy Advance GSF / miniGSF through mGBA. Each file's program
//! section is [entry, offset, size, data]; the data of the libraries and
//! the file are laid into one ROM image.

use anyhow::{bail, Result};

use super::timed::{Engine, Timed};
use super::DecodeOptions;
use crate::meta::TrackMeta;
use crate::psf;
use crate::vfs::Location;
use gsf_sys as ffi;

const VERSION: u8 = 0x22;

fn rom(loc: &Location) -> Result<(Vec<u8>, psf::Psf)> {
    let mut image = Vec::new();
    let p = psf::load_chain(loc, VERSION, &mut |p| {
        let prog = p.program_data()?;
        if prog.len() < 12 {
            return Ok(());
        }
        let le = |o: usize| u32::from_le_bytes(prog[o..o + 4].try_into().unwrap());
        let offset = (le(4) & 0x01ff_ffff) as usize;
        let size = (le(8) as usize).min(prog.len() - 12);
        psf::place(&mut image, offset, &prog[12..12 + size], true);
        Ok(())
    })?;
    if image.is_empty() {
        bail!("the GSF holds no program");
    }
    Ok((image, p))
}

pub struct GsfEngine {
    g: *mut ffi::gsf,
    rom: Vec<u8>,
    rate: u32,
}

// The emulator is owned by this value alone.
unsafe impl Send for GsfEngine {}

impl GsfEngine {
    fn start(rom: &[u8], rate: u32) -> Result<*mut ffi::gsf> {
        let g = unsafe { ffi::gsf_open(rom.as_ptr(), rom.len(), rate) };
        if g.is_null() {
            bail!("mGBA could not start this GSF");
        }
        Ok(g)
    }
}

impl Drop for GsfEngine {
    fn drop(&mut self) {
        unsafe { ffi::gsf_close(self.g) }
    }
}

impl Engine for GsfEngine {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn render(&mut self, out: &mut [i16]) -> Result<()> {
        unsafe { ffi::gsf_render(self.g, out.as_mut_ptr(), (out.len() / 2) as u32) };
        Ok(())
    }
    fn restart(&mut self) -> Result<()> {
        let g = Self::start(&self.rom, self.rate)?;
        unsafe { ffi::gsf_close(self.g) };
        self.g = g;
        Ok(())
    }
}

pub fn open(loc: &Location, opts: &DecodeOptions) -> Result<Timed<GsfEngine>> {
    let (rom, p) = rom(loc)?;
    let (length, fade) = psf::timing(&p, opts);
    let rate = opts.sample_rate;
    let g = GsfEngine::start(&rom, rate)?;
    Ok(Timed::new(GsfEngine { g, rom, rate }, length, fade).with_gain(psf::volume(&p)))
}

pub fn meta(loc: &Location, opts: &DecodeOptions) -> Result<TrackMeta> {
    let p = psf::parse(&loc.read()?)?;
    let format = if loc.extension() == "minigsf" {
        "miniGSF"
    } else {
        "GSF"
    };
    Ok(psf::meta(&p, opts, "Game Boy Advance", format))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::testing::{hex, pitch, psf};
    use crate::decode::Decoder;

    /// A GBA ROM whose code starts square channel 1 at 131072/(2048-1750)
    /// ≈ 440 Hz and loops.
    fn rom() -> Vec<u8> {
        let mut r = vec![0u8; 0xc0];
        r[..4].copy_from_slice(&hex("2e0000ea"));
        r[0xb2] = 0x96;
        r.extend(hex(
            "0103a0e38010a0e3b418c0e17710a0e3111c81e3b018c0e10210a0e3b218c0e1\
             0f1aa0e3801081e3b216c0e1861ca0e3d61081e3b416c0e1feffffea",
        ));
        r
    }

    #[test]
    fn plays_a_minigsf_with_its_library() {
        let dir = tempfile::tempdir().unwrap();
        let rom = rom();
        // The library holds the ROM; the mini file only patches a byte of
        // the header (as a real one would patch the song number).
        let mut lib = Vec::new();
        for v in [0x0800_0000u32, 0x0800_0000, rom.len() as u32] {
            lib.extend(v.to_le_bytes());
        }
        lib.extend(&rom);
        std::fs::write(dir.path().join("game.gsflib"), psf(VERSION, &lib, "")).unwrap();
        let mut mini = Vec::new();
        for v in [0x0800_0000u32, 0x0800_00a0, 1] {
            mini.extend(v.to_le_bytes());
        }
        mini.push(b'C');
        std::fs::write(
            dir.path().join("01.minigsf"),
            psf(
                VERSION,
                &mini,
                "_lib=game.gsflib\ntitle=Beep\nlength=0:02\nfade=0\n",
            ),
        )
        .unwrap();
        let loc = Location::Fs(dir.path().join("01.minigsf"));
        let opts = DecodeOptions::default();
        let m = meta(&loc, &opts).unwrap();
        assert_eq!((m.title.as_str(), m.format.as_str()), ("Beep", "miniGSF"));
        let mut d = open(&loc, &opts).unwrap();
        assert_eq!(d.duration_ms(), Some(2000));
        let mut buf = vec![0f32; 48_000];
        let n = d.read(&mut buf).unwrap();
        let hz = pitch(&buf[n..n * 2], 48_000);
        assert!((hz - 440.0).abs() < 15.0, "{hz} Hz");
    }
}
