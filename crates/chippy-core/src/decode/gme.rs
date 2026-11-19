//! Game Music Emu: NSF/NSFE, SPC, VGM/VGZ, GBS, HES, KSS, AY, SAP, GYM.

use anyhow::{anyhow, bail, Result};
use std::ffi::CStr;
use std::io::Read;
use std::os::raw::c_char;
use std::sync::Arc;

use super::{DecodeOptions, Decoder, SubTrack};
use crate::meta::TrackMeta;
use crate::vfs::Location;
use gme_sys as ffi;

struct Emu(*mut ffi::Music_Emu);
// libgme's emulators carry no thread affinity.
unsafe impl Send for Emu {}
impl Drop for Emu {
    fn drop(&mut self) {
        unsafe { ffi::gme_delete(self.0) }
    }
}

fn check(err: ffi::gme_err_t) -> Result<()> {
    if err.is_null() {
        Ok(())
    } else {
        Err(anyhow!(
            "{}",
            unsafe { CStr::from_ptr(err) }.to_string_lossy()
        ))
    }
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        let bytes = unsafe { CStr::from_ptr(p) }.to_bytes();
        match std::str::from_utf8(bytes) {
            Ok(s) => s.trim().to_string(),
            // Older rips (SPC ID666 tags in particular) are Latin-1.
            Err(_) => bytes
                .iter()
                .map(|&c| c as char)
                .collect::<String>()
                .trim()
                .to_string(),
        }
    }
}

/// The file's bytes, inflated if gzip-compressed (VGZ).
fn load(loc: &Location) -> Result<Arc<Vec<u8>>> {
    let data = loc.read()?;
    if data.len() > 2 && data[0] == 0x1f && data[1] == 0x8b {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&data[..]).read_to_end(&mut out)?;
        return Ok(Arc::new(out));
    }
    Ok(data)
}

fn open_emu(loc: &Location, rate: u32) -> Result<Emu> {
    let data = load(loc)?;
    let mut emu = std::ptr::null_mut();
    check(unsafe {
        ffi::gme_open_data(data.as_ptr().cast(), data.len() as _, &mut emu, rate as i32)
    })?;
    if emu.is_null() {
        bail!("not a supported chiptune");
    }
    let emu = Emu(emu);
    // An M3U playlist next to the file names (and orders) its tracks. NSFE
    // carries its own names, lengths and order, which an M3U would replace
    // (loading one turns off the NSFE's playlist).
    let name = loc.name();
    if crate::formats::extension(&name) == "nsfe" {
        return Ok(emu);
    }
    let (stem, _) = crate::formats::split_name(&name);
    if let Some(m3u) = loc.sibling(&format!("{stem}.m3u")) {
        if let Ok(d) = m3u.read() {
            let r = unsafe { ffi::gme_load_m3u_data(emu.0, d.as_ptr().cast(), d.len() as _) };
            if !r.is_null() {
                log::debug!("ignoring {}: {}", m3u.vpath().display(), cstr(r));
            }
        }
    }
    Ok(emu)
}

struct Info {
    length: i32,
    loop_length: i32,
    intro_length: i32,
    fade: i32,
    system: String,
    game: String,
    song: String,
    author: String,
    copyright: String,
    comment: String,
    dumper: String,
}

fn info(emu: &Emu, track: i32) -> Option<Info> {
    let mut p = std::ptr::null_mut();
    let err = unsafe { ffi::gme_track_info(emu.0, &mut p, track) };
    if !err.is_null() || p.is_null() {
        return None;
    }
    let i = unsafe { &*p };
    let out = Info {
        length: i.length,
        loop_length: i.loop_length,
        intro_length: i.intro_length,
        fade: i.fade_length,
        system: cstr(i.system),
        game: cstr(i.game),
        song: cstr(i.song),
        author: cstr(i.author),
        copyright: cstr(i.copyright),
        comment: cstr(i.comment),
        dumper: cstr(i.dumper),
    };
    unsafe { ffi::gme_free_info(p) };
    Some(out)
}

/// When the fade starts and how long it lasts (ms).
fn timing(i: Option<&Info>, opts: &DecodeOptions) -> (u64, u64) {
    let fade = i
        .map(|i| i.fade)
        .filter(|&f| f > 0)
        .map(|f| f as u64)
        .unwrap_or(opts.fade_ms);
    let start = match i {
        Some(i) if i.length > 0 => i.length as u64,
        Some(i) if i.loop_length > 0 => (i.intro_length.max(0) + 2 * i.loop_length) as u64,
        _ => opts.default_length_ms,
    };
    (start, fade)
}

pub struct GmeDecoder {
    emu: Emu,
    track: i32,
    rate: u32,
    /// Where the fade starts, and how long it is.
    start: u64,
    fade: u64,
    duration: u64,
    buf: Vec<i16>,
}

impl GmeDecoder {
    pub fn open(loc: &Location, track: u32, opts: &DecodeOptions) -> Result<Self> {
        let emu = open_emu(loc, opts.sample_rate)?;
        let count = unsafe { ffi::gme_track_count(emu.0) };
        let track = (track as i32).min(count - 1).max(0);
        unsafe {
            ffi::gme_enable_accuracy(emu.0, opts.gme_accuracy as i32);
            ffi::gme_set_autoload_playback_limit(emu.0, 0);
        }
        let i = info(&emu, track);
        let (start, fade) = timing(i.as_ref(), opts);
        let mut d = GmeDecoder {
            emu,
            track,
            rate: opts.sample_rate,
            start,
            fade,
            duration: start + fade,
            buf: Vec::new(),
        };
        d.restart()?;
        Ok(d)
    }

    /// Back to the start of the track. Starting a track forgets its fade,
    /// so it is set again every time (GME's own backward seek does not).
    fn restart(&mut self) -> Result<()> {
        check(unsafe { ffi::gme_start_track(self.emu.0, self.track) })?;
        unsafe { ffi::gme_set_fade_msecs(self.emu.0, self.start as i32, self.fade as i32) };
        Ok(())
    }
}

impl Decoder for GmeDecoder {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn duration_ms(&self) -> Option<u64> {
        Some(self.duration)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        if unsafe { ffi::gme_track_ended(self.emu.0) } != 0 {
            return Ok(0);
        }
        let n = out.len() & !1;
        self.buf.resize(n, 0);
        check(unsafe { ffi::gme_play(self.emu.0, n as i32, self.buf.as_mut_ptr()) })?;
        for (o, &s) in out.iter_mut().zip(&self.buf) {
            *o = s as f32 / 32768.0;
        }
        Ok(n / 2)
    }
    fn seek(&mut self, ms: u64) -> Result<()> {
        while !self.seek_step(ms)? {}
        Ok(())
    }
    fn seek_step(&mut self, ms: u64) -> Result<bool> {
        let ms = ms.min(self.duration);
        // GME counts whole milliseconds, rounding down: never past `ms`.
        if ms < unsafe { ffi::gme_tell(self.emu.0) }.max(0) as u64 {
            self.restart()?;
        }
        let now = unsafe { ffi::gme_tell(self.emu.0) }.max(0) as u64;
        let next = ms.min(now + super::SEEK_STEP_MS);
        check(unsafe { ffi::gme_seek(self.emu.0, next as i32) })?;
        Ok(next == ms)
    }
}

pub fn track_list(loc: &Location, opts: &DecodeOptions) -> Result<Vec<SubTrack>> {
    let emu = open_emu(loc, 44_100)?;
    let count = unsafe { ffi::gme_track_count(emu.0) }.max(0) as u32;
    Ok((0..count)
        .map(|t| {
            let i = info(&emu, t as i32);
            let (start, fade) = timing(i.as_ref(), opts);
            SubTrack {
                index: t,
                title: i.map(|i| i.song).unwrap_or_default(),
                length_ms: Some(start + fade),
            }
        })
        .collect())
}

pub fn meta(loc: &Location, track: u32, opts: &DecodeOptions) -> Result<TrackMeta> {
    let emu = open_emu(loc, 44_100)?;
    let count = unsafe { ffi::gme_track_count(emu.0) }.max(1) as u32;
    let i = info(&emu, track as i32);
    let (start, fade) = timing(i.as_ref(), opts);
    let system = cstr(unsafe { ffi::gme_type_system(ffi::gme_type(emu.0)) });
    let mut m = TrackMeta {
        length_ms: Some(start + fade),
        format: crate::formats::extension(&loc.name()).to_uppercase(),
        channels: Some(2),
        ..Default::default()
    };
    if let Some(i) = i {
        m.title = i.song;
        m.album = i.game;
        m.artist = i.author;
        m.copyright = i.copyright;
        m.comment = i.comment;
        m.dumper = i.dumper;
        m.system = if i.system.is_empty() {
            system
        } else {
            i.system
        };
        if let Some(y) = year_in(&m.copyright) {
            m.year = y;
        }
    } else {
        m.system = system;
    }
    if count > 1 {
        m.track_no = Some(track + 1);
    }
    Ok(m)
}

/// A four-digit year in a copyright line ("1988 Capcom").
pub(crate) fn year_in(s: &str) -> Option<String> {
    // Works on bytes: the line may hold "©" or other non-ASCII text.
    let b = s.as_bytes();
    (0..b.len().saturating_sub(3)).find_map(|i| {
        let w = &b[i..i + 4];
        let ok = w.iter().all(u8::is_ascii_digit)
            && (w.starts_with(b"19") || w.starts_with(b"20"))
            && (i == 0 || !b[i - 1].is_ascii_digit())
            && (i + 4 == b.len() || !b[i + 4].is_ascii_digit());
        ok.then(|| String::from_utf8_lossy(w).into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years() {
        assert_eq!(year_in("1988 Capcom").as_deref(), Some("1988"));
        assert_eq!(year_in("(c) Konami, 2001").as_deref(), Some("2001"));
        assert_eq!(year_in("\u{a9}1991 Rare").as_deref(), Some("1991"));
        assert_eq!(year_in("\u{a9}\u{a9}").as_deref(), None);
        assert_eq!(year_in("12345"), None);
    }

    #[test]
    fn plays_the_bundled_test_nsf() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/game-music-emu/test.nsf");
        let loc = Location::Fs(p);
        let opts = DecodeOptions::default();
        let tracks = track_list(&loc, &opts).unwrap();
        assert!(!tracks.is_empty());
        let mut d = GmeDecoder::open(&loc, 0, &opts).unwrap();
        let mut buf = vec![0f32; 4096];
        let mut energy = 0f32;
        for _ in 0..50 {
            let n = d.read(&mut buf).unwrap();
            assert_eq!(n, 2048);
            energy += buf.iter().map(|s| s * s).sum::<f32>();
        }
        assert!(energy > 0.0, "the NSF played silence");
        d.seek(10_000).unwrap();
        let m = meta(&loc, 0, &opts).unwrap();
        assert!(!m.system.is_empty());
    }
}
