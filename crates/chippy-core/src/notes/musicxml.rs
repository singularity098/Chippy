//! MusicXML from a [`Score`]: a part per track, each with a staff of
//! notes and a staff of tab below it (the strings and frets). MuseScore,
//! Guitar Pro, TuxGuitar, Sibelius and the rest read it.

use super::score::{Event, Score, Track, Value, DIVISIONS};
use std::fmt::Write;

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// A MIDI key as (step, alter, octave), sharps for black keys.
fn pitch(key: u8) -> (&'static str, i8, i32) {
    const STEPS: [(&str, i8); 12] = [
        ("C", 0),
        ("C", 1),
        ("D", 0),
        ("D", 1),
        ("E", 0),
        ("F", 0),
        ("F", 1),
        ("G", 0),
        ("G", 1),
        ("A", 0),
        ("A", 1),
        ("B", 0),
    ];
    let (step, alter) = STEPS[key as usize % 12];
    (step, alter, key as i32 / 12 - 1)
}

fn type_name(v: Value) -> &'static str {
    match v.base {
        1 => "whole",
        2 => "half",
        4 => "quarter",
        8 => "eighth",
        16 => "16th",
        _ => "32nd",
    }
}

/// The clef for the notes of an instrument with these open strings:
/// (sign, line, octave change).
fn clef(strings: &[u8]) -> (&'static str, u8, i8) {
    let low = strings.iter().copied().min().unwrap_or(40);
    let high = strings.iter().copied().max().unwrap_or(64);
    if high <= 50 {
        ("F", 4, -1)
    } else if low >= 55 {
        ("G", 2, 0)
    } else {
        ("G", 2, -1)
    }
}

/// The score as a MusicXML 4.0 document.
pub fn write(score: &Score) -> String {
    let mut x = String::new();
    x.push_str(concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n",
        "<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" ",
        "\"http://www.musicxml.org/dtds/partwise.dtd\">\n",
        "<score-partwise version=\"4.0\">\n"
    ));
    let title = escape(&score.title);
    let _ = writeln!(
        x,
        "  <work>\n    <work-title>{title}</work-title>\n  </work>\n  <movement-title>{title}</movement-title>"
    );
    x.push_str("  <identification>\n    <encoding>\n      <software>Chippy</software>\n    </encoding>\n  </identification>\n");
    x.push_str("  <part-list>\n");
    let mut channel = 0;
    for (i, t) in score.tracks.iter().enumerate() {
        channel += 1;
        if channel == 10 {
            channel += 1;
        }
        let id = i + 1;
        let name = escape(&t.name);
        let _ = write!(
            x,
            concat!(
                "    <score-part id=\"P{id}\">\n",
                "      <part-name>{name}</part-name>\n",
                "      <score-instrument id=\"P{id}-I1\">\n",
                "        <instrument-name>{name}</instrument-name>\n",
                "      </score-instrument>\n",
                "      <midi-instrument id=\"P{id}-I1\">\n",
                "        <midi-channel>{ch}</midi-channel>\n",
                "        <midi-program>{prog}</midi-program>\n",
                "      </midi-instrument>\n",
                "    </score-part>\n"
            ),
            id = id,
            name = name,
            ch = (channel - 1) % 16 + 1,
            prog = t.program as u32 + 1,
        );
    }
    x.push_str("  </part-list>\n");
    for (i, t) in score.tracks.iter().enumerate() {
        write_part(&mut x, score, t, i + 1);
    }
    x.push_str("</score-partwise>\n");
    x
}

fn write_part(x: &mut String, score: &Score, t: &Track, id: usize) {
    let _ = writeln!(x, "  <part id=\"P{id}\">");
    let strings = t.strings.len();
    let mut last_sig = None;
    let mut last_bpm: Option<f64> = None;
    for (bi, (head, bar)) in score.heads.iter().zip(&t.bars).enumerate() {
        let _ = writeln!(x, "    <measure number=\"{}\">", bi + 1);
        let new_sig = last_sig != Some(head.sig);
        if bi == 0 || new_sig {
            x.push_str("      <attributes>\n");
            if bi == 0 {
                let _ = writeln!(x, "        <divisions>{DIVISIONS}</divisions>");
                x.push_str("        <key>\n          <fifths>0</fifths>\n        </key>\n");
            }
            let _ = writeln!(
                x,
                "        <time>\n          <beats>{}</beats>\n          <beat-type>{}</beat-type>\n        </time>",
                head.sig.0, head.sig.1
            );
            if bi == 0 {
                let (sign, line, octave) = clef(&t.strings);
                x.push_str("        <staves>2</staves>\n");
                let _ = write!(
                    x,
                    "        <clef number=\"1\">\n          <sign>{sign}</sign>\n          <line>{line}</line>\n"
                );
                if octave != 0 {
                    let _ = writeln!(
                        x,
                        "          <clef-octave-change>{octave}</clef-octave-change>"
                    );
                }
                x.push_str("        </clef>\n");
                x.push_str("        <clef number=\"2\">\n          <sign>TAB</sign>\n          <line>5</line>\n        </clef>\n");
                let _ = writeln!(
                    x,
                    "        <staff-details number=\"2\">\n          <staff-lines>{strings}</staff-lines>"
                );
                for (line, &k) in t.strings.iter().enumerate() {
                    let (step, alter, octave) = pitch(k);
                    let _ = write!(
                        x,
                        "          <staff-tuning line=\"{}\">\n            <tuning-step>{step}</tuning-step>\n",
                        line + 1
                    );
                    if alter != 0 {
                        let _ = writeln!(x, "            <tuning-alter>{alter}</tuning-alter>");
                    }
                    let _ = writeln!(
                        x,
                        "            <tuning-octave>{octave}</tuning-octave>\n          </staff-tuning>"
                    );
                }
                x.push_str("        </staff-details>\n");
            }
            x.push_str("      </attributes>\n");
            last_sig = Some(head.sig);
        }
        let bpm = (head.bpm * 100.0).round() / 100.0;
        if last_bpm.is_none_or(|b| (b - bpm).abs() >= 1.0) {
            let shown = bpm.round();
            let _ = write!(
                x,
                concat!(
                    "      <direction placement=\"above\">\n",
                    "        <direction-type>\n",
                    "          <metronome>\n",
                    "            <beat-unit>quarter</beat-unit>\n",
                    "            <per-minute>{shown}</per-minute>\n",
                    "          </metronome>\n",
                    "        </direction-type>\n",
                    "        <staff>1</staff>\n",
                    "        <sound tempo=\"{bpm}\"/>\n",
                    "      </direction>\n"
                ),
                shown = shown,
                bpm = bpm,
            );
            last_bpm = Some(bpm);
        }
        let ticks = head.ticks();
        let rest_bar = bar.iter().all(|e| e.notes.is_empty());
        for staff in [1u8, 2] {
            if staff == 2 {
                let _ = writeln!(
                    x,
                    "      <backup>\n        <duration>{ticks}</duration>\n      </backup>"
                );
            }
            let voice = if staff == 1 { 1 } else { 5 };
            if rest_bar {
                let _ = write!(
                    x,
                    concat!(
                        "      <note>\n",
                        "        <rest measure=\"yes\"/>\n",
                        "        <duration>{ticks}</duration>\n",
                        "        <voice>{voice}</voice>\n",
                        "        <staff>{staff}</staff>\n",
                        "      </note>\n"
                    ),
                    ticks = ticks,
                    voice = voice,
                    staff = staff,
                );
                continue;
            }
            for e in bar {
                write_event(x, e, staff, voice, strings);
            }
        }
        x.push_str("    </measure>\n");
    }
    x.push_str("  </part>\n");
}

fn write_event(x: &mut String, e: &Event, staff: u8, voice: u8, strings: usize) {
    let v = e.value;
    let common = |x: &mut String, tie_in: bool, tie_out: bool| {
        let _ = writeln!(x, "        <duration>{}</duration>", v.ticks());
        if tie_in {
            x.push_str("        <tie type=\"stop\"/>\n");
        }
        if tie_out {
            x.push_str("        <tie type=\"start\"/>\n");
        }
        let _ = writeln!(x, "        <voice>{voice}</voice>");
        let _ = writeln!(x, "        <type>{}</type>", type_name(v));
        if v.dotted {
            x.push_str("        <dot/>\n");
        }
        if v.triplet {
            x.push_str("        <time-modification>\n          <actual-notes>3</actual-notes>\n          <normal-notes>2</normal-notes>\n        </time-modification>\n");
        }
        let _ = writeln!(x, "        <staff>{staff}</staff>");
    };
    let tuplet = |x: &mut String, first: bool| {
        if !first {
            return;
        }
        if e.tuplet_start {
            x.push_str("          <tuplet type=\"start\" bracket=\"yes\"/>\n");
        }
        if e.tuplet_stop {
            x.push_str("          <tuplet type=\"stop\"/>\n");
        }
    };
    if e.notes.is_empty() {
        x.push_str("      <note>\n        <rest/>\n");
        common(x, false, false);
        if e.tuplet_start || e.tuplet_stop {
            x.push_str("        <notations>\n");
            tuplet(x, true);
            x.push_str("        </notations>\n");
        }
        x.push_str("      </note>\n");
        return;
    }
    // Highest first, as chords are usually written.
    let mut notes = e.notes.clone();
    notes.sort_by_key(|n| std::cmp::Reverse(n.key));
    for (i, n) in notes.iter().enumerate() {
        x.push_str("      <note>\n");
        if i > 0 {
            x.push_str("        <chord/>\n");
        }
        let (step, alter, octave) = pitch(n.key);
        let _ = write!(x, "        <pitch>\n          <step>{step}</step>\n");
        if alter != 0 {
            let _ = writeln!(x, "          <alter>{alter}</alter>");
        }
        let _ = writeln!(x, "          <octave>{octave}</octave>\n        </pitch>");
        common(x, n.tie_in, n.tie_out);
        x.push_str("        <notations>\n");
        if n.tie_in {
            x.push_str("          <tied type=\"stop\"/>\n");
        }
        if n.tie_out {
            x.push_str("          <tied type=\"start\"/>\n");
        }
        tuplet(x, i == 0);
        if staff == 2 {
            let _ = write!(
                x,
                "          <technical>\n            <string>{}</string>\n            <fret>{}</fret>\n          </technical>\n",
                strings - n.string as usize,
                n.fret
            );
        }
        x.push_str("        </notations>\n");
        x.push_str("      </note>\n");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::meter::{beats_in, Bar, Meter};
    use super::super::score::{build, PartTab};
    use super::super::tab::{self, TUNINGS};
    use super::super::Note;
    use super::*;

    fn note(t: f64, dur: f64, key: u8) -> Note {
        Note {
            start_ms: t,
            dur_ms: dur,
            key,
            velocity: 100,
            legato: false,
        }
    }

    pub(crate) fn sample() -> Score {
        let sig = (4, 4);
        let per = beats_in(sig);
        let meter = Meter {
            bars: (0..3)
                .map(|i| {
                    let s = (i * per) as f64 * 500.0;
                    Bar {
                        start_ms: s,
                        beats: (0..per).map(|k| s + k as f64 * 500.0).collect(),
                        sig,
                    }
                })
                .collect(),
            end_ms: 6000.0,
            exact: true,
        };
        let lead = [
            note(0.0, 480.0, 64),
            note(500.0, 230.0, 66),
            note(750.0, 230.0, 67),
            note(1000.0, 160.0, 69),
            note(1167.0, 160.0, 71),
            note(1333.0, 160.0, 72),
            note(1500.0, 1000.0, 76),
        ];
        let bass = [note(0.0, 1000.0, 40), note(1000.0, 1000.0, 47)];
        let lt = tab::arrange(&lead, &TUNINGS[0]);
        let bt = tab::arrange(&bass, &TUNINGS[5]);
        build(
            "A & B <Test>",
            &meter,
            &[
                PartTab {
                    name: "Lead".into(),
                    program: 29,
                    tuning: &TUNINGS[0],
                    tab: &lt,
                },
                PartTab {
                    name: "Bass".into(),
                    program: 33,
                    tuning: &TUNINGS[5],
                    tab: &bt,
                },
            ],
        )
    }

    #[test]
    fn writes_tab_staves() {
        let x = write(&sample());
        assert!(x.starts_with("<?xml"));
        assert!(x.contains("<work-title>A &amp; B &lt;Test&gt;</work-title>"));
        assert_eq!(x.matches("<part id=").count(), 2);
        assert_eq!(x.matches("<measure number=").count(), 6);
        assert!(x.contains("<sign>TAB</sign>"));
        assert!(x.contains("<staff-lines>6</staff-lines>"));
        assert!(x.contains("<staff-lines>4</staff-lines>"));
        // The bass reads in the bass clef, the guitar in treble an octave
        // down.
        assert!(x.contains("<sign>F</sign>"));
        assert!(x.contains("<clef-octave-change>-1</clef-octave-change>"));
        assert!(x.contains("<string>"));
        assert!(x.contains("<tuplet type=\"start\""));
        assert!(x.contains("<tied type=\"start\"/>"));
        assert!(x.contains("<rest measure=\"yes\"/>"));
        assert!(x.contains("<per-minute>120</per-minute>"));
        // Balanced.
        assert_eq!(x.matches("<note>").count(), x.matches("</note>").count());
        assert_eq!(
            x.matches("<measure ").count(),
            x.matches("</measure>").count()
        );
        if let Ok(p) = std::env::var("CHIPPY_WRITE_SAMPLES") {
            std::fs::write(format!("{p}/sample.musicxml"), &x).unwrap();
        }
    }
}
