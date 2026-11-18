//! The Audio Overload SDK's engines: PlayStation PSF and SPU, PlayStation 2
//! PSF2, Saturn SSF, Dreamcast DSF and Capcom QSound QSF. Mini files name
//! their libraries in `_lib` tags; the engines ask for them by name, and
//! they are read from beside the file (also inside archives).

use anyhow::{bail, Result};
use std::ffi::CString;
use std::os::raw::c_void;

use super::timed::{Engine, Timed};
use super::DecodeOptions;
use crate::meta::TrackMeta;
use crate::psf;
use crate::vfs::Location;
use aosdk_sys as ffi;

pub const RATE: u32 = 44_100;

/// The engine for a file, from its first bytes.
fn kind_of(data: &[u8]) -> Option<u32> {
    if data.len() < 4 {
        return None;
    }
    match (&data[..3], data[3]) {
        (b"PSF", 0x01) => Some(ffi::TYPE_PSF),
        (b"PSF", 0x02) => Some(ffi::TYPE_PSF2),
        (b"PSF", 0x11) => Some(ffi::TYPE_SSF),
        (b"PSF", 0x12) => Some(ffi::TYPE_DSF),
        (b"PSF", 0x41) => Some(ffi::TYPE_QSF),
        (b"SPU", 0x00) => Some(ffi::TYPE_SPU),
        _ => None,
    }
}

/// (system, format) for the tags.
fn names(kind: u32, ext: &str) -> (&'static str, String) {
    let system = match kind {
        ffi::TYPE_PSF | ffi::TYPE_SPU => "Sony PlayStation",
        ffi::TYPE_PSF2 => "Sony PlayStation 2",
        ffi::TYPE_SSF => "Sega Saturn",
        ffi::TYPE_DSF => "Sega Dreamcast",
        _ => "Capcom QSound",
    };
    let format = match ext {
        "minipsf" | "minipsf1" => "miniPSF".into(),
        "minipsf2" => "miniPSF2".into(),
        "minissf" => "miniSSF".into(),
        "minidsf" => "miniDSF".into(),
        "miniqsf" => "miniQSF".into(),
        e => e.to_uppercase(),
    };
    (system, format)
}

pub struct AoEngine {
    kind: u32,
    handle: *mut c_void,
    loc: Location,
    data: Vec<u8>,
}

// The engine state is owned by this value alone.
unsafe impl Send for AoEngine {}

impl AoEngine {
    fn open(loc: &Location) -> Result<Self> {
        let data = loc.read()?.to_vec();
        let Some(kind) = kind_of(&data) else {
            bail!("not a PSF/SSF/DSF/QSF file");
        };
        let mut e = AoEngine {
            kind,
            handle: std::ptr::null_mut(),
            loc: loc.clone(),
            data,
        };
        e.start()?;
        Ok(e)
    }

    fn start(&mut self) -> Result<()> {
        self.stop();
        // Library names are joined to this "path"'s folder: give the bare
        // name, so the engine asks for them by their tag value.
        let path = CString::new("x").unwrap();
        let loc = self.loc.clone();
        let mut data = self.data.clone();
        let handle = ffi::with_lib_reader(
            move |name| {
                let lib = loc.sibling(name)?;
                lib.read().ok().map(|d| d.to_vec())
            },
            || unsafe {
                ffi::ao_start(
                    self.kind,
                    path.as_ptr(),
                    data.as_mut_ptr(),
                    data.len() as u32,
                )
            },
        );
        if handle.is_null() {
            bail!("the emulator could not load this file (a missing library?)");
        }
        self.handle = handle;
        Ok(())
    }

    fn stop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::ao_stop(self.kind, self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

impl Drop for AoEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Engine for AoEngine {
    fn rate(&self) -> u32 {
        RATE
    }
    fn render(&mut self, out: &mut [i16]) -> Result<()> {
        // The engines render in small steps; keep the calls modest.
        for chunk in out.chunks_mut(2048) {
            unsafe {
                ffi::ao_decode(
                    self.kind,
                    self.handle,
                    chunk.as_mut_ptr(),
                    (chunk.len() / 2) as u32,
                )
            };
        }
        Ok(())
    }
    fn restart(&mut self) -> Result<()> {
        self.start()
    }
}

pub fn open(loc: &Location, opts: &DecodeOptions) -> Result<Timed<AoEngine>> {
    let engine = AoEngine::open(loc)?;
    let (length, fade) = match psf::parse(&engine.data) {
        Ok(p) => psf::timing(&p, opts),
        // SPU files have no tags.
        Err(_) => (opts.default_length_ms, opts.fade_ms),
    };
    Ok(Timed::new(engine, length, fade))
}

pub fn meta(loc: &Location, opts: &DecodeOptions) -> Result<TrackMeta> {
    let data = loc.read()?;
    let Some(kind) = kind_of(&data) else {
        bail!("not a PSF/SSF/DSF/QSF file");
    };
    let (system, format) = names(kind, &loc.extension());
    match psf::parse(&data) {
        Ok(p) => Ok(psf::meta(&p, opts, system, &format)),
        Err(_) => Ok(TrackMeta {
            system: system.into(),
            format,
            channels: Some(2),
            length_ms: Some(opts.default_length_ms + opts.fade_ms),
            ..Default::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A PSF (version `v`) holding `program`, with tags.
    fn psf(v: u8, program: &[u8], tags: &str) -> Vec<u8> {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(program).unwrap();
        let z = z.finish().unwrap();
        let mut f = b"PSF".to_vec();
        f.push(v);
        f.extend(0u32.to_le_bytes());
        f.extend((z.len() as u32).to_le_bytes());
        let crc = {
            let mut h = flate2::Crc::new();
            h.update(&z);
            h.sum()
        };
        f.extend(crc.to_le_bytes());
        f.extend(z);
        f.extend(b"[TAG]");
        f.extend(tags.as_bytes());
        f
    }

    /// A PS-X EXE that loops forever at 0x80010000.
    fn exe() -> Vec<u8> {
        let mut e = vec![0u8; 2048];
        e[..8].copy_from_slice(b"PS-X EXE");
        e[0x10..0x14].copy_from_slice(&0x8001_0000u32.to_le_bytes());
        e[0x18..0x1c].copy_from_slice(&0x8001_0000u32.to_le_bytes());
        e[0x1c..0x20].copy_from_slice(&2048u32.to_le_bytes());
        e[0x30..0x34].copy_from_slice(&0x801f_fff0u32.to_le_bytes());
        e.extend_from_slice("Sony Computer Entertainment Inc. for North America area".as_bytes());
        e.resize(2048 + 2048, 0);
        // j 0x80010000 ; nop
        e[2048..2052].copy_from_slice(&0x0800_4000u32.to_le_bytes());
        e
    }

    #[test]
    fn runs_a_minipsf_with_its_library() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("game.psflib"), psf(1, &exe(), "")).unwrap();
        // The mini file's own program overlays the library's.
        std::fs::write(
            dir.path().join("song.minipsf"),
            psf(1, &exe(), "_lib=GAME.psflib\ntitle=Song\ngame=Game\nlength=0:02\nfade=1\n"),
        )
        .unwrap();
        let loc = Location::Fs(dir.path().join("song.minipsf"));
        let opts = DecodeOptions::default();
        let m = meta(&loc, &opts).unwrap();
        assert_eq!(m.title, "Song");
        assert_eq!(m.system, "Sony PlayStation");
        assert_eq!(m.format, "miniPSF");
        assert_eq!(m.length_ms, Some(3000));
        use crate::decode::Decoder;
        let mut d = open(&loc, &opts).unwrap();
        let mut buf = vec![0f32; 2048];
        let mut total = 0;
        loop {
            let n = d.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            total += n;
        }
        assert_eq!(total as u32, 3 * RATE);
        d.seek(500).unwrap();
        assert!(d.read(&mut buf).unwrap() > 0);
        // Without its library it does not load.
        std::fs::remove_file(dir.path().join("game.psflib")).unwrap();
        assert!(open(&loc, &opts).is_err());
    }
}
