//! Nintendo 64 USF / miniUSF through lazyusf2. A miniUSF names the shared
//! `.usflib` holding the game's code in its `_lib` tag; libraries load first
//! (deepest first), then the file itself, then `_lib2`, `_lib3`, ...

use anyhow::{anyhow, bail, Result};
use std::ffi::CStr;
use std::os::raw::c_void;

use super::{apply_fade, DecodeOptions, Decoder};
use crate::meta::TrackMeta;
use crate::psf::{self, Psf};
use crate::vfs::Location;
use lazyusf_sys as ffi;

const MAX_DEPTH: usize = 10;

/// The reserved sections to upload, in order.
fn sections(loc: &Location, depth: usize, out: &mut Vec<Vec<u8>>) -> Result<Psf> {
    if depth > MAX_DEPTH {
        bail!("USF libraries nest too deep");
    }
    let data = loc.read()?;
    let p = psf::parse(&data)?;
    if p.version != psf::VERSION_USF {
        bail!("not a USF file (PSF version {:#x})", p.version);
    }
    let lib = |key: &str| -> Result<Option<Location>> {
        match p.tag(key) {
            Some(name) if !name.is_empty() => loc
                .sibling(name)
                .map(Some)
                .ok_or_else(|| anyhow!("missing USF library {name}")),
            _ => Ok(None),
        }
    };
    if let Some(l) = lib("_lib")? {
        sections(&l, depth + 1, out)?;
    }
    out.push(p.reserved.clone());
    for n in 2..=9 {
        match lib(&format!("_lib{n}"))? {
            Some(l) => {
                sections(&l, depth + 1, out)?;
            }
            None => break,
        }
    }
    Ok(p)
}

/// The emulator state: an opaque block lazyusf2 asks to be allocated.
struct State {
    mem: Vec<u64>,
    started: bool,
}

impl State {
    fn ptr(&mut self) -> *mut c_void {
        self.mem.as_mut_ptr().cast()
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if self.started {
            unsafe { ffi::usf_shutdown(self.ptr()) }
        }
    }
}

pub struct UsfDecoder {
    state: State,
    rate: u32,
    /// Frames rendered since the start.
    pos: u64,
    fade_start: u64,
    end: u64,
    duration_ms: u64,
    buf: Vec<i16>,
}

// The state is plain memory owned by this decoder.
unsafe impl Send for UsfDecoder {}

fn tag_flag(p: &Psf, key: &str) -> bool {
    p.tag(key).is_some()
}

impl UsfDecoder {
    pub fn open(loc: &Location, opts: &DecodeOptions) -> Result<Self> {
        let mut secs = Vec::new();
        let p = sections(loc, 0, &mut secs)?;
        let size = unsafe { ffi::usf_get_state_size() };
        let mut state = State {
            mem: vec![0u64; size.div_ceil(8)],
            started: false,
        };
        unsafe {
            ffi::usf_clear(state.ptr());
            ffi::usf_set_compare(state.ptr(), tag_flag(&p, "_enablecompare") as i32);
            ffi::usf_set_fifo_full(state.ptr(), tag_flag(&p, "_enablefifofull") as i32);
            ffi::usf_set_hle_audio(state.ptr(), opts.usf_hle as i32);
        }
        for s in &secs {
            if unsafe { ffi::usf_upload_section(state.ptr(), s.as_ptr(), s.len()) } < 0 {
                bail!("invalid USF data");
            }
        }
        let (length, fade) = timing(&p, opts);
        let rate = opts.sample_rate;
        let f = |ms: u64| ms * rate as u64 / 1000;
        let mut d = UsfDecoder {
            state,
            rate,
            pos: 0,
            fade_start: f(length),
            end: f(length + fade),
            duration_ms: length + fade,
            buf: Vec::new(),
        };
        // Render one block now, so a broken file fails here, not mid-play.
        d.render(None, 0)?;
        Ok(d)
    }

    fn render(&mut self, out: Option<&mut [i16]>, frames: usize) -> Result<()> {
        self.state.started = true;
        let ptr = out.map(|o| o.as_mut_ptr()).unwrap_or(std::ptr::null_mut());
        let err =
            unsafe { ffi::usf_render_resampled(self.state.ptr(), ptr, frames, self.rate as i32) };
        if !err.is_null() {
            let msg = unsafe { CStr::from_ptr(err) }
                .to_string_lossy()
                .into_owned();
            bail!("USF emulation stopped: {msg}");
        }
        Ok(())
    }
}

fn timing(p: &Psf, opts: &DecodeOptions) -> (u64, u64) {
    let length = p
        .tag("length")
        .and_then(psf::parse_time_ms)
        .filter(|&l| l > 0);
    let fade = p.tag("fade").and_then(psf::parse_time_ms);
    match length {
        Some(l) => (l, fade.unwrap_or(opts.fade_ms.min(l / 4))),
        None => (opts.default_length_ms, fade.unwrap_or(opts.fade_ms)),
    }
}

impl Decoder for UsfDecoder {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn duration_ms(&self) -> Option<u64> {
        Some(self.duration_ms)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        if self.pos >= self.end {
            return Ok(0);
        }
        let frames = ((out.len() / 2) as u64).min(self.end - self.pos) as usize;
        let mut buf = std::mem::take(&mut self.buf);
        buf.resize(frames * 2, 0);
        let r = self.render(Some(&mut buf), frames);
        for (o, &s) in out.iter_mut().zip(&buf) {
            *o = s as f32 / 32768.0;
        }
        self.buf = buf;
        r?;
        apply_fade(&mut out[..frames * 2], self.pos, self.fade_start, self.end);
        self.pos += frames as u64;
        Ok(frames)
    }
    fn seek(&mut self, ms: u64) -> Result<()> {
        let target = (ms * self.rate as u64 / 1000).min(self.end);
        if target < self.pos {
            unsafe { ffi::usf_restart(self.state.ptr()) };
            self.pos = 0;
        }
        // Emulate up to the target, throwing the sound away.
        while self.pos < target {
            let n = (target - self.pos).min(self.rate as u64) as usize;
            self.render(None, n)?;
            self.pos += n as u64;
        }
        Ok(())
    }
}

pub fn meta(loc: &Location, opts: &DecodeOptions) -> Result<TrackMeta> {
    let data = loc.read()?;
    let p = psf::parse(&data)?;
    let (length, fade) = timing(&p, opts);
    let t = |k: &str| p.tag(k).unwrap_or("").to_string();
    Ok(TrackMeta {
        title: t("title"),
        artist: t("artist"),
        album: t("game"),
        year: t("year"),
        genre: t("genre"),
        comment: t("comment"),
        copyright: t("copyright"),
        dumper: [t("usfby"), t("psfby")]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default(),
        track_no: p
            .tag("track")
            .and_then(|s| s.split('/').next()?.trim().parse().ok()),
        length_ms: Some(length + fade),
        system: "Nintendo 64".into(),
        format: if loc.extension() == "miniusf" {
            "miniUSF".into()
        } else {
            "USF".into()
        },
        channels: Some(2),
        ..Default::default()
    })
}
