//! Reading Guitar Pro 7 and 8 files (`.gp`): a zip holding the score as
//! GPIF, Guitar Pro's XML. The score is kept as lists that refer to each
//! other by number: each bar of the master track names a bar for each
//! staff, each bar its voices, each voice its beats, each beat its notes
//! and its rhythm.
//!
//! Guitar Pro 6's `.gpx` holds the same XML in a compressed container of
//! its own, which isn't read yet.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use crate::song::{build, err, tuplet, Measure, Result, Song, Tempo, Track, Written, DRUM_CHANNEL};

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn text(n: Node, name: &str) -> String {
    child(n, name)
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// A list of numbers ("0 1 -1").
fn numbers(s: &str) -> Vec<i64> {
    s.split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect()
}

/// The elements of a list section (`<Bars>`, `<Notes>`...) by their id.
fn by_id<'a, 'i>(gpif: Node<'a, 'i>, list: &str) -> HashMap<i64, Node<'a, 'i>> {
    child(gpif, list)
        .map(|l| {
            l.children()
                .filter(|c| c.is_element())
                .filter_map(|c| Some((c.attribute("id")?.parse().ok()?, c)))
                .collect()
        })
        .unwrap_or_default()
}

/// The value of a `<Property name="...">`'s first element child.
fn property<'a, 'i>(props: Option<Node<'a, 'i>>, name: &str) -> Option<Node<'a, 'i>> {
    props?
        .children()
        .find(|p| p.has_tag_name("Property") && p.attribute("name") == Some(name))?
        .children()
        .find(|c| c.is_element())
}

/// A rhythm's length in quarter notes.
fn quarters(r: Node) -> f64 {
    let base = match text(r, "NoteValue").as_str() {
        "Long" => 16.0,
        "DoubleWhole" => 8.0,
        "Whole" => 4.0,
        "Half" => 2.0,
        "Quarter" => 1.0,
        "Eighth" => 0.5,
        "16th" => 0.25,
        "32nd" => 0.125,
        "64th" => 0.0625,
        "128th" => 0.03125,
        "256th" => 0.015625,
        _ => 1.0,
    };
    let dots = child(r, "AugmentationDot")
        .and_then(|d| d.attribute("count")?.parse::<i32>().ok())
        .unwrap_or(0)
        .clamp(0, 3);
    let mut q = base * (2.0 - 0.5f64.powi(dots));
    if let Some(t) = child(r, "PrimaryTuplet") {
        let num = t
            .attribute("num")
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(1);
        let den = t.attribute("den").and_then(|n| n.parse::<u32>().ok());
        q *= match den {
            Some(d) if num > 0 => d as f64 / num as f64,
            _ => tuplet(num),
        };
    }
    q
}

fn velocity(dynamic: &str) -> u8 {
    match dynamic {
        "PPP" => 15,
        "PP" => 31,
        "P" => 47,
        "MP" => 63,
        "MF" => 79,
        "FF" => 111,
        "FFF" => 127,
        _ => 95,
    }
}

/// Reads a Guitar Pro 7 or 8 file.
pub fn read_gp(data: &[u8]) -> Result<Song> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data))
        .or_else(|_| err("not a Guitar Pro 7 file (Guitar Pro 6's .gpx isn't read yet)"))?;
    let mut f = zip
        .by_name("Content/score.gpif")
        .or_else(|_| err("no score in the Guitar Pro file"))?;
    let mut xml = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut xml)
        .or_else(|e| err(format!("can't read the score: {e}")))?;
    read_gpif(&xml).map(|mut s| {
        s.format = "Guitar Pro 7".into();
        s
    })
}

/// Reads a score in GPIF, Guitar Pro 6 to 8's XML.
pub fn read_gpif(xml: &[u8]) -> Result<Song> {
    let xml = String::from_utf8_lossy(xml);
    let doc = Document::parse(xml.trim_start_matches('\u{feff}'))
        .or_else(|e| err(format!("the score isn't valid XML: {e}")))?;
    let gpif = doc.root_element();
    if !gpif.has_tag_name("GPIF") {
        return err("not a Guitar Pro score");
    }
    let mut song = Song {
        format: "Guitar Pro".into(),
        ..Default::default()
    };
    if let Some(s) = child(gpif, "Score") {
        song.title = text(s, "Title");
        song.subtitle = text(s, "SubTitle");
        song.artist = text(s, "Artist");
        song.album = text(s, "Album");
        song.words = text(s, "Words");
        song.music = text(s, "Music");
        song.copyright = text(s, "Copyright");
        song.tab_by = text(s, "Tabber");
        for c in [text(s, "Instructions"), text(s, "Notices")] {
            if !c.is_empty() {
                song.comments.push(c);
            }
        }
    }

    // Tempo changes: (bar, fraction of the bar, quarter notes a minute).
    let mut tempos: Vec<(usize, f64, f64)> = Vec::new();
    if let Some(autos) = child(gpif, "MasterTrack").and_then(|m| child(m, "Automations")) {
        for a in autos.children().filter(|a| a.has_tag_name("Automation")) {
            if text(a, "Type") != "Tempo" {
                continue;
            }
            let v: Vec<f64> = text(a, "Value")
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            let Some(&bpm) = v.first() else { continue };
            // The beat the tempo counts: an eighth, a quarter, a dotted
            // quarter, a half, a dotted half.
            let per = match v.get(1).map(|&r| r as i32) {
                Some(1) => 0.5,
                Some(3) => 1.5,
                Some(4) => 2.0,
                Some(5) => 3.0,
                _ => 1.0,
            };
            let bar = text(a, "Bar").parse::<usize>().unwrap_or(0);
            let pos = text(a, "Position")
                .parse::<f64>()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            tempos.push((bar, pos, bpm * per));
        }
    }

    // Tracks, and how many staves each has (a bar of the master track
    // names one bar per staff).
    let mut tracks: Vec<Track> = Vec::new();
    let mut staves: Vec<usize> = Vec::new();
    // For drum kits: each articulation's MIDI key, in order.
    let mut kits: Vec<Vec<u8>> = Vec::new();
    if let Some(list) = child(gpif, "Tracks") {
        for (i, t) in list
            .children()
            .filter(|t| t.has_tag_name("Track"))
            .enumerate()
        {
            let tuning = t
                .descendants()
                .filter(|p| p.has_tag_name("Property") && p.attribute("name") == Some("Tuning"))
                .find_map(|p| child(p, "Pitches"))
                .map(|p| numbers(p.text().unwrap_or("")))
                .unwrap_or_default();
            let capo = t
                .descendants()
                .filter(|p| p.has_tag_name("Property") && p.attribute("name") == Some("CapoFret"))
                .find_map(|p| child(p, "Fret")?.text()?.trim().parse::<i64>().ok())
                .unwrap_or(0);
            let program = t
                .descendants()
                .find(|n| n.has_tag_name("Program"))
                .and_then(|n| n.text()?.trim().parse::<i64>().ok())
                .unwrap_or(25);
            let channel = t
                .descendants()
                .find(|n| n.has_tag_name("PrimaryChannel"))
                .and_then(|n| n.text()?.trim().parse::<i64>().ok());
            let kit_type = t
                .descendants()
                .find(|n| n.has_tag_name("InstrumentSet"))
                .map(|s| text(s, "Type"))
                .unwrap_or_default();
            let drums = channel == Some(DRUM_CHANNEL as i64)
                || kit_type.eq_ignore_ascii_case("drumKit")
                || child(t, "Instrument")
                    .and_then(|n| n.attribute("ref"))
                    .is_some_and(|r| r.to_ascii_lowercase().contains("drum"));
            let kit: Vec<u8> = t
                .descendants()
                .filter(|n| n.has_tag_name("Articulation"))
                .map(|a| {
                    text(a, "OutputMidiNumber")
                        .parse::<i64>()
                        .unwrap_or(0)
                        .clamp(0, 127) as u8
                })
                .collect();
            let name = text(t, "Name");
            staves.push(
                child(t, "Staves")
                    .map(|s| s.children().filter(|c| c.has_tag_name("Staff")).count())
                    .unwrap_or(1)
                    .max(1),
            );
            kits.push(kit);
            tracks.push(Track {
                name: if name.is_empty() {
                    format!("Track {}", i + 1)
                } else {
                    name
                },
                program: program.clamp(0, 127) as u8,
                drums,
                strings: if drums {
                    Vec::new()
                } else {
                    tuning.iter().map(|&k| k.clamp(0, 127) as u8).collect()
                },
                capo: capo.clamp(0, 24) as u8,
            });
        }
    }
    if tracks.is_empty() {
        return err("the score has no tracks");
    }

    let bars = by_id(gpif, "Bars");
    let voices = by_id(gpif, "Voices");
    let beats = by_id(gpif, "Beats");
    let notes_by_id = by_id(gpif, "Notes");
    let rhythms = by_id(gpif, "Rhythms");

    let mut measures: Vec<Measure> = Vec::new();
    let mut written: Vec<Written> = Vec::new();
    let masters: Vec<Node> = child(gpif, "MasterBars")
        .map(|m| {
            m.children()
                .filter(|c| c.has_tag_name("MasterBar"))
                .collect()
        })
        .unwrap_or_default();
    // Whether strings count from the lowest (as they normally do) or the
    // highest: checked against the notes' MIDI numbers where they have both.
    let (mut low_first, mut high_first) = (0, 0);
    let mut pending: Vec<(Written, Option<u8>)> = Vec::new();
    for (mi, mb) in masters.iter().enumerate() {
        let (num, den) = text(*mb, "Time")
            .split_once('/')
            .and_then(|(a, b)| Some((a.trim().parse::<u8>().ok()?, b.trim().parse::<u8>().ok()?)))
            .unwrap_or((4, 4));
        let mut m = Measure::new((num.max(1), den.max(1)));
        if let Some(r) = child(*mb, "Repeat") {
            m.repeat_open = r.attribute("start") == Some("true");
            if r.attribute("end") == Some("true") {
                let count = r
                    .attribute("count")
                    .and_then(|c| c.parse::<u32>().ok())
                    .unwrap_or(2);
                m.repeat_close = count.clamp(2, 33) - 1;
            }
        }
        for e in numbers(&text(*mb, "AlternateEndings")) {
            if (1..=32).contains(&e) {
                m.endings |= 1 << (e - 1);
            }
        }
        let bar_ids = numbers(&text(*mb, "Bars"));
        let mut staff = 0;
        for (ti, track) in tracks.iter().enumerate() {
            for _ in 0..staves[ti] {
                let bar = bar_ids.get(staff).and_then(|id| bars.get(id));
                staff += 1;
                let Some(bar) = bar else { continue };
                for vid in numbers(&text(*bar, "Voices"))
                    .into_iter()
                    .filter(|&v| v >= 0)
                {
                    let Some(voice) = voices.get(&vid) else {
                        continue;
                    };
                    let mut at = 0.0;
                    for bid in numbers(&text(*voice, "Beats")) {
                        let Some(beat) = beats.get(&bid) else {
                            continue;
                        };
                        let q = child(*beat, "Rhythm")
                            .and_then(|r| r.attribute("ref")?.parse::<i64>().ok())
                            .and_then(|r| rhythms.get(&r))
                            .map_or(1.0, |r| quarters(*r));
                        if child(*beat, "GraceNotes").is_some() {
                            continue;
                        }
                        let vel = velocity(&text(*beat, "Dynamic"));
                        for nid in numbers(&text(*beat, "Notes")) {
                            let Some(note) = notes_by_id.get(&nid) else {
                                continue;
                            };
                            let props = child(*note, "Properties");
                            if property(props, "Muted").is_some() {
                                continue;
                            }
                            let int = |name: &str, inner: &str| {
                                property(props, name)
                                    .filter(|n| n.has_tag_name(inner))
                                    .and_then(|n| n.text()?.trim().parse::<i64>().ok())
                            };
                            let midi = int("Midi", "Number").map(|k| k.clamp(0, 127) as u8);
                            let tie = child(*note, "Tie")
                                .is_some_and(|t| t.attribute("destination") == Some("true"));
                            let (key, string) = if track.drums {
                                let art =
                                    text(*note, "InstrumentArticulation").parse::<usize>().ok();
                                match midi.or_else(|| art.and_then(|a| kits[ti].get(a).copied())) {
                                    Some(k) => (k, None),
                                    None => continue,
                                }
                            } else {
                                let s = int("String", "String");
                                let f = int("Fret", "Fret");
                                let n = track.strings.len() as i64;
                                match (s, f) {
                                    (Some(s), Some(f)) if (0..n).contains(&s) && f >= 0 => {
                                        let open = |i: i64| {
                                            track.strings[i as usize] as i64 + track.capo as i64 + f
                                        };
                                        if let Some(k) = midi {
                                            if open(s) == k as i64 {
                                                low_first += 1;
                                            } else if open(n - 1 - s) == k as i64 {
                                                high_first += 1;
                                            }
                                        }
                                        (open(s).clamp(0, 127) as u8, Some(s as u8))
                                    }
                                    _ => match midi {
                                        Some(k) => (k, None),
                                        None => continue,
                                    },
                                }
                            };
                            pending.push((
                                Written {
                                    track: ti,
                                    measure: mi,
                                    at,
                                    quarters: q,
                                    key,
                                    velocity: vel,
                                    string,
                                    tie,
                                },
                                midi,
                            ));
                        }
                        at += q;
                    }
                }
            }
        }
        measures.push(m);
    }
    for (mut w, midi) in pending {
        if high_first > low_first {
            if let Some(s) = w.string {
                let t = &tracks[w.track];
                let s = t.strings.len() as u8 - 1 - s;
                w.string = Some(s);
                w.key = midi.unwrap_or(t.strings[s as usize].saturating_add(t.capo));
            }
        }
        written.push(w);
    }
    // Tempo changes, placed in quarter notes into their bars.
    let tempo_marks: Vec<Tempo> = tempos
        .iter()
        .filter_map(|&(bar, pos, bpm)| {
            Some(Tempo {
                measure: bar,
                at: pos * measures.get(bar)?.quarters(),
                bpm,
            })
        })
        .collect();
    let bpm = tempo_marks
        .iter()
        .find(|t| t.measure == 0 && t.at == 0.0)
        .map_or(120.0, |t| t.bpm);
    Ok(build(song, &measures, &tracks, &written, &tempo_marks, bpm))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guitar in drop D with a capo on 2 and a drum kit: a bar of 3/4 at
    /// 90 bpm (counted in dotted quarters: 60), repeated, with a tie into
    /// a second bar of eighth-note triplets.
    pub const SCORE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<GPIF version="7">
  <Score><Title><![CDATA[Song]]></Title><Artist><![CDATA[Band]]></Artist><Tabber>Me</Tabber></Score>
  <MasterTrack>
    <Tracks>0 1</Tracks>
    <Automations>
      <Automation><Type>Tempo</Type><Linear>false</Linear><Bar>0</Bar><Position>0</Position><Value>60 3</Value></Automation>
    </Automations>
  </MasterTrack>
  <Tracks>
    <Track id="0">
      <Name><![CDATA[Lead]]></Name>
      <Sounds><Sound><MIDI><Program>29</Program></MIDI></Sound></Sounds>
      <MidiConnection><PrimaryChannel>0</PrimaryChannel></MidiConnection>
      <Staves><Staff><Properties>
        <Property name="Tuning"><Pitches>38 45 50 55 59 64</Pitches></Property>
        <Property name="CapoFret"><Fret>2</Fret></Property>
      </Properties></Staff></Staves>
    </Track>
    <Track id="1">
      <Name><![CDATA[Drums]]></Name>
      <InstrumentSet><Type>drumKit</Type><Elements><Element><Articulations>
        <Articulation><OutputMidiNumber>38</OutputMidiNumber></Articulation>
        <Articulation><OutputMidiNumber>36</OutputMidiNumber></Articulation>
      </Articulations></Element></Elements></InstrumentSet>
      <MidiConnection><PrimaryChannel>9</PrimaryChannel></MidiConnection>
      <Staves><Staff/></Staves>
    </Track>
  </Tracks>
  <MasterBars>
    <MasterBar><Time>3/4</Time><Repeat start="true" end="true" count="2"/><Bars>0 1</Bars></MasterBar>
    <MasterBar><Time>3/4</Time><Bars>2 3</Bars></MasterBar>
  </MasterBars>
  <Bars>
    <Bar id="0"><Voices>0 -1 -1 -1</Voices></Bar>
    <Bar id="1"><Voices>1 -1 -1 -1</Voices></Bar>
    <Bar id="2"><Voices>2 -1 -1 -1</Voices></Bar>
    <Bar id="3"><Voices>-1 -1 -1 -1</Voices></Bar>
  </Bars>
  <Voices>
    <Voice id="0"><Beats>0 1</Beats></Voice>
    <Voice id="1"><Beats>2</Beats></Voice>
    <Voice id="2"><Beats>3 4 5</Beats></Voice>
  </Voices>
  <Beats>
    <Beat id="0"><Rhythm ref="0"/><Notes>0 1</Notes></Beat>
    <Beat id="1"><Dynamic>P</Dynamic><Rhythm ref="1"/><Notes>2</Notes></Beat>
    <Beat id="2"><Rhythm ref="2"/><Notes>3</Notes></Beat>
    <Beat id="3"><Rhythm ref="3"/><Notes>4</Notes></Beat>
    <Beat id="4"><Rhythm ref="3"/></Beat>
    <Beat id="5"><Rhythm ref="3"/><Notes>5</Notes></Beat>
  </Beats>
  <Notes>
    <Note id="0"><Properties><Property name="String"><String>0</String></Property><Property name="Fret"><Fret>0</Fret></Property></Properties></Note>
    <Note id="1"><Properties><Property name="String"><String>5</String></Property><Property name="Fret"><Fret>3</Fret></Property><Property name="Midi"><Number>69</Number></Property></Properties></Note>
    <Note id="2"><Tie origin="true" destination="false"/><Properties><Property name="String"><String>4</String></Property><Property name="Fret"><Fret>1</Fret></Property></Properties></Note>
    <Note id="3"><InstrumentArticulation>1</InstrumentArticulation></Note>
    <Note id="4"><Tie origin="false" destination="true"/><Properties><Property name="String"><String>4</String></Property><Property name="Fret"><Fret>1</Fret></Property></Properties></Note>
    <Note id="5"><Properties><Property name="String"><String>3</String></Property><Property name="Fret"><Fret>0</Fret></Property></Properties></Note>
  </Notes>
  <Rhythms>
    <Rhythm id="0"><NoteValue>Half</NoteValue></Rhythm>
    <Rhythm id="1"><NoteValue>Quarter</NoteValue></Rhythm>
    <Rhythm id="2"><NoteValue>Half</NoteValue><AugmentationDot count="1"/></Rhythm>
    <Rhythm id="3"><NoteValue>Eighth</NoteValue><PrimaryTuplet num="3" den="2"/></Rhythm>
  </Rhythms>
</GPIF>"#;

    #[test]
    fn reads_a_score() {
        let song = read_gpif(SCORE.as_bytes()).unwrap();
        assert_eq!(
            (song.title.as_str(), song.artist.as_str()),
            ("Song", "Band")
        );
        assert_eq!(song.tab_by, "Me");
        // 60 dotted quarters a minute: 90 quarters; 2 s a bar of 3/4.
        let starts: Vec<f64> = song.meter.bars.iter().map(|b| b.start_ms).collect();
        assert_eq!(starts, vec![0.0, 2000.0, 4000.0]);
        let g = &song.parts[0];
        assert_eq!((g.name.as_str(), g.program, g.capo), ("Lead", 29, 2));
        assert_eq!(g.strings, vec![40, 47, 52, 57, 61, 66]);
        let ms = 1000.0 / 1.5;
        let got: Vec<(i64, i64, u8, Option<u8>)> = g
            .notes
            .iter()
            .zip(&g.written)
            .map(|(n, s)| {
                (
                    n.start_ms.round() as i64,
                    n.dur_ms.round() as i64,
                    n.key,
                    *s,
                )
            })
            .collect();
        let r = |x: f64| x.round() as i64;
        assert_eq!(
            got,
            vec![
                (0, r(2.0 * ms), 40, Some(0)),
                (0, r(2.0 * ms), 69, Some(5)),
                (r(2.0 * ms), r(ms), 62, Some(4)),
                (2000, r(2.0 * ms), 40, Some(0)),
                (2000, r(2.0 * ms), 69, Some(5)),
                // Tied into the next bar's first triplet eighth.
                (r(2000.0 + 2.0 * ms), r(ms + ms / 3.0), 62, Some(4)),
                (r(4000.0 + 2.0 * ms / 3.0), r(ms / 3.0), 57, Some(3)),
            ]
        );
        // Quieter where marked so.
        assert!(g.notes[2].velocity < g.notes[0].velocity);
        let d = &song.parts[1];
        assert!(d.drums && d.strings.is_empty());
        assert_eq!(
            d.notes.iter().map(|n| n.key).collect::<Vec<_>>(),
            vec![36, 36]
        );
    }

    #[test]
    fn reads_the_zip() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("VERSION", o).unwrap();
            std::io::Write::write_all(&mut z, b"7.0").unwrap();
            z.start_file("Content/score.gpif", o).unwrap();
            std::io::Write::write_all(&mut z, SCORE.as_bytes()).unwrap();
            z.finish().unwrap();
        }
        let song = read_gp(buf.get_ref()).unwrap();
        assert_eq!(song.format, "Guitar Pro 7");
        assert_eq!(song.parts.len(), 2);
        assert!(read_gp(b"BCFZ....").is_err());
    }
}
