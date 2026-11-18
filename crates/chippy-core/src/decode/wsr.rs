//! Bandai WonderSwan WSR through Mednafen's WonderSwan core. A WSR holds
//! the game's sound program and no song list: a NEZplug-style M3U beside
//! it (`game.wsr::WSR,$03,Title,1:30,,5`) names, times and lists its songs;
//! without one, the song the file says to start with plays.
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
struct Listed {
    song: u32,
    title: String,
    length_ms: Option<u64>,
    fade_ms: Option<u64>,
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
            let (_, rest) = line.split_once("::")?;
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

fn listed(loc: &Location) -> Vec<Listed> {
    let name = loc.name();
    let (stem, _) = crate::formats::split_name(&name);
    loc.sibling(&format!("{stem}.m3u"))
        .and_then(|m| m.read().ok())
        .map(|d| parse_m3u(&String::from_utf8_lossy(&d)))
        .unwrap_or_default()
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
            title: e.title.clone(),
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
    Ok(TrackMeta {
        title: entry.map(|e| e.title.clone()).unwrap_or_default(),
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
        let mut buf = vec![0f32; 48_000];
        let n = d.read(&mut buf).unwrap();
        let hz = pitch(&buf[n / 2..n * 2], 48_000);
        assert!((hz - 440.0).abs() < 15.0, "{hz} Hz");
        drop(d);
        assert!(open(&loc, None, &opts).is_ok());
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
    }
}
