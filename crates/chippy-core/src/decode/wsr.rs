//! Bandai WonderSwan WSR through Mednafen's WonderSwan core. A WSR holds
//! the game's sound program and no song list: NEZplug-style M3Us beside it
//! (`game.wsr::WSR,$03,Title,1:30,,5`) name, time and list its songs —
//! either one `game.m3u` for them all, or one M3U per song (`01 Title.m3u`),
//! which the file browser also shows as the songs themselves. Without any,
//! the song the file says to start with plays.
//!
//! The core is one global machine, so only one WSR plays at a time.

use anyhow::{bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};

use super::timed::{Engine, Timed};
use super::{DecodeOptions, SubTrack};
use crate::meta::TrackMeta;
use crate::vfs::Location;
use wsr_sys as ffi;

static IN_USE: AtomicBool = AtomicBool::new(false);

/// A song listed in an M3U: (song number, title, length, fade).
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    /// The file the line points at ("game.wsr").
    file: String,
    song: u32,
    title: String,
    length_ms: Option<u64>,
    fade_ms: Option<u64>,
}

/// One song of a WSR as listed in a per-song M3U: the M3U's name, and the
/// song's index in the WSR's track list.
#[derive(Debug, Clone, PartialEq)]
pub struct M3uSong {
    pub m3u_name: String,
    pub index: u32,
    pub length_ms: Option<u64>,
}

/// M3U text: UTF-8 (with or without a BOM), else Windows-1252,
/// which is what most rips were written in.
pub(crate) fn decode_text(d: &[u8]) -> String {
    let d = d.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(d);
    match std::str::from_utf8(d) {
        Ok(s) => s.to_string(),
        Err(_) => d.iter().map(|&b| cp1252(b)).collect(),
    }
}

/// A Windows-1252 byte as a character (Latin-1, but for 0x80..0x9F).
fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9f => HIGH[(b - 0x80) as usize],
        b => b as char,
    }
}

/// Many rips pack "Title - Artist - Game - ©Copyright" into the title field.
struct Packed<'a> {
    title: &'a str,
    artist: &'a str,
    game: &'a str,
    copyright: &'a str,
}

fn unpack_title(t: &str) -> Option<Packed<'_>> {
    let parts: Vec<&str> = t.split(" - ").collect();
    if parts.len() != 4 {
        return None;
    }
    Some(Packed {
        title: parts[0].trim(),
        artist: parts[1].trim(),
        game: parts[2].trim(),
        copyright: parts[3].trim().trim_start_matches(['\u{a9}', ' ']),
    })
}

fn parse_number(s: &str) -> Option<u32> {
    let s = s.trim();
    match s.strip_prefix('$').or_else(|| s.strip_prefix("0x")) {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// Splits on commas, except escaped ones (`\\,`).
fn split_fields(s: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&',') => {
                out.last_mut().unwrap().push(',');
                chars.next();
            }
            ',' => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out
}

fn parse_m3u(text: &str) -> Vec<Listed> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (file, rest) = line.split_once("::")?;
            let fields = split_fields(rest);
            let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
            if !fields.first()?.trim().eq_ignore_ascii_case("wsr") {
                return None;
            }
            let time = |i: usize| {
                fields
                    .get(i)
                    .and_then(|t| crate::psf::parse_time_ms(t))
                    .filter(|&t| t > 0)
            };
            Some(Listed {
                file: file.trim().replace('\\', "/"),
                song: parse_number(fields.get(1)?)?,
                title: fields.get(2).map(|t| t.to_string()).unwrap_or_default(),
                length_ms: time(3),
                fade_ms: time(5).map(|f| f.min(30_000)).or(fields
                    .get(5)
                    .and_then(|f| f.trim().parse::<u64>().ok().map(|s| s * 1000))),
            })
        })
        .collect()
}

/// The songs of a WSR, each with the M3U that listed it.
fn listed_with_sources(loc: &Location) -> Vec<(Listed, String)> {
    let name = loc.name();
    let (stem, _) = crate::formats::split_name(&name);
    if let Some(m) = loc.sibling(&format!("{stem}.m3u")) {
        if let Ok(d) = m.read() {
            let list = parse_m3u(&decode_text(&d));
            if !list.is_empty() {
                let n = m.name();
                return list.into_iter().map(|l| (l, n.clone())).collect();
            }
        }
    }
    // One M3U per song: every M3U in the folder that points at this file,
    // in name order.
    let mut m3us: Vec<Location> = loc
        .folder_files()
        .into_iter()
        .filter(|l| l.extension() == "m3u")
        .collect();
    m3us.sort_by(|a, b| crate::vfs::natural_cmp(&a.name(), &b.name()));
    let mut out = Vec::new();
    for m in m3us {
        let Ok(d) = m.read() else { continue };
        if d.len() > 64 * 1024 {
            continue;
        }
        let mname = m.name();
        for l in parse_m3u(&decode_text(&d)) {
            let target = l.file.rsplit('/').next().unwrap_or(&l.file);
            if target.eq_ignore_ascii_case(&name) {
                out.push((l, mname.clone()));
            }
        }
    }
    out
}

fn listed(loc: &Location) -> Vec<Listed> {
    listed_with_sources(loc)
        .into_iter()
        .map(|(l, _)| l)
        .collect()
}

/// The songs of `wsr` that per-song M3Us list, for the file browser to show
/// those M3Us as tracks. Empty when one M3U lists them all.
pub fn m3u_songs(wsr: &Location) -> Vec<M3uSong> {
    let name = wsr.name();
    let (stem, _) = crate::formats::split_name(&name);
    let whole = format!("{stem}.m3u");
    listed_with_sources(wsr)
        .into_iter()
        .enumerate()
        .filter(|(_, (_, m))| !m.eq_ignore_ascii_case(&whole))
        .map(|(i, (l, m))| M3uSong {
            m3u_name: m,
            index: i as u32,
            length_ms: l.length_ms.map(|t| t + l.fade_ms.unwrap_or(0)),
        })
        .collect()
}

fn first_song(data: &[u8]) -> Result<u32> {
    if data.len() <= 0x20 || &data[data.len() - 0x20..data.len() - 0x1c] != b"WSRF" {
        bail!("not a WSR file");
    }
    Ok(data[data.len() - 0x20 + 5] as u32)
}

pub struct WsrEngine {
    song: u32,
    rate: u32,
}

impl Drop for WsrEngine {
    fn drop(&mut self) {
        unsafe { ffi::wsr_close() };
        IN_USE.store(false, Ordering::Release);
    }
}

impl Engine for WsrEngine {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn render(&mut self, out: &mut [i16]) -> Result<()> {
        unsafe { ffi::wsr_render(out.as_mut_ptr(), (out.len() / 2) as u32) };
        Ok(())
    }
    fn restart(&mut self) -> Result<()> {
        unsafe { ffi::wsr_reset(self.song) };
        Ok(())
    }
}

pub fn open(loc: &Location, sub: Option<u32>, opts: &DecodeOptions) -> Result<Timed<WsrEngine>> {
    let data = loc.read()?;
    let first = first_song(&data)?;
    let list = listed(loc);
    let entry = sub.and_then(|i| list.get(i as usize));
    let song = entry.map(|e| e.song).unwrap_or(first);
    if IN_USE.swap(true, Ordering::AcqRel) {
        bail!("another WonderSwan song is playing");
    }
    if unsafe { ffi::wsr_open(data.as_ptr(), data.len() as u32, opts.sample_rate) } != 0 {
        IN_USE.store(false, Ordering::Release);
        bail!("the WonderSwan core could not load this file");
    }
    let engine = WsrEngine {
        song,
        rate: opts.sample_rate,
    };
    unsafe { ffi::wsr_reset(song) };
    let length = entry
        .and_then(|e| e.length_ms)
        .unwrap_or(opts.default_length_ms);
    let fade = entry.and_then(|e| e.fade_ms).unwrap_or(opts.fade_ms);
    Ok(Timed::new(engine, length, fade))
}

/// The second copy of the core (for notes) is global too: one at a time.
static NOTES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs song `sub` (silently) for `ms`, or its listed length, handing `f`
/// each sound register write: (ms into the song, port, value). Uses the
/// core's second copy, so it can run while a song plays.
pub(crate) fn capture(
    loc: &Location,
    sub: Option<u32>,
    ms: Option<u64>,
    f: &mut dyn FnMut(f64, u32, u8),
) -> Result<()> {
    const RATE: u32 = 24_000;
    /// The V30's clock, per ms.
    const CLOCKS_PER_MS: f64 = 3072.0;
    struct State<'a> {
        clocks: u64,
        f: &'a mut dyn FnMut(f64, u32, u8),
    }
    unsafe extern "C" fn hook(user: *mut std::os::raw::c_void, ts: u32, port: u32, v: u8) {
        let st = unsafe { &mut *(user as *mut State) };
        if port == 0x10000 {
            st.clocks += ts as u64;
        } else {
            (st.f)((st.clocks + ts as u64) as f64 / CLOCKS_PER_MS, port, v);
        }
    }
    /// Unhooks and closes however capturing ends.
    struct Close;
    impl Drop for Close {
        fn drop(&mut self) {
            unsafe {
                ffi::wsrn_set_hook(None, std::ptr::null_mut());
                ffi::wsrn_close();
            }
        }
    }

    let data = loc.read()?;
    let first = first_song(&data)?;
    let list = listed(loc);
    let entry = sub.and_then(|i| list.get(i as usize));
    let song = entry.map(|e| e.song).unwrap_or(first);
    let opts = DecodeOptions::default();
    let ms = ms.unwrap_or_else(|| {
        entry
            .and_then(|e| e.length_ms)
            .unwrap_or(opts.default_length_ms)
            + entry.and_then(|e| e.fade_ms).unwrap_or(opts.fade_ms)
    });
    let _one = NOTES.lock().unwrap_or_else(|e| e.into_inner());
    if unsafe { ffi::wsrn_open(data.as_ptr(), data.len() as u32, RATE) } != 0 {
        bail!("the WonderSwan core could not load this file");
    }
    let _close = Close;
    let mut st = State { clocks: 0, f };
    unsafe {
        ffi::wsrn_set_hook(Some(hook), (&mut st as *mut State).cast());
        ffi::wsrn_reset(song);
    }
    let mut buf = vec![0i16; 2048];
    let total = ms * RATE as u64 / 1000;
    let mut done = 0;
    while done < total {
        unsafe { ffi::wsrn_render(buf.as_mut_ptr(), 1024) };
        done += 1024;
    }
    Ok(())
}

pub fn track_list(loc: &Location, opts: &DecodeOptions) -> Result<Vec<SubTrack>> {
    first_song(&loc.read()?)?;
    let list = listed(loc);
    if list.is_empty() {
        return Ok(vec![SubTrack {
            index: 0,
            title: loc.name(),
            length_ms: Some(opts.default_length_ms + opts.fade_ms),
        }]);
    }
    Ok(list
        .iter()
        .enumerate()
        .map(|(i, e)| SubTrack {
            index: i as u32,
            title: unpack_title(&e.title)
                .map(|p| p.title.to_string())
                .unwrap_or_else(|| e.title.clone()),
            length_ms: Some(
                e.length_ms.unwrap_or(opts.default_length_ms) + e.fade_ms.unwrap_or(opts.fade_ms),
            ),
        })
        .collect())
}

pub fn meta(loc: &Location, sub: Option<u32>, opts: &DecodeOptions) -> Result<TrackMeta> {
    first_song(&loc.read()?)?;
    let list = listed(loc);
    let entry = sub.and_then(|i| list.get(i as usize));
    let packed = entry.and_then(|e| unpack_title(&e.title));
    let copyright = packed
        .as_ref()
        .map(|p| p.copyright.to_string())
        .unwrap_or_default();
    Ok(TrackMeta {
        title: match &packed {
            Some(p) => p.title.to_string(),
            None => entry.map(|e| e.title.clone()).unwrap_or_default(),
        },
        artist: packed
            .as_ref()
            .map(|p| p.artist.to_string())
            .unwrap_or_default(),
        album: packed
            .as_ref()
            .map(|p| p.game.to_string())
            .unwrap_or_default(),
        year: super::gme::year_in(&copyright).unwrap_or_default(),
        copyright,
        track_no: sub.filter(|_| list.len() > 1).map(|i| i + 1),
        length_ms: Some(
            entry
                .and_then(|e| e.length_ms)
                .unwrap_or(opts.default_length_ms)
                + entry.and_then(|e| e.fade_ms).unwrap_or(opts.fade_ms),
        ),
        system: "Bandai WonderSwan".into(),
        format: "WSR".into(),
        channels: Some(2),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::testing::{hex, pitch};
    use crate::decode::Decoder;

    /// A 64 KB WSR whose code fills a square wave table and starts channel
    /// 1 at 3072000 / 32 / (2048 - 0x726) ≈ 440 Hz.
    fn wsr() -> Vec<u8> {
        let mut rom = vec![0u8; 0x10000];
        let code = hex(
            "fa31c08ed88ec08ed0bc0020fcbf0004b90400b8fffff3ab31c0b90400f3abb010e68f\
             b82607e780b0ffe688b001e690b00fe691ebfe",
        );
        rom[..code.len()].copy_from_slice(&code);
        let f = 0x10000 - 0x20;
        rom[f..f + 4].copy_from_slice(b"WSRF");
        rom[f + 5] = 2;
        rom[f + 0x10..f + 0x15].copy_from_slice(&[0xea, 0, 0, 0, 0xf0]);
        rom
    }

    #[test]
    fn plays_a_tone_and_reads_its_m3u() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("game.wsr");
        std::fs::write(&p, wsr()).unwrap();
        std::fs::write(
            dir.path().join("game.m3u"),
            "game.wsr::WSR,$02,Beep,0:01,,0\n",
        )
        .unwrap();
        let loc = Location::Fs(p);
        let opts = DecodeOptions::default();
        let list = track_list(&loc, &opts).unwrap();
        assert_eq!((list.len(), list[0].title.as_str()), (1, "Beep"));
        let mut d = open(&loc, Some(0), &opts).unwrap();
        assert_eq!(d.duration_ms(), Some(1000));
        // The core is global: a second song cannot open meanwhile.
        assert!(open(&loc, None, &opts).is_err());
        // But its notes can be found: that uses a second copy of the core.
        let t = crate::notes::transcribe(&loc, Some(0), Some(1000)).unwrap();
        let w = crate::notes::part(&t, "Wave 1");
        assert_eq!(w.notes.iter().map(|n| n.key).collect::<Vec<_>>(), vec![69]);
        assert!(w.notes[0].start_ms < 20.0, "{:?}", w.notes);
        let mut buf = vec![0f32; 48_000];
        let n = d.read(&mut buf).unwrap();
        let hz = pitch(&buf[n / 2..n * 2], 48_000);
        assert!((hz - 440.0).abs() < 15.0, "{hz} Hz");
        drop(d);
        assert!(open(&loc, None, &opts).is_ok());
    }

    #[test]
    fn lists_songs_from_one_m3u_per_song() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("SWJ-GAME.wsr");
        std::fs::write(&p, wsr()).unwrap();
        // Latin-1 text, as many rips are: \xa9 is the copyright sign.
        std::fs::write(
            dir.path().join("02 Second.m3u"),
            b"SWJ-GAME.wsr::WSR,9,Second - Composer - The Game - \xa92002 Maker,1:00,,5\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("01 First.m3u"),
            "SWJ-GAME.wsr::WSR,$02,First,0:30,,\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("other.m3u"), "x.wsr::WSR,1,Nope\n").unwrap();
        let loc = Location::Fs(p);
        let opts = DecodeOptions::default();
        let list = track_list(&loc, &opts).unwrap();
        let titles: Vec<&str> = list.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["First", "Second"]);
        let m = meta(&loc, Some(1), &opts).unwrap();
        assert_eq!(
            (m.title.as_str(), m.artist.as_str(), m.album.as_str()),
            ("Second", "Composer", "The Game")
        );
        assert_eq!(
            (m.copyright.as_str(), m.year.as_str()),
            ("2002 Maker", "2002")
        );
        assert_eq!(m.length_ms, Some(65_000));
        // The browser shows the M3Us as the songs.
        let node = crate::vfs::Node::Dir(dir.path().to_path_buf());
        let entries = crate::vfs::list(&node, false, &opts).unwrap();
        let songs: Vec<(String, Option<u32>)> = entries
            .iter()
            .filter(|e| e.kind == crate::vfs::EntryKind::Track)
            .map(|e| (e.name.clone(), e.track.as_ref().and_then(|t| t.sub)))
            .collect();
        assert_eq!(
            songs,
            [
                ("01 First".to_string(), Some(0)),
                ("02 Second".to_string(), Some(1))
            ]
        );
    }

    #[test]
    fn reads_nezplug_m3u() {
        let l =
            parse_m3u("# comment\ngame.wsr::WSR,$0A,Title\\, Two,1:30,,5\ngame.wsr::WSR,3,,,\n");
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].song, 10);
        assert_eq!(l[0].title, "Title, Two");
        assert_eq!(l[0].length_ms, Some(90_000));
        assert_eq!(l[1].song, 3);
        assert_eq!(l[1].length_ms, None);
        assert_eq!(
            decode_text(b"Sting\x82 Square \xa9"),
            "Sting\u{201a} Square \u{a9}"
        );
    }
}
