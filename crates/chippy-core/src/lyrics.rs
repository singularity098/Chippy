//! A track's lyrics: from a file beside it (`Song.lrc` or `Song.txt`), its
//! tags, or a karaoke MIDI file's lyric events. LRC lyrics (and karaoke
//! files) are timed, so the line being sung can be followed.

use crate::formats::{self, Codec, FileKind};
use crate::vfs::{Location, TrackRef};

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// When it is sung (timed lyrics only).
    pub ms: Option<u64>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Lyrics {
    /// Where they came from: "Song.lrc", "the file's tags", "karaoke".
    pub source: String,
    /// The lines are timed.
    pub synced: bool,
    pub lines: Vec<Line>,
    /// From LRC tags: [ar:], [ti:], [al:].
    pub artist: String,
    pub title: String,
}

impl Lyrics {
    /// The line being sung at `ms` (timed lyrics), if one has started.
    pub fn line_at(&self, ms: u64) -> Option<usize> {
        if !self.synced {
            return None;
        }
        let n = self
            .lines
            .partition_point(|l| l.ms.is_some_and(|t| t <= ms));
        n.checked_sub(1)
    }

    /// The lyrics as plain text, a line each.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// When a line is sung, as shown beside it: "[1:23]".
pub fn stamp(ms: u64) -> String {
    let s = ms / 1000;
    format!("[{}:{:02}]", s / 60, s % 60)
}

/// A time tag: `mm:ss`, `mm:ss.xx`, `mm:ss:xx` or `hh:mm:ss.xx`, in ms.
fn time_of(tag: &str) -> Option<u64> {
    let tag = tag.trim();
    let parts: Vec<&str> = tag.split(':').collect();
    let (h, m, s) = match parts.as_slice() {
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        // mm:ss:xx (hundredths), or hh:mm:ss.
        [a, b, c] if c.len() == 2 && !c.contains('.') && a.len() <= 2 && b.len() == 2 => {
            let cs: f64 = c.parse().ok()?;
            (0.0, a.parse().ok()?, b.parse::<f64>().ok()? + cs / 100.0)
        }
        [h, m, s] => (h.parse().ok()?, m.parse().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    if m < 0.0 || s < 0.0 || !tag.chars().next()?.is_ascii_digit() {
        return None;
    }
    Some(((h * 3600.0 + m * 60.0 + s) * 1000.0).round() as u64)
}

/// Removes word timings (`<00:12.34>`) from an enhanced LRC line.
fn strip_word_times(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        match rest[i..].find('>') {
            Some(j) if time_of(&rest[i + 1..i + j]).is_some() => {
                out.push_str(&rest[..i]);
                rest = &rest[i + j + 1..];
            }
            _ => {
                out.push_str(&rest[..=i]);
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reads lyrics text: LRC if it has time tags, else plain lines.
pub fn parse(text: &str, source: &str) -> Lyrics {
    let text = text.trim_start_matches('\u{feff}');
    let mut l = Lyrics {
        source: source.to_string(),
        ..Default::default()
    };
    let mut offset: i64 = 0;
    let mut timed: Vec<(i64, String)> = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        let mut rest = line;
        let mut times = Vec::new();
        let mut tagged = false;
        while let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else { break };
            let tag = &r[..end];
            if let Some(t) = time_of(tag) {
                times.push(t as i64);
            } else if let Some((k, v)) = tag.split_once(':') {
                if k.chars().all(|c| c.is_ascii_alphabetic()) {
                    tagged = true;
                    let v = v.trim().to_string();
                    match k.to_ascii_lowercase().as_str() {
                        "ar" => l.artist = v,
                        "ti" => l.title = v,
                        "offset" => offset = v.parse().unwrap_or(0),
                        _ => {}
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
            rest = &r[end + 1..];
        }
        if times.is_empty() {
            if !tagged {
                plain.push(strip_word_times(line));
            }
            continue;
        }
        let words = strip_word_times(rest);
        for t in times {
            timed.push((t, words.clone()));
        }
    }
    if timed.len() >= 2 || (timed.len() == 1 && plain.iter().all(|p| p.is_empty())) {
        l.synced = true;
        // A positive offset shows the lyrics sooner.
        timed.sort_by_key(|t| t.0);
        l.lines = timed
            .into_iter()
            .map(|(t, text)| Line {
                ms: Some((t - offset).max(0) as u64),
                text,
            })
            .collect();
    } else {
        // Trim blank lines at the ends; keep the ones between verses.
        while plain.last().is_some_and(|p| p.is_empty()) {
            plain.pop();
        }
        let start = plain
            .iter()
            .position(|p| !p.is_empty())
            .unwrap_or(plain.len());
        l.lines = plain[start..]
            .iter()
            .map(|t| Line {
                ms: None,
                text: t.clone(),
            })
            .collect();
    }
    l
}

/// Karaoke lyric events as lines: `/` or `\` (or a line break) starts a
/// new line, and each line is timed by its first syllable.
pub fn from_karaoke(events: &[(f64, String)]) -> Lyrics {
    let mut lines: Vec<Line> = Vec::new();
    let mut cur = String::new();
    let mut start: Option<f64> = None;
    let flush = |cur: &mut String, start: &mut Option<f64>, lines: &mut Vec<Line>| {
        let t = cur.trim().to_string();
        if !t.is_empty() {
            lines.push(Line {
                ms: Some(start.unwrap_or(0.0).max(0.0) as u64),
                text: t,
            });
        }
        cur.clear();
        *start = None;
    };
    for (ms, s) in events {
        let mut s = s.as_str();
        if s.starts_with('/') || s.starts_with('\\') {
            flush(&mut cur, &mut start, &mut lines);
            s = &s[1..];
        }
        for (i, part) in s.split(['\r', '\n']).enumerate() {
            if i > 0 {
                flush(&mut cur, &mut start, &mut lines);
            }
            if !part.is_empty() {
                start.get_or_insert(*ms);
                cur.push_str(part);
            }
        }
    }
    flush(&mut cur, &mut start, &mut lines);
    Lyrics {
        source: "karaoke".into(),
        synced: !lines.is_empty(),
        lines,
        ..Default::default()
    }
}

fn read_text(loc: &Location) -> Option<String> {
    let data = loc.read().ok()?;
    Some(match std::str::from_utf8(&data) {
        Ok(s) => s.to_string(),
        // Older files are often Latin-1 / Windows-1252.
        Err(_) => data.iter().map(|&c| c as char).collect(),
    })
}

/// A track's lyrics, if any can be found. Timed lyrics are preferred: an
/// `.lrc` file beside it, then its tags, then (MIDI) karaoke events, then
/// untimed ones from its tags or a `.txt` file beside it.
pub fn find(track: &TrackRef) -> Option<Lyrics> {
    let loc = &track.loc;
    let name = loc.name();
    let (stem, _) = formats::split_name(&name);
    let mut found: Vec<Lyrics> = Vec::new();
    // A multi-track file's own track may have its own: "Game 03.lrc".
    let mut stems = vec![stem.to_string()];
    if let Some(sub) = track.sub {
        stems.insert(0, format!("{stem} {:02}", sub + 1));
        stems.insert(1, format!("{stem} - {:02}", sub + 1));
    }
    for st in &stems {
        if let Some(t) = loc
            .sibling(&format!("{st}.lrc"))
            .and_then(|l| read_text(&l))
        {
            found.push(parse(&t, &format!("{st}.lrc")));
            break;
        }
    }
    if matches!(formats::kind_of(&name), FileKind::Audio(Codec::Common)) {
        if let Some(t) = crate::decode::sym::lyrics(loc) {
            found.push(parse(&t, "the file's tags"));
        }
    }
    if matches!(formats::kind_of(&name), FileKind::Audio(Codec::Midi)) {
        if let Ok(ev) = loc
            .read()
            .map_err(|_| ())
            .and_then(|d| crate::notes::smf::lyrics(&d).map_err(|_| ()))
        {
            let k = from_karaoke(&ev);
            if !k.lines.is_empty() {
                found.push(k);
            }
        }
    }
    for st in &stems {
        if let Some(t) = loc
            .sibling(&format!("{st}.txt"))
            .and_then(|l| read_text(&l))
        {
            found.push(parse(&t, &format!("{st}.txt")));
            break;
        }
    }
    found.retain(|l| !l.lines.is_empty());
    let i = found.iter().position(|l| l.synced).unwrap_or(0);
    (i < found.len()).then(|| found.swap_remove(i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lrc() {
        let l = parse(
            "[ar:Someone]\n[ti:A Song]\n[offset:+500]\n\n[00:12.00]First line\n\
             [00:17.20][01:02.50]Chorus <00:17.50>word\n[00:21.10]Third\n",
            "a.lrc",
        );
        assert!(l.synced);
        assert_eq!((l.artist.as_str(), l.title.as_str()), ("Someone", "A Song"));
        let got: Vec<_> = l
            .lines
            .iter()
            .map(|x| (x.ms.unwrap(), x.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (11_500, "First line"),
                (16_700, "Chorus word"),
                (20_600, "Third"),
                (62_000, "Chorus word")
            ]
        );
        assert_eq!(l.line_at(0), None);
        assert_eq!(l.line_at(17_000), Some(1));
        assert_eq!(l.line_at(1_000_000), Some(3));
    }

    #[test]
    fn reads_plain_text() {
        let l = parse("\n\nVerse one\nline two\n\nVerse two\n\n", "a.txt");
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 4);
        assert_eq!(l.line_at(5000), None);
        // Brackets that aren't time tags are words.
        let l = parse("[Chorus]\nla la", "b.txt");
        assert_eq!(l.lines[0].text, "[Chorus]");
    }

    #[test]
    fn times() {
        assert_eq!(time_of("01:02.5"), Some(62_500));
        assert_eq!(time_of("01:02:50"), Some(62_500));
        assert_eq!(time_of("1:01:02.5"), Some(3_662_500));
        assert_eq!(time_of("ar:x"), None);
    }

    #[test]
    fn karaoke_lines() {
        let ev: Vec<(f64, String)> = [
            (0.0, "\\Hel"),
            (100.0, "lo "),
            (200.0, "world"),
            (900.0, "/Next "),
            (1000.0, "line"),
        ]
        .iter()
        .map(|(t, s)| (*t, s.to_string()))
        .collect();
        let l = from_karaoke(&ev);
        let got: Vec<_> = l
            .lines
            .iter()
            .map(|x| (x.ms.unwrap(), x.text.as_str()))
            .collect();
        assert_eq!(got, vec![(0, "Hello world"), (900, "Next line")]);
    }

    #[test]
    fn finds_a_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("Song.mp3");
        std::fs::write(&song, b"not really").unwrap();
        assert!(find(&TrackRef::file(Location::Fs(song.clone()))).is_none());
        std::fs::write(dir.path().join("Song.txt"), "plain words").unwrap();
        std::fs::write(dir.path().join("song.LRC"), "[00:01.00]timed words").unwrap();
        let l = find(&TrackRef::file(Location::Fs(song))).unwrap();
        assert!(l.synced);
        assert_eq!(l.lines[0].text, "timed words");
    }
}
