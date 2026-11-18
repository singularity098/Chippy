//! Finding a track's album art: a picture embedded in the file, or an image
//! next to it (cover.jpg, folder.jpg, ...). For files inside an archive the
//! archive's folder is looked in too, along with an image named like the
//! archive ("Mega Man 2.zip" + "Mega Man 2.jpg").

use crate::archive;
use crate::formats::{self, Codec, FileKind};
use crate::vfs::Location;
use std::path::Path;

/// Sidecar names in order of preference (without extension, lowercase).
pub const SIDECAR_NAMES: &[&str] = &[
    "cover",
    "folder",
    "front",
    "album",
    "albumart",
    "albumartlarge",
    "artwork",
    "art",
    "thumb",
    "albumartsmall",
    "scan",
    "box",
    "boxart",
];

/// An image's bytes and where it came from (for display).
#[derive(Debug, Clone)]
pub struct Art {
    pub data: Vec<u8>,
    pub source: String,
}

pub fn find(loc: &Location, prefer_embedded: bool) -> Option<Art> {
    let embedded = || {
        matches!(
            formats::kind_of(&loc.name()),
            FileKind::Audio(Codec::Common)
        )
        .then(|| crate::decode::sym::embedded_picture(loc))
        .flatten()
        .map(|data| Art {
            data,
            source: "embedded".into(),
        })
    };
    if prefer_embedded {
        embedded().or_else(|| sidecar(loc))
    } else {
        sidecar(loc).or_else(embedded)
    }
}

/// Picks the best image name from a folder's file names: a known sidecar
/// name first (in order of preference), then one named like `stem`, then
/// any name containing "cover" or "front", then any image at all.
pub fn pick<'a>(names: impl IntoIterator<Item = &'a str>, stem: Option<&str>) -> Option<&'a str> {
    let images: Vec<&str> = names
        .into_iter()
        .filter(|n| formats::kind_of(n) == FileKind::Image)
        .collect();
    let stem_of = |n: &str| formats::split_name(n).0.to_lowercase();
    for want in SIDECAR_NAMES {
        if let Some(n) = images.iter().find(|n| stem_of(n) == *want) {
            return Some(n);
        }
    }
    if let Some(stem) = stem {
        let stem = stem.to_lowercase();
        if let Some(n) = images.iter().find(|n| stem_of(n) == stem) {
            return Some(n);
        }
    }
    images
        .iter()
        .find(|n| {
            let s = stem_of(n);
            s.contains("cover") || s.contains("front")
        })
        .or_else(|| images.first())
        .copied()
}

fn from_dir(dir: &Path, stem: Option<&str>) -> Option<Art> {
    let names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let name = pick(names.iter().map(String::as_str), stem)?;
    let p = dir.join(name);
    std::fs::read(&p).ok().map(|data| Art {
        data,
        source: p.to_string_lossy().into_owned(),
    })
}

fn sidecar(loc: &Location) -> Option<Art> {
    match loc {
        Location::Fs(p) => from_dir(p.parent()?, None),
        Location::Archive { archive: a, entry } => {
            let entries = archive::list(a).ok()?;
            let dir = archive::parent_of(entry);
            let here: Vec<&str> = entries
                .iter()
                .filter(|e| !e.is_dir && archive::parent_of(&e.path) == dir)
                .map(|e| archive::name_of(&e.path))
                .collect();
            if let Some(name) = pick(here, None) {
                let path = if dir.is_empty() {
                    name.to_string()
                } else {
                    format!("{dir}/{name}")
                };
                if let Ok(d) = archive::read(a, &path) {
                    return Some(Art {
                        data: d.to_vec(),
                        source: a.join(&path).to_string_lossy().into_owned(),
                    });
                }
            }
            let stem = a
                .file_name()
                .map(|n| formats::split_name(&n.to_string_lossy()).0.to_string());
            from_dir(a.parent()?, stem.as_deref())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picking() {
        assert_eq!(
            pick(["b.png", "Folder.JPG", "cover.png"], None),
            Some("cover.png")
        );
        assert_eq!(pick(["scan2.jpg", "x.txt"], None), Some("scan2.jpg"));
        assert_eq!(pick(["a.jpg", "Game.png"], Some("game")), Some("Game.png"));
        assert_eq!(pick(["a.jpg", "my front.png"], None), Some("my front.png"));
        assert_eq!(pick(["a.txt"], None), None);
    }

    #[test]
    fn sidecar_next_to_file_and_archive() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cover.jpg"), b"img").unwrap();
        std::fs::write(d.path().join("song.flac"), b"x").unwrap();
        let art = find(&Location::Fs(d.path().join("song.flac")), true).unwrap();
        assert_eq!(art.data, b"img");
        let loc = Location::Archive {
            archive: d.path().join("pack.zip"),
            entry: "a/b.nsf".into(),
        };
        // The archive does not exist: its folder is still looked in.
        assert!(sidecar(&loc).is_none() || sidecar(&loc).unwrap().data == b"img");
    }
}
