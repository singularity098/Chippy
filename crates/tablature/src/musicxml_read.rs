//! Reading MusicXML (`.musicxml`, and `.mxl`, the same zipped): each part's
//! notes through the bars and their repeats, on the strings and frets of a
//! tab staff where it has one (with its tuning and capo), and the tempo as
//! the score marks it.
//!
//! Guitar Pro, MuseScore and Chippy itself write a guitar part as a staff of
//! notes over a staff of tab, the same notes on each: they are read once,
//! keeping the string and fret.

use roxmltree::{Document, Node, ParsingOptions};

use crate::song::{build, err, Measure, Result, Song, Tempo, Track, Written, DRUM_CHANNEL};

/// The velocity MusicXML's dynamics are a percentage of (forte).
const FORTE: f64 = 90.0;

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn children<'a, 'i: 'a>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.has_tag_name(name))
}

fn text(n: Node, name: &str) -> String {
    child(n, name)
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn num(n: Node, name: &str) -> Option<f64> {
    text(n, name).parse().ok()
}

/// A pitch's MIDI key from its step, alteration and octave.
fn key(step: &str, alter: f64, octave: f64) -> Option<u8> {
    let s = match step.trim().to_ascii_uppercase().as_str() {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    };
    let k = (octave + 1.0) * 12.0 + s as f64 + alter.round();
    (0.0..=127.0).contains(&k).then_some(k as u8)
}

/// Endings ("1", "1, 2", "1-3") as bits, 1 the first pass.
fn endings(s: &str) -> u32 {
    let mut bits = 0u32;
    for part in s.split([',', ' ']).filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        if let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
            for n in a.max(1)..=b.min(32) {
                bits |= 1 << (n - 1);
            }
        }
    }
    bits
}

/// Opens an XML document as MusicXML files are written (with a DOCTYPE,
/// maybe a byte-order mark).
fn parse(xml: &str) -> Result<Document<'_>> {
    let opts = ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    Document::parse_with_options(xml.trim_start_matches('\u{feff}'), opts)
        .or_else(|e| err(format!("not valid XML: {e}")))
}

/// Reads a compressed MusicXML file (`.mxl`): a zip with the score in it.
pub fn read_mxl(data: &[u8]) -> Result<Song> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data))
        .or_else(|e| err(format!("not a compressed MusicXML file: {e}")))?;
    let mut entry = |name: &str| -> Option<Vec<u8>> {
        let mut f = zip.by_name(name).ok()?;
        let mut v = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut v).ok()?;
        Some(v)
    };
    // The container says where the score is.
    let root = entry("META-INF/container.xml").and_then(|c| {
        let c = String::from_utf8(c).ok()?;
        let doc = parse(&c).ok()?;
        doc.descendants()
            .find(|n| n.has_tag_name("rootfile"))
            .and_then(|n| n.attribute("full-path"))
            .map(str::to_string)
    });
    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    let name = root
        .filter(|r| names.contains(r))
        .or_else(|| {
            names
                .iter()
                .find(|n| {
                    !n.starts_with("META-INF") && (n.ends_with(".xml") || n.ends_with(".musicxml"))
                })
                .cloned()
        })
        .ok_or_else(|| crate::song::Error("no score in the compressed MusicXML file".into()))?;
    let data = zip_read(&mut zip, &name)?;
    read(&data).map(|mut s| {
        s.format = "MusicXML (compressed)".into();
        s
    })
}

fn zip_read(zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    let mut f = zip
        .by_name(name)
        .or_else(|e| err(format!("can't open {name} in the file: {e}")))?;
    let mut v = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut v)
        .or_else(|e| err(format!("can't read {name}: {e}")))?;
    Ok(v)
}

/// What a part's list entry says about it.
#[derive(Default)]
struct PartInfo {
    name: String,
    program: u8,
    drums: bool,
    /// Instrument ids to the drum key they play.
    unpitched: Vec<(String, u8)>,
}

/// Reads a MusicXML score.
pub fn read(data: &[u8]) -> Result<Song> {
    let xml = String::from_utf8_lossy(data);
    let doc = parse(&xml)?;
    let score = doc.root_element();
    if score.has_tag_name("score-timewise") {
        return err("MusicXML scores written bar by bar (timewise) aren't read yet");
    }
    if !score.has_tag_name("score-partwise") {
        return err("not a MusicXML score");
    }
    let mut song = Song {
        format: "MusicXML".into(),
        ..Default::default()
    };
    song.title = child(score, "work")
        .map(|w| text(w, "work-title"))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| text(score, "movement-title"));
    if let Some(id) = child(score, "identification") {
        for c in children(id, "creator") {
            let who = c.text().unwrap_or("").trim().to_string();
            match c.attribute("type") {
                Some("composer") => song.music = who,
                Some("lyricist") | Some("poet") => song.words = who,
                Some("arranger") => song.tab_by = who,
                _ if song.artist.is_empty() => song.artist = who,
                _ => {}
            }
        }
        song.copyright = text(id, "rights");
    }
    if song.artist.is_empty() {
        song.artist = song.music.clone();
    }
    for c in score
        .children()
        .filter(|c| c.has_tag_name("credit"))
        .flat_map(|c| children(c, "credit-words"))
    {
        let t = c.text().unwrap_or("").trim();
        if song.title.is_empty() && !t.is_empty() {
            song.title = t.to_string();
        }
    }

    // The parts as listed.
    let mut infos: Vec<(String, PartInfo)> = Vec::new();
    if let Some(list) = child(score, "part-list") {
        for sp in children(list, "score-part") {
            let mut info = PartInfo {
                name: text(sp, "part-name"),
                ..Default::default()
            };
            let mut channel = None;
            for mi in children(sp, "midi-instrument") {
                if let Some(p) = num(mi, "midi-program") {
                    info.program = (p as i32 - 1).clamp(0, 127) as u8;
                }
                if let Some(c) = num(mi, "midi-channel") {
                    channel.get_or_insert(c as i32 - 1);
                }
                if let (Some(id), Some(u)) = (mi.attribute("id"), num(mi, "midi-unpitched")) {
                    info.unpitched
                        .push((id.to_string(), (u as i32 - 1).clamp(0, 127) as u8));
                }
            }
            info.drums = channel == Some(DRUM_CHANNEL as i32);
            infos.push((sp.attribute("id").unwrap_or("").to_string(), info));
        }
    }

    let mut measures: Vec<Measure> = Vec::new();
    let mut tracks: Vec<Track> = Vec::new();
    let mut notes: Vec<Written> = Vec::new();
    let mut tempos: Vec<Tempo> = Vec::new();
    for (ti, part) in children(score, "part").enumerate() {
        let id = part.attribute("id").unwrap_or("");
        let unlisted = PartInfo::default();
        let info = infos
            .iter()
            .find(|(i, _)| i == id)
            .map_or(&unlisted, |(_, p)| p);
        let mut track = Track {
            name: if info.name.is_empty() {
                format!("Part {}", ti + 1)
            } else {
                info.name.clone()
            },
            program: info.program,
            drums: info.drums,
            ..Default::default()
        };
        let first = ti == 0;
        let mut divisions = 1.0f64;
        let mut sig = (4u8, 4u8);
        let mut velocity = 90u8;
        let mut ending = 0u32;
        let mut part_notes: Vec<Written> = Vec::new();
        for (mi, measure) in children(part, "measure").enumerate() {
            let mut header = Measure::new(sig);
            let mut cursor = 0.0f64;
            let mut last_start = 0.0f64;
            let mut ends_ending = false;
            if ending != 0 {
                header.endings = ending;
            }
            for el in measure.children().filter(|c| c.is_element()) {
                match el.tag_name().name() {
                    "attributes" => {
                        if let Some(d) = num(el, "divisions").filter(|d| *d > 0.0) {
                            divisions = d;
                        }
                        if let Some(t) = child(el, "time") {
                            let beats: f64 = text(t, "beats")
                                .split('+')
                                .filter_map(|b| b.trim().parse::<f64>().ok())
                                .sum();
                            let unit = num(t, "beat-type").unwrap_or(4.0);
                            if beats >= 1.0 && unit >= 1.0 {
                                sig = (beats.min(64.0) as u8, unit.min(64.0) as u8);
                                header.sig = sig;
                            }
                        }
                        for sd in children(el, "staff-details") {
                            let mut tuning: Vec<(u32, u8)> = children(sd, "staff-tuning")
                                .filter_map(|st| {
                                    let line = st.attribute("line")?.parse().ok()?;
                                    let k = key(
                                        &text(st, "tuning-step"),
                                        num(st, "tuning-alter").unwrap_or(0.0),
                                        num(st, "tuning-octave")?,
                                    )?;
                                    Some((line, k))
                                })
                                .collect();
                            if !tuning.is_empty() {
                                tuning.sort();
                                track.strings = tuning.into_iter().map(|t| t.1).collect();
                            }
                            if let Some(c) = num(sd, "capo") {
                                track.capo = c.clamp(0.0, 24.0) as u8;
                            }
                        }
                    }
                    "direction" | "sound" => {
                        let sound = if el.has_tag_name("sound") {
                            Some(el)
                        } else {
                            child(el, "sound")
                        };
                        if let Some(s) = sound {
                            if let Some(t) =
                                s.attribute("tempo").and_then(|t| t.parse::<f64>().ok())
                            {
                                if first {
                                    tempos.push(Tempo {
                                        measure: mi,
                                        at: cursor,
                                        bpm: t,
                                    });
                                }
                            }
                            if let Some(d) =
                                s.attribute("dynamics").and_then(|d| d.parse::<f64>().ok())
                            {
                                velocity = (d / 100.0 * FORTE).clamp(1.0, 127.0) as u8;
                            }
                        }
                    }
                    "backup" => {
                        cursor = (cursor - num(el, "duration").unwrap_or(0.0) / divisions).max(0.0);
                    }
                    "forward" => cursor += num(el, "duration").unwrap_or(0.0) / divisions,
                    "barline" => {
                        if let Some(r) = child(el, "repeat") {
                            match r.attribute("direction") {
                                Some("forward") => header.repeat_open = true,
                                Some("backward") => {
                                    let times = r
                                        .attribute("times")
                                        .and_then(|t| t.parse::<u32>().ok())
                                        .unwrap_or(2);
                                    header.repeat_close = times.clamp(2, 33) - 1;
                                }
                                _ => {}
                            }
                        }
                        if let Some(e) = child(el, "ending") {
                            match e.attribute("type") {
                                Some("start") => {
                                    ending = endings(e.attribute("number").unwrap_or("1"));
                                    header.endings = ending;
                                }
                                _ => ends_ending = true,
                            }
                        }
                    }
                    "note" => {
                        let dur = num(el, "duration").unwrap_or(0.0) / divisions;
                        if child(el, "grace").is_some() || child(el, "cue").is_some() {
                            continue;
                        }
                        let chord = child(el, "chord").is_some();
                        let at = if chord { last_start } else { cursor };
                        if !chord {
                            last_start = cursor;
                            cursor += dur;
                        }
                        if child(el, "rest").is_some() {
                            continue;
                        }
                        let k = if let Some(p) = child(el, "pitch") {
                            key(
                                &text(p, "step"),
                                num(p, "alter").unwrap_or(0.0),
                                num(p, "octave").unwrap_or(4.0),
                            )
                        } else if let Some(u) = child(el, "unpitched") {
                            let by_id = child(el, "instrument")
                                .and_then(|i| i.attribute("id"))
                                .and_then(|id| info.unpitched.iter().find(|(i, _)| i == id))
                                .map(|(_, k)| *k);
                            by_id.or_else(|| {
                                key(
                                    &text(u, "display-step"),
                                    0.0,
                                    num(u, "display-octave").unwrap_or(4.0),
                                )
                            })
                        } else {
                            None
                        };
                        let Some(k) = k else { continue };
                        let tech = child(el, "notations").and_then(|n| child(n, "technical"));
                        let string = tech.and_then(|t| num(t, "string")).map(|s| s as i32);
                        let tie = children(el, "tie").any(|t| t.attribute("type") == Some("stop"));
                        let vel = el
                            .attribute("dynamics")
                            .and_then(|d| d.parse::<f64>().ok())
                            .map_or(velocity, |d| (d / 100.0 * FORTE).clamp(1.0, 127.0) as u8);
                        part_notes.push(Written {
                            track: ti,
                            measure: mi,
                            at,
                            quarters: dur,
                            key: k,
                            velocity: vel,
                            // Numbered from the highest string; turned
                            // round once the tuning is known.
                            string: string.filter(|s| (1..=16).contains(s)).map(|s| s as u8),
                            tie,
                        });
                    }
                    _ => {}
                }
            }
            if ends_ending {
                ending = 0;
            }
            if first {
                measures.push(header);
            } else if let Some(m) = measures.get_mut(mi) {
                // Repeats marked only in a later part.
                m.repeat_open |= header.repeat_open;
                m.repeat_close = m.repeat_close.max(header.repeat_close);
                if m.endings == 0 {
                    m.endings = header.endings;
                }
            }
        }
        // Strings from the top, now the tuning is known; the same note on
        // two staves (notes and tab) kept once, with its string.
        let n = track.strings.len() as u8;
        for w in &mut part_notes {
            w.string = w.string.and_then(|s| n.checked_sub(s));
        }
        part_notes.sort_by(|a, b| {
            (a.measure, a.key)
                .cmp(&(b.measure, b.key))
                .then(a.at.total_cmp(&b.at))
                .then(b.string.is_some().cmp(&a.string.is_some()))
        });
        part_notes.dedup_by(|b, a| {
            a.measure == b.measure && a.key == b.key && (a.at - b.at).abs() < 1e-6
        });
        if track.drums {
            for w in &mut part_notes {
                w.string = None;
            }
            track.strings.clear();
        }
        notes.extend(part_notes);
        tracks.push(track);
    }
    if tracks.is_empty() {
        return err("the score has no parts");
    }
    let bpm = tempos
        .iter()
        .find(|t| t.measure == 0 && t.at == 0.0)
        .map_or(120.0, |t| t.bpm);
    Ok(build(song, &measures, &tracks, &notes, &tempos, bpm))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_chippy_writes() {
        let score = crate::musicxml::tests::sample();
        let x = crate::musicxml::write(&score);
        let song = read(x.as_bytes()).unwrap();
        assert_eq!(song.format, "MusicXML");
        assert_eq!(song.title, score.title);
        assert_eq!(song.parts.len(), score.tracks.len());
        assert_eq!(song.meter.bars.len(), score.heads.len());
        for (p, t) in song.parts.iter().zip(&score.tracks) {
            assert_eq!(p.name, t.name);
            assert_eq!(p.program, t.program);
            assert_eq!(p.strings, t.strings);
            // Every note once (not once per staff), each on its string.
            let written: usize = t
                .bars
                .iter()
                .flatten()
                .map(|e| e.notes.iter().filter(|n| !n.tie_in).count())
                .sum();
            assert_eq!(p.notes.len(), written, "{}", p.name);
            assert_eq!(p.written.len(), p.notes.len());
            let tuning = crate::tab::Tuning::new("", p.strings.clone());
            let tab = crate::tab::written(&p.notes, &p.written, &tuning).unwrap();
            let frets: Vec<u8> = tab.notes.iter().map(|n| n.fret).collect();
            let expected: Vec<u8> = t
                .bars
                .iter()
                .flatten()
                .flat_map(|e| e.notes.iter().filter(|n| !n.tie_in))
                .map(|n| n.fret)
                .collect();
            let mut a = frets.clone();
            let mut b = expected.clone();
            a.sort();
            b.sort();
            assert_eq!(a, b, "{}", p.name);
        }
    }

    const SCORE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE score-partwise PUBLIC "-//Recordare//DTD MusicXML 4.0 Partwise//EN" "http://www.musicxml.org/dtds/partwise.dtd">
<score-partwise version="4.0">
  <work><work-title>Étude</work-title></work>
  <identification>
    <creator type="composer">Someone</creator>
    <rights>Public domain</rights>
  </identification>
  <part-list>
    <score-part id="P1"><part-name>Guitar</part-name>
      <midi-instrument id="P1-I1"><midi-channel>1</midi-channel><midi-program>26</midi-program></midi-instrument>
    </score-part>
    <score-part id="P2"><part-name>Drums</part-name>
      <midi-instrument id="P2-I36"><midi-channel>10</midi-channel><midi-unpitched>37</midi-unpitched></midi-instrument>
    </score-part>
  </part-list>
  <part id="P1">
    <measure number="1">
      <attributes>
        <divisions>2</divisions>
        <time><beats>2</beats><beat-type>4</beat-type></time>
        <staff-details><staff-lines>6</staff-lines>
          <staff-tuning line="1"><tuning-step>D</tuning-step><tuning-octave>2</tuning-octave></staff-tuning>
          <staff-tuning line="2"><tuning-step>A</tuning-step><tuning-octave>2</tuning-octave></staff-tuning>
          <staff-tuning line="3"><tuning-step>D</tuning-step><tuning-octave>3</tuning-octave></staff-tuning>
          <staff-tuning line="4"><tuning-step>G</tuning-step><tuning-octave>3</tuning-octave></staff-tuning>
          <staff-tuning line="5"><tuning-step>B</tuning-step><tuning-octave>3</tuning-octave></staff-tuning>
          <staff-tuning line="6"><tuning-step>E</tuning-step><tuning-octave>4</tuning-octave></staff-tuning>
          <capo>1</capo>
        </staff-details>
      </attributes>
      <direction><sound tempo="60"/></direction>
      <note><pitch><step>D</step><octave>2</octave><alter>1</alter></pitch><duration>2</duration>
        <notations><technical><string>6</string><fret>0</fret></technical></notations></note>
      <note><chord/><pitch><step>F</step><octave>4</octave></pitch><duration>2</duration>
        <notations><technical><string>1</string><fret>0</fret></technical></notations></note>
      <note><grace/><pitch><step>C</step><octave>4</octave></pitch></note>
      <note><pitch><step>G</step><octave>4</octave></pitch><duration>2</duration><tie type="start"/>
        <notations><technical><string>1</string><fret>2</fret></technical></notations></note>
      <barline location="right"><repeat direction="backward"/></barline>
    </measure>
    <measure number="2">
      <note><pitch><step>G</step><octave>4</octave></pitch><duration>2</duration><tie type="stop"/>
        <notations><technical><string>1</string><fret>2</fret></technical></notations></note>
      <note><rest/><duration>2</duration></note>
    </measure>
  </part>
  <part id="P2">
    <measure number="1">
      <attributes><divisions>1</divisions></attributes>
      <note><unpitched><display-step>F</display-step><display-octave>4</display-octave></unpitched>
        <duration>1</duration><instrument id="P2-I36"/></note>
    </measure>
    <measure number="2"><note><rest/><duration>2</duration></note></measure>
  </part>
</score-partwise>"#;

    #[test]
    fn reads_a_score() {
        let song = read(SCORE.as_bytes()).unwrap();
        assert_eq!(song.title, "Étude");
        assert_eq!(
            (song.music.as_str(), song.copyright.as_str()),
            ("Someone", "Public domain")
        );
        // Bar 1 twice, then bar 2; 2/4 at 60 bpm.
        let starts: Vec<f64> = song.meter.bars.iter().map(|b| b.start_ms).collect();
        assert_eq!(starts, vec![0.0, 2000.0, 4000.0]);
        let g = &song.parts[0];
        assert_eq!((g.name.as_str(), g.program, g.capo), ("Guitar", 25, 1));
        // Drop D, with the capo.
        assert_eq!(g.strings, vec![39, 46, 51, 56, 60, 65]);
        let got: Vec<(f64, f64, u8, Option<u8>)> = g
            .notes
            .iter()
            .zip(&g.written)
            .map(|(n, s)| (n.start_ms, n.dur_ms, n.key, *s))
            .collect();
        assert_eq!(
            got,
            vec![
                (0.0, 1000.0, 39, Some(0)),
                (0.0, 1000.0, 65, Some(5)),
                (1000.0, 1000.0, 67, Some(5)),
                (2000.0, 1000.0, 39, Some(0)),
                (2000.0, 1000.0, 65, Some(5)),
                // Tied over into bar 2.
                (3000.0, 2000.0, 67, Some(5)),
            ]
        );
        let d = &song.parts[1];
        assert!(d.drums);
        assert_eq!(
            d.notes.iter().map(|n| n.key).collect::<Vec<_>>(),
            vec![36, 36]
        );
    }

    #[test]
    fn reads_endings() {
        assert_eq!(endings("1"), 1);
        assert_eq!(endings("1, 2"), 3);
        assert_eq!(endings("2-3"), 6);
        assert!(read(b"<html/>").is_err());
        assert!(read(b"not xml").is_err());
    }
}
