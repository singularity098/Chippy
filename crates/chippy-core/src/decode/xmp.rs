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

/// A module's sample, as mono floats.
pub(crate) struct ModSample {
    pub name: String,
    pub pcm: Vec<f32>,
    pub loop_start: Option<usize>,
}

/// What [`scan`] found besides the frames.
pub(crate) struct Scanned {
    /// Indexed by sample number; `None` for empty ones.
    pub samples: Vec<Option<ModSample>>,
    pub instrument_names: Vec<String>,
    pub end_ms: f64,
}

fn latin(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    let b = unsafe { CStr::from_ptr(p) }.to_bytes();
    b.iter().map(|&c| c as char).collect::<String>().trim().to_string()
}

/// Plays a module through once (silently, a frame at a time) up to
/// `max_ms`, handing `f` each frame: (ms, (order position, row), each
/// channel's state). Then gives its samples and instrument names.
pub(crate) fn scan(
    loc: &Location,
    max_ms: Option<u64>,
    mut f: impl FnMut(f64, (i32, i32), &[ffi::Chan]),
) -> Result<Scanned> {
    // The rate only sets how much each frame mixes: low is quicker.
    let ctx = Ctx::open(loc, 8_000)?;
    let mut chans = vec![ffi::Chan::default(); 64];
    let max = max_ms.map_or(f64::INFINITY, |m| m as f64);
    let mut end_ms = 0.0;
    // A song that never ends (some jump back on themselves without
    // libxmp seeing a loop) stops after three hours.
    for _ in 0..(3 * 60 * 60 * 50 * 4) {
        let (mut t, mut pos, mut row) = (0, 0, 0);
        let n = unsafe {
            ffi::cxmp_scan(ctx.c, &mut t, &mut pos, &mut row, chans.as_mut_ptr(), chans.len() as c_int)
        };
        if n < 0 || t as f64 > max {
            break;
        }
        end_ms = t as f64;
        f(t as f64, (pos, row), &chans[..n as usize]);
    }
    let count = unsafe { ffi::cxmp_num_samples(ctx.c) }.max(0);
    let samples = (0..count)
        .map(|i| {
            let (mut name, mut data) = (std::ptr::null(), std::ptr::null());
            let (mut len, mut lps, mut lpe, mut flags) = (0, 0, 0, 0);
            let ok = unsafe {
                ffi::cxmp_sample(ctx.c, i, &mut name, &mut data, &mut len, &mut lps, &mut lpe, &mut flags)
            };
            if ok == 0 || data.is_null() {
                return None;
            }
            let len = len as usize;
            let step = if flags & ffi::SAMPLE_STEREO != 0 { 2 } else { 1 };
            let pcm: Vec<f32> = if flags & ffi::SAMPLE_16BIT != 0 {
                let d = unsafe { std::slice::from_raw_parts(data as *const i16, len * step) };
                d.iter().step_by(step).map(|&s| s as f32 / 32768.0).collect()
            } else {
                let d = unsafe { std::slice::from_raw_parts(data as *const i8, len * step) };
                d.iter().step_by(step).map(|&s| s as f32 / 128.0).collect()
            };
            let looped = flags & ffi::SAMPLE_LOOP != 0 && lpe > lps && (lpe as usize) <= len;
            let mut pcm = pcm;
            if looped {
                pcm.truncate(lpe as usize);
            }
            Some(ModSample {
                name: latin(name),
                pcm,
                loop_start: looped.then_some(lps.max(0) as usize),
            })
        })
        .collect();
    let instrument_names = (0..unsafe { ffi::cxmp_num_instruments(ctx.c) }.max(0))
        .map(|i| latin(unsafe { ffi::cxmp_instrument_name(ctx.c, i) }))
        .collect();
    Ok(Scanned {
        samples,
        instrument_names,
        end_ms,
    })
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
