//! Beats and bars: where they fall in a track, for bar lines in the guitar
//! tab, nudging its fingering a bar at a time, and writing the notes out
//! as sheet music.
//!
//! MIDI files say where they fall (their tempo and time signatures), and
//! tracker modules nearly do: their notes are written in rows, four rows
//! to a beat by custom, and patterns start on a bar. Other chiptunes hold
//! only their sound driver's code, so the beat is worked out from the
//! notes ([`guess`]): the shortest regular step they move in (the driver's
//! rows), the beat as the multiple of it that the notes lean on most, and
//! the bars from where the bass and the drums put their weight. It can be
//! wrong (twice or half the speed, a bar line a beat early), so a
//! [`MeterAdjust`] puts it right by hand.

use crate::Part;

/// One bar: when it starts, when each of its beats does, and its time
/// signature.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub start_ms: f64,
    /// When each beat starts; the first is `start_ms`.
    pub beats: Vec<f64>,
    /// (beats, note value): (4, 4), (3, 4), (6, 8)...
    pub sig: (u8, u8),
}

/// Compound time (6/8, 9/8, 12/8): each beat is three eighths.
pub fn compound(sig: (u8, u8)) -> bool {
    sig.1 == 8 && sig.0.is_multiple_of(3) && sig.0 > 3
}

/// How many beats a bar of this signature has.
pub fn beats_in(sig: (u8, u8)) -> usize {
    if compound(sig) {
        sig.0 as usize / 3
    } else {
        sig.0.max(1) as usize
    }
}

/// The time signatures that can be chosen by hand.
pub const SIGNATURES: &[(u8, u8)] = &[
    (2, 4),
    (3, 4),
    (4, 4),
    (5, 4),
    (6, 8),
    (7, 8),
    (9, 8),
    (12, 8),
];

impl Bar {
    /// Quarter notes in one of its beats.
    pub fn beat_quarters(&self) -> f64 {
        4.0 / self.sig.1.max(1) as f64 * if compound(self.sig) { 3.0 } else { 1.0 }
    }

    /// Quarter notes in the bar.
    pub fn quarters(&self) -> f64 {
        self.sig.0 as f64 * 4.0 / self.sig.1.max(1) as f64
    }
}

/// Where the bars of a track fall.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meter {
    pub bars: Vec<Bar>,
    /// When the last bar ends.
    pub end_ms: f64,
    /// Read from the file, rather than worked out from the notes.
    pub exact: bool,
}

/// Changes made by hand to where the bars fall.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MeterAdjust {
    /// The time signature, if not as found.
    pub sig: Option<(u8, u8)>,
    /// The beat twice as fast for each step up (half as fast down).
    pub speed: i32,
    /// The bar lines moved this many beats later (earlier if negative).
    pub shift: i32,
}

/// How far the speed can be doubled or halved.
pub const MAX_SPEED: i32 = 2;

impl Meter {
    /// When bar `i` ends.
    pub fn bar_end(&self, i: usize) -> f64 {
        self.bars.get(i + 1).map_or(self.end_ms, |b| b.start_ms)
    }

    /// The bar playing at `ms` (the first before it starts, the last
    /// after it ends).
    pub fn bar_at(&self, ms: f64) -> Option<usize> {
        if self.bars.is_empty() {
            return None;
        }
        Some(self.bars.partition_point(|b| b.start_ms <= ms).max(1) - 1)
    }

    /// When each bar starts.
    pub fn starts(&self) -> Vec<f64> {
        self.bars.iter().map(|b| b.start_ms).collect()
    }

    /// The tempo of bar `i`, in quarter notes a minute.
    pub fn bpm(&self, i: usize) -> f64 {
        let Some(b) = self.bars.get(i) else {
            return 120.0;
        };
        let ms = (self.bar_end(i) - b.start_ms).max(1.0);
        60_000.0 * b.quarters() / ms
    }

    /// The tempo in beats a minute (of the beat the signature counts:
    /// dotted quarters in 6/8), over most of the track.
    pub fn beats_per_minute(&self) -> f64 {
        let mut v: Vec<f64> = (0..self.bars.len())
            .map(|i| self.bpm(i) / self.bars[i].beat_quarters())
            .collect();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(120.0)
    }

    /// The time signature most bars have.
    pub fn main_sig(&self) -> (u8, u8) {
        let mut counts: Vec<((u8, u8), usize)> = Vec::new();
        for b in &self.bars {
            match counts.iter_mut().find(|c| c.0 == b.sig) {
                Some(c) => c.1 += 1,
                None => counts.push((b.sig, 1)),
            }
        }
        counts.iter().max_by_key(|c| c.1).map_or((4, 4), |c| c.0)
    }

    /// For turning times into quarter notes from the first bar's start.
    pub fn ruler(&self) -> Ruler<'_> {
        let mut q = Vec::with_capacity(self.bars.len());
        let mut at = 0.0;
        for b in &self.bars {
            q.push(at);
            at += b.quarters();
        }
        Ruler { m: self, q }
    }

    /// The bars moved as `a` says.
    pub fn adjusted(&self, a: &MeterAdjust) -> Meter {
        if *a == MeterAdjust::default() || self.bars.is_empty() {
            return self.clone();
        }
        // Every beat, and the end.
        let mut beats: Vec<f64> = self
            .bars
            .iter()
            .flat_map(|b| b.beats.iter().copied())
            .collect();
        beats.push(self.end_ms);
        let speed = a.speed.clamp(-MAX_SPEED, MAX_SPEED);
        if speed > 0 {
            let n = 1usize << speed;
            let mut faster = Vec::with_capacity(beats.len() * n);
            for w in beats.windows(2) {
                for k in 0..n {
                    faster.push(w[0] + (w[1] - w[0]) * k as f64 / n as f64);
                }
            }
            faster.push(*beats.last().unwrap());
            beats = faster;
        } else if speed < 0 {
            let n = 1usize << -speed;
            let last = *beats.last().unwrap();
            let mut slower: Vec<f64> = beats[..beats.len() - 1]
                .iter()
                .copied()
                .step_by(n)
                .collect();
            // The last beat runs as long as the others.
            let step = match slower.len() {
                0 | 1 => last - slower.first().copied().unwrap_or(0.0),
                k => slower[k - 1] - slower[k - 2],
            };
            slower.push((slower.last().copied().unwrap_or(0.0) + step).max(last));
            beats = slower;
        }
        let sig = a.sig.unwrap_or_else(|| self.main_sig());
        let n = beats_in(sig);
        let step = |i: usize, beats: &[f64]| (beats[i + 1] - beats[i]).max(1.0);
        // A bar line `shift` beats later: the beats before it make up a
        // whole bar with ones from before the start.
        let s = a.shift.rem_euclid(n as i32) as usize;
        if s > 0 && beats.len() > 1 {
            let d = step(0, &beats);
            let first = beats[0];
            let mut before: Vec<f64> = (1..=n - s).rev().map(|k| first - d * k as f64).collect();
            before.extend(beats);
            beats = before;
        }
        // The last bar made whole.
        let end = beats.pop().unwrap_or(0.0);
        if beats.is_empty() {
            return self.clone();
        }
        let d = (end - beats[beats.len() - 1]).max(1.0);
        let mut end = end;
        while !beats.len().is_multiple_of(n) {
            beats.push(end);
            end += d;
        }
        let bars = beats
            .chunks(n)
            .map(|c| Bar {
                start_ms: c[0],
                beats: c.to_vec(),
                sig,
            })
            .collect();
        Meter {
            bars,
            end_ms: end,
            exact: self.exact,
        }
    }

    /// "4/4, ♩ = 120".
    pub fn describe(&self) -> String {
        let sig = self.main_sig();
        let bpm = self.beats_per_minute().round();
        let note = if compound(sig) {
            "♩."
        } else {
            match sig.1 {
                2 => "𝅗𝅥",
                8 => "♪",
                _ => "♩",
            }
        };
        format!("{}/{}, {note} = {bpm}", sig.0, sig.1)
    }
}

/// Turns times into quarter notes, counted along the beats.
pub struct Ruler<'a> {
    m: &'a Meter,
    /// Quarter notes before each bar.
    q: Vec<f64>,
}

impl Ruler<'_> {
    /// Quarter notes from the start of the first bar to `ms` (negative
    /// before it).
    pub fn quarters(&self, ms: f64) -> f64 {
        let bars = &self.m.bars;
        let Some(first) = bars.first() else {
            return ms / 500.0;
        };
        if ms < first.start_ms {
            let beat = first.beats.get(1).copied().unwrap_or(self.m.bar_end(0)) - first.start_ms;
            return (ms - first.start_ms) / beat.max(1.0) * first.beat_quarters();
        }
        let i = self.m.bar_at(ms).unwrap_or(0);
        let b = &bars[i];
        let j = b.beats.partition_point(|&t| t <= ms).max(1) - 1;
        let t0 = b.beats[j];
        let t1 = b.beats.get(j + 1).copied().unwrap_or(self.m.bar_end(i));
        let frac = (ms - t0) / (t1 - t0).max(1e-6);
        self.q[i] + (j as f64 + frac) * b.beat_quarters()
    }

    /// Quarter notes before bar `i`.
    pub fn bar_start(&self, i: usize) -> f64 {
        self.q.get(i).copied().unwrap_or(0.0)
    }
}

/// The bars of a Standard MIDI file: its time signatures (`(tick, (beats,
/// note value))`) at `ppq` ticks a quarter note, timed through `ms`, up to
/// `end_tick`.
pub fn from_ticks(
    sigs: &[(u64, (u8, u8))],
    ppq: u64,
    end_tick: u64,
    ms: impl Fn(u64) -> f64,
) -> Option<Meter> {
    if ppq == 0 {
        return None;
    }
    let mut bars = Vec::new();
    let (mut tick, mut sig, mut si) = (0u64, (4u8, 4u8), 0);
    while tick < end_tick.max(1) && bars.len() < 100_000 {
        while si < sigs.len() && sigs[si].0 <= tick {
            sig = sigs[si].1;
            si += 1;
        }
        let bar_ticks = (sig.0 as u64 * ppq * 4 / sig.1.max(1) as u64).max(1);
        let n = beats_in(sig) as u64;
        bars.push(Bar {
            start_ms: ms(tick),
            beats: (0..n).map(|k| ms(tick + k * bar_ticks / n)).collect(),
            sig,
        });
        tick += bar_ticks;
    }
    Some(Meter {
        bars,
        end_ms: ms(tick),
        exact: true,
    })
}

/// The bars of a tracker module, from when each row started: (ms, order
/// position, row number), in playing order.
pub fn from_rows(rows: &[(f64, i32, i32)], end_ms: f64) -> Option<Meter> {
    if rows.len() < 8 {
        return None;
    }
    let mut steps: Vec<f64> = rows
        .windows(2)
        .map(|w| w[1].0 - w[0].0)
        .filter(|&d| d > 0.0)
        .collect();
    steps.sort_by(f64::total_cmp);
    let row_ms = steps.get(steps.len() / 2).copied()?.max(1.0);
    // Four rows to a beat, unless that makes the beat absurd.
    let rpb: i32 = if row_ms * 4.0 < 240.0 {
        8
    } else if row_ms * 4.0 > 1200.0 {
        2
    } else {
        4
    };
    // Runs of rows through one pattern.
    let mut runs: Vec<&[(f64, i32, i32)]> = Vec::new();
    let mut start = 0;
    for i in 1..=rows.len() {
        if i == rows.len() || rows[i].1 != rows[i - 1].1 || rows[i].2 <= rows[i - 1].2 {
            runs.push(&rows[start..i]);
            start = i;
        }
    }
    // The bar: four beats, or three if the patterns come in threes.
    let mut lengths: Vec<(i32, usize)> = Vec::new();
    for r in &runs {
        let len = r.last().unwrap().2 + 1;
        match lengths.iter_mut().find(|l| l.0 == len) {
            Some(l) => l.1 += 1,
            None => lengths.push((len, 1)),
        }
    }
    let common = lengths.iter().max_by_key(|l| l.1).map_or(64, |l| l.0);
    let beats_per_bar = if common % (4 * rpb) != 0 && common % (3 * rpb) == 0 {
        3
    } else {
        4
    };
    let per_bar = beats_per_bar * rpb;
    let mut bars: Vec<Bar> = Vec::new();
    for r in runs {
        let mut i = 0;
        while i < r.len() {
            // This bar's rows, as numbered in the pattern.
            let first = r[i].2;
            let bar_end_row = (first.div_euclid(per_bar) + 1) * per_bar;
            let j = i + r[i..].iter().take_while(|x| x.2 < bar_end_row).count();
            let beats: Vec<f64> = r[i..j]
                .iter()
                .enumerate()
                .filter(|(k, x)| *k == 0 || x.2 % rpb == 0)
                .map(|(_, x)| x.0)
                .collect();
            let whole = first % per_bar == 0 && j - i == per_bar as usize;
            bars.push(Bar {
                start_ms: r[i].0,
                sig: if whole {
                    (beats_per_bar as u8, 4)
                } else {
                    (beats.len().max(1) as u8, 4)
                },
                beats,
            });
            i = j.max(i + 1);
        }
    }
    Some(Meter {
        bars,
        end_ms: end_ms.max(rows.last().unwrap().0 + row_ms),
        exact: true,
    })
}

/// A note start, weighed for finding the beat (`w`) and the bar (`accent`).
#[derive(Clone, Copy, Debug)]
struct Onset {
    t: f64,
    w: f64,
    accent: f64,
}

/// Notes starting this close together count as one start.
const TOGETHER_MS: f64 = 12.0;

fn onsets(parts: &[Part]) -> Vec<Onset> {
    let median = |p: &Part| {
        let mut k: Vec<u8> = p.notes.iter().map(|n| n.key).collect();
        k.sort_unstable();
        k.get(k.len() / 2).copied().unwrap_or(60)
    };
    // The bass: the lowest pitched part with a fair number of notes.
    let most = parts
        .iter()
        .filter(|p| !p.drums)
        .map(|p| p.notes.len())
        .max()
        .unwrap_or(0);
    let bass = parts
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.drums && p.notes.len() * 6 >= most && p.notes.len() >= 8)
        .min_by_key(|(_, p)| median(p))
        .map(|(i, _)| i);
    let mut v = Vec::new();
    for (pi, p) in parts.iter().enumerate() {
        let mut prev_key = None;
        for n in &p.notes {
            let long = (n.dur_ms / 300.0).clamp(0.3, 2.0);
            let (w, accent) = if p.drums {
                match n.key {
                    35 | 36 => (1.5, 2.5),
                    38 | 40 => (1.2, 0.6),
                    _ => (0.6, 0.15),
                }
            } else if n.legato {
                (0.3, 0.0)
            } else if Some(pi) == bass {
                let change = if prev_key == Some(n.key) { 1.0 } else { 2.0 };
                (1.2, change * long)
            } else {
                (1.0, 0.4 * long)
            };
            prev_key = Some(n.key);
            v.push(Onset {
                t: n.start_ms,
                w,
                accent,
            });
        }
    }
    v.sort_by(|a, b| a.t.total_cmp(&b.t));
    let mut out: Vec<Onset> = Vec::with_capacity(v.len());
    for o in v {
        match out.last_mut() {
            Some(l) if o.t - l.t < TOGETHER_MS => {
                l.w += o.w;
                l.accent += o.accent;
            }
            _ => out.push(o),
        }
    }
    // Weights level off: a big chord isn't worth ten notes.
    for o in &mut out {
        o.w = o.w.min(3.0);
    }
    out
}

/// How regularly the onsets fall on a grid `period` ms apart, 0..1: the
/// alignment of their phases, over windows short enough that a period a
/// little out still lines up.
fn coherence(on: &[Onset], period: f64) -> f64 {
    const WINDOW_MS: f64 = 6000.0;
    let (mut total, mut weight) = (0.0, 0.0);
    let (mut re, mut im, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
    let mut window_end = on.first().map_or(0.0, |o| o.t) + WINDOW_MS;
    for o in on {
        if o.t >= window_end {
            total += (re * re + im * im).sqrt();
            weight += wsum;
            (re, im, wsum) = (0.0, 0.0, 0.0);
            while window_end <= o.t {
                window_end += WINDOW_MS;
            }
        }
        let a = std::f64::consts::TAU * o.t / period;
        re += o.w * a.cos();
        im += o.w * a.sin();
        wsum += o.w;
    }
    total += (re * re + im * im).sqrt();
    weight += wsum;
    if weight > 0.0 {
        total / weight
    } else {
        0.0
    }
}

/// The shortest regular step the notes move in (the driver's rows, or
/// the beat itself for slow parts), ms.
fn tatum(on: &[Onset]) -> Option<f64> {
    const LOW: f64 = 70.0;
    const HIGH: f64 = 1500.0;
    let mut scored = Vec::new();
    let mut p = LOW;
    while p <= HIGH {
        scored.push((p, coherence(on, p)));
        p *= 1.001;
    }
    let best = scored.iter().map(|s| s.1).fold(0.0, f64::max);
    if best < 0.3 {
        return None;
    }
    // Shorter steps fit as well (every other one empty): the longest
    // that fits nearly as well as the best, at the top of its peak.
    let (mut at, _) = *scored.iter().rev().find(|s| s.1 >= 0.8 * best)?;
    let i = scored.iter().position(|s| s.0 == at)?;
    let mut k = i;
    while k > 0 && scored[k - 1].1 >= scored[k].1 {
        k -= 1;
    }
    let mut j = i;
    while j + 1 < scored.len() && scored[j + 1].1 >= scored[j].1 {
        j += 1;
    }
    if scored[k].1 > scored[j].1 {
        at = scored[k].0;
    } else {
        at = scored[j].0;
    }
    Some(at)
}

/// The grid of steps `period` apart through the onsets, from the first,
/// following them as they drift: each step lands on a note starting near
/// where it is due, and the period follows what the notes do.
fn track(on: &[Onset], period: f64, end_ms: f64) -> Vec<f64> {
    let Some(first) = on.first() else {
        return Vec::new();
    };
    let mut grid = vec![first.t];
    let mut p = period;
    let (mut last_n, mut last_t) = (0usize, first.t);
    let mut k = 0;
    let end = end_ms.max(on.last().unwrap().t) + period;
    while *grid.last().unwrap() < end && grid.len() < 200_000 {
        let n = grid.len();
        let due = grid[n - 1] + p;
        while k < on.len() && on[k].t < due - 0.25 * p {
            k += 1;
        }
        let near = on[k..]
            .iter()
            .take_while(|o| o.t <= due + 0.25 * p)
            .filter(|o| o.w >= 0.5)
            .min_by(|a, b| (a.t - due).abs().total_cmp(&(b.t - due).abs()));
        match near {
            Some(o) => {
                let seen = (o.t - last_t) / (n - last_n) as f64;
                p = (p + 0.2 * (seen - p)).clamp(period * 0.85, period * 1.15);
                (last_n, last_t) = (n, o.t);
                grid.push(o.t);
            }
            None => grid.push(due),
        }
    }
    grid
}

/// The grid step a time falls on, if it is near one.
fn place(grid: &[f64], t: f64, step: f64) -> Option<usize> {
    let i = grid.partition_point(|&g| g <= t);
    [i.saturating_sub(1), i.min(grid.len().saturating_sub(1))]
        .into_iter()
        .filter(|&n| n < grid.len())
        .map(|n| (n, (grid[n] - t).abs()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|&(_, d)| d <= 0.3 * step)
        .map(|(n, _)| n)
}

/// How alike beats `lag` beats apart are, 0..1: the share of what starts
/// in them (which note, in which part, where in the beat) they have in
/// common. Beats are grid steps `j`, `j + k`, ...
fn repeats(parts: &[Part], grid: &[f64], step: f64, j: usize, k: usize, lag: usize) -> f64 {
    let beats = grid.len() / k + 1;
    let mut seen: Vec<Vec<(usize, u8, usize)>> = vec![Vec::new(); beats];
    for (pi, p) in parts.iter().enumerate() {
        for n in &p.notes {
            let Some(g) = place(grid, n.start_ms, step) else {
                continue;
            };
            if g < j {
                continue;
            }
            seen[(g - j) / k].push(((g - j) % k, n.key, pi));
        }
    }
    for s in &mut seen {
        s.sort_unstable();
        s.dedup();
    }
    let (mut same, mut all) = (0usize, 0usize);
    for b in 0..beats.saturating_sub(lag) {
        let (x, y) = (&seen[b], &seen[b + lag]);
        if x.is_empty() && y.is_empty() {
            continue;
        }
        let common = x.iter().filter(|e| y.binary_search(e).is_ok()).count();
        same += common;
        all += x.len() + y.len() - common;
    }
    if all == 0 {
        0.0
    } else {
        same as f64 / all as f64
    }
}

/// The beats and bars of a track, worked out from its parts' notes, up to
/// `end_ms`. None if there are too few notes to tell.
pub fn guess(parts: &[Part], end_ms: f64) -> Option<Meter> {
    let on = onsets(parts);
    if on.len() < 8 {
        return None;
    }
    // Two minutes is plenty to find the step.
    let cut = on.partition_point(|o| o.t < on[0].t + 120_000.0);
    let step = tatum(&on[..cut])?;
    let grid = track(&on, step, end_ms);
    if grid.len() < 4 {
        return None;
    }
    // Which grid step each onset falls on.
    let mut placed: Vec<(usize, Onset)> = Vec::new();
    for o in &on {
        let i = grid.partition_point(|&g| g <= o.t);
        let cands = [i.saturating_sub(1), i.min(grid.len() - 1)];
        let (n, d) = cands
            .iter()
            .map(|&n| (n, (grid[n] - o.t).abs()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        if d <= 0.3 * step {
            placed.push((n, *o));
        }
    }
    // The beat: a multiple of the step the notes lean on, near half a
    // second.
    let lean = |k: usize, value: &dyn Fn(&Onset) -> f64| -> (f64, usize) {
        let mut by = vec![0.0; k];
        for (n, o) in &placed {
            by[n % k] += value(o);
        }
        let total: f64 = by.iter().sum();
        let (j, top) = by
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(j, &v)| (j, v))
            .unwrap_or((0, 0.0));
        if total <= 0.0 {
            (1.0, 0)
        } else {
            (k as f64 * top / total, j)
        }
    };
    let beat_weight = |o: &Onset| o.w;
    // Threes only if the notes lean on them clearly more than on twos.
    let mut best: Option<(f64, usize, usize)> = None;
    let mut best_two = 0.0f64;
    let mut threes: Vec<(f64, usize, usize, f64)> = Vec::new();
    for k in [1usize, 2, 3, 4, 6, 8, 12, 16] {
        let beat = step * k as f64;
        if !(250.0..=1500.0).contains(&beat) {
            continue;
        }
        let (lift, j) = lean(k, &beat_weight);
        let octaves = (beat / 520.0).log2();
        let prior = (-0.5 * (octaves / 0.55).powi(2)).exp();
        let score = lift * prior;
        if k % 3 == 0 {
            threes.push((score, k, j, lift));
            continue;
        }
        best_two = best_two.max(lift);
        if best.is_none_or(|b| score > b.0) {
            best = Some((score, k, j));
        }
    }
    for (score, k, j, lift) in threes {
        if lift >= 1.25 * best_two && best.is_none_or(|b| score > b.0) {
            best = Some((score, k, j));
        }
    }
    let (_, k, _) = best.or_else(|| {
        // A step longer than any beat: it is the beat.
        (step > 1500.0).then_some((0.0, 1, 0))
    })?;
    // Which steps are the beats: where the weight falls, the bass and
    // the kick drum counting most.
    let (_, j) = lean(k, &|o: &Onset| o.w + 2.0 * o.accent);
    let compound_time = k % 3 == 0;
    // The bar: three or four beats, from where the accents fall.
    let beat_of = |n: usize| (n - j) / k;
    let accents: Vec<(usize, f64)> = placed
        .iter()
        .filter(|(n, _)| n % k == j)
        .map(|(n, o)| (beat_of(*n), o.accent + 0.2 * o.w))
        .collect();
    // Tunes mostly start on a bar (or just before one): the first beat
    // with a note counts for a little more.
    let first_beat = accents.first().map_or(0, |a| a.0);
    let bar_lean = |b: usize| -> (f64, usize) {
        let mut by = vec![0.0; b];
        for (beat, a) in &accents {
            by[beat % b] += a;
        }
        by[first_beat % b] *= 1.2;
        let total: f64 = by.iter().sum();
        let (p, top) = by
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(p, &v)| (p, v))
            .unwrap_or((0, 0.0));
        (
            if total > 0.0 {
                b as f64 * top / total
            } else {
                1.0
            },
            p,
        )
    };
    let (beats_per_bar, phase, sig) = if compound_time {
        let (_, p) = bar_lean(4);
        if step * k as f64 * 4.0 <= 2600.0 {
            (4, p, (12, 8))
        } else {
            (2, bar_lean(2).1, (6, 8))
        }
    } else {
        let (l4, p4) = bar_lean(4);
        let (l3, p3) = bar_lean(3);
        // Music repeats itself a bar on: how alike beats three and four
        // apart are.
        let (r3, r4) = (
            repeats(parts, &grid, step, j, k, 3),
            repeats(parts, &grid, step, j, k, 4),
        );
        if l3 * r3 > l4 * r4 * 1.15 {
            (3, p3, (3, 4))
        } else {
            (4, p4, (4, 4))
        }
    };
    // The beats: grid steps j, j + k, ...; bars every `beats_per_bar`
    // beats from beat `phase`, the beats before it (and the first notes,
    // if they come before the first beat) made up into a whole bar with
    // beats from before the start.
    let mut beats: Vec<f64> = grid.iter().skip(j).step_by(k).copied().collect();
    let beat_ms = step * k as f64;
    let lead = if phase > 0 {
        beats_per_bar - phase
    } else if j > 0 {
        beats_per_bar
    } else {
        0
    };
    if lead > 0 {
        let b0 = beats[0];
        let mut before: Vec<f64> = (1..=lead).rev().map(|i| b0 - beat_ms * i as f64).collect();
        before.extend(beats);
        beats = before;
    }
    let mut end = end_ms.max(on.last().unwrap().t + 1.0);
    while beats.len() > beats_per_bar && beats[beats.len() - 1] >= end {
        beats.pop();
    }
    while !beats.len().is_multiple_of(beats_per_bar) || *beats.last().unwrap() + beat_ms < end {
        let last = *beats.last().unwrap();
        beats.push(last + beat_ms);
    }
    end = *beats.last().unwrap() + beat_ms;
    let bars: Vec<Bar> = beats
        .chunks(beats_per_bar)
        .map(|c| Bar {
            start_ms: c[0],
            beats: c.to_vec(),
            sig,
        })
        .collect();
    Some(Meter {
        bars,
        end_ms: end,
        exact: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Note;

    fn note(t: f64, key: u8, dur: f64) -> Note {
        Note {
            start_ms: t,
            dur_ms: dur,
            key,
            velocity: 100,
            legato: false,
        }
    }

    /// A tune in 4/4 (or 3/4) at `beat` ms a beat, starting at `offset`:
    /// a bass on the first beat of each bar, a melody of eighths and
    /// sixteenths, and drums.
    fn tune(beat: f64, per_bar: usize, bars: usize, offset: f64) -> Vec<Part> {
        let mut bass = Vec::new();
        let mut melody = Vec::new();
        let mut drums = Vec::new();
        let roots = [36u8, 41, 43, 38];
        for b in 0..bars {
            let t0 = offset + (b * per_bar) as f64 * beat;
            bass.push(note(t0, roots[b % 4], beat * 1.5));
            bass.push(note(t0 + 2.0 * beat, roots[b % 4] + 7, beat * 0.5));
            drums.push(note(t0, 36, 50.0));
            for i in 0..per_bar {
                let t = t0 + i as f64 * beat;
                if i % 2 == 1 {
                    drums.push(note(t, 38, 50.0));
                }
                drums.push(note(t + beat / 2.0, 42, 30.0));
                melody.push(note(t, 72 + (i as u8 * 2) % 7, beat * 0.4));
                if (b + i) % 3 != 0 {
                    melody.push(note(t + beat / 2.0, 74, beat * 0.2));
                    melody.push(note(t + beat * 0.75, 76, beat * 0.2));
                }
            }
        }
        vec![
            Part::new("Lead", 80, melody),
            Part::new("Bass", 33, bass),
            Part::drums("Drums", drums),
        ]
    }

    fn check(m: &Meter, beat: f64, per_bar: usize, offset: f64) {
        let b = &m.bars[2];
        let bar_ms = m.bar_end(2) - b.start_ms;
        assert!(
            (bar_ms - beat * per_bar as f64).abs() < 2.0,
            "bar {bar_ms} for {per_bar} beats of {beat}"
        );
        let from = (b.start_ms - offset) / (beat * per_bar as f64);
        assert!(
            (from - from.round()).abs() < 0.01,
            "bar line at {} with bars from {offset}",
            b.start_ms
        );
        assert_eq!(b.beats.len(), per_bar);
    }

    #[test]
    fn finds_four_four() {
        for (beat, offset) in [(500.0, 0.0), (400.0, 1000.0), (612.0, 37.0)] {
            let parts = tune(beat, 4, 24, offset);
            let m = guess(&parts, offset + 24.0 * 4.0 * beat).unwrap();
            assert!(!m.exact);
            assert_eq!(m.main_sig(), (4, 4));
            check(&m, beat, 4, offset);
            // The bars cover the notes.
            assert!(m.bars[0].start_ms <= offset);
            assert!(m.end_ms >= offset + 24.0 * 4.0 * beat);
        }
    }

    #[test]
    fn finds_three_four() {
        let parts = tune(450.0, 3, 24, 200.0);
        let m = guess(&parts, 200.0 + 24.0 * 3.0 * 450.0).unwrap();
        assert_eq!(m.main_sig(), (3, 4));
        check(&m, 450.0, 3, 200.0);
    }

    #[test]
    fn follows_a_frame_clock() {
        // Rows of 7 and 6 frames at 60 Hz in turn: a jittery beat.
        let frame = 1000.0 / 60.0988;
        let mut t = 0.0;
        let mut rows = Vec::new();
        for i in 0..512 {
            rows.push(t);
            t += frame * if i % 2 == 0 { 7.0 } else { 6.0 };
        }
        let mut lead = Vec::new();
        let mut bass = Vec::new();
        for (i, &r) in rows.iter().enumerate() {
            if i % 4 == 0 || i % 16 == 6 || i % 16 == 10 {
                lead.push(note(r, 70 + (i % 5) as u8, 80.0));
            }
            if i % 16 == 0 {
                bass.push(note(r, 40, 400.0));
            } else if i % 8 == 0 {
                bass.push(note(r, 47, 200.0));
            }
        }
        let parts = vec![Part::new("Lead", 80, lead), Part::new("Bass", 33, bass)];
        let m = guess(&parts, t).unwrap();
        // A beat is four rows, a bar sixteen.
        for (bi, b) in m.bars.iter().enumerate().skip(1).take(20) {
            let row = rows.iter().position(|&r| (r - b.start_ms).abs() < 1.0);
            assert!(
                row.is_some_and(|r| r % 16 == 0),
                "bar {bi} at {}",
                b.start_ms
            );
        }
    }

    #[test]
    fn rulers_count_quarters() {
        let parts = tune(500.0, 4, 8, 0.0);
        let m = guess(&parts, 16_000.0).unwrap();
        let r = m.ruler();
        let q0 = r.quarters(m.bars[1].start_ms);
        assert!((r.quarters(m.bars[1].start_ms + 750.0) - q0 - 1.5).abs() < 0.01);
        assert!((m.bpm(1) - 120.0).abs() < 0.5);
        assert_eq!(m.describe(), "4/4, ♩ = 120");
    }

    fn even(beat: f64, n: usize, sig: (u8, u8)) -> Meter {
        let per = beats_in(sig);
        let bars = (0..n)
            .map(|i| {
                let s = (i * per) as f64 * beat;
                Bar {
                    start_ms: s,
                    beats: (0..per).map(|k| s + k as f64 * beat).collect(),
                    sig,
                }
            })
            .collect();
        Meter {
            bars,
            end_ms: (n * per) as f64 * beat,
            exact: true,
        }
    }

    #[test]
    fn adjusts_by_hand() {
        let m = even(500.0, 8, (4, 4));
        // Twice as fast: bars of four half-length beats.
        let f = m.adjusted(&MeterAdjust {
            speed: 1,
            ..Default::default()
        });
        assert_eq!(f.bars.len(), 16);
        assert_eq!(f.bars[1].start_ms, 1000.0);
        assert!((f.bpm(0) - 240.0).abs() < 1e-6);
        // Half as fast.
        let s = m.adjusted(&MeterAdjust {
            speed: -1,
            ..Default::default()
        });
        assert_eq!(s.bars[1].start_ms, 4000.0);
        assert!(s.end_ms >= m.end_ms);
        // In three.
        let t = m.adjusted(&MeterAdjust {
            sig: Some((3, 4)),
            ..Default::default()
        });
        assert_eq!(t.bars[1].start_ms, 1500.0);
        assert_eq!(t.bars[0].sig, (3, 4));
        assert!(t.end_ms >= m.end_ms);
        // A beat later: the first bar starts three beats before the start.
        let l = m.adjusted(&MeterAdjust {
            shift: 1,
            ..Default::default()
        });
        assert_eq!(l.bars[0].start_ms, -1500.0);
        assert_eq!(l.bars[1].start_ms, 500.0);
        // A beat earlier is three later.
        let e = m.adjusted(&MeterAdjust {
            shift: -1,
            ..Default::default()
        });
        assert_eq!(e.bars[1].start_ms, 1500.0);
        // 6/8 keeps the beats, two to a bar.
        let c = m.adjusted(&MeterAdjust {
            sig: Some((6, 8)),
            ..Default::default()
        });
        assert_eq!(c.bars[1].start_ms, 1000.0);
        assert_eq!(c.bars[0].beat_quarters(), 1.5);
        assert_eq!(c.describe(), "6/8, ♩. = 120");
    }

    #[test]
    fn reads_rows() {
        // Two 64-row patterns at 125 ms a row, the second broken off after
        // 40 rows.
        let mut rows = Vec::new();
        let mut t = 0.0;
        for r in 0..64 {
            rows.push((t, 0, r));
            t += 125.0;
        }
        for r in 0..40 {
            rows.push((t, 1, r));
            t += 125.0;
        }
        let m = from_rows(&rows, t).unwrap();
        assert_eq!(m.bars.len(), 4 + 3);
        assert_eq!(m.bars[1].start_ms, 2000.0);
        assert_eq!(m.bars[0].sig, (4, 4));
        assert_eq!(m.bars[0].beats, vec![0.0, 500.0, 1000.0, 1500.0]);
        // The broken-off bar has the beats it got to.
        assert_eq!(m.bars[6].sig, (2, 4));
        assert_eq!(m.bars[4].start_ms, 8000.0);
    }

    #[test]
    fn reads_midi_time_signatures() {
        // 480 ticks a quarter at 500 ms; 3/4 from bar 3.
        let m = from_ticks(&[(0, (4, 4)), (3840, (3, 4))], 480, 6720, |t| {
            t as f64 * 500.0 / 480.0
        })
        .unwrap();
        assert_eq!(m.bars.len(), 4);
        assert_eq!(m.bars[2].sig, (3, 4));
        assert_eq!(m.bars[3].start_ms, 5500.0);
        assert_eq!(m.end_ms, 7000.0);
    }
}
