//! The guitar tab view's state, shared by the desktop and Android: the
//! playing track's notes and bars (worked out on a thread of their own),
//! which part is shown on which instrument, the bars as put right by hand,
//! the fingering moved along the neck (the whole part, or chosen bars),
//! a tuning of the player's own, the tab as standard notation, and where
//! playback is in the tab.

use crate::instrument::{self, Kind};
use chippy_core::meta::format_ms;
use chippy_core::notes::meter::{self, Meter, MeterAdjust};
use chippy_core::notes::score::{self, Score};
use chippy_core::notes::staff::{self, Clef};
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
    /// (x 0..1, string (0 lowest), fret, soft: played quieter, an echo).
    pub cells: Vec<(f32, u8, u8, bool)>,
    /// (ms, x 0..1): where the playhead is when; straight between them.
    pub marks: Vec<(f64, f32)>,
    /// Bar lines across it: (x 0..1, the bar starting there).
    pub bars: Vec<(f32, usize)>,
    /// The bars across it: (from x, to x, bar), 0..1.
    pub segments: Vec<(f32, f32, usize)>,
    /// Each chord's time and place across it (x 0..1), in order.
    pub chords: Vec<(f64, f32)>,
    /// The line in standard notation.
    pub staff: Vec<staff::Chord>,
}

impl Line {
    /// When the chord at `x` (0..1) across the line is played, if one is
    /// there.
    pub fn chord_at(&self, x: f32) -> Option<f64> {
        self.chords
            .iter()
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
            .filter(|c| (c.1 - x).abs() < 1e-3)
            .map(|c| c.0)
    }
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
    /// Parts made by merging others (the parts of the transcription as
    /// found, by index), per track, for as long as the app runs: one tune
    /// split across channels (to pan it, or to let notes ring on) read as
    /// one. Added after the parts found.
    merges: HashMap<TrackRef, Vec<Vec<usize>>>,
    /// How many parts the transcription had as found.
    found_parts: usize,
    /// Parts muted and soloed, per track, by index among the parts as
    /// found (for as long as the app runs).
    mutes: HashMap<TrackRef, Mutes>,
    /// The shown part's notes on the fretboard, cut into bars.
    fretboard: Option<Fretboard>,
    /// A tuning of the player's own (open strings, as listed, lowest
    /// first), once one is made: kept across parts and tracks until the
    /// app closes.
    custom: Vec<u8>,
    /// The staff the shown tab is written on, and how far above how it
    /// sounds.
    clef: (Clef, i32),
}

/// Which of a track's parts (as found) are muted, and which soloed.
#[derive(Clone, Debug, Default, PartialEq)]
struct Mutes {
    muted: Vec<bool>,
    solo: Vec<bool>,
}

impl Mutes {
    fn get(v: &[bool], i: usize) -> bool {
        v.get(i).copied().unwrap_or(false)
    }

    fn set(v: &mut Vec<bool>, i: usize, on: bool) {
        if v.len() <= i {
            v.resize(i + 1, false);
        }
        v[i] = on;
    }

    /// Whether part `i` is heard: soloed, if any are, else not muted.
    fn audible(&self, i: usize) -> bool {
        if self.solo.iter().any(|&s| s) {
            Self::get(&self.solo, i)
        } else {
            !Self::get(&self.muted, i)
        }
    }

    fn is_empty(&self) -> bool {
        !self.muted.iter().chain(&self.solo).any(|&b| b)
    }
}

/// A row of the list of parts: a part the tab can show, or the drums
/// (which it can't, but which can be muted and soloed).
#[derive(Clone, Debug, PartialEq)]
pub struct MixPart {
    pub name: String,
    pub kind: Kind,
    /// Its index among the parts the tab shows (None for the drums).
    pub pick: Option<usize>,
    pub muted: bool,
    pub soloed: bool,
    /// Heard, as muting and soloing leave it.
    pub audible: bool,
}

/// The shown part's notes laid out on the neck, through the song: each
/// note in a group of notes (a bar, or a line of tab where there are no
/// bars), for the fretboard to show the notes of the bar playing, and
/// the ones sounding.
#[derive(Clone, Debug, PartialEq)]
pub struct Fretboard {
    /// String names, lowest first.
    pub strings: Vec<String>,
    /// How many frets to draw (the highest fret used, at least 12).
    pub frets: u8,
    pub notes: Vec<FretNote>,
    /// Where each group starts and ends (ms), in time order.
    pub groups: Vec<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FretNote {
    pub start_ms: f64,
    pub end_ms: f64,
    /// 0 is the lowest string.
    pub string: u8,
    pub fret: u8,
    /// The note's name, "Bb".
    pub name: String,
    pub group: usize,
}

/// A note on the fretboard at a moment: sounding now, or (dimmed) played
/// elsewhere in the same bar.
#[derive(Clone, Debug, PartialEq)]
pub struct FretDot {
    pub string: u8,
    pub fret: u8,
    pub name: String,
    pub now: bool,
}

/// A note's name as a guitarist reads it, without its octave: sharps for
/// F# and C#, flats for the others ("Bb", "Eb").
pub fn note_name(key: u8) -> &'static str {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
    ];
    NAMES[key as usize % 12]
}

impl Fretboard {
    /// The notes of the group playing at `ms`: those sounding marked.
    pub fn dots(&self, ms: f64) -> Vec<FretDot> {
        let Some(g) = self.groups.iter().position(|&(a, b)| ms >= a && ms < b) else {
            return Vec::new();
        };
        let mut out: Vec<FretDot> = Vec::new();
        for n in self.notes.iter().filter(|n| n.group == g) {
            let now = ms >= n.start_ms && ms < n.end_ms;
            match out
                .iter_mut()
                .find(|d| d.string == n.string && d.fret == n.fret)
            {
                Some(d) => d.now |= now,
                None => out.push(FretDot {
                    string: n.string,
                    fret: n.fret,
                    name: n.name.clone(),
                    now,
                }),
            }
        }
        out
    }
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
            merges: HashMap::new(),
            found_parts: 0,
            mutes: HashMap::new(),
            fretboard: None,
            custom: Vec::new(),
            clef: (Clef::Treble, 12),
        }
    }
}

/// Parts `sources` of `t` as one: their notes together, in time order, a
/// note both play at once kept once (the louder).
fn merge_parts(t: &Transcription, sources: &[usize]) -> Part {
    let mut notes: Vec<notes::Note> = sources
        .iter()
        .filter_map(|&i| t.parts.get(i))
        .flat_map(|p| p.notes.iter().cloned())
        .collect();
    notes.sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms).then(a.key.cmp(&b.key)));
    let mut out: Vec<notes::Note> = Vec::with_capacity(notes.len());
    for n in notes {
        // The same note in both parts, started together.
        if let Some(m) = out
            .iter_mut()
            .rev()
            .take_while(|m| n.start_ms - m.start_ms < 25.0)
            .find(|m| m.key == n.key)
        {
            let end = (m.start_ms + m.dur_ms).max(n.start_ms + n.dur_ms);
            m.velocity = m.velocity.max(n.velocity);
            m.dur_ms = end - m.start_ms;
            continue;
        }
        out.push(n);
    }
    let first = sources.first().and_then(|&i| t.parts.get(i));
    Part {
        name: sources
            .iter()
            .filter_map(|&i| t.parts.get(i))
            .map(|p| p.name.split(" · ").next().unwrap_or(&p.name).to_string())
            .collect::<Vec<_>>()
            .join(" + "),
        program: first.map_or(0, |p| p.program),
        notes: out,
        ..Default::default()
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

/// The index, among the tunings to choose from, of the strings a part read
/// from a tab file is written for: after [`tab::TUNINGS`].
const WRITTEN: usize = tab::TUNINGS.len();
/// The index of the tuning of the player's own: after [`WRITTEN`].
const CUSTOM: usize = WRITTEN + 1;
/// The most strings a tuning of one's own can have.
pub const MAX_STRINGS: usize = 10;
/// The lowest and highest an open string can be tuned (MIDI keys).
const STRING_RANGE: (u8, u8) = (16, 84);

/// "Custom (D A D G B E)".
fn custom_name(strings: &[u8]) -> String {
    let names: Vec<String> = strings.iter().map(|&k| pitch_name(k)).collect();
    format!("Custom ({})", names.join(" "))
}

/// A note's name without its octave ("F#").
fn pitch_name(key: u8) -> String {
    notes::key_name(key)
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '-')
        .to_string()
}

/// "As written (D A D G B E, capo 2)": the tuning a part read from a tab
/// file is written for.
fn written_name(p: &Part) -> String {
    let names: Vec<String> = p
        .strings
        .iter()
        .map(|&k| pitch_name(k.saturating_sub(p.capo)))
        .collect();
    let capo = if p.capo > 0 {
        format!(", capo {}", p.capo)
    } else {
        String::new()
    };
    format!("As written ({}{capo})", names.join(" "))
}

/// Tuning `i` (an index into [`tab::TUNINGS`], [`WRITTEN`] or
/// [`CUSTOM`], whose strings are `custom`) for part `p`.
fn tuning_of(p: &Part, i: usize, custom: &[u8]) -> tab::Tuning {
    match tab::TUNINGS.get(i) {
        Some(t) => t.clone(),
        None if i == CUSTOM && !custom.is_empty() => {
            tab::Tuning::new(custom_name(custom), custom.to_vec())
        }
        None if i == WRITTEN && !p.strings.is_empty() => {
            tab::Tuning::new(written_name(p), p.strings.clone())
        }
        None => tab::TUNINGS[0].clone(),
    }
}

/// Part `p` laid out on tuning `i` (a tuning of one's own: `custom`),
/// moved by hand as `a` says: on the strings and frets the tab file gives,
/// as written, until it is moved.
fn lay_out(p: &Part, i: usize, custom: &[u8], a: tab::Adjust) -> tab::Tab {
    let tuning = tuning_of(p, i, custom);
    let mut t = match i == WRITTEN && a == tab::Adjust::default() {
        true => tab::written(&p.notes, &p.written, &tuning),
        false => None,
    }
    .unwrap_or_else(|| tab::arrange_with(&p.notes, &tuning, a));
    if i == WRITTEN && p.capo > 0 {
        // Frets count from the capo; the strings are named as tuned.
        t.string_names = p
            .strings
            .iter()
            .map(|&k| pitch_name(k.saturating_sub(p.capo)))
            .collect();
    }
    t
}

/// The parts of `t`, laid out as `tabs` says ((part index, tuning, tab)),
/// written out in the bars of `meter`; a tuning of one's own has the
/// strings `custom`.
fn build_score(
    t: &Transcription,
    title: &str,
    meter: &Meter,
    tabs: &[(usize, usize, tab::Tab)],
    custom: &[u8],
) -> Score {
    let tunings: Vec<tab::Tuning> = tabs
        .iter()
        .map(|(i, tuning, _)| tuning_of(&t.parts[*i], *tuning, custom))
        .collect();
    let parts: Vec<score::PartTab> = tabs
        .iter()
        .zip(&tunings)
        .map(|((i, _, tb), tuning)| {
            let p = &t.parts[*i];
            score::PartTab {
                name: p.name.clone(),
                program: p.program,
                tuning,
                tab: tb,
            }
        })
        .collect();
    score::build(title, meter, &parts)
}

/// A track's notes as tab in bars, as the tab view first shows them (each
/// pitched part as a tab file writes it, or else on a guitar, or a bass if
/// it is low), over `meter`'s bars. None if there are no pitched notes.
pub fn default_score(t: &Transcription, title: &str, meter: &Meter) -> Option<Score> {
    let tabs: Vec<(usize, usize, tab::Tab)> = t
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.drums && !p.notes.is_empty())
        .map(|(i, p)| {
            let tuning = if !p.strings.is_empty() {
                WRITTEN
            } else if is_bass(p) {
                tab::tuning_named("Bass (4 strings)").unwrap_or(0)
            } else {
                0
            };
            (i, tuning, lay_out(p, tuning, &[], tab::Adjust::default()))
        })
        .collect();
    (!tabs.is_empty()).then(|| build_score(t, title, meter, &tabs, &[]))
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
        self.fretboard = None;
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
        self.fretboard = None;
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
                self.found_parts = t.parts.len();
            }
            self.found = Some(r);
            // Parts merged before, back (the track was shown earlier).
            let merges = self
                .track
                .as_ref()
                .and_then(|k| self.merges.get(k))
                .cloned()
                .unwrap_or_default();
            if let Some(Ok(t)) = &mut self.found {
                for m in &merges {
                    let p = merge_parts(t, m);
                    t.parts.push(p);
                    self.parts.push(t.parts.len() - 1);
                }
            }
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

    pub fn tuning_name(&self) -> String {
        match (tab::TUNINGS.get(self.tuning), self.current_part()) {
            (Some(t), _) => t.name.to_string(),
            (None, _) if self.tuning == CUSTOM && !self.custom.is_empty() => {
                custom_name(&self.custom)
            }
            (None, Some(p)) if !p.strings.is_empty() => written_name(p),
            _ => tab::TUNINGS[0].name.to_string(),
        }
    }

    /// The tunings listed (indices into [`tab::TUNINGS`], [`WRITTEN`] and
    /// [`CUSTOM`]), in the order [`TabView::tuning_names`] gives them.
    fn tuning_list(&self) -> Vec<usize> {
        let mut list: Vec<usize> = (0..tab::TUNINGS.len()).collect();
        if self.current_part().is_some_and(|p| !p.strings.is_empty()) {
            list.push(WRITTEN);
        }
        if !self.custom.is_empty() {
            list.push(CUSTOM);
        }
        list
    }

    /// Where the tuning in use is in [`TabView::tuning_names`].
    pub fn tuning_index(&self) -> usize {
        self.tuning_list()
            .iter()
            .position(|&i| i == self.tuning)
            .unwrap_or(0)
    }

    /// The tunings to choose from: the usual ones, the one the shown part
    /// is written for if it was read from a tab file, and the player's own
    /// once there is one.
    pub fn tuning_names(&self) -> Vec<String> {
        self.tuning_list()
            .into_iter()
            .map(|i| match tab::TUNINGS.get(i) {
                Some(t) => t.name.to_string(),
                None if i == CUSTOM => custom_name(&self.custom),
                None => self.current_part().map(written_name).unwrap_or_default(),
            })
            .collect()
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

    /// The parts as found that the shown part is (itself, or the parts
    /// merged into it).
    fn sources(&self, part: usize) -> Vec<usize> {
        let Some(&i) = self.parts.get(part) else {
            return Vec::new();
        };
        if i < self.found_parts {
            return vec![i];
        }
        self.track
            .as_ref()
            .and_then(|k| self.merges.get(k))
            .and_then(|m| m.get(i - self.found_parts))
            .cloned()
            .unwrap_or_default()
    }

    /// Whether the shown part was made by merging others.
    pub fn is_merged(&self) -> bool {
        self.parts
            .get(self.part)
            .is_some_and(|&i| i >= self.found_parts)
    }

    /// The parts the shown one can be merged with (indices into
    /// `part_names`), and their names: those with notes it doesn't have
    /// already.
    pub fn merge_choices(&self) -> Vec<(usize, String)> {
        let mine = self.sources(self.part);
        let names = self.part_names();
        (0..self.parts.len())
            .filter(|&k| k != self.part)
            .filter(|&k| !self.sources(k).iter().all(|i| mine.contains(i)))
            .map(|k| (k, names[k].clone()))
            .collect()
    }

    /// Adds a part with the notes of the shown part and of part `other`
    /// (an index into `part_names`) together, and shows it; a merge made
    /// before is shown again.
    pub fn merge_with(&mut self, other: usize) {
        let Some(track) = self.track.clone() else {
            return;
        };
        if other == self.part || other >= self.parts.len() {
            return;
        }
        let mut sources = self.sources(self.part);
        for i in self.sources(other) {
            if !sources.contains(&i) {
                sources.push(i);
            }
        }
        sources.sort_unstable();
        let found = self.found_parts;
        let merges = self.merges.entry(track).or_default();
        let k = match merges.iter().position(|m| *m == sources) {
            Some(k) => k,
            None => {
                let Some(Ok(t)) = &mut self.found else { return };
                let p = merge_parts(t, &sources);
                t.parts.push(p);
                self.parts.push(t.parts.len() - 1);
                merges.push(sources);
                merges.len() - 1
            }
        };
        if let Some(part) = self.parts.iter().position(|&i| i == found + k) {
            self.part = part;
        }
        self.arrange();
    }

    /// Takes the shown part away if it was made by merging others, showing
    /// the first of them.
    pub fn unmerge(&mut self) {
        if !self.is_merged() {
            return;
        }
        let Some(track) = self.track.clone() else {
            return;
        };
        let i = self.parts[self.part];
        let k = i - self.found_parts;
        let first = self.sources(self.part).first().copied();
        if let Some(m) = self.merges.get_mut(&track) {
            if k < m.len() {
                m.remove(k);
            }
        }
        // Merged parts after it move down one; their adjustments with them.
        if let Some(Ok(t)) = &mut self.found {
            t.parts.remove(i);
        }
        self.parts.retain(|&p| p != i);
        for p in &mut self.parts {
            if *p > i {
                *p -= 1;
            }
        }
        let moved: Vec<((TrackRef, usize), tab::Adjust)> = self
            .adjusts
            .keys()
            .filter(|(t, p)| *t == track && *p >= i)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|key| self.adjusts.remove_entry(&key))
            .collect();
        for ((t, p), a) in moved {
            if p > i {
                self.adjusts.insert((t, p - 1), a);
            }
        }
        self.part = first
            .and_then(|f| self.parts.iter().position(|&p| p == f))
            .unwrap_or(0);
        self.arrange();
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
        let n = self.tuning_names().len() as i32;
        self.set_tuning((self.tuning as i32 + d).rem_euclid(n) as usize);
    }

    /// Uses tuning `i` of [`TabView::tuning_names`].
    pub fn set_tuning(&mut self, i: usize) {
        if let Some(&t) = self.tuning_list().get(i) {
            self.tuning = t;
            self.tuning_chosen = true;
            self.arrange();
        }
    }

    // ---- a tuning of one's own

    /// The open strings of the tuning in use, lowest first.
    fn open_strings(&self) -> Vec<u8> {
        match self.current_part() {
            Some(p) => tuning_of(p, self.tuning, &self.custom).strings.to_vec(),
            None => tuning_of(&Part::default(), self.tuning, &self.custom)
                .strings
                .to_vec(),
        }
    }

    /// Whether the tuning in use is the player's own.
    pub fn is_custom(&self) -> bool {
        self.tuning == CUSTOM && !self.custom.is_empty()
    }

    /// The strings of the player's own tuning, lowest first, with their
    /// octaves ("E2", "A2"...); empty until there is one.
    pub fn custom_strings(&self) -> Vec<String> {
        self.custom.iter().map(|&k| notes::key_name(k)).collect()
    }

    /// Changes the player's own tuning with `f` (starting from the tuning
    /// in use if there isn't one yet) and uses it.
    fn edit_custom(&mut self, f: impl FnOnce(&mut Vec<u8>)) {
        let mut strings = if !self.is_custom() {
            self.open_strings()
        } else {
            self.custom.clone()
        };
        f(&mut strings);
        if strings.is_empty() {
            return;
        }
        for k in &mut strings {
            *k = (*k).clamp(STRING_RANGE.0, STRING_RANGE.1);
        }
        self.custom = strings;
        self.tuning = CUSTOM;
        self.tuning_chosen = true;
        self.arrange();
    }

    /// Makes a tuning of the player's own from the one in use (or uses it
    /// again, if there is one).
    pub fn start_custom(&mut self) {
        if self.custom.is_empty() {
            self.edit_custom(|_| {});
        } else if !self.is_custom() {
            self.tuning = CUSTOM;
            self.tuning_chosen = true;
            self.arrange();
        }
    }

    /// Starts the player's own tuning again from tuning `i` of
    /// [`TabView::tuning_names`].
    pub fn custom_from(&mut self, i: usize) {
        let Some(&t) = self.tuning_list().get(i) else {
            return;
        };
        let strings = match self.current_part() {
            Some(p) => tuning_of(p, t, &self.custom).strings.to_vec(),
            None => tuning_of(&Part::default(), t, &self.custom)
                .strings
                .to_vec(),
        };
        self.edit_custom(|s| *s = strings);
    }

    /// Every string of the player's own tuning `d` half steps higher
    /// (lower, if negative).
    pub fn custom_shift(&mut self, d: i32) {
        self.edit_custom(|s| {
            let can = s.iter().all(|&k| {
                (STRING_RANGE.0 as i32..=STRING_RANGE.1 as i32).contains(&(k as i32 + d))
            });
            if can {
                for k in s.iter_mut() {
                    *k = (*k as i32 + d) as u8;
                }
            }
        });
    }

    /// String `i` (0 the lowest) of the player's own tuning `d` half steps
    /// higher (lower, if negative).
    pub fn custom_shift_string(&mut self, i: usize, d: i32) {
        self.edit_custom(|s| {
            if let Some(k) = s.get_mut(i) {
                *k = (*k as i32 + d).clamp(STRING_RANGE.0 as i32, STRING_RANGE.1 as i32) as u8;
            }
        });
    }

    /// A string more on the player's own tuning: a fourth above the highest
    /// (`high`), or below the lowest.
    pub fn custom_add(&mut self, high: bool) {
        self.edit_custom(|s| {
            if s.len() >= MAX_STRINGS {
                return;
            }
            if high {
                let top = s.last().copied().unwrap_or(64);
                s.push(top.saturating_add(5));
            } else {
                let bottom = s.first().copied().unwrap_or(40);
                s.insert(0, bottom.saturating_sub(5));
            }
        });
    }

    /// The highest (`high`) or lowest string off the player's own tuning,
    /// leaving one at least.
    pub fn custom_remove(&mut self, high: bool) {
        self.edit_custom(|s| {
            if s.len() > 1 {
                if high {
                    s.pop();
                } else {
                    s.remove(0);
                }
            }
        });
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

    /// When bar `i` starts (not before the track does), to play from: the
    /// next whole millisecond, so it is in the bar, not just before it.
    pub fn bar_start(&self, i: usize) -> Option<u64> {
        self.bar_span(i).map(|b| b.0.max(0.0).ceil() as u64)
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
        let written = !p.strings.is_empty();
        let chosen = match self.tuning {
            WRITTEN => written,
            CUSTOM => !self.custom.is_empty(),
            _ => true,
        };
        if self.tuning_chosen && chosen {
            self.tuning
        } else if written {
            // As the tab file writes it.
            WRITTEN
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
        self.fretboard = None;
        self.lines.clear();
        self.stuck = None;
        let adjust = self.fit_spans(&self.adjust());
        let Some(p) = self.current_part() else { return };
        let tuning = self.tuning_for(p);
        let tab = lay_out(p, tuning, &self.custom, adjust.clone());
        self.clef = Clef::for_strings(&tuning_of(p, tuning, &self.custom).strings);
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
        let rows = tab::layout(tab, self.width, self.length_ms, &bars);
        let (clef, octave) = self.clef;
        let staves = staff::staff(tab, &rows, clef, octave, self.meter.as_ref());
        self.lines = rows
            .into_iter()
            .zip(staves)
            .map(|(r, staff)| Line {
                label: format_ms(r.start_ms.max(0.0) as u64),
                start_ms: r.start_ms,
                end_ms: r.end_ms,
                segments: r.segments(),
                bars: r.bars,
                cells: r.cells,
                marks: r.marks,
                chords: r.chords,
                staff,
            })
            .collect();
        self.fretboard = self.make_fretboard();
    }

    /// The shown part on the fretboard, a bar at a time (or a line of tab
    /// at a time, without bars).
    fn make_fretboard(&self) -> Option<Fretboard> {
        let tab = self.tab.as_ref()?;
        let groups: Vec<(f64, f64)> = match &self.meter {
            Some(m) if !m.bars.is_empty() => {
                let starts = m.starts();
                starts
                    .iter()
                    .enumerate()
                    .map(|(i, &a)| (a, starts.get(i + 1).copied().unwrap_or(m.end_ms.max(a))))
                    .collect()
            }
            _ => self.lines.iter().map(|l| (l.start_ms, l.end_ms)).collect(),
        };
        let mut groups = groups;
        // Notes before the first bar or after the last still show.
        if let Some(first) = groups.first_mut() {
            first.0 = first.0.min(0.0);
        }
        if let Some(last) = groups.last_mut() {
            last.1 = last.1.max(self.length_ms).max(
                tab.notes
                    .iter()
                    .map(|n| n.start_ms + n.dur_ms)
                    .fold(0.0, f64::max)
                    + 1.0,
            );
        }
        let notes: Vec<FretNote> = tab
            .notes
            .iter()
            .filter_map(|n| {
                let g = groups.partition_point(|&(a, _)| a <= n.start_ms);
                let group = g.checked_sub(1)?;
                Some(FretNote {
                    start_ms: n.start_ms,
                    end_ms: n.start_ms + n.dur_ms.max(1.0),
                    string: n.string,
                    fret: n.fret,
                    name: note_name(n.key).to_string(),
                    group,
                })
            })
            .collect();
        let highest = notes.iter().map(|n| n.fret).max().unwrap_or(0);
        Some(Fretboard {
            strings: self.strings(),
            frets: highest.clamp(12, 24),
            notes,
            groups,
        })
    }

    /// The shown part on the fretboard, through the song.
    pub fn fretboard(&self) -> Option<&Fretboard> {
        self.fretboard.as_ref()
    }

    /// The notes on the fretboard at `ms`: the bar's, those sounding marked.
    pub fn fret_dots(&self, ms: u64) -> Vec<FretDot> {
        self.fretboard
            .as_ref()
            .map(|f| f.dots(ms as f64))
            .unwrap_or_default()
    }

    // ------------------------------------------------- muting and soloing

    fn mutes(&self) -> Mutes {
        self.track
            .as_ref()
            .and_then(|t| self.mutes.get(t))
            .cloned()
            .unwrap_or_default()
    }

    /// The drum parts as found (the tab can't show them).
    fn drum_parts(&self) -> Vec<usize> {
        match self.transcription() {
            Some(t) => (0..self.found_parts.min(t.parts.len()))
                .filter(|&i| t.parts[i].drums && !t.parts[i].notes.is_empty())
                .collect(),
            None => Vec::new(),
        }
    }

    /// The parts as found that row `row` of [`TabView::mix_parts`] is.
    fn row_sources(&self, row: usize) -> Vec<usize> {
        if row < self.parts.len() {
            return self.sources(row);
        }
        self.drum_parts()
            .get(row - self.parts.len())
            .map(|&i| vec![i])
            .unwrap_or_default()
    }

    /// Every part, for the list of parts: those the tab shows (as
    /// [`TabView::part_names`] gives them), then the drums; each with what
    /// it is played on and whether it is muted, soloed and heard.
    pub fn mix_parts(&self) -> Vec<MixPart> {
        let Some(t) = self.transcription() else {
            return Vec::new();
        };
        let m = self.mutes();
        let names = self.part_names();
        let drums = self.drum_parts();
        let rows = self.parts.len() + drums.len();
        (0..rows)
            .map(|row| {
                let sources = self.row_sources(row);
                let (name, part) = if row < self.parts.len() {
                    (names[row].clone(), &t.parts[self.parts[row]])
                } else {
                    let p = &t.parts[sources[0]];
                    (p.name.clone(), p)
                };
                let all = |f: &dyn Fn(usize) -> bool| {
                    !sources.is_empty() && sources.iter().all(|&i| f(i))
                };
                MixPart {
                    name,
                    kind: instrument::kind_of(part),
                    pick: (row < self.parts.len()).then_some(row),
                    muted: all(&|i| Mutes::get(&m.muted, i)),
                    soloed: all(&|i| Mutes::get(&m.solo, i)),
                    audible: sources.iter().any(|&i| m.audible(i)),
                }
            })
            .collect()
    }

    /// Mutes row `row` of [`TabView::mix_parts`], or lets it back in.
    pub fn toggle_mute(&mut self, row: usize) {
        self.change_mutes(row, |m| &mut m.muted);
    }

    /// Solos row `row` of [`TabView::mix_parts`] (with any other parts
    /// soloed), or takes it out of the solo.
    pub fn toggle_solo(&mut self, row: usize) {
        self.change_mutes(row, |m| &mut m.solo);
    }

    /// Nothing muted or soloed.
    pub fn clear_mutes(&mut self) {
        if let Some(t) = self.track.clone() {
            self.mutes.remove(&t);
            crate::mutes::set(&t, 0);
        }
    }

    /// Something is muted or soloed.
    pub fn any_muted(&self) -> bool {
        !self.mutes().is_empty()
    }

    fn change_mutes(&mut self, row: usize, which: fn(&mut Mutes) -> &mut Vec<bool>) {
        let Some(track) = self.track.clone() else {
            return;
        };
        let sources = self.row_sources(row);
        if sources.is_empty() {
            return;
        }
        let mut m = self.mutes();
        let v = which(&mut m);
        let on = !sources.iter().all(|&i| Mutes::get(v, i));
        for &i in &sources {
            Mutes::set(v, i, on);
        }
        let channels = self.muted_channels(&m);
        if m.is_empty() {
            self.mutes.remove(&track);
        } else {
            self.mutes.insert(track.clone(), m);
        }
        crate::mutes::set(&track, channels);
    }

    /// The MIDI channels to silence for `m`: those whose parts are all
    /// muted (or left out of a solo).
    fn muted_channels(&self, m: &Mutes) -> u16 {
        let Some(t) = self.transcription() else {
            return 0;
        };
        let found = &t.parts[..self.found_parts.min(t.parts.len())];
        let channels = notes::midi::channels(found);
        let mut heard = 0u16;
        let mut used = 0u16;
        for (i, (p, &c)) in found.iter().zip(&channels).enumerate() {
            if p.notes.is_empty() {
                continue;
            }
            used |= 1 << c;
            if m.audible(i) {
                heard |= 1 << c;
            }
        }
        used & !heard
    }

    /// Whether muting is heard on this track: it plays through the
    /// SoundFont (a MIDI or tab file, or a chiptune in MIDI mode).
    pub fn can_mute(&self) -> bool {
        self.track.as_ref().is_some_and(|t| {
            chippy_core::formats::plays_through_soundfont(&t.loc.name())
                || (crate::midimode::get().is_some() && crate::midimode::possible(t))
        })
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
                (i, tuning, lay_out(p, tuning, &self.custom, a))
            })
            .collect();
        Some(build_score(t, title, m, &tabs, &self.custom))
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// The staff the shown tab is written on in standard notation.
    pub fn clef(&self) -> Clef {
        self.clef.0
    }

    /// The strings' names, lowest first.
    pub fn strings(&self) -> Vec<String> {
        match &self.tab {
            Some(t) => t.string_names.clone(),
            None => self.open_strings().iter().map(|&k| pitch_name(k)).collect(),
        }
    }

    /// What is in view with the view scrolled `top` lines down: the time
    /// the line nearest the top starts at, and how far above or below that
    /// line the view starts (in lines). See [`TabView::keep_in_view`].
    pub fn in_view(&self, top: f32) -> Option<(f64, f32)> {
        if self.lines.is_empty() {
            return None;
        }
        let i = (top.max(0.0).round() as usize).min(self.lines.len() - 1);
        Some((self.lines[i].start_ms, top - i as f32))
    }

    /// Where to scroll (lines from the top) so what [`TabView::in_view`]
    /// gave, before the lines changed, is in view again: the line with
    /// that time in it, as far down the view as before.
    pub fn keep_in_view(&self, (ms, offset): (f64, f32)) -> f32 {
        // A hair in, so a line ending where the old one started isn't it.
        let i = self
            .lines
            .iter()
            .position(|l| ms + 1.0 < l.end_ms)
            .unwrap_or(self.lines.len().saturating_sub(1));
        (i as f32 + offset).max(0.0)
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
            chords: Vec::new(),
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
        if t.notes.iter().any(|n| n.soft) {
            out.push("Notes in brackets are played softer (echoes).".into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use chippy_core::notes::Note;

    fn note(t: f64, key: u8) -> Note {
        Note {
            start_ms: t,
            dur_ms: 100.0,
            key,
            velocity: 100,
            legato: false,
        }
    }

    /// A view showing `t`, as if worked out for a MIDI file.
    fn showing(t: Transcription, name: &str) -> TabView {
        let mut v = TabView::default();
        let track = TrackRef::file(chippy_core::Location::Fs(name.into()));
        v.track = Some(track);
        v.tx.send((v.request, Ok((t, None)))).unwrap();
        assert!(v.poll());
        v
    }

    #[test]
    fn mutes_and_solos_parts() {
        let t = Transcription {
            parts: vec![
                Part::new("Lead Vocals", 52, vec![note(0.0, 64), note(200.0, 67)]),
                Part::new("Bass", 33, vec![note(0.0, 40)]),
                Part::drums("Drums", vec![note(0.0, 36)]),
            ],
            ..Default::default()
        };
        let mut v = showing(t, "/tmp/mutes-and-solos.mid");
        let track = v.track.clone().unwrap();
        let mix = v.mix_parts();
        let kinds: Vec<Kind> = mix.iter().map(|p| p.kind).collect();
        assert_eq!(kinds, vec![Kind::Vocals, Kind::Bass, Kind::Drums]);
        assert_eq!(mix[2].pick, None);
        assert!(v.can_mute());
        // Written out as MIDI: channels 1 and 2, the drums on 10.
        v.toggle_mute(1);
        assert_eq!(crate::mutes::get(&track), 1 << 1);
        assert!(!v.mix_parts()[1].audible && v.mix_parts()[1].muted);
        // A solo leaves out everything else, the muted bass included.
        v.toggle_solo(2);
        assert_eq!(crate::mutes::get(&track), 0b11);
        assert!(v.mix_parts()[2].audible && v.mix_parts()[2].soloed);
        v.toggle_solo(2);
        assert_eq!(crate::mutes::get(&track), 1 << 1);
        v.clear_mutes();
        assert_eq!(crate::mutes::get(&track), 0);
        assert!(!v.any_muted());
    }

    #[test]
    fn tunes_the_strings_by_hand() {
        let t = Transcription {
            parts: vec![Part::new(
                "Gtr",
                25,
                vec![note(0.0, 40), note(200.0, 45), note(400.0, 64)],
            )],
            ..Default::default()
        };
        let mut v = showing(t, "/tmp/tunes-by-hand.mid");
        let listed = v.tuning_names().len();
        assert!(!v.is_custom() && v.custom_strings().is_empty());
        // Made from standard tuning, then down a half step: the low E is
        // played a fret up on the low string.
        v.start_custom();
        assert!(v.is_custom());
        assert_eq!(v.tuning_names().len(), listed + 1);
        assert_eq!(v.tuning_index(), listed);
        assert_eq!(v.custom_strings()[0], "E2");
        v.custom_shift(-1);
        assert_eq!(v.tuning_name(), "Custom (D# G# C# F# A# D#)");
        assert_eq!(v.strings()[0], "D#");
        let low = v.tab.as_ref().unwrap().notes[0].clone();
        assert_eq!((low.string, low.fret), (0, 1));
        // Drop the low string to C#, add a high one, take the low one off.
        v.custom_shift_string(0, -2);
        assert_eq!(v.custom_strings()[0], "C#2");
        v.custom_add(true);
        assert_eq!(v.custom_strings().len(), 7);
        assert_eq!(v.custom_strings()[6], "G#4");
        v.custom_remove(false);
        assert_eq!(v.custom_strings().len(), 6);
        assert_eq!(v.strings().len(), 6);
        // Started again from the bass's strings.
        let bass = v
            .tuning_names()
            .iter()
            .position(|n| n == "Bass (4 strings)")
            .unwrap();
        v.custom_from(bass);
        assert_eq!(v.custom_strings(), ["E1", "A1", "D2", "G2"]);
        // Another tuning, and back to one's own as it was left.
        v.set_tuning(0);
        assert!(!v.is_custom());
        v.set_tuning(listed);
        assert!(v.is_custom());
        assert_eq!(v.custom_strings().len(), 4);
        // Each line has its notation, and its chords' times to click on.
        let line = &v.lines()[0];
        assert_eq!(line.staff.len(), line.chords.len());
        assert_eq!(line.chord_at(line.chords[1].1), Some(200.0));
        assert_eq!(v.clef(), Clef::Bass);
    }

    #[test]
    fn fretboard_shows_the_bar() {
        let t = Transcription {
            parts: vec![Part::new(
                "Gtr",
                29,
                vec![note(0.0, 58), note(500.0, 60), note(2100.0, 64)],
            )],
            ..Default::default()
        };
        let mut v = showing(t, "/tmp/fretboard.mid");
        v.found_meter = Some(Meter {
            bars: (0..2)
                .map(|b| meter::Bar {
                    start_ms: b as f64 * 2000.0,
                    beats: (0..4)
                        .map(|k| b as f64 * 2000.0 + k as f64 * 500.0)
                        .collect(),
                    sig: (4, 4),
                })
                .collect(),
            end_ms: 4000.0,
            exact: true,
        });
        v.fit_meter();
        v.arrange();
        let dots = v.fret_dots(50);
        assert_eq!(dots.len(), 2, "{dots:?}");
        assert_eq!(dots.iter().filter(|d| d.now).count(), 1);
        assert_eq!(dots.iter().find(|d| d.now).unwrap().name, "Bb");
        // Between notes: the bar's notes, none lit.
        assert!(v.fret_dots(300).iter().all(|d| !d.now));
        // The next bar has only its own note.
        let next = v.fret_dots(2150);
        assert_eq!(next.len(), 1);
        assert_eq!((next[0].name.as_str(), next[0].now), ("E", true));
        assert!(v.fretboard().unwrap().frets >= 12);
    }

    #[test]
    fn merged_parts_interleave() {
        // An arpeggio split across two channels, one note shared.
        let t = Transcription {
            parts: vec![
                Part::new(
                    "Pulse 1",
                    80,
                    vec![note(0.0, 67), note(400.0, 74), note(800.0, 60)],
                ),
                Part::new(
                    "Pulse 2",
                    80,
                    vec![note(200.0, 70), note(600.0, 75), note(805.0, 60)],
                ),
            ],
            ..Default::default()
        };
        let m = merge_parts(&t, &[0, 1]);
        assert_eq!(m.name, "Pulse 1 + Pulse 2");
        let keys: Vec<u8> = m.notes.iter().map(|n| n.key).collect();
        assert_eq!(keys, vec![67, 70, 74, 75, 60]);
        assert_eq!(m.notes[4].dur_ms, 105.0);
    }

    /// A part read from a tab file: open D, then fret 7 on the D string
    /// (which Chippy would play elsewhere), in drop D with a capo on 2.
    fn written_part() -> Part {
        Part {
            name: "Gtr".into(),
            program: 25,
            notes: vec![note(0.0, 40), note(500.0, 59)],
            strings: vec![40, 47, 52, 57, 61, 66],
            capo: 2,
            written: vec![Some(0), Some(2)],
            ..Default::default()
        }
    }

    #[test]
    fn tab_files_show_as_written() {
        let p = written_part();
        assert_eq!(written_name(&p), "As written (D A D G B E, capo 2)");
        let t = lay_out(&p, WRITTEN, &[], tab::Adjust::default());
        let places: Vec<(u8, u8)> = t.notes.iter().map(|n| (n.string, n.fret)).collect();
        assert_eq!(places, vec![(0, 0), (2, 7)]);
        assert_eq!(t.string_names, vec!["D", "A", "D", "G", "B", "E"]);
        // Moved along the neck, Chippy finds a fingering of its own on the
        // same strings.
        let moved = lay_out(
            &p,
            WRITTEN,
            &[],
            tab::Adjust {
                position: -1,
                ..Default::default()
            },
        );
        assert_eq!(moved.notes.len(), 2);
        assert!(moved.notes.iter().all(|n| n.key == 40 || n.key == 59));
        // Any other tuning ignores what is written.
        let std = lay_out(&p, 0, &[], tab::Adjust::default());
        assert_eq!(std.string_names, vec!["E", "A", "D", "G", "B", "E"]);

        // Written out, it keeps the file's tuning and frets.
        let t = Transcription {
            parts: vec![p],
            ..Default::default()
        };
        let m = meter::guess(&t.parts, 2000.0).unwrap_or(Meter {
            bars: vec![meter::Bar {
                start_ms: 0.0,
                beats: vec![0.0, 500.0, 1000.0, 1500.0],
                sig: (4, 4),
            }],
            end_ms: 2000.0,
            exact: true,
        });
        let s = default_score(&t, "Song", &m).unwrap();
        assert_eq!(s.tracks[0].strings, vec![40, 47, 52, 57, 61, 66]);
        let frets: Vec<u8> = s.tracks[0]
            .bars
            .iter()
            .flatten()
            .flat_map(|e| e.notes.iter().filter(|n| !n.tie_in).map(|n| n.fret))
            .collect();
        assert_eq!(frets, vec![0, 7]);
    }
}
