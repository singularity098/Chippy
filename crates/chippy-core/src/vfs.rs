//! One path space for folders, archives and multi-track files.
//!
//! A *virtual path* looks like an ordinary path, but may run on through an
//! archive and into a file of several tracks:
//! `/music/nes.zip/Mega Man 2/mm2.nsfe` is the track list of an NSFE inside
//! a folder of a zip. [`resolve`] turns one into a [`Node`] (something that
//! can be listed), and [`list`] lists it.

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::archive;
use crate::decode::{self, DecodeOptions};
use crate::formats::{self, FileKind};

/// Where a file's bytes live.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Location {
    Fs(PathBuf),
    /// A file inside an archive on disk; `entry` is '/'-separated.
    Archive {
        archive: PathBuf,
        entry: String,
    },
}

impl Location {
    pub fn name(&self) -> String {
        match self {
            Location::Fs(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Location::Archive { entry, .. } => archive::name_of(entry).to_string(),
        }
    }

    /// The virtual path of the file itself.
    pub fn vpath(&self) -> PathBuf {
        match self {
            Location::Fs(p) => p.clone(),
            Location::Archive { archive, entry } => archive.join(entry),
        }
    }

    pub fn read(&self) -> Result<Arc<Vec<u8>>> {
        match self {
            Location::Fs(p) => Ok(Arc::new(std::fs::read(p)?)),
            Location::Archive { archive, entry } => archive::read(archive, entry),
        }
    }

    /// The node holding this file (its folder, or its folder in the archive).
    pub fn parent_node(&self) -> Node {
        match self {
            Location::Fs(p) => Node::Dir(
                p.parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("/")),
            ),
            Location::Archive { archive, entry } => Node::ArchiveDir {
                archive: archive.clone(),
                dir: archive::parent_of(entry).to_string(),
            },
        }
    }

    /// Another file named relative to this one's folder (a USF `_lib`, an
    /// M3U next to an NSF, a companion header). Letter case is ignored if
    /// the exact name is not there.
    pub fn sibling(&self, rel: &str) -> Option<Location> {
        let rel = rel.replace('\\', "/");
        match self {
            Location::Fs(p) => {
                let dir = p.parent()?;
                let exact = dir.join(&rel);
                if exact.is_file() {
                    return Some(Location::Fs(exact));
                }
                // Case-insensitive match on the last component.
                let full = dir.join(&rel);
                let (pdir, name) = (
                    full.parent()?,
                    full.file_name()?.to_string_lossy().to_lowercase(),
                );
                std::fs::read_dir(pdir).ok()?.flatten().find_map(|e| {
                    (e.file_name().to_string_lossy().to_lowercase() == name && e.path().is_file())
                        .then(|| Location::Fs(e.path()))
                })
            }
            Location::Archive { archive, entry } => {
                let dir = archive::parent_of(entry);
                let want = if dir.is_empty() {
                    rel
                } else {
                    format!("{dir}/{rel}")
                };
                let entries = archive::list(archive).ok()?;
                archive::find_ci(&entries, &want).map(|entry| Location::Archive {
                    archive: archive.clone(),
                    entry,
                })
            }
        }
    }

    /// The other files in this file's folder (or folder in the archive),
    /// itself included.
    pub fn folder_files(&self) -> Vec<Location> {
        match self {
            Location::Fs(p) => p
                .parent()
                .and_then(|d| std::fs::read_dir(d).ok())
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_file())
                        .map(Location::Fs)
                        .collect()
                })
                .unwrap_or_default(),
            Location::Archive { archive, entry } => archive::list(archive)
                .map(|entries| {
                    archive::children(&entries, archive::parent_of(entry))
                        .into_iter()
                        .filter(|e| !e.is_dir)
                        .map(|e| Location::Archive {
                            archive: archive.clone(),
                            entry: e.path,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// The file's extension, lowercase.
    pub fn extension(&self) -> String {
        formats::extension(&self.name())
    }

    /// The real file on disk, when the file is not inside an archive.
    pub fn fs_path(&self) -> Option<&Path> {
        match self {
            Location::Fs(p) => Some(p),
            _ => None,
        }
    }
}

/// A playable track: a file, or one track of a file holding several.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackRef {
    pub loc: Location,
    /// 0-based track within the file; `None` plays the file's default track.
    pub sub: Option<u32>,
}

impl TrackRef {
    pub fn file(loc: Location) -> Self {
        TrackRef { loc, sub: None }
    }
}

/// Something the file browser can list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Dir(PathBuf),
    /// A folder inside an archive ("" is its top).
    ArchiveDir {
        archive: PathBuf,
        dir: String,
    },
    /// The tracks of a multi-track file.
    Tracks(Location),
}

impl Node {
    pub fn vpath(&self) -> PathBuf {
        match self {
            Node::Dir(p) => p.clone(),
            Node::ArchiveDir { archive, dir } if dir.is_empty() => archive.clone(),
            Node::ArchiveDir { archive, dir } => archive.join(dir),
            Node::Tracks(loc) => loc.vpath(),
        }
    }

    /// The node one level up, if there is one.
    pub fn parent(&self) -> Option<Node> {
        match self {
            Node::Dir(p) => p.parent().map(|p| Node::Dir(p.to_path_buf())),
            Node::ArchiveDir { archive, dir } if dir.is_empty() => {
                archive.parent().map(|p| Node::Dir(p.to_path_buf()))
            }
            Node::ArchiveDir { archive, dir } => Some(Node::ArchiveDir {
                archive: archive.clone(),
                dir: archive::parent_of(dir).to_string(),
            }),
            Node::Tracks(loc) => Some(loc.parent_node()),
        }
    }

    pub fn display_name(&self) -> String {
        let p = self.vpath();
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string_lossy().into_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    Archive,
    /// A music file (which may turn out to hold several tracks).
    Audio,
    /// One track of a multi-track file.
    Track,
    Image,
    Other,
}

impl EntryKind {
    pub fn is_container(self) -> bool {
        matches!(self, EntryKind::Dir | EntryKind::Archive)
    }
    pub fn is_playable(self) -> bool {
        matches!(self, EntryKind::Audio | EntryKind::Track)
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub vpath: PathBuf,
    pub size: u64,
    /// What plays when the entry is opened (music files and tracks).
    pub track: Option<TrackRef>,
    /// Track length, when known (tracks of multi-track files).
    pub length_ms: Option<u64>,
    /// Whether opening it may reveal a list of tracks.
    pub may_hold_tracks: bool,
}

/// Turns a virtual path into the node it names.
pub fn resolve(vpath: &Path) -> Result<Node> {
    if vpath.as_os_str().is_empty() {
        bail!("empty path");
    }
    if vpath.is_dir() {
        return Ok(Node::Dir(vpath.to_path_buf()));
    }
    let mut prefix = vpath.to_path_buf();
    let mut rest: Vec<String> = Vec::new();
    while !prefix.is_file() {
        let name = prefix
            .file_name()
            .ok_or_else(|| anyhow!("{} not found", vpath.display()))?;
        rest.push(name.to_string_lossy().into_owned());
        if !prefix.pop() {
            bail!("{} not found", vpath.display());
        }
    }
    rest.reverse();
    let inner = rest.join("/");
    if archive::archive_kind(&prefix).is_some() {
        if inner.is_empty() {
            return Ok(Node::ArchiveDir {
                archive: prefix,
                dir: String::new(),
            });
        }
        let entries = archive::list(&prefix)?;
        if let Some(e) = entries.iter().find(|e| e.path == inner) {
            return Ok(if e.is_dir {
                Node::ArchiveDir {
                    archive: prefix,
                    dir: inner,
                }
            } else {
                Node::Tracks(Location::Archive {
                    archive: prefix,
                    entry: inner,
                })
            });
        }
        bail!("{} not found in {}", inner, prefix.display());
    }
    if inner.is_empty() {
        Ok(Node::Tracks(Location::Fs(prefix)))
    } else {
        bail!("{} not found", vpath.display())
    }
}

/// Lists a node. Folders and archives come first, then files, in natural
/// order ("Track 2" before "Track 10"). Unless `show_all`, only folders,
/// archives and music are listed (and no hidden files).
pub fn list(node: &Node, show_all: bool, opts: &DecodeOptions) -> Result<Vec<Entry>> {
    let mut out = match node {
        Node::Dir(dir) => list_dir(dir, show_all)?,
        Node::ArchiveDir { archive, dir } => list_archive(archive, dir, show_all)?,
        Node::Tracks(loc) => return list_tracks(loc, opts),
    };
    out.sort_by(|a, b| {
        b.kind
            .is_container()
            .cmp(&a.kind.is_container())
            .then_with(|| natural_cmp(&a.name, &b.name))
    });
    Ok(out)
}

fn file_entry(name: String, vpath: PathBuf, size: u64, loc: Location) -> Entry {
    let (kind, track) = match formats::kind_of(&name) {
        FileKind::Archive(_) => (EntryKind::Archive, None),
        FileKind::Audio(_) => (EntryKind::Audio, Some(TrackRef::file(loc))),
        FileKind::Image => (EntryKind::Image, None),
        FileKind::Other => (EntryKind::Other, None),
    };
    let may_hold_tracks = kind == EntryKind::Audio && formats::may_hold_tracks(&name);
    Entry {
        name,
        kind,
        vpath,
        size,
        track,
        length_ms: None,
        may_hold_tracks,
    }
}

fn shown(e: &Entry, show_all: bool) -> bool {
    show_all
        || (!e.name.starts_with('.')
            && matches!(
                e.kind,
                EntryKind::Dir | EntryKind::Archive | EntryKind::Audio | EntryKind::Track
            ))
}

fn list_dir(dir: &Path, show_all: bool) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        // Follows symlinks.
        let Ok(md) = std::fs::metadata(&path) else {
            continue;
        };
        let entry = if md.is_dir() {
            Entry {
                name,
                kind: EntryKind::Dir,
                vpath: path,
                size: 0,
                track: None,
                length_ms: None,
                may_hold_tracks: false,
            }
        } else {
            file_entry(name, path.clone(), md.len(), Location::Fs(path))
        };
        out.push(entry);
    }
    promote_m3us(&mut out);
    out.retain(|e| shown(e, show_all));
    Ok(out)
}

fn list_archive(archive_path: &Path, dir: &str, show_all: bool) -> Result<Vec<Entry>> {
    let entries = archive::list(archive_path)?;
    let mut out = Vec::new();
    for e in archive::children(&entries, dir) {
        let name = archive::name_of(&e.path).to_string();
        let vpath = archive_path.join(&e.path);
        let entry = if e.is_dir {
            Entry {
                name,
                kind: EntryKind::Dir,
                vpath,
                size: 0,
                track: None,
                length_ms: None,
                may_hold_tracks: false,
            }
        } else {
            let mut en = file_entry(
                name,
                vpath,
                e.size,
                Location::Archive {
                    archive: archive_path.to_path_buf(),
                    entry: e.path.clone(),
                },
            );
            // Archives inside archives are not opened.
            if en.kind == EntryKind::Archive {
                en.kind = EntryKind::Other;
            }
            en
        };
        out.push(entry);
    }
    promote_m3us(&mut out);
    out.retain(|e| shown(e, show_all));
    Ok(out)
}

/// Turns per-song M3Us (`01 Title.m3u` holding `game.wsr::WSR,$03,...`)
/// into tracks that play their song, named like the M3U.
fn promote_m3us(entries: &mut [Entry]) {
    let wsrs: Vec<Location> = entries
        .iter()
        .filter(|e| formats::extension(&e.name) == "wsr")
        .filter_map(|e| e.track.as_ref().map(|t| t.loc.clone()))
        .collect();
    for wsr in wsrs {
        for song in decode::wsr::m3u_songs(&wsr) {
            let Some(e) = entries
                .iter_mut()
                .find(|e| e.kind == EntryKind::Other && e.name == song.m3u_name)
            else {
                continue;
            };
            e.name = formats::split_name(&e.name).0.to_string();
            e.kind = EntryKind::Track;
            e.track = Some(TrackRef {
                loc: wsr.clone(),
                sub: Some(song.index),
            });
            e.length_ms = song.length_ms;
        }
    }
}

fn list_tracks(loc: &Location, opts: &DecodeOptions) -> Result<Vec<Entry>> {
    let tracks = decode::track_list(loc, opts)?;
    let width = tracks.len().to_string().len().max(2);
    Ok(tracks
        .into_iter()
        .map(|t| Entry {
            name: if t.title.is_empty() {
                format!("{:0width$}", t.index + 1)
            } else {
                format!("{:0width$}  {}", t.index + 1, t.title)
            },
            kind: EntryKind::Track,
            vpath: loc.vpath(),
            size: 0,
            track: Some(TrackRef {
                loc: loc.clone(),
                sub: Some(t.index),
            }),
            length_ms: t.length_ms,
            may_hold_tracks: false,
        })
        .collect())
}

/// Compares names the way people count: digit runs by value, the rest
/// ignoring case.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = a.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    a.next();
                }
                let mut nb = String::new();
                while let Some(c) = b.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    b.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| na.len().cmp(&nb.len()));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Whether `name` passes the browser's filter: every word of the filter
/// appears in it, ignoring case.
pub fn matches_filter(name: &str, filter: &str) -> bool {
    let name = name.to_lowercase();
    filter
        .split_whitespace()
        .all(|w| name.contains(&w.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn natural_order() {
        let mut v = vec!["Track 10", "track 2", "Track 1", "Alpha", "track 02b"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            vec!["Alpha", "Track 1", "track 2", "track 02b", "Track 10"]
        );
    }

    #[test]
    fn filter() {
        assert!(matches_filter("Mega Man 2 - Wily.nsfe", "mega wily"));
        assert!(!matches_filter("Mega Man 2", "zelda"));
        assert!(matches_filter("anything", "  "));
    }

    #[test]
    fn resolving_through_archives() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/a.flac"), b"x").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        let z = dir.path().join("pack.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("Game/song.nsf", o).unwrap();
            w.write_all(b"NESM").unwrap();
            w.finish().unwrap();
        }
        assert_eq!(resolve(dir.path()).unwrap(), Node::Dir(dir.path().into()));
        assert_eq!(
            resolve(&z).unwrap(),
            Node::ArchiveDir {
                archive: z.clone(),
                dir: String::new()
            }
        );
        assert_eq!(
            resolve(&z.join("Game")).unwrap(),
            Node::ArchiveDir {
                archive: z.clone(),
                dir: "Game".into()
            }
        );
        assert_eq!(
            resolve(&z.join("Game/song.nsf")).unwrap(),
            Node::Tracks(Location::Archive {
                archive: z.clone(),
                entry: "Game/song.nsf".into()
            })
        );
        assert!(resolve(&z.join("nope")).is_err());
        let up = Node::ArchiveDir {
            archive: z.clone(),
            dir: "Game".into(),
        }
        .parent()
        .unwrap();
        assert_eq!(
            up,
            Node::ArchiveDir {
                archive: z.clone(),
                dir: String::new()
            }
        );
        assert_eq!(up.parent().unwrap(), Node::Dir(dir.path().into()));

        let opts = DecodeOptions::default();
        let names: Vec<_> = list(&Node::Dir(dir.path().into()), false, &opts)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["pack.zip", "sub"]);
        let all = list(&Node::Dir(dir.path().into()), true, &opts).unwrap();
        assert_eq!(all.len(), 3);
        let inside = list(
            &Node::ArchiveDir {
                archive: z.clone(),
                dir: "Game".into(),
            },
            false,
            &opts,
        )
        .unwrap();
        assert_eq!(inside[0].kind, EntryKind::Audio);
        assert!(inside[0].may_hold_tracks);
        let loc = inside[0].track.as_ref().unwrap().loc.clone();
        assert_eq!(&**loc.read().unwrap(), b"NESM");
    }
}
