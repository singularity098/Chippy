//! The guitar tab view's state, shared by the desktop and Android: the
//! playing track's notes (worked out on a thread of their own), which part
//! is shown on which instrument, and where playback is in the tab.

use chippy_core::meta::format_ms;
use chippy_core::notes::{self, gm, tab, Part, Transcription};
use chippy_core::TrackRef;
use crossbeam_channel::{Receiver, Sender};

/// The notes of a track, found in the background: (request, result).
type Found = (u64, Result<Transcription, String>);

/// A line of tab, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start_ms: f64,
    pub end_ms: f64,
    /// "1:04".
    pub label: String,
    /// (x 0..1, string (0 lowest), fret).
    pub cells: Vec<(f32, u8, u8)>,
}

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
        }
    }
}

/// The part to show first: a guitar if there is one, else the busiest
/// part (usually the melody or the bass).
fn first_part(t: &Transcription, parts: &[usize]) -> usize {
    let busiest = |guitar: bool| {
        parts
            .iter()
            .enumerate()
            .filter(|(_, &i)| !guitar || gm::is_guitar(t.parts[i].program))
            .max_by_key(|(_, &i)| t.parts[i].notes.len())
            .map(|(k, _)| k)
    };
    busiest(true).or_else(|| busiest(false)).unwrap_or(0)
}

fn median_key(p: &Part) -> u8 {
    let mut keys: Vec<u8> = p.notes.iter().map(|n| n.key).collect();
    notes::instrument::median(&mut keys).unwrap_or(60)
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
        self.length_ms = length_ms.unwrap_or(0) as f64;
        let (id, tx, track) = (self.request, self.tx.clone(), track.clone());
        std::thread::Builder::new()
            .name("tabs".into())
            .spawn(move || {
                crate::background_thread();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    notes::transcribe(&track.loc, track.sub, length_ms)
                        .map_err(|e| format!("{e:#}"))
                }))
                .unwrap_or_else(|_| Err("finding the notes failed".into()));
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
    }

    /// Takes in finished work; true if the view changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok((id, r)) = self.rx.try_recv() {
            if id != self.request {
                continue;
            }
            if let Ok(t) = &r {
                self.parts = (0..t.parts.len())
                    .filter(|&i| !t.parts[i].drums && !t.parts[i].notes.is_empty())
                    .collect();
                self.part = first_part(t, &self.parts);
                self.length_ms = self.length_ms.max(t.end_ms());
            }
            self.found = Some(r);
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

    fn arrange(&mut self) {
        self.tab = None;
        self.lines.clear();
        let Some(p) = self.current_part() else { return };
        let tuning = if self.tuning_chosen {
            self.tuning
        } else {
            // Low parts on a bass, the rest on a guitar.
            let bass = median_key(p) < 45 || (32..=39).contains(&p.program);
            if bass {
                3
            } else {
                0
            }
        };
        let tab = tab::arrange(&p.notes, &tab::TUNINGS[tuning]);
        self.tuning = tuning;
        let row_ms = tab::row_length_ms(&tab);
        self.lines = tab::rows(&tab, row_ms, self.length_ms)
            .into_iter()
            .map(|r| Line {
                label: format_ms(r.start_ms as u64),
                start_ms: r.start_ms,
                end_ms: r.end_ms,
                cells: r.cells,
            })
            .collect();
        // Trailing silence makes no lines worth scrolling to.
        while self.lines.len() > 1 && self.lines.last().is_some_and(|l| l.cells.is_empty()) {
            self.lines.pop();
        }
        self.tab = Some(tab);
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
        Some((
            i,
            ((ms - l.start_ms) / (l.end_ms - l.start_ms)).clamp(0.0, 1.0) as f32,
        ))
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
                tab::ascii(t, 80)
            ),
            None => String::new(),
        }
    }
}
