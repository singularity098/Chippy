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

/// What [`capture`] tells of the running GBA.
pub(crate) trait Watcher {
    /// A write to the sound registers (the I/O offset, $60-$89), of
    /// `bytes` bytes, `ms` into the tune.
    fn write(&mut self, ms: f64, address: u32, value: u32, bytes: i32);
    /// A video frame ended, `ms` into the tune; `mem` reads the GBA's
    /// memory.
    fn frame(&mut self, ms: f64, mem: &dyn Memory);
}

/// The GBA's memory, as the CPU sees it.
pub(crate) trait Memory {
    fn read8(&self, address: u32) -> u8;
    fn read32(&self, address: u32) -> u32;
}

struct Bus(*mut ffi::gsf);

impl Memory for Bus {
    fn read8(&self, address: u32) -> u8 {
        unsafe { ffi::gsf_read8(self.0, address) as u8 }
    }
    fn read32(&self, address: u32) -> u32 {
        unsafe { ffi::gsf_read32(self.0, address) }
    }
}

/// The GBA's CPU clock.
const CLOCK_HZ: f64 = 16_777_216.0;

/// Runs the tune (silently) a frame at a time for `ms`, or its tagged
/// length and fade, telling `w` of it. Returns how long was played, ms.
pub(crate) fn capture(loc: &Location, ms: Option<u64>, w: &mut dyn Watcher) -> Result<f64> {
    struct Hooked<'a, 'b> {
        w: &'a mut (dyn Watcher + 'b),
        clock: Clock,
    }
    /// mGBA's 32-bit cycle count, unwrapped (calls come in time order).
    #[derive(Default)]
    struct Clock {
        last: u32,
        total: u64,
    }
    impl Clock {
        fn ms(&mut self, cycles: u64) -> f64 {
            let c = cycles as u32;
            self.total += c.wrapping_sub(self.last) as u64;
            self.last = c;
            self.total as f64 * 1000.0 / CLOCK_HZ
        }
    }
    unsafe extern "C" fn hook(
        user: *mut std::os::raw::c_void,
        cycles: u64,
        address: u32,
        value: u32,
        bytes: std::os::raw::c_int,
    ) {
        let h = unsafe { &mut *(user as *mut Hooked) };
        let ms = h.clock.ms(cycles);
        h.w.write(ms, address, value, bytes);
    }

    let (rom, p) = rom(loc)?;
    let opts = DecodeOptions::default();
    let ms = ms.unwrap_or_else(|| {
        let (length, fade) = psf::timing(&p, &opts);
        length + fade
    });
    let engine = GsfEngine {
        g: GsfEngine::start(&rom, 32_768)?,
        rom,
        rate: 32_768,
    };
    let g = engine.g;
    let mut hooked = Hooked {
        w,
        clock: Clock {
            last: unsafe { ffi::gsf_cycles(g) } as u32,
            total: 0,
        },
    };
    unsafe { ffi::gsf_set_notes_hook(g, Some(hook), (&mut hooked as *mut Hooked).cast()) };
    let mut now = 0.0;
    while now < ms as f64 {
        unsafe { ffi::gsf_run_frame(g) };
        now = hooked.clock.ms(unsafe { ffi::gsf_cycles(g) });
        hooked.w.frame(now, &Bus(g));
    }
    unsafe { ffi::gsf_set_notes_hook(g, None, std::ptr::null_mut()) };
    Ok(now)
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
        // Its notes: no m4a engine here, so from the Game Boy channel.
        let t = crate::notes::transcribe(&loc, None, Some(1000)).unwrap();
        let p = crate::notes::part(&t, "Pulse 1");
        assert_eq!(p.notes.iter().map(|n| n.key).collect::<Vec<_>>(), vec![69]);
    }
}
