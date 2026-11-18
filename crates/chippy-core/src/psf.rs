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
    /// The program section, zlib-compressed.
    pub program: Vec<u8>,
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
        program: data[rend..pend].to_vec(),
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

impl Psf {
    /// The program section, inflated.
    pub fn program_data(&self) -> Result<Vec<u8>> {
        use std::io::Read;
        let mut out = Vec::new();
        if !self.program.is_empty() {
            flate2::read::ZlibDecoder::new(&self.program[..])
                .read_to_end(&mut out)
                .map_err(|e| anyhow::anyhow!("bad PSF program data: {e}"))?;
        }
        Ok(out)
    }
}

/// Loads a PSF-family file with its libraries in the order players use:
/// `_lib` (and its own libraries) first, then the file, then `_lib2`,
/// `_lib3`... `visit` gets each file in that order. Returns the file's own
/// tags.
pub fn load_chain(
    loc: &crate::vfs::Location,
    version: u8,
    visit: &mut dyn FnMut(&Psf) -> Result<()>,
) -> Result<Psf> {
    fn go(
        loc: &crate::vfs::Location,
        version: u8,
        depth: usize,
        visit: &mut dyn FnMut(&Psf) -> Result<()>,
    ) -> Result<Psf> {
        if depth > 10 {
            bail!("PSF libraries nest too deep");
        }
        let p = parse(&loc.read()?)?;
        if p.version != version {
            bail!(
                "unexpected PSF version {:#x} (wanted {version:#x})",
                p.version
            );
        }
        let lib = |key: &str| -> Result<Option<crate::vfs::Location>> {
            match p.tag(key) {
                Some(name) if !name.is_empty() => loc
                    .sibling(name)
                    .map(Some)
                    .ok_or_else(|| anyhow::anyhow!("missing library {name}")),
                _ => Ok(None),
            }
        };
        if let Some(l) = lib("_lib")? {
            go(&l, version, depth + 1, visit)?;
        }
        visit(&p)?;
        for n in 2..=9 {
            match lib(&format!("_lib{n}"))? {
                Some(l) => {
                    go(&l, version, depth + 1, visit)?;
                }
                None => break,
            }
        }
        Ok(p)
    }
    go(loc, version, 0, visit)
}

/// Copies `data` to `offset` in `image`, growing it (to a power of two when
/// `pow2`) as needed.
pub fn place(image: &mut Vec<u8>, offset: usize, data: &[u8], pow2: bool) {
    let end = offset + data.len();
    if image.len() < end {
        image.resize(if pow2 { end.next_power_of_two() } else { end }, 0);
    }
    image[offset..end].copy_from_slice(data);
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
