//! Standard notation for tab: each line of tab as a line of music on a
//! staff, the notes placed across it where the tab puts their fret numbers.
//!
//! Guitars and basses are written an octave above how they sound (guitar
//! on the treble clef, bass on the bass clef); a ukulele as it sounds. The
//! notes are spelled as guitarists read them (F#, C#, but Bb, Eb, Ab),
//! without a key signature: an accidental lasts to the end of its bar, and
//! a natural cancels it. Each chord's note value comes from how long it
//! lasts against the bar's beats, shown with its head, stem, flags and
//! dot (no beams, rests or ties: the line is spaced like the tab, not by
//! the beat).

use crate::meter::Meter;
use crate::tab::{Row, Tab};

/// Which clef a staff is written on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clef {
    /// Its bottom line is E4.
    Treble,
    /// Its bottom line is G2.
    Bass,
}

impl Clef {
    /// The clef for an instrument with these open strings, and the octaves
    /// it is written above how it sounds (in semitones).
    pub fn for_strings(strings: &[u8]) -> (Clef, i32) {
        let low = strings.iter().copied().min().unwrap_or(40);
        if low < 36 {
            (Clef::Bass, 12)
        } else if low >= 55 {
            (Clef::Treble, 0)
        } else {
            (Clef::Treble, 12)
        }
    }

    /// The step (see [`step_of`]) of the staff's bottom line.
    fn bottom(self) -> i32 {
        match self {
            Clef::Treble => 5 * 7 + 2,
            Clef::Bass => 3 * 7 + 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accidental {
    None,
    Sharp,
    Flat,
    Natural,
}

/// A note head on the staff.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    /// Where on the staff: 0 is the bottom line, 1 the space above it, 8
    /// the top line; below 0 and above 8 on ledger lines.
    pub pos: i32,
    pub accidental: Accidental,
    /// Set beside the stem rather than on it (the upper note of a second
    /// with the stem up, the lower with it down): -1 left, 1 right.
    pub side: i8,
    /// Played softer (an echo).
    pub soft: bool,
}

/// A chord (or a single note) on the staff.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    /// Across the line, 0..1: where the tab has it.
    pub x: f32,
    /// When it is played.
    pub ms: f64,
    /// Lowest first.
    pub heads: Vec<Head>,
    /// Whole and half notes have hollow heads.
    pub hollow: bool,
    /// Whole notes have none.
    pub stem: bool,
    /// Its stem goes up from the heads (else down).
    pub up: bool,
    /// Eighths have one, sixteenths two, thirty-seconds three.
    pub flags: u8,
    pub dotted: bool,
}

/// A note's letter (0 C to 6 B) and its sharp (1) or flat (-1), spelled as
/// a guitarist reads it.
fn spell(key: u8) -> (i32, i8) {
    const SPELLING: [(i32, i8); 12] = [
        (0, 0),
        (0, 1),
        (1, 0),
        (2, -1),
        (2, 0),
        (3, 0),
        (3, 1),
        (4, 0),
        (5, -1),
        (5, 0),
        (6, -1),
        (6, 0),
    ];
    SPELLING[key as usize % 12]
}

/// The step of a key as written: its letter counted up from C-1 (7 steps
/// an octave), with its sharp or flat.
fn step_of(key: i32) -> (i32, i8) {
    let key = key.clamp(0, 127);
    let (letter, alter) = spell(key as u8);
    ((key / 12) * 7 + letter, alter)
}

/// The note value `quarters` long, nearest of those written: (hollow,
/// stem, flags, dotted).
fn value(quarters: f64) -> (bool, bool, u8, bool) {
    const VALUES: [(f64, bool, bool, u8, bool); 9] = [
        (4.0, true, false, 0, false),
        (3.0, true, true, 0, true),
        (2.0, true, true, 0, false),
        (1.5, false, true, 0, true),
        (1.0, false, true, 0, false),
        (0.75, false, true, 1, true),
        (0.5, false, true, 1, false),
        (0.25, false, true, 2, false),
        (0.125, false, true, 3, false),
    ];
    let q = quarters.clamp(0.1, 6.0).log2();
    let v = VALUES
        .iter()
        .min_by(|a, b| (a.0.log2() - q).abs().total_cmp(&(b.0.log2() - q).abs()))
        .unwrap();
    (v.1, v.2, v.3, v.4)
}

/// The tab's notes on a staff, a line of music for each of `rows` (the
/// lines [`crate::tab::layout`] cut it into), on `clef` written `octave`
/// semitones above how it sounds, with note values counted along `meter`'s
/// beats (or at 120 a minute without one).
pub fn staff(
    tab: &Tab,
    rows: &[Row],
    clef: Clef,
    octave: i32,
    meter: Option<&Meter>,
) -> Vec<Vec<Chord>> {
    let none = Meter::default();
    let m = meter.unwrap_or(&none);
    let ruler = m.ruler();
    let bottom = clef.bottom();
    // Chords: the notes starting together, as the tab groups them.
    let mut chords: Vec<(f64, Vec<usize>)> = Vec::new();
    for (i, n) in tab.notes.iter().enumerate() {
        match chords.last_mut() {
            Some((t, v)) if n.start_ms - *t < 25.0 => v.push(i),
            _ => chords.push((n.start_ms, vec![i])),
        }
    }
    // Accidentals in force: (bar, step, sharp or flat).
    let mut bar = None;
    let mut altered: Vec<(i32, i8)> = Vec::new();
    let mut out = Vec::with_capacity(rows.len());
    for (ri, row) in rows.iter().enumerate() {
        let mut line = Vec::new();
        for &(t, x) in &row.chords {
            let c = chords.partition_point(|ch| ch.0 < t);
            let Some((_, idx)) = chords.get(c).filter(|ch| ch.0 == t) else {
                continue;
            };
            let next = chords.get(c + 1).map(|n| n.0);
            // Accidentals last to the end of the bar (or the line, without
            // bars).
            let b = m.bar_at(t + 15.0).unwrap_or(ri);
            if bar != Some(b) {
                bar = Some(b);
                altered.clear();
            }
            let mut keys: Vec<(i32, bool)> = idx
                .iter()
                .map(|&i| (tab.notes[i].key as i32 + octave, tab.notes[i].soft))
                .collect();
            keys.sort_unstable();
            keys.dedup_by_key(|k| k.0);
            let mut heads: Vec<Head> = keys
                .iter()
                .map(|&(key, soft)| {
                    let (step, alter) = step_of(key);
                    let before = altered.iter().find(|a| a.0 == step).map_or(0, |a| a.1);
                    let accidental = if alter == before {
                        Accidental::None
                    } else {
                        match alter {
                            1 => Accidental::Sharp,
                            -1 => Accidental::Flat,
                            _ => Accidental::Natural,
                        }
                    };
                    match altered.iter_mut().find(|a| a.0 == step) {
                        Some(a) => a.1 = alter,
                        None => altered.push((step, alter)),
                    }
                    Head {
                        pos: step - bottom,
                        accidental,
                        side: 0,
                        soft,
                    }
                })
                .collect();
            // How long it lasts: on to the next chord if the gap before it
            // is short (a chip's notes are often clipped), else its own
            // length.
            let dur = idx
                .iter()
                .map(|&i| tab.notes[i].dur_ms)
                .fold(0.0, f64::max)
                .max(1.0);
            let end = match next {
                Some(n) if n - (t + dur) < dur.max(60.0) => n,
                _ => t + dur,
            };
            let quarters = ruler.quarters(end) - ruler.quarters(t);
            let (hollow, stem, flags, dotted) = value(quarters);
            // Stems go down from notes on the middle line or above.
            let (lo, hi) = (heads[0].pos, heads[heads.len() - 1].pos);
            let up = lo + hi < 8;
            // Seconds: every other head of a run one apart to the side.
            if up {
                for k in 1..heads.len() {
                    if heads[k].pos - heads[k - 1].pos == 1 && heads[k - 1].side == 0 {
                        heads[k].side = 1;
                    }
                }
            } else {
                for k in (0..heads.len().saturating_sub(1)).rev() {
                    if heads[k + 1].pos - heads[k].pos == 1 && heads[k + 1].side == 0 {
                        heads[k].side = -1;
                    }
                }
            }
            line.push(Chord {
                x,
                ms: t,
                heads,
                hollow,
                stem,
                up,
                flags,
                dotted,
            });
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab::{arrange, layout, TUNINGS};
    use crate::Note;

    fn note(t: f64, dur: f64, key: u8) -> Note {
        Note {
            start_ms: t,
            dur_ms: dur,
            key,
            velocity: 100,
            legato: false,
        }
    }

    #[test]
    fn writes_guitar_an_octave_up_on_the_treble_clef() {
        assert_eq!(Clef::for_strings(&TUNINGS[0].strings), (Clef::Treble, 12));
        assert_eq!(Clef::for_strings(&[28, 33, 38, 43]), (Clef::Bass, 12));
        assert_eq!(Clef::for_strings(&[67, 60, 64, 69]), (Clef::Treble, 0));
        // Middle C on a guitar is written on the third space (C5).
        let tab = arrange(&[note(0.0, 500.0, 60)], &TUNINGS[0]);
        let rows = layout(&tab, 40.0, 2000.0, &[]);
        let s = staff(&tab, &rows, Clef::Treble, 12, None);
        assert_eq!(s.len(), rows.len());
        assert_eq!(s[0][0].heads[0].pos, 5);
        assert_eq!(s[0][0].x, rows[0].cells[0].0);
        // Low E on a guitar: three ledger lines below (E3).
        let tab = arrange(&[note(0.0, 500.0, 40)], &TUNINGS[0]);
        let rows = layout(&tab, 40.0, 2000.0, &[]);
        let s = staff(&tab, &rows, Clef::Treble, 12, None);
        assert_eq!(s[0][0].heads[0].pos, -7);
        assert!(s[0][0].up);
    }

    #[test]
    fn values_and_accidentals() {
        // At 120 a minute (no bars): a quarter, two eighths, a half; F#
        // then F (a natural) then F again (nothing), in one line.
        let notes = vec![
            note(0.0, 480.0, 54),
            note(500.0, 230.0, 53),
            note(750.0, 230.0, 53),
            note(1000.0, 1000.0, 55),
        ];
        let tab = arrange(&notes, &TUNINGS[0]);
        let rows = layout(&tab, 80.0, 2000.0, &[]);
        assert_eq!(rows.len(), 1);
        let s = &staff(&tab, &rows, Clef::Treble, 12, None)[0];
        assert_eq!(s.len(), 4);
        let acc: Vec<Accidental> = s.iter().map(|c| c.heads[0].accidental).collect();
        assert_eq!(
            acc,
            [
                Accidental::Sharp,
                Accidental::Natural,
                Accidental::None,
                Accidental::None
            ]
        );
        let flags: Vec<u8> = s.iter().map(|c| c.flags).collect();
        assert_eq!(flags, [0, 1, 1, 0]);
        assert!(!s[0].hollow && s[3].hollow && s[3].stem);
    }

    #[test]
    fn seconds_sit_beside_each_other() {
        // G and A together (a second), low enough for the stem to go up.
        let tab = arrange(&[note(0.0, 500.0, 55), note(0.0, 500.0, 57)], &TUNINGS[0]);
        let rows = layout(&tab, 40.0, 2000.0, &[]);
        let c = &staff(&tab, &rows, Clef::Treble, 12, None)[0][0];
        assert!(c.up);
        assert_eq!(c.heads.iter().map(|h| h.side).collect::<Vec<_>>(), [0, 1]);
    }
}
