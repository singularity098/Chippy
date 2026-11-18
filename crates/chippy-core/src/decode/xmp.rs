//! Tracker modules through libxmp: MOD, XM, S3M, IT and dozens of rarer
//! Amiga and PC formats. They play once through, to their own end.

use anyhow::{bail, Result};
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

use super::{DecodeOptions, Decoder};
use crate::meta::TrackMeta;
use crate::vfs::Location;
use xmp_sys as ffi;

struct Ctx {
    c: ffi::Context,
    started: bool,
}

// The context is owned by this value alone.
unsafe impl Send for Ctx {}

impl Drop for Ctx {
    fn drop(&mut self) {
        unsafe { ffi::cxmp_close(self.c, self.started as c_int) }
    }
}

impl Ctx {
    /// Loads a module; `rate` > 0 also starts the player at that rate.
    fn open(loc: &Location, rate: u32) -> Result<Ctx> {
        let data = loc.read()?;
        let c = unsafe { ffi::cxmp_open(data.as_ptr().cast(), data.len() as _, rate as c_int) };
        if c.is_null() {
            bail!("not a module libxmp can play");
        }
        Ok(Ctx {
            c,
            started: rate > 0,
        })
    }

    /// (title, format, comment, length ms, channels).
    fn info(&self) -> (String, String, String, u64, u32) {
        let s = |p: *const c_char| -> String {
            if p.is_null() {
                return String::new();
            }
            let b = unsafe { CStr::from_ptr(p) }.to_bytes();
            match std::str::from_utf8(b) {
                Ok(s) => s.trim().to_string(),
                Err(_) => b
                    .iter()
                    .map(|&c| c as char)
                    .collect::<String>()
                    .trim()
                    .to_string(),
            }
        };
        let (mut n, mut t, mut cm) = (std::ptr::null(), std::ptr::null(), std::ptr::null());
        let (mut len, mut chn) = (0, 0);
        unsafe { ffi::cxmp_info(self.c, &mut n, &mut t, &mut cm, &mut len, &mut chn) };
        (s(n), s(t), s(cm), len.max(0) as u64, chn.max(0) as u32)
    }
}

pub struct XmpDecoder {
    ctx: Ctx,
    rate: u32,
    length_ms: u64,
    done: bool,
    buf: Vec<i16>,
}

impl XmpDecoder {
    pub fn open(loc: &Location, opts: &DecodeOptions) -> Result<Self> {
        let ctx = Ctx::open(loc, opts.sample_rate)?;
        let length_ms = ctx.info().3;
        Ok(XmpDecoder {
            ctx,
            rate: opts.sample_rate,
            length_ms,
            done: false,
            buf: Vec::new(),
        })
    }
}

impl Decoder for XmpDecoder {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn duration_ms(&self) -> Option<u64> {
        (self.length_ms > 0).then_some(self.length_ms)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        if self.done {
            return Ok(0);
        }
        let frames = out.len() / 2;
        self.buf.resize(frames * 2, 0);
        if unsafe { ffi::cxmp_render(self.ctx.c, self.buf.as_mut_ptr(), frames as c_int) } == 0 {
            self.done = true;
            return Ok(0);
        }
        for (o, &s) in out.iter_mut().zip(&self.buf) {
            *o = s as f32 / 32768.0;
        }
        Ok(frames)
    }
    fn seek(&mut self, ms: u64) -> Result<()> {
        unsafe { ffi::cxmp_seek(self.ctx.c, ms as c_int) };
        self.done = false;
        Ok(())
    }
}

pub fn meta(loc: &Location) -> Result<TrackMeta> {
    let ctx = Ctx::open(loc, 0)?;
    let (title, format, comment, length, chn) = ctx.info();
    // "Fast Tracker II (XM)" -> "XM" where the format names itself so.
    let short = format
        .rsplit_once('(')
        .and_then(|(_, r)| r.strip_suffix(')'))
        .map(str::to_string)
        .unwrap_or_else(|| loc.extension().to_uppercase());
    Ok(TrackMeta {
        title,
        comment: if comment.is_empty() {
            format!("{format}, {chn} channels")
        } else {
            comment
        },
        format: short,
        length_ms: (length > 0).then_some(length),
        channels: Some(2),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_a_module() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/libxmp/test-dev/data/Gaffeltruck.mod");
        if !p.exists() {
            return;
        }
        let loc = Location::Fs(p);
        let m = meta(&loc).unwrap();
        assert_eq!(m.title, "gaffeltruck");
        let mut d = XmpDecoder::open(&loc, &DecodeOptions::default()).unwrap();
        assert!(d.duration_ms().unwrap() > 60_000);
        let mut buf = vec![0f32; 48_000];
        assert_eq!(d.read(&mut buf).unwrap(), 24_000);
        d.seek(30_000).unwrap();
        assert_eq!(d.read(&mut buf).unwrap(), 24_000);
        assert!(buf.iter().any(|s| s.abs() > 0.01));
    }
}
