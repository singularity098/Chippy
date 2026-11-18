//! Commodore 64 SID music (PSID/RSID) through libsidplayfp and reSIDfp.
//! A file with several songs opens like a folder. Lengths come from the
//! HVSC song length database (`DOCUMENTS/Songlengths.md5`) when one is
//! found in a folder above the tune; otherwise the default length is used.

use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::timed::{Engine, Timed};
use super::{DecodeOptions, SubTrack};
use crate::meta::TrackMeta;
use crate::vfs::Location;
use sidplayfp_sys as ffi;

struct Tune {
    p: *mut ffi::csid,
    // libsidplayfp keeps pointers into the tune's data only while loading,
    // but keep it alive with the player anyway.
    _data: Arc<Vec<u8>>,
}

// The player is owned by this value alone.
unsafe impl Send for Tune {}

impl Drop for Tune {
    fn drop(&mut self) {
        unsafe { ffi::csid_close(self.p) }
    }
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    let b = unsafe { CStr::from_ptr(p) }.to_bytes();
    // SID tags are Latin-1 (PETSCII rendered to ASCII, mostly).
    match std::str::from_utf8(b) {
        Ok(s) => s.trim().to_string(),
        Err(_) => b
            .iter()
            .map(|&c| c as char)
            .collect::<String>()
            .trim()
            .to_string(),
    }
}

impl Tune {
    fn open(loc: &Location) -> Result<Tune> {
        let data = loc.read()?;
        let mut err = [0 as c_char; 256];
        let p = unsafe {
            ffi::csid_open(
                data.as_ptr(),
                data.len() as u32,
                err.as_mut_ptr(),
                err.len(),
            )
        };
        if p.is_null() {
            bail!("not a SID tune: {}", cstr(err.as_ptr()));
        }
        Ok(Tune { p, _data: data })
    }

    fn songs(&self) -> u32 {
        unsafe { ffi::csid_songs(self.p) }.max(1)
    }

    fn start(&mut self, song: u32, rate: u32) -> Result<()> {
        let mut err = [0 as c_char; 256];
        let r = unsafe { ffi::csid_start(self.p, song, rate, err.as_mut_ptr(), err.len()) };
        if r != 0 {
            bail!("cannot play the SID tune: {}", cstr(err.as_ptr()));
        }
        Ok(())
    }

    fn md5(&self) -> String {
        let mut out = [0 as c_char; 64];
        unsafe { ffi::csid_md5(self.p, out.as_mut_ptr()) };
        cstr(out.as_ptr()).to_lowercase()
    }
}

// ---------------------------------------------------- song length database

type Lengths = HashMap<String, Vec<u64>>;

fn parse_songlengths(text: &str) -> Lengths {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with([';', '[']) {
            continue;
        }
        let Some((md5, times)) = line.split_once('=') else {
            continue;
        };
        let lens = times
            .split_whitespace()
            .filter_map(|t| {
                // "3:25", "3:25.500", maybe followed by "(G)"-style flags.
                let t = t.split('(').next()?;
                crate::psf::parse_time_ms(t)
            })
            .collect();
        out.insert(md5.trim().to_lowercase(), lens);
    }
    out
}

/// The database found in a folder above `file`, if any (cached).
fn songlengths_for(file: &Path) -> Option<Arc<Lengths>> {
    static DBS: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<Lengths>>>>> = OnceLock::new();
    let dbs = DBS.get_or_init(Default::default);
    let mut dir = file.parent();
    for _ in 0..8 {
        let d = dir?;
        let cached = dbs.lock().unwrap().get(d).cloned();
        if let Some(found) = cached {
            if found.is_some() {
                return found;
            }
        } else {
            let candidates = [
                d.join("DOCUMENTS").join("Songlengths.md5"),
                d.join("Songlengths.md5"),
                d.join("songlengths.md5"),
            ];
            let found = candidates
                .iter()
                .find_map(|c| std::fs::read(c).ok())
                .map(|b| Arc::new(parse_songlengths(&String::from_utf8_lossy(&b))));
            dbs.lock().unwrap().insert(d.to_path_buf(), found.clone());
            if found.is_some() {
                return found;
            }
        }
        dir = d.parent();
    }
    None
}

/// The song's length (ms) from the database, when listed.
fn db_length(loc: &Location, tune: &Tune, song: u32) -> Option<u64> {
    let file = match loc {
        Location::Fs(p) => p.clone(),
        Location::Archive { archive, .. } => archive.clone(),
    };
    let db = songlengths_for(&file)?;
    let lens = db.get(&tune.md5())?;
    lens.get(song.saturating_sub(1) as usize).copied()
}

/// (length, fade) of song `song` (1-based).
fn timing(loc: &Location, tune: &Tune, song: u32, opts: &DecodeOptions) -> (u64, u64) {
    match db_length(loc, tune, song) {
        // Listed lengths are exact: the tune's end, no fade needed beyond
        // a short one against clicks.
        Some(l) if l > 0 => (l, 0),
        _ => (opts.default_length_ms, opts.fade_ms),
    }
}

// --------------------------------------------------------------- playback

pub struct SidEngine {
    tune: Tune,
    song: u32,
    rate: u32,
}

impl Engine for SidEngine {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn render(&mut self, out: &mut [i16]) -> Result<()> {
        let frames = (out.len() / 2) as u32;
        let mut done = 0;
        while done < frames {
            let n = unsafe {
                ffi::csid_play(
                    self.tune.p,
                    out[done as usize * 2..].as_mut_ptr(),
                    frames - done,
                )
            };
            if n == 0 {
                // The emulation stopped: the rest is silence.
                out[done as usize * 2..].fill(0);
                break;
            }
            done += n;
        }
        Ok(())
    }
    fn restart(&mut self) -> Result<()> {
        self.tune.start(self.song, self.rate)
    }
}

fn song_of(tune: &Tune, sub: Option<u32>) -> u32 {
    match sub {
        Some(i) => i + 1,
        None => unsafe { ffi::csid_start_song(tune.p) }.max(1),
    }
}

pub fn open(loc: &Location, sub: Option<u32>, opts: &DecodeOptions) -> Result<Timed<SidEngine>> {
    let mut tune = Tune::open(loc)?;
    let song = song_of(&tune, sub).min(tune.songs());
    let (length, fade) = timing(loc, &tune, song, opts);
    tune.start(song, opts.sample_rate)?;
    Ok(Timed::new(
        SidEngine {
            tune,
            song,
            rate: opts.sample_rate,
        },
        length,
        fade,
    ))
}

pub fn track_list(loc: &Location, opts: &DecodeOptions) -> Result<Vec<SubTrack>> {
    let tune = Tune::open(loc)?;
    let title = cstr(unsafe { ffi::csid_info(tune.p, 0) });
    let n = tune.songs();
    Ok((0..n)
        .map(|i| {
            let (l, f) = timing(loc, &tune, i + 1, opts);
            SubTrack {
                index: i,
                title: if n > 1 {
                    format!("{title} ({}/{n})", i + 1)
                } else {
                    title.clone()
                },
                length_ms: Some(l + f),
            }
        })
        .collect())
}

pub fn meta(loc: &Location, sub: Option<u32>, opts: &DecodeOptions) -> Result<TrackMeta> {
    let tune = Tune::open(loc)?;
    let info = |i| cstr(unsafe { ffi::csid_info(tune.p, i) });
    let n = tune.songs();
    let song = song_of(&tune, sub).min(n);
    let (l, f) = timing(loc, &tune, song, opts);
    let released = info(2);
    let model = match unsafe { ffi::csid_sid_model(tune.p) } {
        2 => "MOS 8580",
        _ => "MOS 6581",
    };
    let clock = match unsafe { ffi::csid_clock(tune.p) } {
        2 => "NTSC",
        _ => "PAL",
    };
    let chips = unsafe { ffi::csid_sid_chips(tune.p) };
    let format =
        String::from_utf8_lossy(loc.read()?.get(..4).unwrap_or(b"PSID")).into_owned();
    Ok(TrackMeta {
        title: info(0),
        artist: info(1),
        album: String::new(),
        year: super::gme::year_in(&released).unwrap_or_default(),
        copyright: released,
        track_no: (n > 1).then_some(song),
        length_ms: Some(l + f),
        system: "Commodore 64".into(),
        format,
        comment: format!(
            "{model}{}, {clock}",
            if chips > 1 {
                format!(" ×{chips}")
            } else {
                String::new()
            }
        ),
        channels: Some(2),
        ..Default::default()
    })
    .map_err(|e: anyhow::Error| anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_songlengths() {
        let db = parse_songlengths(
            "[Database]\n; /MUSICIANS/A/Artist/Tune.sid\nABCDEF0123=1:02 0:30.500(G)\n",
        );
        assert_eq!(db.get("abcdef0123"), Some(&vec![62_000, 30_500]));
    }
}
