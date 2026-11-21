//! Guitar (and bass) tablature from a part's notes.
//!
//! Each note can be played on several strings; the fingering is chosen
//! over the whole part at once (a shortest path), preferring to keep the
//! hand in one place, low frets, and chords within a hand's reach. A part
//! outside the instrument's range is moved by octaves to fit; single notes
//! still out of reach are folded into it an octave at a time.

use super::Note;

/// An instrument's open strings, lowest first, as MIDI keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tuning {
    pub name: &'static str,
    pub strings: &'static [u8],
}

pub const TUNINGS: &[Tuning] = &[
    Tuning {
        name: "Guitar (standard)",
        strings: &[40, 45, 50, 55, 59, 64],
    },
    Tuning {
        name: "Guitar (drop D)",
        strings: &[38, 45, 50, 55, 59, 64],
    },
    Tuning {
        name: "Guitar (half step down)",
        strings: &[39, 44, 49, 54, 58, 63],
    },
    Tuning {
        name: "Bass (4 strings)",
        strings: &[28, 33, 38, 43],
    },
    Tuning {
        name: "Bass (5 strings)",
        strings: &[23, 28, 33, 38, 43],
    },
    Tuning {
        name: "Ukulele",
        strings: &[67, 60, 64, 69],
    },
];

/// The highest fret used.
pub const FRETS: u8 = 22;
/// Notes starting this close together are played as one chord.
const CHORD_MS: f64 = 25.0;

#[derive(Clone, Debug, PartialEq)]
pub struct TabNote {
    pub start_ms: f64,
    pub dur_ms: f64,
    /// 0 is the lowest string.
    pub string: u8,
    pub fret: u8,
    /// The key as played (after any transposing).
    pub key: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Tab {
    pub notes: Vec<TabNote>,
    /// Semitones the part was moved to fit (a multiple of 12).
    pub transpose: i32,
    /// Notes moved by an octave or more on their own to fit.
    pub folded: usize,
    /// The strings' names, lowest first ("E", "A", ...).
    pub string_names: Vec<String>,
}

fn name(key: u8) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    N[key as usize % 12].to_string()
}

/// One way to play a chord: a fret per note, on distinct strings.
#[derive(Clone, Debug)]
struct Fingering {
    /// (string, fret) per note of the chord, in the chord's order.
    places: Vec<(u8, u8)>,
    cost: f64,
    /// The lowest and highest fretted frets (None: all open).
    pos: Option<(f64, f64)>,
}

fn fingerings(keys: &[u8], tuning: &Tuning) -> Vec<Fingering> {
    let mut out = Vec::new();
    let mut cur: Vec<(u8, u8)> = Vec::new();
    fn go(
        i: usize,
        keys: &[u8],
        tuning: &Tuning,
        cur: &mut Vec<(u8, u8)>,
        out: &mut Vec<Fingering>,
    ) {
        if out.len() > 400 {
            return;
        }
        if i == keys.len() {
            let fretted: Vec<f64> = cur.iter().filter(|p| p.1 > 0).map(|p| p.1 as f64).collect();
            let (lo, hi) = fretted
                .iter()
                .fold((f64::MAX, 0.0f64), |(a, b), &f| (a.min(f), b.max(f)));
            let span = if fretted.is_empty() { 0.0 } else { hi - lo };
            if span > 5.0 {
                return;
            }
            let pos = (!fretted.is_empty()).then_some((lo, hi));
            // High frets are harder to reach and read; wide chords strain.
            let cost = cur.iter().map(|p| p.1 as f64 * 0.03).sum::<f64>()
                + if span > 3.0 { (span - 3.0) * 1.5 } else { 0.0 };
            out.push(Fingering {
                places: cur.clone(),
                cost,
                pos,
            });
            return;
        }
        for (s, &open) in tuning.strings.iter().enumerate() {
            let s = s as u8;
            if keys[i] < open || keys[i] - open > FRETS || cur.iter().any(|p| p.0 == s) {
                continue;
            }
            cur.push((s, keys[i] - open));
            go(i + 1, keys, tuning, cur, out);
            cur.pop();
        }
    }
    go(0, keys, tuning, &mut cur, &mut out);
    out
}

/// Where the hand goes for a fingering, from where it was (the first fret
/// its four fingers cover), and the cost of getting there: none if the
/// frets are already under the fingers, else a shift.
fn moving(hand: Option<f64>, frets: Option<(f64, f64)>) -> (f64, Option<f64>) {
    match (hand, frets) {
        (Some(w), Some((lo, hi))) => {
            let to = if hi - 3.0 <= lo {
                w.clamp(hi - 3.0, lo)
            } else {
                lo
            };
            let d = (to - w).abs();
            (if d > 0.0 { 0.5 + d * 0.3 } else { 0.0 }, Some(to))
        }
        (None, Some((lo, _))) => (0.0, Some(lo)),
        (hand, None) => (0.0, hand),
    }
}

/// Lays `notes` out on an instrument tuned `tuning`.
pub fn arrange(notes: &[Note], tuning: &Tuning) -> Tab {
    let low = *tuning.strings.iter().min().unwrap_or(&40);
    let high = tuning.strings.iter().max().unwrap_or(&64) + FRETS;
    let string_names = tuning.strings.iter().map(|&k| name(k)).collect();
    if notes.is_empty() {
        return Tab {
            string_names,
            ..Default::default()
        };
    }
    // The octave move that fits the most notes, the smallest such move.
    let transpose = (-5..=5)
        .map(|o| o * 12)
        .max_by_key(|&s: &i32| {
            let fit = notes
                .iter()
                .filter(|n| (low as i32..=high as i32).contains(&(n.key as i32 + s)))
                .count();
            (fit, -s.abs())
        })
        .unwrap_or(0);
    let mut folded = 0;
    let mut sorted: Vec<(f64, f64, u8)> = notes
        .iter()
        .map(|n| {
            let mut k = n.key as i32 + transpose;
            if k < low as i32 || k > high as i32 {
                folded += 1;
            }
            while k < low as i32 {
                k += 12;
            }
            while k > high as i32 {
                k -= 12;
            }
            (n.start_ms, n.dur_ms, k as u8)
        })
        .collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));

    // Chords: notes starting together, at most one per string.
    let mut chords: Vec<Vec<(f64, f64, u8)>> = Vec::new();
    for n in sorted {
        match chords.last_mut() {
            Some(c) if n.0 - c[0].0 < CHORD_MS => {
                if !c.iter().any(|m| m.2 == n.2) {
                    c.push(n);
                }
            }
            _ => chords.push(vec![n]),
        }
    }
    for c in &mut chords {
        if c.len() > tuning.strings.len() {
            // Keep the outer notes and those between, highest first.
            c.sort_by_key(|n| std::cmp::Reverse(n.2));
            c.truncate(tuning.strings.len());
        }
    }

    // The best fingering of each chord, over the whole part (Viterbi).
    let options: Vec<Vec<Fingering>> = chords
        .iter()
        .map(|c| {
            let keys: Vec<u8> = c.iter().map(|n| n.2).collect();
            let mut f = fingerings(&keys, tuning);
            if f.is_empty() {
                // No way to play it all within reach: its top note alone.
                let top = *keys.iter().max().unwrap();
                f = fingerings(&[top], tuning);
            }
            f
        })
        .collect();
    // Per option: (total cost, best previous option, where the hand is,
    // carried through open strings).
    let mut best: Vec<Vec<(f64, usize, Option<f64>)>> = Vec::with_capacity(options.len());
    for (i, opts) in options.iter().enumerate() {
        let row: Vec<(f64, usize, Option<f64>)> = opts
            .iter()
            .map(|o| {
                if i == 0 {
                    return (o.cost, 0, moving(None, o.pos).1);
                }
                best[i - 1]
                    .iter()
                    .enumerate()
                    .map(|(j, &(c, _, hand))| {
                        let (m, to) = moving(hand, o.pos);
                        (c + m + o.cost, j, to)
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .unwrap_or((o.cost, 0, None))
            })
            .collect();
        best.push(row);
    }
    let mut pick = vec![0usize; options.len()];
    if let Some(last) = best.last() {
        let mut j = last
            .iter()
            .enumerate()
            .min_by(|a, b| a.1 .0.total_cmp(&b.1 .0))
            .map_or(0, |(j, _)| j);
        for i in (0..options.len()).rev() {
            pick[i] = j;
            j = best[i][j].1;
        }
    }
    let mut out = Vec::new();
    for ((chord, opts), &j) in chords.iter().zip(&options).zip(&pick) {
        let Some(f) = opts.get(j) else { continue };
        let played: Vec<&(f64, f64, u8)> = if f.places.len() == chord.len() {
            chord.iter().collect()
        } else {
            chord.iter().max_by_key(|n| n.2).into_iter().collect()
        };
        for (n, &(string, fret)) in played.iter().zip(&f.places) {
            out.push(TabNote {
                start_ms: n.0,
                dur_ms: n.1,
                string,
                fret,
                key: n.2,
            });
        }
    }
    Tab {
        notes: out,
        transpose,
        folded,
        string_names,
    }
}

/// A line of tab covering `start_ms..end_ms`, for drawing: each note's
/// place across the line (0..1).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub start_ms: f64,
    pub end_ms: f64,
    /// (x 0..1, string, fret).
    pub cells: Vec<(f32, u8, u8)>,
}

/// How long a line of tab should be for these notes: about sixteen notes
/// to a line, in whole seconds.
pub fn row_length_ms(tab: &Tab) -> f64 {
    let mut starts: Vec<f64> = tab.notes.iter().map(|n| n.start_ms).collect();
    starts.dedup_by(|a, b| (*a - *b).abs() < CHORD_MS);
    let mut gaps: Vec<f64> = starts.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_by(f64::total_cmp);
    let median = gaps.get(gaps.len() / 2).copied().unwrap_or(250.0);
    ((median * 16.0) / 1000.0).round().clamp(2.0, 10.0) * 1000.0
}

/// The tab cut into lines of `row_ms`, from the start to `end_ms`.
pub fn rows(tab: &Tab, row_ms: f64, end_ms: f64) -> Vec<Row> {
    let end = end_ms.max(
        tab.notes
            .iter()
            .map(|n| n.start_ms + 1.0)
            .fold(0.0, f64::max),
    );
    let count = (end / row_ms).ceil().max(1.0) as usize;
    let mut rows: Vec<Row> = (0..count)
        .map(|i| Row {
            start_ms: i as f64 * row_ms,
            end_ms: (i + 1) as f64 * row_ms,
            cells: Vec::new(),
        })
        .collect();
    for n in &tab.notes {
        let i = ((n.start_ms / row_ms) as usize).min(count - 1);
        let r = &mut rows[i];
        let x = ((n.start_ms - r.start_ms) / row_ms) as f32;
        r.cells.push((x.clamp(0.0, 1.0), n.string, n.fret));
    }
    rows
}

/// The tab as text, the usual way: a line of dashes per string (highest
/// first), notes spaced by the time between them, wrapped at `width`.
pub fn ascii(tab: &Tab, width: usize) -> String {
    let strings = tab.string_names.len();
    if strings == 0 {
        return String::new();
    }
    // Columns: each chord, then dashes for the time until the next.
    let mut cols: Vec<Vec<Option<u8>>> = Vec::new();
    let mut i = 0;
    let notes = &tab.notes;
    while i < notes.len() {
        let t = notes[i].start_ms;
        let mut col = vec![None; strings];
        let mut j = i;
        while j < notes.len() && notes[j].start_ms - t < CHORD_MS {
            col[notes[j].string as usize] = Some(notes[j].fret);
            j += 1;
        }
        cols.push(col);
        let gap = notes.get(j).map_or(250.0, |n| n.start_ms - t);
        let dashes = (gap / 125.0).round().clamp(1.0, 8.0) as usize;
        for _ in 1..dashes {
            cols.push(vec![None; strings]);
        }
        i = j;
    }
    let label_w = tab.string_names.iter().map(|n| n.len()).max().unwrap_or(1);
    let mut out = String::new();
    let mut start = 0;
    let body = width.saturating_sub(label_w + 2).max(8);
    while start < cols.len() {
        // Columns as wide as their widest fret number.
        let mut used = 0;
        let mut end = start;
        while end < cols.len() {
            let w = cols[end]
                .iter()
                .map(|f| f.map_or(1, |f| f.to_string().len()))
                .max()
                .unwrap_or(1)
                + 1;
            if used + w > body && end > start {
                break;
            }
            used += w;
            end += 1;
        }
        for s in (0..strings).rev() {
            out.push_str(&format!("{:<label_w$}|", tab.string_names[s]));
            for col in &cols[start..end] {
                let w = col
                    .iter()
                    .map(|f| f.map_or(1, |f| f.to_string().len()))
                    .max()
                    .unwrap_or(1);
                let cell = col[s].map_or_else(|| "-".repeat(w), |f| format!("{f:-<w$}"));
                out.push_str(&cell);
                out.push('-');
            }
            out.push_str("|\n");
        }
        out.push('\n');
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(t: f64, key: u8) -> Note {
        Note {
            start_ms: t,
            dur_ms: 200.0,
            key,
            velocity: 100,
            legato: false,
        }
    }

    #[test]
    fn plays_a_scale_in_one_place() {
        // C major from C4: should stay low on the neck, no jumps.
        let keys = [60, 62, 64, 65, 67, 69, 71, 72];
        let notes: Vec<Note> = keys
            .iter()
            .enumerate()
            .map(|(i, &k)| note(i as f64 * 250.0, k))
            .collect();
        let tab = arrange(&notes, &TUNINGS[0]);
        assert_eq!(tab.notes.len(), 8);
        assert_eq!(tab.transpose, 0);
        for (n, &k) in tab.notes.iter().zip(&keys) {
            assert_eq!(TUNINGS[0].strings[n.string as usize] + n.fret, k);
        }
        let frets: Vec<u8> = tab.notes.iter().map(|n| n.fret).collect();
        let span = frets.iter().max().unwrap() - frets.iter().filter(|&&f| f > 0).min().unwrap();
        assert!(span <= 5, "{frets:?}");
    }

    #[test]
    fn moves_parts_into_range_and_plays_chords() {
        // Two octaves too high, with a chord.
        let notes = vec![
            note(0.0, 100),
            note(0.0, 103),
            note(0.0, 107),
            note(500.0, 110),
        ];
        let tab = arrange(&notes, &TUNINGS[0]);
        assert_eq!(tab.transpose, -24);
        let chord: Vec<_> = tab.notes.iter().filter(|n| n.start_ms == 0.0).collect();
        assert_eq!(chord.len(), 3);
        let strings: std::collections::HashSet<u8> = chord.iter().map(|n| n.string).collect();
        assert_eq!(strings.len(), 3);
        let text = ascii(&tab, 60);
        assert_eq!(text.lines().filter(|l| l.contains('|')).count(), 6);
        assert!(text.starts_with("E|"), "{text}");
    }

    #[test]
    fn cuts_into_rows() {
        let notes: Vec<Note> = (0..40).map(|i| note(i as f64 * 250.0, 64)).collect();
        let tab = arrange(&notes, &TUNINGS[0]);
        let ms = row_length_ms(&tab);
        assert_eq!(ms, 4000.0);
        let rows = rows(&tab, ms, 10_000.0);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].cells.len(), 16);
    }
}
