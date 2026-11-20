//! Paths written for people: the shared storage the browser starts in is
//! "Internal storage", as Android's own Files app calls it, and an SD
//! card's volume is "SD card".

use std::path::{Component, Path, PathBuf};

pub const INTERNAL: &str = "Internal storage";

/// The shared storage's top (`HOME`, set by the engine at start-up).
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.components().count() > 1)
}

/// A path as shown: "Internal storage/Music/NES".
pub fn pretty(p: &Path) -> String {
    crumbs(p)
        .into_iter()
        .map(|(label, _)| label)
        .collect::<Vec<_>>()
        .join("/")
        .replacen("//", "/", 1)
}

/// The levels of a path, for the path bar: (label, path). The shared
/// storage and SD cards are one level each.
pub fn crumbs(p: &Path) -> Vec<(String, PathBuf)> {
    let home = home();
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut acc = PathBuf::new();
    for c in p.components() {
        acc.push(c.as_os_str());
        match c {
            Component::RootDir => out.push(("/".into(), acc.clone())),
            Component::Prefix(pre) => {
                out.push((pre.as_os_str().to_string_lossy().into_owned(), acc.clone()))
            }
            _ => {
                if home.as_deref() == Some(acc.as_path()) {
                    out.clear();
                    out.push((INTERNAL.into(), acc.clone()));
                    continue;
                }
                // /storage/XXXX-XXXX is a removable card.
                let parent = acc.parent().map(Path::to_path_buf);
                if parent.as_deref() == Some(Path::new("/storage"))
                    && acc != Path::new("/storage/emulated")
                    && acc != Path::new("/storage/self")
                {
                    out.clear();
                    out.push(("SD card".into(), acc.clone()));
                    continue;
                }
                out.push((c.as_os_str().to_string_lossy().into_owned(), acc.clone()));
            }
        }
    }
    out
}

/// The last part of a path.
pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Where a search result is, below the folder searched ("" for right
/// there).
pub fn found_in(here: Option<&Path>, vpath: &Path) -> String {
    let parent = vpath.parent().unwrap_or(vpath);
    match here.and_then(|h| parent.strip_prefix(h).ok()) {
        Some(p) if p.as_os_str().is_empty() => String::new(),
        Some(p) => p.to_string_lossy().into_owned(),
        None => pretty(parent),
    }
}

pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_is_named() {
        std::env::set_var("HOME", "/storage/emulated/0");
        assert_eq!(
            pretty(Path::new("/storage/emulated/0/Music/nes.zip")),
            "Internal storage/Music/nes.zip"
        );
        assert_eq!(pretty(Path::new("/storage/emulated/0")), "Internal storage");
        assert_eq!(pretty(Path::new("/storage/1A2B-3C4D/VGM")), "SD card/VGM");
        assert_eq!(pretty(Path::new("/data/x")), "/data/x");
        let c = crumbs(Path::new("/storage/emulated/0/Music"));
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].1, PathBuf::from("/storage/emulated/0/Music"));
        assert_eq!(
            found_in(Some(Path::new("/a")), Path::new("/a/b/c.nsf")),
            "b"
        );
        assert_eq!(found_in(Some(Path::new("/a")), Path::new("/a/c.nsf")), "");
        assert_eq!(human_size(2048), "2.0 KB");
    }
}
