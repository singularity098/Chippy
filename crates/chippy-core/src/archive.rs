//! Reading archives as if they were folders: listing their contents and
//! pulling one file out into memory. Listings and the last few files read
//! are cached, since the file browser lists the same archive repeatedly and
//! a track is read once to probe it and again to play it.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use crate::formats::{kind_of_path, ArchiveKind, FileKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Path inside the archive, '/'-separated, no leading or trailing slash.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

type Stamp = (PathBuf, Option<SystemTime>, u64);

fn stamp(path: &Path) -> Stamp {
    let md = std::fs::metadata(path).ok();
    (
        path.to_path_buf(),
        md.as_ref().and_then(|m| m.modified().ok()),
        md.map(|m| m.len()).unwrap_or(0),
    )
}

fn listings() -> &'static Mutex<HashMap<Stamp, Arc<Vec<ArchiveEntry>>>> {
    static L: OnceLock<Mutex<HashMap<Stamp, Arc<Vec<ArchiveEntry>>>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

const FILE_CACHE: usize = 6;
/// Recently read files: (archive stamp, entry) and the bytes.
type FileCache = Mutex<VecDeque<((Stamp, String), Arc<Vec<u8>>)>>;
fn files() -> &'static FileCache {
    static F: OnceLock<FileCache> = OnceLock::new();
    F.get_or_init(Default::default)
}

pub fn archive_kind(path: &Path) -> Option<ArchiveKind> {
    match kind_of_path(path) {
        FileKind::Archive(k) => Some(k),
        _ => None,
    }
}

/// Normalises a path stored in an archive.
pub fn clean(p: &str) -> String {
    p.replace('\\', "/")
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Every entry of the archive, with folders that are only implied by the
/// files' paths added.
pub fn list(path: &Path) -> Result<Arc<Vec<ArchiveEntry>>> {
    let key = stamp(path);
    if let Some(l) = listings().lock().unwrap().get(&key) {
        return Ok(l.clone());
    }
    let kind = archive_kind(path).ok_or_else(|| anyhow!("not an archive: {}", path.display()))?;
    let mut raw = match kind {
        ArchiveKind::Zip => list_zip(path)?,
        ArchiveKind::SevenZip => list_7z(path)?,
        ArchiveKind::Tar => list_tar(File::open(path)?)?,
        ArchiveKind::TarGz => list_tar(flate2::read::GzDecoder::new(File::open(path)?))?,
    };
    raw.retain(|e| !e.path.is_empty());
    let mut dirs: BTreeSet<String> = raw
        .iter()
        .filter(|e| e.is_dir)
        .map(|e| e.path.clone())
        .collect();
    for e in &raw {
        let mut p = e.path.as_str();
        while let Some(i) = p.rfind('/') {
            p = &p[..i];
            dirs.insert(p.to_string());
        }
    }
    let mut out: Vec<ArchiveEntry> = raw.into_iter().filter(|e| !e.is_dir).collect();
    out.extend(dirs.into_iter().map(|path| ArchiveEntry {
        path,
        is_dir: true,
        size: 0,
    }));
    let out = Arc::new(out);
    let mut l = listings().lock().unwrap();
    if l.len() > 64 {
        l.clear();
    }
    l.insert(key, out.clone());
    Ok(out)
}

/// The direct children of folder `dir` ("" is the archive's top).
pub fn children(entries: &[ArchiveEntry], dir: &str) -> Vec<ArchiveEntry> {
    entries
        .iter()
        .filter(|e| parent_of(&e.path) == dir)
        .cloned()
        .collect()
}

pub fn parent_of(p: &str) -> &str {
    p.rfind('/').map(|i| &p[..i]).unwrap_or("")
}

pub fn name_of(p: &str) -> &str {
    p.rfind('/').map(|i| &p[i + 1..]).unwrap_or(p)
}

/// Reads one file of the archive into memory.
pub fn read(path: &Path, entry: &str) -> Result<Arc<Vec<u8>>> {
    let entry = clean(entry);
    let key = (stamp(path), entry.clone());
    {
        let f = files().lock().unwrap();
        if let Some((_, d)) = f.iter().find(|(k, _)| *k == key) {
            return Ok(d.clone());
        }
    }
    let kind = archive_kind(path).ok_or_else(|| anyhow!("not an archive: {}", path.display()))?;
    let data = match kind {
        ArchiveKind::Zip => read_zip(path, &entry)?,
        ArchiveKind::SevenZip => read_7z(path, &entry)?,
        ArchiveKind::Tar => read_tar(File::open(path)?, &entry)?,
        ArchiveKind::TarGz => read_tar(flate2::read::GzDecoder::new(File::open(path)?), &entry)?,
    };
    let data = Arc::new(data);
    let mut f = files().lock().unwrap();
    f.push_back((key, data.clone()));
    while f.len() > FILE_CACHE {
        f.pop_front();
    }
    Ok(data)
}

/// Looks up an entry ignoring letter case (companion files are often named
/// with different case than referenced).
pub fn find_ci(entries: &[ArchiveEntry], path: &str) -> Option<String> {
    let want = clean(path);
    entries
        .iter()
        .find(|e| !e.is_dir && e.path == want)
        .or_else(|| {
            entries
                .iter()
                .find(|e| !e.is_dir && e.path.eq_ignore_ascii_case(&want))
        })
        .map(|e| e.path.clone())
}

// ------------------------------------------------------------------ zip

fn list_zip(path: &Path) -> Result<Vec<ArchiveEntry>> {
    let mut z = zip::ZipArchive::new(BufReader::new(File::open(path)?)).context("reading zip")?;
    let mut out = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        let f = z.by_index_raw(i)?;
        out.push(ArchiveEntry {
            path: clean(&zip_name(&f)),
            is_dir: f.is_dir(),
            size: f.size(),
        });
    }
    Ok(out)
}

fn zip_name<R: Read>(f: &zip::read::ZipFile<'_, R>) -> String {
    f.name().to_string()
}

fn read_zip(path: &Path, entry: &str) -> Result<Vec<u8>> {
    let mut z = zip::ZipArchive::new(BufReader::new(File::open(path)?))?;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        if clean(&zip_name(&f)) == entry {
            let mut buf = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("{entry} not found in {}", path.display())
}

// ------------------------------------------------------------------- 7z

fn list_7z(path: &Path) -> Result<Vec<ArchiveEntry>> {
    let r = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())
        .map_err(|e| anyhow!("reading 7z: {e}"))?;
    Ok(r.archive()
        .files
        .iter()
        .map(|f| ArchiveEntry {
            path: clean(f.name()),
            is_dir: f.is_directory(),
            size: f.size(),
        })
        .collect())
}

fn read_7z(path: &Path, entry: &str) -> Result<Vec<u8>> {
    let mut r = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())
        .map_err(|e| anyhow!("reading 7z: {e}"))?;
    let mut found = None;
    r.for_each_entries(|e, rd| {
        if clean(e.name()) == entry {
            let mut buf = Vec::with_capacity(e.size() as usize);
            rd.read_to_end(&mut buf)?;
            found = Some(buf);
            return Ok(false);
        }
        // Solid archives need the data before ours read through.
        std::io::copy(rd, &mut std::io::sink())?;
        Ok(true)
    })
    .map_err(|e| anyhow!("reading 7z: {e}"))?;
    found.ok_or_else(|| anyhow!("{entry} not found in {}", path.display()))
}

// ------------------------------------------------------------------ tar

fn list_tar<R: Read>(r: R) -> Result<Vec<ArchiveEntry>> {
    let mut a = tar::Archive::new(r);
    let mut out = Vec::new();
    for e in a.entries()? {
        let e = e?;
        let t = e.header().entry_type();
        if !(t.is_file() || t.is_dir()) {
            continue;
        }
        out.push(ArchiveEntry {
            path: clean(&e.path()?.to_string_lossy()),
            is_dir: t.is_dir(),
            size: e.size(),
        });
    }
    Ok(out)
}

fn read_tar<R: Read>(r: R, entry: &str) -> Result<Vec<u8>> {
    let mut a = tar::Archive::new(r);
    for e in a.entries()? {
        let mut e = e?;
        if e.header().entry_type().is_file() && clean(&e.path()?.to_string_lossy()) == entry {
            let mut buf = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    bail!("{entry} not found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn zip_listing_and_reading() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&p).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("Game/01 Intro.nsf", o).unwrap();
            w.write_all(b"hello").unwrap();
            w.start_file("top.flac", o).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let l = list(&p).unwrap();
        let top: Vec<_> = children(&l, "")
            .into_iter()
            .map(|e| (e.path, e.is_dir))
            .collect();
        assert!(top.contains(&("Game".to_string(), true)));
        assert!(top.contains(&("top.flac".to_string(), false)));
        assert_eq!(children(&l, "Game").len(), 1);
        assert_eq!(&**read(&p, "Game/01 Intro.nsf").unwrap(), b"hello");
        assert_eq!(
            find_ci(&l, "game/01 intro.NSF").as_deref(),
            Some("Game/01 Intro.nsf")
        );
    }

    #[test]
    fn cleaning() {
        assert_eq!(clean("./a\\b//c/"), "a/b/c");
        assert_eq!(parent_of("a/b/c"), "a/b");
        assert_eq!(name_of("a/b/c"), "c");
        assert_eq!(parent_of("c"), "");
    }
}
