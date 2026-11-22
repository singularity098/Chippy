//! The guitar tab view's state, shared by the desktop and Android: the
//! playing track's notes and bars (worked out on a thread of their own),
//! which part is shown on which instrument, the bars as put right by hand,
//! the fingering moved along the neck (the whole part, or chosen bars),
//! and where playback is in the tab.

use chippy_core::meta::format_ms;
use chippy_core::notes::meter::{self, Meter, MeterAdjust};
use chippy_core::notes::score::{self, Score};
use chippy_core::notes::{self, gm, tab, Part, Transcription};
use chippy_core::TrackRef;
use crossbeam_channel::{Receiver, Sender};
use std::collections::HashMap;

/// The notes of a track and its bars, found in the background: (request,
/// result).
type Found = (u64, Result<(Transcription, Option<Meter>), String>);

/// A line of tab, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start_ms: f64,
    pub end_ms: f64,
    /// "1:04".
    pub label: String,
    /// (x 0..1, string (0 lowest), fret).
    pub cells: Vec<(f32, u8, u8)>,
    /// (ms, x 0..1): where the playhead is when; straight between them.
    pub marks: Vec<(f64, f32)>,
    /// Bar lines across it: (x 0..1, the bar starting there).
    pub bars: Vec<(f32, usize)>,
    /// The bars across it: (from x, to x, bar), 0..1.
    pub segments: Vec<(f32, f32, usize)>,
}

/// How wide a line is taken to be until the view says (in digits).
pub const DEFAULT_WIDTH: f32 = 72.0;

pub struct TabView {
    /// The track shown (or being worked out).
    pub track: Option<TrackRef>,
    request: u64,
    tx: Sender<Found>,
    rx: Receiver<Found>,
    found: Option<Result<Transcription, String>>,
    /// Indices of the pitched parts in the transcription.
    parts: Vec<usize>,
    part: usize,
    tuning: usize,
    /// The tuning was chosen by hand (kept across parts and tracks).
    tuning_chosen: bool,
    tab: Option<tab::Tab>,
    lines: Vec<Line>,
    length_ms: f64,
    /// How many digits (fret numbers) fit across a line.
    width: f32,
    /// Octaves and steps along the neck chosen by hand, per track and
    /// part, for as long as the app runs (never saved). Bars nudged on
    /// their own are spans from their start to their end.
    adjusts: HashMap<(TrackRef, usize), tab::Adjust>,
    /// The last step along the neck, up (true) or down, moved nothing.
    stuck: Option<bool>,
    /// The bars as found, and as put right by hand (per track, until the
    /// app closes).
    found_meter: Option<Meter>,
    meter_adjusts: HashMap<TrackRef, MeterAdjust>,
    meter: Option<Meter>,
    /// Bars chosen, to nudge on their own: (first, last).
    selected: Option<(usize, usize)>,
    /// Where a chosen run of bars started (a shift-click extends from it).
    anchor: usize,
}

impl Default for TabView {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        TabView {
            track: None,
            request: 0,
            tx,
            rx,
            found: None,
            parts: Vec::new(),
            part: 0,
            tuning: 0,
            tuning_chosen: false,
            tab: None,
            lines: Vec::new(),
            length_ms: 0.0,
            width: DEFAULT_WIDTH,
            adjusts: HashMap::new(),
            stuck: None,
            found_meter: None,
            meter_adjusts: HashMap::new(),
            meter: None,
            selected: None,
            anchor: 0,
        }
    }
}

/// The part to show first: a guitar if there is one, else the lead: the
/// busiest of the parts above the bass, preferring higher ones when about
/// as busy (on a chip, the first square channel usually carries the tune;
/// the triangle the bass).
fn first_part(t: &Transcription, parts: &[usize]) -> usize {
    let busiest = |guitar: bool| {
        parts
            .iter()
            .enumerate()
            // A guitar, that is, not a bass (which GM counts among them).
            .filter(|(_, &i)| {
                let p = &t.parts[i];
                !guitar || (gm::is_guitar(p.program) && !is_bass(p))
            })
            .max_by_key(|(_, &i)| t.parts[i].notes.len())
            .map(|(k, _)| k)
    };
    if let Some(k) = busiest(true) {
        return k;
    }
    let named = |p: &Part, words: &[&str]| {
        let n = p.name.to_lowercase();
        words.iter().any(|w| n.contains(w))
    };
    let leads: Vec<(usize, &Part)> = parts
        .iter()
        .enumerate()
        .map(|(k, &i)| (k, &t.parts[i]))
        .filter(|(_, p)| !is_bass(p) && !named(p, &["bass", "triangle", "noise", "dpcm", "dmc"]))
        .collect();
    if let Some((k, _)) = leads
        .iter()
        .find(|(_, p)| named(p, &["lead", "melody", "vocal", "voice"]) && !p.notes.is_empty())
    {
        return *k;
    }
    let most = leads.iter().map(|(_, p)| p.notes.len()).max().unwrap_or(0);
    // Of the parts at least half as busy as the busiest, the highest; ties
    // to the first (Square 1 before Square 2).
    leads
        .iter()
        .filter(|(_, p)| p.notes.len() * 2 >= most)
        .max_by_key(|(k, p)| (median_key(p), std::cmp::Reverse(*k)))
        .map(|(k, _)| *k)
        .or_else(|| busiest(false))
        .unwrap_or(0)
}

/// A part low enough, or named so, to be played on a bass.
fn is_bass(p: &Part) -> bool {
    median_key(p) < 45 || (32..=39).contains(&p.program)
}

fn median_key(p: &Part) -> u8 {
    let mut keys: Vec<u8> = p.notes.iter().map(|n| n.key).collect();
    notes::instrument::median(&mut keys).unwrap_or(60)
}

/// The parts of `t`, laid out as `tabs` says ((part index, tuning, tab)),
/// written out in the bars of `meter`.
fn build_score(
    t: &Transcription,
    title: &str,
    meter: &Meter,
    tabs: &[(usize, usize, tab::Tab)],
) -> Score {
    let parts: Vec<score::PartTab> = tabs
        .iter()
        .map(|(i, tuning, tb)| {
            let p = &t.parts[*i];
            score::PartTab {
                name: p.name.clone(),
                program: p.program,
                tuning: &tab::TUNINGS[*tuning],
                tab: tb,
            }
        })
        .collect();
    score::build(title, meter, &parts)
}

/// A track's notes as tab in bars, as the tab view first shows them
/// (each pitched part on a guitar, or a bass if it is low), over
/// `meter`'s bars. None if there are no pitched notes.
pub fn default_score(t: &Transcription, title: &str, meter: &Meter) -> Option<Score> {
    let tabs: Vec<(usize, usize, tab::Tab)> = t
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.drums && !p.notes.is_empty())
        .map(|(i, p)| {
            let tuning = if is_bass(p) {
                tab::tuning_named("Bass (4 strings)").unwrap_or(0)
            } else {
                0
            };
            (i, tuning, tab::arrange(&p.notes, &tab::TUNINGS[tuning]))
        })
        .collect();
    (!tabs.is_empty()).then(|| build_score(t, title, meter, &tabs))
}

impl TabView {
    /// Starts working out `track`'s notes over `length_ms` (unless it is
    /// the one already shown).
    pub fn load(&mut self, track: &TrackRef, length_ms: Option<u64>) {
        if self.track.as_ref() == Some(track) {
            return;
        }
        self.track = Some(track.clone());
        self.request += 1;
        self.found = None;
        self.tab = None;
        self.lines.clear();
        self.parts.clear();
        self.found_meter = None;
        self.meter = None;
        self.selected = None;
        self.length_ms = length_ms.unwrap_or(0) as f64;
        let (id, tx, track) = (self.request, self.tx.clone(), track.clone());
        std::thread::Builder::new()
            .name("tabs".into())
            .spawn(move || {
                crate::background_thread();
                // Shared with MIDI mode, which may have them already.
                let r = crate::midimode::cached(&track, length_ms).map(|t| {
                    let end = t.end_ms().max(length_ms.unwrap_or(0) as f64);
                    let m = meter::find(&t, end);
                    ((*t).clone(), m)
                });
                let _ = tx.send((id, r));
            })
            .ok();
    }

    /// Forgets the track (the view was closed); work under way is ignored.
    pub fn clear(&mut self) {
        self.track = None;
        self.request += 1;
        self.found = None;
        self.tab = None;
        self.lines.clear();
        self.parts.clear();
        self.found_meter = None;
        self.meter = None;
        self.selected = None;
    }

    /// Takes in finished work; true if the view changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok((id, r)) = self.rx.try_recv() {
            if id != self.request {
                continue;
            }
            let r = r.map(|(t, m)| {
                self.found_meter = m;
                t
            });
            if let Ok(t) = &r {
                self.parts = (0..t.parts.len())
                    .filter(|&i| !t.parts[i].drums && !t.parts[i].notes.is_empty())
                    .collect();
                self.part = first_part(t, &self.parts);
                self.length_ms = self.length_ms.max(t.end_ms());
            }
            self.found = Some(r);
            self.fit_meter();
            self.arrange();
            changed = true;
        }
        changed
    }

    pub fn busy(&self) -> bool {
        self.track.is_some() && self.found.is_none()
    }

    /// Why there is no tab, if there isn't one.
    pub fn message(&self) -> String {
        match &self.found {
            None => String::new(),
            Some(Err(e)) => format!("The notes of this track can't be found: {e}."),
            Some(Ok(_)) if self.parts.is_empty() => "No notes were found in this track.".into(),
            Some(Ok(_)) => String::new(),
        }
    }

    fn transcription(&self) -> Option<&Transcription> {
        self.found.as_ref().and_then(|r| r.as_ref().ok())
    }

    fn current_part(&self) -> Option<&Part> {
        let t = self.transcription()?;
        Some(&t.parts[*self.parts.get(self.part)?])
    }

    pub fn part_count(&self) -> usize {
        self.parts.len()
    }

    pub fn part_index(&self) -> usize {
        self.part
    }

    /// "Voice 3 · Flute", with how many there are.
    pub fn part_name(&self) -> String {
        match self.current_part() {
            Some(p) if p.name.contains('·') => {
                format!("{} ({}/{})", p.name, self.part + 1, self.parts.len())
            }
            Some(p) => format!(
                "{} · {} ({}/{})",
                p.name,
                gm::program_name(p.program),
                self.part + 1,
                self.parts.len()
            ),
            None => "–".into(),
        }
    }

    /// The names of the pitched parts, as `part_name` gives them.
    pub fn part_names(&self) -> Vec<String> {
        let Some(t) = self.transcription() else {
            return Vec::new();
        };
        self.parts
            .iter()
            .map(|&i| {
                let p = &t.parts[i];
                if p.name.contains('·') {
                    p.name.clone()
                } else {
                    format!("{} · {}", p.name, gm::program_name(p.program))
                }
            })
            .collect()
    }

    pub fn tuning_name(&self) -> &'static str {
        tab::TUNINGS[self.tuning].name
    }

    pub fn tuning_index(&self) -> usize {
        self.tuning
    }

    pub fn tuning_names() -> Vec<String> {
        tab::TUNINGS.iter().map(|t| t.name.to_string()).collect()
    }

    pub fn step_part(&mut self, d: i32) {
        let n = self.parts.len() as i32;
        if n == 0 {
            return;
        }
        self.part = (self.part as i32 + d).rem_euclid(n) as usize;
        self.arrange();
    }

    pub fn set_part(&mut self, i: usize) {
        if i < self.parts.len() {
            self.part = i;
            self.arrange();
        }
    }

    /// Sets how many digits (fret numbers with their padding) fit across a
    /// line; true if the lines changed.
    pub fn set_width(&mut self, digits: f32) -> bool {
        let digits = digits.round().max(12.0);
        if (digits - self.width).abs() < 0.5 {
            return false;
        }
        self.width = digits;
        self.layout();
        true
    }

    /// The track's notes were looked for and none can be shown.
    pub fn failed(&self) -> bool {
        match &self.found {
            Some(Err(_)) => true,
            Some(Ok(_)) => self.parts.is_empty(),
            None => false,
        }
    }

    pub fn step_tuning(&mut self, d: i32) {
        let n = tab::TUNINGS.len() as i32;
        self.set_tuning((self.tuning as i32 + d).rem_euclid(n) as usize);
    }

    pub fn set_tuning(&mut self, i: usize) {
        if i < tab::TUNINGS.len() {
            self.tuning = i;
            self.tuning_chosen = true;
            self.arrange();
        }
    }

    /// Where the shown part's adjustments are kept.
    fn adjust_key(&self) -> Option<(TrackRef, usize)> {
        Some((self.track.clone()?, *self.parts.get(self.part)?))
    }

    /// The shown part's octave and steps along the neck, chosen by hand.
    pub fn adjust(&self) -> tab::Adjust {
        self.adjust_key()
            .and_then(|k| self.adjusts.get(&k).cloned())
            .unwrap_or_default()
    }

    fn set_adjust(&mut self, a: tab::Adjust) {
        let Some(k) = self.adjust_key() else { return };
        if a == tab::Adjust::default() {
            self.adjusts.remove(&k);
        } else {
            self.adjusts.insert(k, a);
        }
        self.arrange();
    }

    /// Shows the part `d` octaves higher (lower, if negative).
    pub fn shift_octave(&mut self, d: i32) {
        let mut a = self.adjust();
        a.octave = (a.octave + d).clamp(-tab::MAX_OCTAVES, tab::MAX_OCTAVES);
        self.set_adjust(a);
    }

    /// Moves the fingering `d` steps up the neck (down, if negative): the
    /// chosen bars', or else the whole part's.
    pub fn nudge(&mut self, d: i32) {
        let before = self.steps();
        let mut a = self.adjust();
        match self.selected_span() {
            Some((from, to)) => {
                let bars: Vec<(f64, f64)> = (from..=to).filter_map(|i| self.bar_span(i)).collect();
                for (start, end) in bars {
                    match a.spans.iter_mut().find(|s| s.start_ms == start) {
                        Some(s) => s.steps = (s.steps + d).clamp(-tab::MAX_STEPS, tab::MAX_STEPS),
                        None => a.spans.push(tab::Span {
                            start_ms: start,
                            end_ms: end,
                            steps: d.clamp(-tab::MAX_STEPS, tab::MAX_STEPS),
                        }),
                    }
                }
                a.spans.retain(|s| s.steps != 0);
            }
            None => a.position = (a.position + d).clamp(-tab::MAX_STEPS, tab::MAX_STEPS),
        }
        self.set_adjust(a);
        if d != 0 && self.steps() == before {
            self.stuck = Some(d > 0);
        }
    }

    /// Steps along the neck of what the neck buttons move: the first
    /// chosen bar, or else the whole part.
    pub fn steps(&self) -> i32 {
        let a = self.adjust();
        match self
            .selected_span()
            .and_then(|(from, _)| self.bar_span(from))
        {
            Some((start, _)) => a
                .spans
                .iter()
                .find(|s| s.start_ms == start)
                .map_or(0, |s| s.steps),
            None => a.position,
        }
    }

    /// Back to the part as it fits best (the chosen bars, if any are).
    pub fn reset_adjust(&mut self) {
        match self.selected_span() {
            Some((from, to)) => {
                let starts: Vec<f64> = (from..=to)
                    .filter_map(|i| self.bar_span(i))
                    .map(|b| b.0)
                    .collect();
                let mut a = self.adjust();
                a.spans.retain(|s| !starts.contains(&s.start_ms));
                self.set_adjust(a);
            }
            None => self.set_adjust(tab::Adjust::default()),
        }
    }

    /// Something to put back: the part moved, or (with bars chosen) one of
    /// them.
    pub fn adjusted(&self) -> bool {
        let a = self.adjust();
        match self.selected_span() {
            Some(_) => self.steps() != 0,
            None => a != tab::Adjust::default(),
        }
    }

    /// Whether the fingering can be moved further up (`up`) or down the
    /// neck: false once a step that way moved nothing.
    pub fn can_nudge(&self, up: bool) -> bool {
        let Some(t) = &self.tab else { return false };
        let p = self.steps();
        !t.notes.is_empty()
            && self.stuck != Some(up)
            && if up {
                p < tab::MAX_STEPS
            } else {
                p > -tab::MAX_STEPS
            }
    }

    // ---- bars

    /// The bars as shown (as found, put right by hand).
    pub fn meter(&self) -> Option<&Meter> {
        self.meter.as_ref()
    }

    /// Where bar `i` starts and ends, ms.
    fn bar_span(&self, i: usize) -> Option<(f64, f64)> {
        let m = self.meter.as_ref()?;
        Some((m.bars.get(i)?.start_ms, m.bar_end(i)))
    }

    /// When bar `i` starts (not before the track does), to play from.
    pub fn bar_start(&self, i: usize) -> Option<u64> {
        self.bar_span(i).map(|b| b.0.max(0.0) as u64)
    }

    fn meter_adjust(&self) -> MeterAdjust {
        self.track
            .as_ref()
            .and_then(|t| self.meter_adjusts.get(t).copied())
            .unwrap_or_default()
    }

    fn set_meter_adjust(&mut self, a: MeterAdjust) {
        let Some(t) = self.track.clone() else { return };
        if a == MeterAdjust::default() {
            self.meter_adjusts.remove(&t);
        } else {
            self.meter_adjusts.insert(t, a);
        }
        self.selected = None;
        self.fit_meter();
        self.arrange();
    }

    /// The bars from those found and the changes made by hand.
    fn fit_meter(&mut self) {
        let a = self.meter_adjust();
        self.meter = self.found_meter.as_ref().map(|m| m.adjusted(&a));
    }

    /// "4/4, ♩ = 120 · guessed from the notes".
    pub fn meter_text(&self) -> String {
        match (&self.meter, &self.found_meter) {
            (Some(m), Some(f)) => {
                let how = if self.meter_adjust() != MeterAdjust::default() {
                    "set by hand"
                } else if f.exact {
                    "from the file"
                } else {
                    "guessed from the notes"
                };
                format!("{} · {how}", m.describe())
            }
            _ => String::new(),
        }
    }

    /// The time signatures to choose from: "As found", then "2/4"...
    pub fn sig_names() -> Vec<String> {
        std::iter::once("As found".to_string())
            .chain(meter::SIGNATURES.iter().map(|s| format!("{}/{}", s.0, s.1)))
            .collect()
    }

    /// Which of [`Self::sig_names`] is in use.
    pub fn sig_index(&self) -> usize {
        self.meter_adjust()
            .sig
            .and_then(|s| meter::SIGNATURES.iter().position(|&x| x == s))
            .map_or(0, |i| i + 1)
    }

    pub fn set_sig(&mut self, i: usize) {
        let mut a = self.meter_adjust();
        a.sig = i
            .checked_sub(1)
            .and_then(|i| meter::SIGNATURES.get(i))
            .copied();
        self.set_meter_adjust(a);
    }

    /// The beat twice as fast (`d` = 1) or half as fast (-1).
    pub fn step_speed(&mut self, d: i32) {
        let mut a = self.meter_adjust();
        a.speed = (a.speed + d).clamp(-meter::MAX_SPEED, meter::MAX_SPEED);
        self.set_meter_adjust(a);
    }

    pub fn speed(&self) -> i32 {
        self.meter_adjust().speed
    }

    /// The bar lines a beat later (`d` = 1) or earlier.
    pub fn shift_bars(&mut self, d: i32) {
        let mut a = self.meter_adjust();
        a.shift += d;
        self.set_meter_adjust(a);
    }

    /// The bars back as found.
    pub fn reset_meter(&mut self) {
        self.set_meter_adjust(MeterAdjust::default());
    }

    pub fn meter_adjusted(&self) -> bool {
        self.meter_adjust() != MeterAdjust::default()
    }

    /// Chooses bar `i` to nudge on its own; again, chooses none. With
    /// `extend`, chooses the bars from the one chosen before to it.
    pub fn select_bar(&mut self, i: usize, extend: bool) {
        let Some(m) = &self.meter else { return };
        if i >= m.bars.len() {
            return;
        }
        self.selected = match self.selected {
            Some(_) if extend => Some((self.anchor.min(i), self.anchor.max(i))),
            Some((a, b)) if a == i && b == i => None,
            _ => {
                self.anchor = i;
                Some((i, i))
            }
        };
        self.stuck = None;
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.stuck = None;
    }

    /// The bars chosen: (first, last).
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.selected
    }

    fn selected_span(&self) -> Option<(usize, usize)> {
        self.selected.filter(|_| self.meter.is_some())
    }

    /// The bars nudged on their own in the shown part.
    pub fn nudged_bars(&self) -> Vec<usize> {
        let Some(m) = &self.meter else {
            return Vec::new();
        };
        self.adjust()
            .spans
            .iter()
            .filter(|s| s.steps != 0)
            .filter_map(|s| m.bar_at(s.start_ms + 0.5))
            .collect()
    }

    /// The tuning to show part `i` (an index into the transcription's
    /// parts) on.
    fn tuning_for(&self, p: &Part) -> usize {
        if self.tuning_chosen {
            self.tuning
        } else if is_bass(p) {
            // Low parts on a bass, the rest on a guitar.
            tab::tuning_named("Bass (4 strings)").unwrap_or(0)
        } else {
            0
        }
    }

    /// `a` with its spans moved onto the bars they start in (the bars may
    /// have been moved since).
    fn fit_spans(&self, a: &tab::Adjust) -> tab::Adjust {
        let mut a = a.clone();
        if let Some(m) = &self.meter {
            let mut spans: Vec<tab::Span> = Vec::new();
            for s in &a.spans {
                let Some(i) = m.bar_at(s.start_ms + 0.5) else {
                    continue;
                };
                let (start, end) = (m.bars[i].start_ms, m.bar_end(i));
                match spans.iter_mut().find(|x| x.start_ms == start) {
                    Some(x) => x.steps = s.steps,
                    None => spans.push(tab::Span {
                        start_ms: start,
                        end_ms: end,
                        steps: s.steps,
                    }),
                }
            }
            a.spans = spans;
        }
        a
    }

    fn arrange(&mut self) {
        self.tab = None;
        self.lines.clear();
        self.stuck = None;
        let adjust = self.fit_spans(&self.adjust());
        let Some(p) = self.current_part() else { return };
        let tuning = self.tuning_for(p);
        let tab = tab::arrange_with(&p.notes, &tab::TUNINGS[tuning], adjust.clone());
        // Steps asked for past where anything moves are forgotten, so a
        // step back the other way shows at once.
        let mut kept = tab::Adjust {
            position: tab.position,
            ..adjust.clone()
        };
        for (s, &got) in kept.spans.iter_mut().zip(&tab.spans) {
            s.steps = got;
        }
        kept.spans.retain(|s| s.steps != 0);
        if kept != self.adjust() {
            if let Some(k) = self.adjust_key() {
                if kept == tab::Adjust::default() {
                    self.adjusts.remove(&k);
                } else {
                    self.adjusts.insert(k, kept);
                }
            }
        }
        self.tuning = tuning;
        self.tab = Some(tab);
        self.layout();
    }

    /// When each bar starts, for drawing.
    fn bar_starts(&self) -> Vec<f64> {
        self.meter.as_ref().map(Meter::starts).unwrap_or_default()
    }

    /// Cuts the tab into lines as wide as the view.
    fn layout(&mut self) {
        let Some(tab) = &self.tab else { return };
        let bars = self.bar_starts();
        self.lines = tab::layout(tab, self.width, self.length_ms, &bars)
            .into_iter()
            .map(|r| Line {
                label: format_ms(r.start_ms.max(0.0) as u64),
                start_ms: r.start_ms,
                end_ms: r.end_ms,
                segments: r.segments(),
                bars: r.bars,
                cells: r.cells,
                marks: r.marks,
            })
            .collect();
    }

    /// The track shown, as set up here, for an export of `tracks` to
    /// `format`: its bars if they were changed by hand, and (for tab) its
    /// parts as shown.
    pub fn shown_for_export(
        &self,
        tracks: &[(TrackRef, chippy_core::TrackMeta)],
        format: crate::export::Format,
    ) -> Vec<crate::export::Shown> {
        use crate::export::{Format, Shown};
        let Some(track) = self.track.clone() else {
            return Vec::new();
        };
        let Some((_, meta)) = tracks.iter().find(|(t, _)| *t == track) else {
            return Vec::new();
        };
        let tab = matches!(format, Format::MusicXml | Format::GuitarPro);
        vec![Shown {
            meter: self.meter_adjusted().then(|| self.meter.clone()).flatten(),
            score: tab.then(|| self.score(&meta.title)).flatten(),
            track,
        }]
    }

    /// The parts as tab in bars, for writing out as sheet music: every
    /// pitched part, each on its instrument and moved as it was by hand.
    pub fn score(&self, title: &str) -> Option<Score> {
        let t = self.transcription()?;
        let m = self.meter.as_ref()?;
        let track = self.track.clone()?;
        let tabs: Vec<(usize, usize, tab::Tab)> = self
            .parts
            .iter()
            .map(|&i| {
                let p = &t.parts[i];
                let tuning = self.tuning_for(p);
                let a = self
                    .adjusts
                    .get(&(track.clone(), i))
                    .map(|a| self.fit_spans(a))
                    .unwrap_or_default();
                (
                    i,
                    tuning,
                    tab::arrange_with(&p.notes, &tab::TUNINGS[tuning], a),
                )
            })
            .collect();
        Some(build_score(t, title, m, &tabs))
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// The strings' names, lowest first.
    pub fn strings(&self) -> Vec<String> {
        match &self.tab {
            Some(t) => t.string_names.clone(),
            None => tab::TUNINGS[self.tuning]
                .strings
                .iter()
                .map(|&k| {
                    let n = notes::key_name(k);
                    n.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-')
                        .to_string()
                })
                .collect(),
        }
    }

    /// The line playing at `ms`, and how far across it (0..1).
    pub fn position(&self, ms: u64) -> Option<(usize, f32)> {
        let ms = ms as f64;
        let i = self.lines.iter().position(|l| ms < l.end_ms)?;
        let l = &self.lines[i];
        let row = tab::Row {
            start_ms: l.start_ms,
            end_ms: l.end_ms,
            cells: Vec::new(),
            marks: l.marks.clone(),
            bar: None,
            bars: Vec::new(),
        };
        Some((i, tab::playhead(&row, ms)))
    }

    /// "Moved down an octave to fit. 3 notes moved by an octave."
    pub fn detail(&self) -> String {
        let Some(t) = &self.tab else {
            return String::new();
        };
        let mut out = Vec::new();
        let octaves = t.transpose.abs() / 12;
        if octaves > 0 {
            out.push(format!(
                "Moved {} {} to fit.",
                if t.transpose < 0 { "down" } else { "up" },
                match octaves {
                    1 => "an octave".to_string(),
                    n => format!("{n} octaves"),
                }
            ));
        }
        if t.octave != 0 {
            out.push(format!(
                "Shown {} {} by hand.",
                match t.octave.abs() {
                    1 => "an octave".to_string(),
                    n => format!("{n} octaves"),
                },
                if t.octave > 0 { "higher" } else { "lower" },
            ));
        }
        let bars = t.spans.iter().filter(|&&s| s != 0).count();
        if bars > 0 {
            out.push(format!(
                "{bars} {} moved along the neck on {}.",
                if bars == 1 { "bar" } else { "bars" },
                if bars == 1 { "its own" } else { "their own" },
            ));
        }
        if t.position != 0 {
            out.push(format!(
                "Moved {} {} the neck.",
                match t.position.abs() {
                    1 => "a step".to_string(),
                    n => format!("{n} steps"),
                },
                if t.position > 0 { "up" } else { "down" },
            ));
        }
        if t.folded > 0 {
            out.push(format!(
                "{} {} out of reach moved by octaves.",
                t.folded,
                if t.folded == 1 { "note" } else { "notes" }
            ));
        }
        if let Some(Ok(tr)) = &self.found {
            if let Some(n) = tr.notes.first() {
                out.push(n.clone());
            }
        }
        out.join(" ")
    }

    /// The whole tab as text, to copy.
    pub fn text(&self, title: &str) -> String {
        match &self.tab {
            Some(t) => format!(
                "{title}\n{} — {}\n\n{}",
                self.part_names()
                    .get(self.part)
                    .cloned()
                    .unwrap_or_default(),
                self.tuning_name(),
                tab::ascii(t, 80, &self.bar_starts())
            ),
            None => String::new(),
        }
    }
}

/// Whether a track has lyrics and notes to show, so their buttons can be
/// greyed out before anything is opened. Lyrics are looked for on a
/// thread of their own (they may be in an archive or the file's tags).
pub struct Available {
    track: Option<TrackRef>,
    request: u64,
    lyrics: Option<bool>,
    notes: bool,
    tx: Sender<(u64, bool)>,
    rx: Receiver<(u64, bool)>,
}

impl Default for Available {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Available {
            track: None,
            request: 0,
            lyrics: None,
            notes: false,
            tx,
            rx,
        }
    }
}

impl Available {
    /// Looks into `track` if it is not the one already looked into; true
    /// if what is known changed.
    pub fn check(&mut self, track: Option<&TrackRef>) -> bool {
        if self.track.as_ref() == track {
            return false;
        }
        self.track = track.cloned();
        self.request += 1;
        self.lyrics = None;
        self.notes = track.is_some_and(|t| notes::supported(&t.loc.name()));
        if let Some(t) = track {
            let (id, tx, t) = (self.request, self.tx.clone(), t.clone());
            std::thread::Builder::new()
                .name("lyrics".into())
                .spawn(move || {
                    crate::background_thread();
                    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        chippy_core::lyrics::find(&t)
                    }))
                    .ok()
                    .flatten()
                    .is_some_and(|l| !l.lines.is_empty());
                    let _ = tx.send((id, found));
                })
                .ok();
        }
        true
    }

    /// Takes in a finished look; true if what is known changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok((id, found)) = self.rx.try_recv() {
            if id == self.request {
                self.lyrics = Some(found);
                changed = true;
            }
        }
        changed
    }

    /// Still looking for the track's lyrics.
    pub fn pending(&self) -> bool {
        self.track.is_some() && self.lyrics.is_none()
    }

    /// The track has lyrics (false until known).
    pub fn lyrics(&self) -> bool {
        self.lyrics == Some(true)
    }

    /// The track's notes can be looked for, and `view` (if it shows this
    /// track) found some.
    pub fn notes(&self, view: &TabView) -> bool {
        self.notes && !(view.track == self.track && view.failed())
    }
}
