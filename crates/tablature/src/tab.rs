//! Guitar (and bass) tablature from a part's notes.
//!
//! Each note can be played on several strings; the fingering is chosen
//! over the whole part at once (a shortest path), preferring to keep the
//! hand in one place, low frets, and chords within a hand's reach. A part
//! outside the instrument's range is moved by octaves to fit; single notes
//! still out of reach are folded into it an octave at a time.
//!
//! By hand, a part can be shown an octave or more higher or lower, and its
//! shapes nudged up or down the neck: each step up moves the lowest-fretted
//! notes of every passage onto a lower string, where they are played
//! higher up (each step down, the highest onto a higher string), and the
//! rest of the fingering follows to stay comfortable. A stretch of it (a
//! bar, say) can be nudged on its own the same way.

use crate::Note;

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
        name: "7-string guitar (B standard)",
        strings: &[35, 40, 45, 50, 55, 59, 64],
    },
    Tuning {
        name: "7-string guitar (drop A)",
        strings: &[33, 40, 45, 50, 55, 59, 64],
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

/// The tuning named `name`, if there is one.
pub fn tuning_named(name: &str) -> Option<usize> {
    TUNINGS.iter().position(|t| t.name == name)
}

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
    /// Played softer than the notes around it: an echo or a dying repeat
    /// (see [`soft_notes`]), shown in brackets.
    pub soft: bool,
}

/// How far back an echo looks for the louder note it repeats.
const ECHO_MS: f64 = 1000.0;

/// Which of `notes` are soft: quieter than a note of the same key shortly
/// before (an echo, the way chip tunes fake a delay or reverb, or a
/// repeat dying away), or far quieter than anything near them.
pub fn soft_notes(notes: &[Note]) -> Vec<bool> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|&a, &b| notes[a].start_ms.total_cmp(&notes[b].start_ms));
    let mut soft = vec![false; notes.len()];
    for (k, &i) in order.iter().enumerate() {
        let n = &notes[i];
        let v = n.velocity as u32;
        let mut loudest = 0u32;
        let mut echo = false;
        for &j in order[..k].iter().rev() {
            let m = &notes[j];
            if n.start_ms - m.start_ms > ECHO_MS {
                break;
            }
            loudest = loudest.max(m.velocity as u32);
            // Not the same note in two pieces (a chord, or a slide).
            if m.key == n.key
                && n.start_ms - m.start_ms >= CHORD_MS
                && v * 100 <= m.velocity as u32 * 92
            {
                echo = true;
            }
        }
        for &j in &order[k + 1..] {
            let m = &notes[j];
            if m.start_ms - n.start_ms > ECHO_MS {
                break;
            }
            loudest = loudest.max(m.velocity as u32);
        }
        soft[i] = echo || v * 10 <= loudest * 6;
    }
    soft
}

/// Changes made by hand to how a part is shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Adjust {
    /// Octaves up (or down) from where the part fits best.
    pub octave: i32,
    /// Steps up (or down) the neck from the most comfortable fingering.
    pub position: i32,
    /// Stretches nudged along the neck on their own, after the whole part
    /// is: each a further number of steps for the notes starting in it.
    pub spans: Vec<Span>,
}

/// A stretch of a part (a bar, or a few), nudged along the neck on its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start_ms: f64,
    pub end_ms: f64,
    /// Steps up (or down) the neck.
    pub steps: i32,
}

/// The most octaves a part can be moved by hand.
pub const MAX_OCTAVES: i32 = 3;
/// The most steps a part can be nudged along the neck.
pub const MAX_STEPS: i32 = 12;

#[derive(Clone, Debug, Default)]
pub struct Tab {
    pub notes: Vec<TabNote>,
    /// Semitones the part was moved to fit (a multiple of 12), not
    /// counting `octave`.
    pub transpose: i32,
    /// Octaves moved by hand.
    pub octave: i32,
    /// Steps nudged along the neck that changed something (fewer than
    /// asked for once nothing more can move).
    pub position: i32,
    /// The same, for each of the adjustment's spans, in order.
    pub spans: Vec<i32>,
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
    arrange_with(notes, tuning, Adjust::default())
}

/// The most comfortable fingering of each chord over the whole part (a
/// Viterbi search), notes outside each chord's `limits` (lowest, highest
/// fret) counting heavily against it, and chords `keep` gives a fingering
/// for kept to it: an index into each chord's options.
fn choose(options: &[Vec<Fingering>], limits: &[(u8, u8)], keep: &[Option<usize>]) -> Vec<usize> {
    let outside = |f: &Fingering, (lo, hi): (u8, u8)| -> f64 {
        f.places
            .iter()
            .map(|&(_, fret)| {
                let d = if fret < lo {
                    lo - fret
                } else {
                    fret.saturating_sub(hi)
                };
                if d > 0 {
                    4.0 + d as f64
                } else {
                    0.0
                }
            })
            .sum()
    };
    // Per option: (total cost, best previous option, where the hand is,
    // carried through open strings).
    let mut best: Vec<Vec<(f64, usize, Option<f64>)>> = Vec::with_capacity(options.len());
    for (i, opts) in options.iter().enumerate() {
        let row: Vec<(f64, usize, Option<f64>)> = opts
            .iter()
            .enumerate()
            .map(|(oi, o)| {
                let kept = match keep.get(i).copied().flatten() {
                    Some(k) if k != oi => 1000.0,
                    _ => 0.0,
                };
                let own = o.cost + outside(o, limits[i]) + kept;
                if i == 0 {
                    return (own, 0, moving(None, o.pos).1);
                }
                best[i - 1]
                    .iter()
                    .enumerate()
                    .map(|(j, &(c, _, hand))| {
                        let (m, to) = moving(hand, o.pos);
                        (c + m + own, j, to)
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .unwrap_or((own, 0, None))
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
    pick
}

/// How far either side of a chord its passage reaches, for nudging.
const PASSAGE_MS: f64 = 2000.0;

/// One step up (`up`) or down the neck from `pick` for the chords in
/// `range`: each chord's limits close in past the lowest (highest) fret
/// played around it, so those notes move to another string. Notes that
/// could not move before are passed over: the next ones in are moved
/// instead.
fn step_limits(
    options: &[Vec<Fingering>],
    pick: &[usize],
    times: &[f64],
    limits: &mut [(u8, u8)],
    range: std::ops::Range<usize>,
    up: bool,
) {
    let frets: Vec<Vec<u8>> = options
        .iter()
        .zip(pick)
        .map(|(o, &j)| {
            o.get(j)
                .map_or(Vec::new(), |f| f.places.iter().map(|p| p.1).collect())
        })
        .collect();
    let old = limits.to_vec();
    let (mut a, mut b) = (range.start, range.start);
    for i in range.clone() {
        while times[a] < times[i] - PASSAGE_MS {
            a += 1;
        }
        while b + 1 < range.end && times[b + 1] <= times[i] + PASSAGE_MS {
            b += 1;
        }
        let (lo, hi) = old[i];
        let near = frets[a..=b].iter().flatten().copied();
        if up {
            let m = near.filter(|&f| f >= lo).min().unwrap_or(lo);
            limits[i].0 = (m.max(lo) + 1).min(FRETS);
        } else {
            let m = near.filter(|&f| f <= hi).max().unwrap_or(hi);
            limits[i].1 = m.min(hi).saturating_sub(1);
        }
    }
}

/// Moves the chords in `range` `wanted` steps along the neck (up if
/// positive), a step at a time, each one moving something; it stops when
/// nothing more will move. The chords outside it stay as they are, unless
/// it is the whole part. The steps taken.
fn step_along(
    options: &[Vec<Fingering>],
    pick: &mut Vec<usize>,
    times: &[f64],
    limits: &mut [(u8, u8)],
    range: std::ops::Range<usize>,
    wanted: i32,
) -> i32 {
    let wanted = wanted.clamp(-MAX_STEPS, MAX_STEPS);
    let mut position = 0;
    if range.is_empty() || wanted == 0 {
        return 0;
    }
    let keep: Vec<Option<usize>> = (0..options.len())
        .map(|i| (!range.contains(&i)).then_some(pick[i]))
        .collect();
    'steps: while position != wanted {
        let up = wanted > 0;
        for _ in 0..FRETS {
            let before = limits.to_vec();
            step_limits(options, pick, times, limits, range.clone(), up);
            if limits == before.as_slice() {
                break 'steps;
            }
            let next = choose(options, limits, &keep);
            let moved = range.clone().any(|i| {
                options[i].get(next[i]).map(|f| &f.places)
                    != options[i].get(pick[i]).map(|f| &f.places)
            });
            *pick = next;
            if moved {
                position += if up { 1 } else { -1 };
                continue 'steps;
            }
        }
        break;
    }
    position
}

/// Lays `notes` out on an instrument tuned `tuning`, moved by hand as
/// `adjust` says.
pub fn arrange_with(notes: &[Note], tuning: &Tuning, adjust: Adjust) -> Tab {
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
    let octave = adjust.octave.clamp(-MAX_OCTAVES, MAX_OCTAVES);
    let shift = transpose + octave * 12;
    let mut folded = 0;
    let soft = soft_notes(notes);
    let mut sorted: Vec<(f64, f64, u8, bool)> = notes
        .iter()
        .zip(soft)
        .map(|(n, soft)| {
            let mut k = n.key as i32 + shift;
            if k < low as i32 || k > high as i32 {
                folded += 1;
            }
            while k < low as i32 {
                k += 12;
            }
            while k > high as i32 {
                k -= 12;
            }
            (n.start_ms, n.dur_ms, k as u8, soft)
        })
        .collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));

    // Chords: notes starting together, at most one per string.
    let mut chords: Vec<Vec<(f64, f64, u8, bool)>> = Vec::new();
    for n in sorted {
        match chords.last_mut() {
            Some(c) if n.0 - c[0].0 < CHORD_MS => match c.iter_mut().find(|m| m.2 == n.2) {
                // The same key twice (two parts together): loud if either is.
                Some(m) => m.3 &= n.3,
                None => c.push(n),
            },
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
    let mut limits = vec![(0u8, FRETS); options.len()];
    let mut pick = choose(&options, &limits, &[]);
    // Along the neck: the whole part, then each span on its own.
    let times: Vec<f64> = chords.iter().map(|c| c[0].0).collect();
    let position = step_along(
        &options,
        &mut pick,
        &times,
        &mut limits,
        0..options.len(),
        adjust.position,
    );
    let spans = adjust
        .spans
        .iter()
        .map(|sp| {
            let from = times.partition_point(|&t| t < sp.start_ms);
            let to = times.partition_point(|&t| t < sp.end_ms);
            step_along(
                &options,
                &mut pick,
                &times,
                &mut limits,
                from..to.max(from),
                sp.steps,
            )
        })
        .collect();
    let mut out = Vec::new();
    for ((chord, opts), &j) in chords.iter().zip(&options).zip(&pick) {
        let Some(f) = opts.get(j) else { continue };
        let played: Vec<&(f64, f64, u8, bool)> = if f.places.len() == chord.len() {
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
                soft: n.3,
            });
        }
    }
    Tab {
        notes: out,
        transpose,
        octave,
        position,
        spans,
        folded,
        string_names,
    }
}

/// A line of tab covering `start_ms..end_ms`, for drawing: each note's
/// place across the line (0..1), where the bar lines are, and where
/// playback is at which time.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub start_ms: f64,
    pub end_ms: f64,
    /// (x 0..1, string, fret, soft).
    pub cells: Vec<(f32, u8, u8, bool)>,
    /// (ms, x 0..1), in time order, from the line's start to its end: the
    /// playhead goes straight from one to the next.
    pub marks: Vec<(f64, f32)>,
    /// The bar the line starts in, if there are bars.
    pub bar: Option<usize>,
    /// Bar lines across the line: (x 0..1, the bar starting there). A line
    /// starting with a bar has it at 0.
    pub bars: Vec<(f32, usize)>,
}

impl Row {
    /// The bars across the line: (from x, to x, bar), 0..1.
    pub fn segments(&self) -> Vec<(f32, f32, usize)> {
        let mut out: Vec<(f32, f32, usize)> = Vec::new();
        let mut from = (0.0f32, self.bar);
        for &(x, i) in &self.bars {
            if let Some(b) = from.1 {
                if x > from.0 {
                    out.push((from.0, x, b));
                }
            }
            from = (x, Some(i));
        }
        if let Some(b) = from.1 {
            out.push((from.0, 1.0, b));
        }
        out
    }
}

/// Room kept at either end of a line, in digits.
const MARGIN: f32 = 1.0;
/// Space kept between two notes' numbers, in digits.
const SPACE: f32 = 1.2;
/// Space kept between a bar line and a note's number, in digits.
const BAR_SPACE: f32 = 0.8;
/// The most a rest can stretch, in digits.
const LONGEST_REST: f32 = 8.0;
/// How far apart the usual time between notes puts them, in digits.
const USUAL_GAP: f32 = 2.8;
/// A note this little before a bar line is played on it.
const EARLY_MS: f64 = 15.0;
/// A line breaks at a bar line if one is at least this far across.
const BREAK_AT_BAR: f32 = 0.6;

/// Something across a line: a chord (its time, (string, fret, soft) per
/// note, and width in digits) or a bar line (its time, and which bar
/// starts).
enum Item {
    Chord(f64, Vec<(u8, u8, bool)>, f32),
    Bar(f64, usize),
}

impl Item {
    fn t(&self) -> f64 {
        match self {
            Item::Chord(t, ..) | Item::Bar(t, _) => *t,
        }
    }
    fn w(&self) -> f32 {
        match self {
            Item::Chord(_, _, w) => *w,
            Item::Bar(..) => 0.0,
        }
    }
    fn space(&self) -> f32 {
        match self {
            Item::Chord(..) => SPACE,
            Item::Bar(..) => BAR_SPACE,
        }
    }
}

/// The tab cut into lines `width` digits wide (a digit: about the width
/// of a fret number, with its padding), from the start to `end_ms`, with
/// bar lines where `bars` start (ms, in order).
///
/// Notes are spaced by the time between them, but never closer than
/// their numbers allow, and rests are kept short: a line fills with as
/// many notes as fit legibly, breaking at a bar line where one is near
/// its end.
pub fn layout(tab: &Tab, width: f32, end_ms: f64, bars: &[f64]) -> Vec<Row> {
    let width = width.max(12.0);
    let mut items: Vec<Item> = Vec::new();
    let mut next_bar = bars.partition_point(|&b| b <= 0.5);
    let bar_at = |ms: f64| -> Option<usize> {
        (!bars.is_empty()).then(|| bars.partition_point(|&b| b <= ms + EARLY_MS).max(1) - 1)
    };
    let mut push_bars = |items: &mut Vec<Item>, until: f64| {
        while next_bar < bars.len() && bars[next_bar] - EARLY_MS <= until && bars[next_bar] < end_ms
        {
            items.push(Item::Bar(bars[next_bar], next_bar));
            next_bar += 1;
        }
    };
    for n in &tab.notes {
        match items.last_mut() {
            Some(Item::Chord(t, frets, _)) if n.start_ms - *t < CHORD_MS => {
                frets.push((n.string, n.fret, n.soft))
            }
            _ => {
                push_bars(&mut items, n.start_ms);
                items.push(Item::Chord(
                    n.start_ms,
                    vec![(n.string, n.fret, n.soft)],
                    0.0,
                ));
            }
        }
    }
    let end_ms = end_ms.max(items.last().map_or(0.0, |c| c.t() + 1.0));
    push_bars(&mut items, end_ms - EARLY_MS - 1.0);
    for it in &mut items {
        if let Item::Chord(_, frets, w) = it {
            // Two digits take two; brackets about one more.
            *w = frets
                .iter()
                .map(|p| if p.1 >= 10 { 2.0 } else { 1.0 } + if p.2 { 1.0 } else { 0.0 })
                .fold(1.0, f32::max);
        }
    }
    if items.is_empty() {
        return vec![Row {
            start_ms: 0.0,
            end_ms,
            cells: Vec::new(),
            marks: vec![(0.0, 0.0), (end_ms, 1.0)],
            bar: bar_at(0.0),
            bars: Vec::new(),
        }];
    }
    // Digits per ms: the usual gap between notes gets USUAL_GAP.
    let mut gaps: Vec<f64> = items
        .iter()
        .filter(|i| matches!(i, Item::Chord(..)))
        .map(Item::t)
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|&g| g > 0.0)
        .collect();
    gaps.sort_by(f64::total_cmp);
    let median = gaps.get(gaps.len() / 2).copied().unwrap_or(250.0).max(20.0);
    let scale = USUAL_GAP as f64 / median;
    let spread = |ms: f64| ((ms.max(0.0) * scale) as f32).min(LONGEST_REST);
    // How far each item is from the one before it on an endless line.
    let gap = |a: &Item, b: &Item| -> f32 {
        let room = (a.w() + b.w()) / 2.0
            + if matches!(a, Item::Bar(..)) || matches!(b, Item::Bar(..)) {
                BAR_SPACE
            } else {
                SPACE
            };
        spread(b.t() - a.t()).max(room)
    };
    // Where a line starting with `it` puts it.
    let first_x = |it: &Item, line_start: bool| -> f32 {
        match it {
            Item::Bar(..) if !line_start => 0.0,
            Item::Bar(t, _) => MARGIN / 2.0 + spread(*t).min(LONGEST_REST / 2.0),
            Item::Chord(t, _, w) if line_start => {
                MARGIN + w / 2.0 + spread(*t).min(LONGEST_REST / 2.0)
            }
            Item::Chord(_, _, w) => MARGIN + w / 2.0,
        }
    };

    let mut rows: Vec<Row> = Vec::new();
    let mut s = 0;
    while s < items.len() {
        let first_line = s == 0;
        // How far this line runs.
        let mut xs = vec![first_x(&items[s], first_line)];
        let mut e = s + 1;
        while e < items.len() {
            let x = xs[xs.len() - 1] + gap(&items[e - 1], &items[e]);
            if x + items[e].w() / 2.0 + MARGIN > width {
                break;
            }
            xs.push(x);
            e += 1;
        }
        // Break at a bar line instead, if one is far enough across.
        if e < items.len() {
            if let Some(b) = (s + 1..e)
                .rev()
                .find(|&i| matches!(items[i], Item::Bar(..)) && xs[i - s] >= BREAK_AT_BAR * width)
            {
                e = b;
                xs.truncate(b - s);
            }
        }
        let start_ms = if first_line { 0.0 } else { items[s].t() };
        let (row_end, tail) = match items.get(e) {
            Some(next) => {
                let last = &items[e - 1];
                let tail = xs[xs.len() - 1]
                    + spread(next.t() - last.t()).max(last.w() / 2.0 + last.space());
                (next.t(), tail.min(width - MARGIN / 2.0))
            }
            None => {
                let last = &items[e - 1];
                let tail = xs[xs.len() - 1] + spread(end_ms - last.t()).max(last.w() / 2.0 + SPACE);
                (end_ms, tail.min(width - MARGIN / 2.0))
            }
        };
        let mut row = Row {
            start_ms,
            end_ms: row_end,
            cells: Vec::new(),
            marks: Vec::new(),
            bar: bar_at(start_ms),
            bars: Vec::new(),
        };
        if first_line && items[0].t() > 0.0 {
            row.marks.push((0.0, 0.0));
        }
        for (it, &x) in items[s..e].iter().zip(&xs) {
            row.marks.push((it.t(), x / width));
            match it {
                Item::Chord(_, frets, _) => {
                    for &(string, fret, soft) in frets {
                        row.cells.push((x / width, string, fret, soft));
                    }
                }
                Item::Bar(_, i) => row.bars.push((x / width, *i)),
            }
        }
        if row_end > items[e - 1].t() {
            row.marks.push((row_end, tail / width));
        }
        // Times a hair apart (a note just before its bar line) kept in order.
        for k in 1..row.marks.len() {
            if row.marks[k].0 < row.marks[k - 1].0 {
                row.marks[k].0 = row.marks[k - 1].0;
            }
        }
        rows.push(row);
        s = e;
    }
    rows
}

/// Where playback is across `row` at `ms`, 0..1.
pub fn playhead(row: &Row, ms: f64) -> f32 {
    let m = &row.marks;
    let Some(i) = m.iter().position(|&(t, _)| t > ms) else {
        return m.last().map_or(0.0, |p| p.1);
    };
    if i == 0 {
        return m[0].1;
    }
    let (t0, x0) = m[i - 1];
    let (t1, x1) = m[i];
    let f = ((ms - t0) / (t1 - t0).max(1e-6)).clamp(0.0, 1.0) as f32;
    x0 + (x1 - x0) * f
}

/// The tab as text, the usual way: a line of dashes per string (highest
/// first), notes spaced by the time between them, with bar lines where
/// `bars` start (ms, in order), wrapped at `width`.
pub fn ascii(tab: &Tab, width: usize, bars: &[f64]) -> String {
    let strings = tab.string_names.len();
    if strings == 0 {
        return String::new();
    }
    // Columns: each chord, then dashes for the time until the next; a bar
    // line is a column of its own, empty.
    // A fret as written: soft notes in brackets.
    let mut cols: Vec<Vec<Option<String>>> = Vec::new();
    let mut i = 0;
    let notes = &tab.notes;
    let mut next_bar = bars.partition_point(|&b| b <= 0.5);
    while i < notes.len() {
        let t = notes[i].start_ms;
        while next_bar < bars.len() && bars[next_bar] - EARLY_MS <= t {
            cols.push(Vec::new());
            next_bar += 1;
        }
        let mut col = vec![None; strings];
        let mut j = i;
        while j < notes.len() && notes[j].start_ms - t < CHORD_MS {
            let n = &notes[j];
            col[n.string as usize] = Some(if n.soft {
                format!("({})", n.fret)
            } else {
                n.fret.to_string()
            });
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
                .map(|f| f.as_ref().map_or(1, String::len))
                .max()
                .map_or(1, |w| w + 1);
            if used + w > body && end > start {
                break;
            }
            used += w;
            end += 1;
        }
        for s in (0..strings).rev() {
            out.push_str(&format!("{:<label_w$}|", tab.string_names[s]));
            for (k, col) in cols[start..end].iter().enumerate() {
                if col.is_empty() {
                    // A bar line; the line's own ends stand for one there.
                    if k > 0 && start + k + 1 < end {
                        out.push('|');
                    }
                    continue;
                }
                let w = col
                    .iter()
                    .map(|f| f.as_ref().map_or(1, String::len))
                    .max()
                    .unwrap_or(1);
                let cell = col[s]
                    .as_ref()
                    .map_or_else(|| "-".repeat(w), |f| format!("{f:-<w$}"));
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
        let text = ascii(&tab, 60, &[]);
        assert_eq!(text.lines().filter(|l| l.contains('|')).count(), 6);
        assert!(text.starts_with("E|"), "{text}");
    }

    #[test]
    fn lays_out_lines_without_overlaps() {
        // Fast notes, some with two-digit frets: on a narrow line no two
        // numbers may touch, and every note is somewhere.
        let notes: Vec<Note> = (0..64)
            .map(|i| note(i as f64 * 60.0, if i % 3 == 0 { 76 } else { 64 }))
            .collect();
        let tab = arrange(&notes, &TUNINGS[0]);
        for width in [24.0, 40.0, 80.0] {
            let rows = layout(&tab, width, 10_000.0, &[]);
            assert!(rows.len() > 1);
            assert_eq!(rows.iter().map(|r| r.cells.len()).sum::<usize>(), 64);
            for r in &rows {
                let mut xs: Vec<f32> = r.cells.iter().map(|c| c.0 * width).collect();
                xs.dedup();
                for w in xs.windows(2) {
                    assert!(w[1] - w[0] >= 2.0, "{width}: {xs:?}");
                }
                assert!(xs.iter().all(|&x| x > 0.0 && x < width), "{xs:?}");
                // The playhead goes forwards through the line.
                assert!(r
                    .marks
                    .windows(2)
                    .all(|m| m[0].0 <= m[1].0 && m[0].1 <= m[1].1));
            }
            // The lines follow on from each other.
            for w in rows.windows(2) {
                assert_eq!(w[0].end_ms, w[1].start_ms);
            }
            assert_eq!(rows.last().unwrap().end_ms, 10_000.0);
        }
        // Narrower lines hold fewer notes.
        assert!(layout(&tab, 24.0, 0.0, &[]).len() > layout(&tab, 80.0, 0.0, &[]).len());
    }

    #[test]
    fn playhead_reaches_each_note() {
        let notes: Vec<Note> = (0..8)
            .map(|i| note(1000.0 + i as f64 * 250.0, 64))
            .collect();
        let tab = arrange(&notes, &TUNINGS[0]);
        let rows = layout(&tab, 60.0, 5000.0, &[]);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        for (i, c) in r.cells.iter().enumerate() {
            let at = playhead(r, 1000.0 + i as f64 * 250.0);
            assert!((at - c.0).abs() < 1e-4, "{at} {}", c.0);
        }
        assert!(playhead(r, 0.0) < r.cells[0].0);
        assert!(playhead(r, 5000.0) > r.cells[7].0);
    }

    fn riff() -> Vec<Note> {
        // A riff low on the neck: G A B C D, twice.
        [55u8, 57, 59, 60, 62, 55, 57, 59, 60, 62]
            .iter()
            .enumerate()
            .map(|(i, &k)| note(i as f64 * 200.0, k))
            .collect()
    }

    fn lowest_fret(tab: &Tab) -> u8 {
        tab.notes.iter().map(|n| n.fret).min().unwrap()
    }

    #[test]
    fn nudges_up_and_down_the_neck() {
        let notes = riff();
        let g = &TUNINGS[0];
        let base = arrange(&notes, g);
        let mut last = lowest_fret(&base);
        for steps in 1..=4 {
            let tab = arrange_with(
                &notes,
                g,
                Adjust {
                    octave: 0,
                    position: steps,
                    ..Default::default()
                },
            );
            assert_eq!(tab.position, steps);
            // The same notes, higher on the neck.
            for (n, m) in tab.notes.iter().zip(&base.notes) {
                assert_eq!(n.key, m.key);
                assert_eq!(g.strings[n.string as usize] + n.fret, n.key);
            }
            let low = lowest_fret(&tab);
            assert!(low > last, "{steps}: {low} after {last}");
            last = low;
        }
        // Down from there comes back towards the nut.
        let up = arrange_with(
            &notes,
            g,
            Adjust {
                octave: 0,
                position: 4,
                ..Default::default()
            },
        );
        let down = arrange_with(
            &notes,
            g,
            Adjust {
                octave: 0,
                position: -2,
                ..Default::default()
            },
        );
        assert!(down.position < 0);
        let top = |t: &Tab| t.notes.iter().map(|n| n.fret).max().unwrap();
        assert!(top(&down) <= top(&base) && top(&base) < top(&up));
        // As far as it goes, and no further.
        let far = arrange_with(
            &notes,
            g,
            Adjust {
                octave: 0,
                position: MAX_STEPS,
                ..Default::default()
            },
        );
        assert!(far.position > 0 && far.position <= MAX_STEPS);
    }

    #[test]
    fn shifts_by_octaves() {
        let notes = riff();
        let g = &TUNINGS[0];
        let base = arrange(&notes, g);
        let up = arrange_with(
            &notes,
            g,
            Adjust {
                octave: 1,
                position: 0,
                ..Default::default()
            },
        );
        assert_eq!(up.octave, 1);
        assert_eq!(up.transpose, base.transpose);
        for (n, m) in up.notes.iter().zip(&base.notes) {
            assert_eq!(n.key, m.key + 12);
        }
        let down = arrange_with(
            &notes,
            g,
            Adjust {
                octave: -1,
                position: 0,
                ..Default::default()
            },
        );
        for (n, m) in down.notes.iter().zip(&base.notes) {
            assert_eq!(n.key + 12, m.key);
        }
    }

    #[test]
    fn seven_strings() {
        let i = tuning_named("7-string guitar (drop A)").unwrap();
        let tab = arrange(&[note(0.0, 33)], &TUNINGS[i]);
        assert_eq!(tab.transpose, 0);
        assert_eq!((tab.notes[0].string, tab.notes[0].fret), (0, 0));
        assert_eq!(tab.string_names.len(), 7);
    }

    #[test]
    fn draws_bar_lines() {
        // Eighths at 250 ms, a bar every 2 s from 0.
        let notes: Vec<Note> = (0..64)
            .map(|i| note(i as f64 * 250.0, 60 + (i % 5) as u8))
            .collect();
        let tab = arrange(&notes, &TUNINGS[0]);
        let bars: Vec<f64> = (0..8).map(|i| i as f64 * 2000.0).collect();
        for width in [30.0, 50.0, 90.0] {
            let rows = layout(&tab, width, 16_000.0, &bars);
            assert_eq!(rows.iter().map(|r| r.cells.len()).sum::<usize>(), 64);
            // Every bar but the first starts at a line's start or a line.
            let mut seen: Vec<usize> = rows
                .iter()
                .flat_map(|r| r.bars.iter().map(|b| b.1))
                .collect();
            seen.dedup();
            assert_eq!(seen, (1..8).collect::<Vec<_>>(), "{width}");
            for r in &rows {
                // Bar lines clear of the numbers.
                for &(bx, _) in &r.bars {
                    for c in &r.cells {
                        assert!((c.0 - bx).abs() * width >= 1.0, "{width}: {bx} {}", c.0);
                    }
                }
                // The segments cover the line, in order, one bar each.
                let segs = r.segments();
                assert_eq!(segs.first().unwrap().0, 0.0);
                assert_eq!(segs.last().unwrap().1, 1.0);
                for w in segs.windows(2) {
                    assert_eq!(w[0].1, w[1].0);
                    assert_eq!(w[0].2 + 1, w[1].2);
                }
                assert!(r
                    .marks
                    .windows(2)
                    .all(|m| m[0].0 <= m[1].0 && m[0].1 <= m[1].1));
            }
            for w in rows.windows(2) {
                assert_eq!(w[0].end_ms, w[1].start_ms);
            }
        }
        // Wide enough lines break at bar lines.
        let rows = layout(&tab, 50.0, 16_000.0, &bars);
        assert!(rows.len() > 1);
        for r in &rows[1..] {
            assert_eq!(r.bars.first().map(|b| b.0), Some(0.0));
            assert_eq!(r.bar, Some(r.bars[0].1));
        }
        let text = ascii(&tab, 400, &bars);
        let top = text.lines().next().unwrap();
        assert_eq!(top.matches('|').count(), 2 + 7, "{top}");
    }

    #[test]
    fn nudges_a_bar_on_its_own() {
        let notes = riff();
        let g = &TUNINGS[0];
        let base = arrange(&notes, g);
        // The second half (from 1 s) up two steps; the first as it was.
        let tab = arrange_with(
            &notes,
            g,
            Adjust {
                spans: vec![Span {
                    start_ms: 1000.0,
                    end_ms: 2000.0,
                    steps: 2,
                }],
                ..Default::default()
            },
        );
        assert_eq!(tab.spans, vec![2]);
        assert_eq!(tab.position, 0);
        for (n, m) in tab.notes.iter().zip(&base.notes) {
            assert_eq!(n.key, m.key);
            if n.start_ms < 1000.0 {
                assert_eq!((n.string, n.fret), (m.string, m.fret));
            }
        }
        let low = |t: &Tab| {
            t.notes
                .iter()
                .filter(|n| n.start_ms >= 1000.0)
                .map(|n| n.fret)
                .min()
                .unwrap()
        };
        assert!(low(&tab) > low(&base));
        // A span with no notes moves nothing.
        let empty = arrange_with(
            &notes,
            g,
            Adjust {
                spans: vec![Span {
                    start_ms: 5000.0,
                    end_ms: 6000.0,
                    steps: 3,
                }],
                ..Default::default()
            },
        );
        assert_eq!(empty.spans, vec![0]);
    }

    #[test]
    fn echoes_are_soft() {
        let at = |t: f64, key: u8, velocity: u8| Note {
            velocity,
            ..note(t, key)
        };
        // A tune, each note echoed quieter a little later; a note repeated
        // as loud, and one played softly on its own, are not echoes.
        let notes = vec![
            at(0.0, 60, 110),
            at(150.0, 60, 70),
            at(300.0, 64, 110),
            at(450.0, 64, 70),
            at(600.0, 64, 110),
            at(2000.0, 67, 70),
        ];
        assert_eq!(
            soft_notes(&notes),
            vec![false, true, false, true, false, false]
        );
        let t = arrange(&notes, &TUNINGS[0]);
        assert_eq!(t.notes.iter().filter(|n| n.soft).count(), 2);
        let text = ascii(&t, 80, &[]);
        assert_eq!(text.matches('(').count(), 2, "{text}");
        let rows = layout(&t, 40.0, 3000.0, &[]);
        assert_eq!(
            rows.iter().flat_map(|r| &r.cells).filter(|c| c.3).count(),
            2
        );
    }
}
