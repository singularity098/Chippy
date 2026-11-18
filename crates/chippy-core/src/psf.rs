//! The PSF container, shared by many rip formats (PSF, PSF2, SSF, DSF,
//! QSF, USF, GSF, 2SF...): a header, a "reserved" area, a compressed
//! program area, and a `[TAG]` block of `key=value` lines.

use anyhow::{bail, Result};

use crate::decode::DecodeOptions;
use crate::meta::TrackMeta;

pub const VERSION_USF: u8 = 0x21;

#[derive(Debug, Clone, Default)]
pub struct Psf {
    pub version: u8,
    pub reserved: Vec<u8>,
    /// Tags in file order; keys are lowercase. Multi-line values are joined
    /// with newlines.
    pub tags: Vec<(String, String)>,
}

impl Psf {
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

pub fn parse(data: &[u8]) -> Result<Psf> {
    if data.len() < 16 || &data[0..3] != b"PSF" {
        bail!("not a PSF file");
    }
    let version = data[3];
    let rsize = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let psize = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    let rstart: usize = 16;
    let rend = rstart.checked_add(rsize).filter(|&e| e <= data.len());
    let Some(rend) = rend else {
        bail!("PSF reserved area runs past the end")
    };
    let pend = rend.saturating_add(psize).min(data.len());
    let mut psf = Psf {
        version,
        reserved: data[rstart..rend].to_vec(),
        tags: Vec::new(),
    };
    let rest = &data[pend..];
    if rest.len() >= 5 && &rest[..5] == b"[TAG]" {
        let text = String::from_utf8(rest[5..].to_vec())
            .unwrap_or_else(|_| rest[5..].iter().map(|&b| b as char).collect());
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim().to_lowercase(), v.trim().to_string());
            if k.is_empty() {
                continue;
            }
            match psf.tags.iter_mut().find(|(kk, _)| *kk == k) {
                Some((_, old)) => {
                    old.push('\n');
                    old.push_str(&v);
                }
                None => psf.tags.push((k, v)),
            }
        }
    }
    Ok(psf)
}

/// When a track fades and for how long (ms), from its `length` and `fade`
/// tags, or the defaults for an untimed track.
pub fn timing(p: &Psf, opts: &DecodeOptions) -> (u64, u64) {
    let length = p.tag("length").and_then(parse_time_ms).filter(|&l| l > 0);
    let fade = p.tag("fade").and_then(parse_time_ms);
    match length {
        Some(l) => (l, fade.unwrap_or(opts.fade_ms.min(l / 4))),
        None => (opts.default_length_ms, fade.unwrap_or(opts.fade_ms)),
    }
}

/// The tags of a PSF-family file as track metadata.
pub fn meta(p: &Psf, opts: &DecodeOptions, system: &str, format: &str) -> TrackMeta {
    let (length, fade) = timing(p, opts);
    let t = |k: &str| p.tag(k).unwrap_or("").to_string();
    TrackMeta {
        title: t("title"),
        artist: t("artist"),
        album: t("game"),
        year: t("year"),
        genre: t("genre"),
        comment: t("comment"),
        copyright: t("copyright"),
        dumper: p
            .tags
            .iter()
            .find(|(k, v)| k.ends_with("by") && !v.is_empty())
            .map(|(_, v)| v.clone())
            .unwrap_or_default(),
        track_no: p
            .tag("track")
            .and_then(|s| s.split('/').next()?.trim().parse().ok()),
        length_ms: Some(length + fade),
        system: system.into(),
        format: format.into(),
        channels: Some(2),
        ..Default::default()
    }
}

/// Parses a PSF time: "3:25.5", "1:02:03", "95", "95.25" (seconds).
pub fn parse_time_ms(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let mut total = 0f64;
    for part in s.split(':') {
        let v: f64 = part.trim().parse().ok()?;
        total = total * 60.0 + v;
    }
    (total >= 0.0).then(|| (total * 1000.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(reserved: &[u8], tags: &str) -> Vec<u8> {
        let mut v = b"PSF\x21".to_vec();
        v.extend((reserved.len() as u32).to_le_bytes());
        v.extend(0u32.to_le_bytes());
        v.extend(0u32.to_le_bytes());
        v.extend(reserved);
        v.extend(b"[TAG]");
        v.extend(tags.as_bytes());
        v
    }

    #[test]
    fn parses_tags_and_reserved() {
        let p = parse(&build(
            b"ROMDATA",
            "title=Hello\n_lib=game.usflib\ncomment=a\ncomment=b\nLength=1:02.5\n",
        ))
        .unwrap();
        assert_eq!(p.version, VERSION_USF);
        assert_eq!(p.reserved, b"ROMDATA");
        assert_eq!(p.tag("title"), Some("Hello"));
        assert_eq!(p.tag("_lib"), Some("game.usflib"));
        assert_eq!(p.tag("comment"), Some("a\nb"));
        assert_eq!(p.tag("length").and_then(parse_time_ms), Some(62_500));
    }

    #[test]
    fn times() {
        assert_eq!(parse_time_ms("95"), Some(95_000));
        assert_eq!(parse_time_ms("1:02:03"), Some(3_723_000));
        assert_eq!(parse_time_ms("0:10,5"), Some(10_500));
        assert_eq!(parse_time_ms("x"), None);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"nope").is_err());
    }
}
