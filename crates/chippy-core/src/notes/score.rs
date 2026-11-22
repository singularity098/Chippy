//! Tab written out in bars, for sheet music and tab programs: each part's
//! notes (on the strings and frets of its [`Tab`]) put onto the beats of a
//! [`Meter`] and spelled as note values (quarters, dotted eighths,
//! triplets...), tied across beats and bars where they must be.
//!
//! Times are rounded to the nearest sixteenth, or to the nearest triplet
//! eighth in a beat whose notes fit triplets clearly better. Short gaps
//! between notes are closed up (a chip's notes are often clipped short),
//! so a run of eighths reads as eighths rather than as sixteenths and
//! rests; longer gaps are rests.

use super::meter::{beats_in, Meter};
use super::tab::{Tab, Tuning};

/// Ticks to a quarter note.
pub const DIVISIONS: u32 = 24;

/// A written note value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value {
    /// 1 whole, 2 half, 4 quarter, 8 eighth, 16, 32.
    pub base: u8,
    pub dotted: bool,
    /// Three in the time of two.
    pub triplet: bool,
}

impl Value {
    /// Its length in ticks.
    pub fn ticks(self) -> u32 {
        let t = DIVISIONS * 4 / self.base as u32;
        let t = if self.dotted { t * 3 / 2 } else { t };
        if self.triplet {
            t * 2 / 3
        } else {
            t
        }
    }

    /// The value `ticks` long, if there is one.
    fn of(ticks: u32, triplet: bool) -> Option<Value> {
        [1u8, 2, 4, 8, 16, 32]
            .iter()
            .flat_map(|&base| {
                [false, true].map(|dotted| Value {
                    base,
                    dotted,
                    triplet,
                })
            })
            .filter(|v| !(v.dotted && v.triplet))
            .find(|v| v.ticks() == ticks)
    }
}

/// A note as played: where on the neck, and whether it is tied to the
/// same note before or after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Played {
    /// 0 is the lowest string.
    pub string: u8,
    pub fret: u8,
    /// The MIDI key that sounds.
    pub key: u8,
    /// Carried on from the event before (no new attack).
    pub tie_in: bool,
    /// Carried on into the event after.
    pub tie_out: bool,
}

/// A chord, a note or a rest, in one voice.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub value: Value,
    /// What sounds (nothing: a rest).
    pub notes: Vec<Played>,
    /// The first of a triplet group.
    pub tuplet_start: bool,
    /// The last of one.
    pub tuplet_stop: bool,
}

/// A bar's time signature and tempo, the same in every track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Head {
    pub sig: (u8, u8),
    /// Quarter notes a minute.
    pub bpm: f64,
}

impl Head {
    pub fn ticks(&self) -> u32 {
        self.sig.0 as u32 * DIVISIONS * 4 / self.sig.1 as u32
    }

    /// Ticks in one of its beats.
    fn beat_ticks(&self) -> u32 {
        self.ticks() / beats_in(self.sig) as u32
    }
}

/// One instrument's part.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub name: String,
    /// General MIDI program.
    pub program: u8,
    /// Open strings, lowest first, as MIDI keys.
    pub strings: Vec<u8>,
    /// What each bar holds, in order.
    pub bars: Vec<Vec<Event>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    pub title: String,
    pub heads: Vec<Head>,
    pub tracks: Vec<Track>,
}

/// A part to write: its name, General MIDI program, instrument, and tab.
pub struct PartTab<'a> {
    pub name: String,
    pub program: u8,
    pub tuning: &'a Tuning,
    pub tab: &'a Tab,
}

/// The time signatures this can write: up to 32nd-note beats.
fn writable(sig: (u8, u8)) -> (u8, u8) {
    let den = match sig.1 {
        0..=1 => 1,
        2..=3 => 2,
        4..=7 => 4,
        8..=15 => 8,
        16..=31 => 16,
        _ => 32,
    };
    (sig.0.clamp(1, 32), den)
}

/// Writes the parts out in the bars of `meter`.
pub fn build(title: &str, meter: &Meter, parts: &[PartTab]) -> Score {
    let heads: Vec<Head> = (0..meter.bars.len())
        .map(|i| Head {
            sig: writable(meter.bars[i].sig),
            bpm: meter.bpm(i),
        })
        .collect();
    let tracks = parts
        .iter()
        .map(|p| Track {
            name: p.name.clone(),
            program: p.program,
            strings: p.tuning.strings.to_vec(),
            bars: write_part(p.tab, meter, &heads),
        })
        .collect();
    Score {
        title: title.to_string(),
        heads,
        tracks,
    }
}

/// A beat: where it starts (ticks from the first bar), how long it is,
/// and the step its notes are rounded to.
#[derive(Clone, Copy, Debug)]
struct Beat {
    start: u32,
    len: u32,
    step: u32,
}

impl Beat {
    /// Its notes are rounded to triplets.
    fn triplet(&self) -> bool {
        self.step == 8
    }
}

/// A chord in exact ticks: start, end, and (string, fret, key) per note.
type RawChord = (f64, f64, Vec<(u8, u8, u8)>);

/// The beat tick `t` falls in.
fn beat_index(beats: &[Beat], t: f64) -> usize {
    beats
        .partition_point(|b| b.start as f64 <= t)
        .max(1)
        .min(beats.len())
        - 1
}

/// A chord in ticks: when, until when, and its notes.
struct Chord {
    start: u32,
    end: u32,
    notes: Vec<(u8, u8, u8)>,
}

fn write_part(tab: &Tab, meter: &Meter, heads: &[Head]) -> Vec<Vec<Event>> {
    let ruler = meter.ruler();
    let mut bar_starts = Vec::with_capacity(heads.len() + 1);
    let mut at = 0u32;
    for h in heads {
        bar_starts.push(at);
        at += h.ticks();
    }
    let total = at;
    bar_starts.push(total);
    // Where each meter bar starts in quarter notes, to put times in ticks
    // even where a bar's signature had to be written differently.
    let tick_of = |ms: f64| -> f64 {
        let q = ruler.quarters(ms);
        let Some(i) = meter.bar_at(ms) else {
            return q * DIVISIONS as f64;
        };
        let into = (q - ruler.bar_start(i)) / meter.bars[i].quarters().max(1e-6);
        let h = &heads[i];
        bar_starts[i] as f64 + into * h.ticks() as f64
    };
    // Chords, as exact (unrounded) ticks.
    let mut raw: Vec<RawChord> = Vec::new();
    for n in &tab.notes {
        let s = tick_of(n.start_ms).max(0.0);
        if s >= total as f64 {
            break;
        }
        let e = tick_of(n.start_ms + n.dur_ms).min(total as f64);
        match raw.last_mut() {
            Some(c) if (s - c.0).abs() < 1.0 => {
                c.1 = c.1.max(e);
                if !c.2.iter().any(|x| x.0 == n.string) {
                    c.2.push((n.string, n.fret, n.key));
                }
            }
            _ => raw.push((s, e, vec![(n.string, n.fret, n.key)])),
        }
    }
    // The beats, each with the step that fits its notes best.
    let mut beats: Vec<Beat> = Vec::new();
    for (i, h) in heads.iter().enumerate() {
        let len = h.beat_ticks().max(1);
        let mut s = bar_starts[i];
        while s < bar_starts[i + 1] {
            let len = len.min(bar_starts[i + 1] - s);
            beats.push(Beat {
                start: s,
                len,
                step: if len % 6 == 0 { 6 } else { 3 },
            });
            s += len;
        }
    }
    {
        let mut onsets: Vec<Vec<f64>> = vec![Vec::new(); beats.len()];
        for c in &raw {
            // Just before a beat counts as on it.
            let b = beat_index(&beats, c.0 + 3.0);
            onsets[b].push(c.0);
        }
        for (b, ons) in beats.iter_mut().zip(&onsets) {
            if ons.is_empty() {
                continue;
            }
            let err = |step: u32| -> f64 {
                ons.iter()
                    .map(|&x| {
                        let r = (x - b.start as f64) / step as f64;
                        (r - r.round()).abs() * step as f64
                    })
                    .sum()
            };
            let mut best = (b.step, err(b.step));
            if b.len % 8 == 0 && b.len % 3 == 0 && b.len % 9 != 0 {
                // A binary beat: triplets if they fit clearly better.
                let e = err(8);
                if e + 0.5 < best.1 * 0.6 {
                    best = (8, e);
                }
            }
            if b.len % 3 == 0 {
                let e = err(3);
                if e + 0.5 < best.1 * 0.5 {
                    best = (3, e);
                }
            }
            b.step = best.0;
        }
    }
    let snap = |x: f64, b: &Beat| -> u32 {
        let r = ((x - b.start as f64) / b.step as f64).round().max(0.0) as u32;
        (b.start + r * b.step).min(b.start + b.len)
    };
    let mut chords: Vec<Chord> = Vec::new();
    for (s, e, notes) in raw {
        let b = &beats[beat_index(&beats, s + 3.0)];
        let start = snap(s, b).min(total);
        let eb = &beats[beat_index(&beats, e)];
        let end = snap(e, eb).max(start + b.step).min(total);
        if start >= total {
            continue;
        }
        match chords.last_mut() {
            Some(c) if c.start == start => {
                c.end = c.end.max(end);
                for n in notes {
                    if !c.notes.iter().any(|x| x.0 == n.0) {
                        c.notes.push(n);
                    }
                }
            }
            _ => chords.push(Chord { start, end, notes }),
        }
    }
    // Each chord lasts until the next, or a little gap closes up.
    for i in 0..chords.len() {
        let next = chords.get(i + 1).map_or(total, |c| c.start);
        let c = &mut chords[i];
        c.end = c.end.min(next);
        let gap_beat = &beats[beat_index(&beats, c.end as f64)];
        if next > c.end && next - c.end <= gap_beat.len / 2 {
            c.end = next;
        }
        c.notes.sort_unstable();
    }
    // Spelled out, rests between.
    let mut bars: Vec<Vec<Event>> = vec![Vec::new(); heads.len()];
    let mut cursor = 0;
    for c in &chords {
        if c.start > cursor {
            spell(cursor, c.start, &[], &beats, &bar_starts, &mut bars);
        }
        spell(c.start, c.end, &c.notes, &beats, &bar_starts, &mut bars);
        cursor = c.end;
    }
    if cursor < total {
        spell(cursor, total, &[], &beats, &bar_starts, &mut bars);
    }
    bars
}

/// Writes `notes` (or a rest) from tick `a` to `b` into the bars, as note
/// values that fit the beats, tied together.
fn spell(
    a: u32,
    b: u32,
    notes: &[(u8, u8, u8)],
    beats: &[Beat],
    bar_starts: &[u32],
    bars: &mut [Vec<Event>],
) {
    let mut pieces: Vec<(usize, Value, bool, bool)> = Vec::new();
    let mut p = a;
    while p < b {
        let bar = bar_starts.partition_point(|&s| s <= p) - 1;
        let bar_end = bar_starts[bar + 1].min(b);
        let bi = beats.partition_point(|x| x.start <= p) - 1;
        let beat = beats[bi];
        let rel = p - beat.start;
        let beat_end = beat.start + beat.len;
        let left = bar_end - p;
        let whole_beat = rel == 0 && bar_end >= beat_end;
        let (d, v, start, stop) = if beat.triplet() && !whole_beat {
            // Inside a triplet beat: triplet values, none past its end.
            let room = (beat_end.min(bar_end)) - p;
            let d = [32u32, 16, 8, 4]
                .into_iter()
                .find(|&d| {
                    d <= room && rel.is_multiple_of(d.min(8)) && Value::of(d, true).is_some()
                })
                .unwrap_or(room.clamp(1, 4));
            let v = Value::of(d, true).unwrap_or(Value {
                base: 16,
                dotted: false,
                triplet: true,
            });
            (d, v, rel == 0, rel + d >= beat.len)
        } else {
            let d = [144u32, 96, 72, 48, 36, 24, 18, 12, 9, 6, 3]
                .into_iter()
                .find(|&d| {
                    if d > left {
                        return false;
                    }
                    let Some(v) = Value::of(d, false) else {
                        return false;
                    };
                    if d >= beat.len {
                        // Whole beats, from a beat; not into a triplet one
                        // partly.
                        rel == 0
                            && d % beat.len == 0
                            && beats[bi..]
                                .iter()
                                .take_while(|x| x.start < p + d)
                                .all(|x| x.start + x.len <= p + d)
                    } else {
                        let base = if v.dotted { d * 2 / 3 } else { d };
                        rel + d <= beat.len && rel.is_multiple_of(base)
                    }
                })
                .unwrap_or(left.min(3));
            let v = Value::of(d, false).unwrap_or(Value {
                base: 32,
                dotted: false,
                triplet: false,
            });
            (d, v, false, false)
        };
        pieces.push((bar, v, start, stop));
        p += d.max(1);
    }
    let n = pieces.len();
    for (k, (bar, value, start, stop)) in pieces.into_iter().enumerate() {
        bars[bar].push(Event {
            value,
            notes: notes
                .iter()
                .map(|&(string, fret, key)| Played {
                    string,
                    fret,
                    key,
                    tie_in: k > 0,
                    tie_out: k + 1 < n,
                })
                .collect(),
            tuplet_start: start,
            tuplet_stop: stop,
        });
    }
}

impl Score {
    /// Bars where every track rests.
    pub fn is_empty(&self) -> bool {
        self.tracks
            .iter()
            .all(|t| t.bars.iter().flatten().all(|e| e.notes.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::meter::Bar;
    use super::super::tab::{self, TUNINGS};
    use super::super::Note;
    use super::*;

    pub(crate) fn even(beat: f64, n: usize, sig: (u8, u8)) -> Meter {
        let per = beats_in(sig);
        Meter {
            bars: (0..n)
                .map(|i| {
                    let s = (i * per) as f64 * beat;
                    Bar {
                        start_ms: s,
                        beats: (0..per).map(|k| s + k as f64 * beat).collect(),
                        sig,
                    }
                })
                .collect(),
            end_ms: (n * per) as f64 * beat,
            exact: true,
        }
    }

    fn note(t: f64, dur: f64, key: u8) -> Note {
        Note {
            start_ms: t,
            dur_ms: dur,
            key,
            velocity: 100,
            legato: false,
        }
    }

    fn score(notes: &[Note], meter: &Meter) -> Score {
        let t = tab::arrange(notes, &TUNINGS[0]);
        build(
            "Test",
            meter,
            &[PartTab {
                name: "Lead".into(),
                program: 29,
                tuning: &TUNINGS[0],
                tab: &t,
            }],
        )
    }

    fn total(bar: &[Event]) -> u32 {
        bar.iter().map(|e| e.value.ticks()).sum()
    }

    #[test]
    fn values_have_lengths() {
        let v = |base, dotted, triplet| Value {
            base,
            dotted,
            triplet,
        };
        assert_eq!(v(4, false, false).ticks(), 24);
        assert_eq!(v(8, true, false).ticks(), 18);
        assert_eq!(v(8, false, true).ticks(), 8);
        assert_eq!(v(1, false, false).ticks(), 96);
        assert_eq!(Value::of(36, false), Some(v(4, true, false)));
        assert_eq!(Value::of(16, true), Some(v(4, false, true)));
    }

    #[test]
    fn writes_quarters_and_eighths() {
        // 120 BPM: a quarter, two eighths, a half; then a bar's rest.
        let m = even(500.0, 2, (4, 4));
        let notes = [
            note(0.0, 480.0, 64),
            note(500.0, 200.0, 65),
            note(750.0, 200.0, 67),
            note(1000.0, 1000.0, 69),
        ];
        let s = score(&notes, &m);
        assert_eq!(s.heads[0].sig, (4, 4));
        assert!((s.heads[0].bpm - 120.0).abs() < 1e-6);
        let bar = &s.tracks[0].bars[0];
        let bases: Vec<u8> = bar.iter().map(|e| e.value.base).collect();
        assert_eq!(bases, vec![4, 8, 8, 2], "{bar:?}");
        assert!(bar.iter().all(|e| e.notes.len() == 1));
        assert_eq!(total(bar), 96);
        // The empty bar: rests that fill it.
        let rest = &s.tracks[0].bars[1];
        assert_eq!(total(rest), 96);
        assert!(rest.iter().all(|e| e.notes.is_empty()));
    }

    #[test]
    fn rounds_and_ties() {
        // Slightly off the beat, and a note held over the bar line.
        let m = even(500.0, 2, (4, 4));
        let notes = [note(8.0, 480.0, 64), note(1495.0, 1000.0, 67)];
        let s = score(&notes, &m);
        let (a, b) = (&s.tracks[0].bars[0], &s.tracks[0].bars[1]);
        assert_eq!(total(a), 96);
        assert_eq!(total(b), 96);
        assert_eq!(a[0].value.base, 4);
        let last = a.last().unwrap();
        assert!(last.notes[0].tie_out && !last.notes[0].tie_in);
        assert!(b[0].notes[0].tie_in);
        assert_eq!(b[0].notes[0].key, last.notes[0].key);
    }

    #[test]
    fn finds_triplets() {
        // Triplet eighths on beat 2.
        let m = even(600.0, 1, (4, 4));
        let notes = [
            note(0.0, 550.0, 64),
            note(600.0, 190.0, 65),
            note(800.0, 190.0, 67),
            note(1000.0, 190.0, 69),
            note(1200.0, 1100.0, 71),
        ];
        let s = score(&notes, &m);
        let bar = &s.tracks[0].bars[0];
        assert_eq!(total(bar), 96);
        let trip: Vec<&Event> = bar.iter().filter(|e| e.value.triplet).collect();
        assert_eq!(trip.len(), 3, "{bar:?}");
        assert!(trip[0].tuplet_start && trip[2].tuplet_stop);
        assert!(!trip[1].tuplet_start && !trip[1].tuplet_stop);
    }

    #[test]
    fn writes_compound_time() {
        // 6/8: a dotted quarter, then three eighths.
        let m = even(600.0, 1, (6, 8));
        let notes = [
            note(0.0, 590.0, 64),
            note(600.0, 190.0, 65),
            note(800.0, 190.0, 67),
            note(1000.0, 190.0, 69),
        ];
        let s = score(&notes, &m);
        let bar = &s.tracks[0].bars[0];
        assert_eq!(total(bar), 72);
        let v: Vec<(u8, bool)> = bar.iter().map(|e| (e.value.base, e.value.dotted)).collect();
        assert_eq!(v, vec![(4, true), (8, false), (8, false), (8, false)]);
    }

    #[test]
    fn fills_odd_bars() {
        // A 3/4 bar and a 2/4 one (as tracker patterns broken off give).
        let mut m = even(500.0, 2, (3, 4));
        m.bars[1].sig = (2, 4);
        m.bars[1].beats.truncate(2);
        m.end_ms = 2500.0;
        let notes = [note(250.0, 2000.0, 60)];
        let s = score(&notes, &m);
        assert_eq!(total(&s.tracks[0].bars[0]), 72);
        assert_eq!(total(&s.tracks[0].bars[1]), 48);
    }
}
