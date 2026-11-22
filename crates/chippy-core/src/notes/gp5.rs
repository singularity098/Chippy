//! Guitar Pro 5 files (`.gp5`, version 5.10) from a [`Score`]: a track per
//! part, with its tuning, its instrument, and the notes on its strings.
//! Guitar Pro 5 to 8, TuxGuitar, MuseScore and Songsterr open them.
//!
//! The layout follows what Guitar Pro itself writes, with every option
//! Chippy doesn't use left at its default (as PyGuitarPro writes them).

use super::meter::compound;
use super::score::{Event, Head, Score};

#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn i8(&mut self, v: i8) {
        self.0.push(v as u8);
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn zeros(&mut self, n: usize) {
        self.0.resize(self.0.len() + n, 0);
    }
    /// Text in Guitar Pro's 8-bit encoding (Latin-1; anything else as ?).
    fn encode(s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect()
    }
    /// A length byte, then the text padded to `size`.
    fn byte_string(&mut self, s: &str, size: usize) {
        let mut b = Self::encode(s);
        b.truncate(size);
        self.u8(b.len() as u8);
        let n = b.len();
        self.0.extend(b);
        self.zeros(size - n);
    }
    /// The length (plus one) as 32 bits, then as a byte, then the text.
    fn int_byte_string(&mut self, s: &str) {
        let mut b = Self::encode(s);
        b.truncate(255);
        self.i32(b.len() as i32 + 1);
        self.u8(b.len() as u8);
        self.0.extend(b);
    }
    /// The length as 32 bits, then the text.
    fn int_string(&mut self, s: &str) {
        let b = Self::encode(s);
        self.i32(b.len() as i32);
        self.0.extend(b);
    }
}

/// MIDI channels (0..64, four ports of 16) for each track: a channel and
/// an effects channel, skipping the drums' channel.
fn channels(tracks: usize) -> Vec<(u8, u8)> {
    let free: Vec<u8> = (0..64u8).filter(|c| c % 16 != 9).collect();
    (0..tracks)
        .map(|i| {
            let k = (i * 2) % (free.len() - 1);
            (free[k], free[k + 1])
        })
        .collect()
}

/// A channel's volume and the like, as Guitar Pro stores them.
fn level(v: i32) -> i8 {
    ((v >> 3) - 1).clamp(-128, 127) as i8 + 1
}

/// How eighths are beamed in a bar.
fn beams(head: &Head) -> [u8; 4] {
    if compound(head.sig) {
        let groups = head.sig.0 as usize / 3;
        let mut b = [0u8; 4];
        for g in b.iter_mut().take(groups.min(4)) {
            *g = 3;
        }
        b
    } else {
        [2, 2, 2, 2]
    }
}

/// The score as a Guitar Pro 5 file.
pub fn write(score: &Score) -> Vec<u8> {
    let mut o = Out::default();
    o.byte_string("FICHIER GUITAR PRO v5.10", 30);
    // Title, subtitle, artist, album, words, music, copyright, tab,
    // instructions; no notice.
    o.int_byte_string(&score.title);
    for _ in 0..8 {
        o.int_byte_string("");
    }
    o.i32(0);
    // No lyrics.
    o.i32(0);
    for _ in 0..5 {
        o.i32(1);
        o.int_string("");
    }
    // Master effect: volume, and a flat equalizer.
    o.i32(100);
    o.i32(0);
    o.zeros(11);
    // Page setup: A4, its margins, and the usual header and footer.
    for v in [210, 297, 10, 10, 15, 10, 100] {
        o.i32(v);
    }
    o.u8(0xFF);
    o.u8(0x01);
    for s in [
        "%title%",
        "%subtitle%",
        "%artist%",
        "%album%",
        "Words by %words%",
        "Music by %music%",
        "Words & Music by %WORDSMUSIC%",
        "Copyright %copyright%",
        "All Rights Reserved - International Copyright Secured",
        "Page %N%/%P%",
    ] {
        o.int_byte_string(s);
    }
    let tempo = |bpm: f64| bpm.round().clamp(1.0, 999.0) as i32;
    let first_tempo = score.heads.first().map_or(120, |h| tempo(h.bpm));
    o.int_byte_string("Moderate");
    o.i32(first_tempo);
    o.u8(0); // tempo shown
    o.i8(0); // C major
    o.i32(0);
    // The 64 MIDI channels: the tracks' own, the rest at their defaults.
    let chans = channels(score.tracks.len());
    for c in 0..64u8 {
        let program = chans
            .iter()
            .position(|&(a, b)| a == c || b == c)
            .map(|t| score.tracks[t].program as i32);
        match program {
            Some(p) => o.i32(p),
            None if c % 16 == 9 => o.i32(-1),
            None => o.i32(25),
        }
        o.i8(level(104));
        o.i8(level(64));
        o.zeros(4);
        o.zeros(2);
    }
    // No directions (coda, segno...); no reverb.
    for _ in 0..19 {
        o.i16(-1);
    }
    o.i32(0);
    o.i32(score.heads.len() as i32);
    o.i32(score.tracks.len() as i32);

    // Bar headers: time signatures where they change.
    let mut prev: Option<&Head> = None;
    for h in &score.heads {
        let mut flags = 0u8;
        match prev {
            None => flags |= 0x01 | 0x02 | 0x40,
            Some(p) => {
                o.zeros(1);
                if p.sig.0 != h.sig.0 {
                    flags |= 0x01;
                }
                if p.sig.1 != h.sig.1 {
                    flags |= 0x02;
                }
                if beams(p) != beams(h) {
                    flags |= 0x03;
                }
            }
        }
        o.u8(flags);
        if flags & 0x01 != 0 {
            o.i8(h.sig.0 as i8);
        }
        if flags & 0x02 != 0 {
            o.i8(h.sig.1 as i8);
        }
        if flags & 0x40 != 0 {
            o.i8(0);
            o.i8(0);
        }
        if flags & 0x03 != 0 {
            for b in beams(h) {
                o.u8(b);
            }
        }
        o.zeros(1);
        o.u8(0); // no triplet feel
        prev = Some(h);
    }

    // Tracks.
    for (i, (t, &(ch, fx))) in score.tracks.iter().zip(&chans).enumerate() {
        if i == 0 {
            o.zeros(1);
        }
        o.u8(0x08); // shown
        o.byte_string(&t.name, 40);
        o.i32(t.strings.len().min(7) as i32);
        for k in 0..7 {
            // Highest string first.
            let s = t
                .strings
                .len()
                .checked_sub(k + 1)
                .and_then(|s| t.strings.get(s));
            o.i32(s.map_or(0, |&k| k as i32));
        }
        o.i32(ch as i32 / 16 + 1);
        o.i32(ch as i32 + 1);
        o.i32(fx as i32 + 1);
        o.i32(24); // frets
        o.i32(0); // capo
        for c in [255, 0, 0, 0] {
            o.u8(c);
        }
        o.i16(0x43); // tab, notes, chord diagrams
        o.u8(0); // accentuation
        o.u8(0); // bank
        o.u8(0); // humanize
        let low = t.strings.iter().copied().min().unwrap_or(40);
        let clef = if low < 35 { 12 } else { 0 };
        o.i32(clef);
        o.i32(clef);
        o.i32(100);
        o.zeros(12);
        // No RSE sound: its instrument, equalizer and effect.
        for _ in 0..4 {
            o.i32(-1);
        }
        o.zeros(4);
        o.int_byte_string("");
        o.int_byte_string("");
    }
    o.zeros(1);

    // The bars, each track's in turn.
    let mut tempos: Vec<Option<i32>> = Vec::new();
    let mut shown = first_tempo;
    for h in &score.heads {
        let t = tempo(h.bpm);
        if (t - shown).abs() >= 1 {
            tempos.push(Some(t));
            shown = t;
        } else {
            tempos.push(None);
        }
    }
    for (bi, change) in tempos.iter().enumerate() {
        for (ti, t) in score.tracks.iter().enumerate() {
            let events = t.bars.get(bi).map(Vec::as_slice).unwrap_or(&[]);
            let n_strings = t.strings.len().min(7) as u8;
            let change = if ti == 0 { *change } else { None };
            if events.is_empty() {
                // A bar with nothing in it at all: one rest a bar long.
                o.i32(1);
                o.u8(0x40);
                o.u8(2);
                o.i8(-2);
                o.u8(0);
                o.i16(0);
            } else {
                o.i32(events.len() as i32);
                for (k, e) in events.iter().enumerate() {
                    write_beat(&mut o, e, n_strings, if k == 0 { change } else { None });
                }
            }
            // The second voice, empty.
            o.i32(1);
            o.u8(0x40);
            o.u8(0);
            o.i8(0);
            o.u8(0);
            o.i16(0);
            o.u8(0); // no line break
        }
    }
    o.0
}

fn write_beat(o: &mut Out, e: &Event, strings: u8, tempo: Option<i32>) {
    let v = e.value;
    let rest = e.notes.is_empty();
    let mut flags = 0u8;
    if v.dotted {
        flags |= 0x01;
    }
    if tempo.is_some() {
        flags |= 0x10;
    }
    if v.triplet {
        flags |= 0x20;
    }
    if rest {
        flags |= 0x40;
    }
    o.u8(flags);
    if rest {
        o.u8(2);
    }
    o.i8(v.base.trailing_zeros() as i8 - 2);
    if v.triplet {
        o.i32(3);
    }
    if let Some(t) = tempo {
        // A mix table change: only the tempo.
        o.i8(-1);
        for _ in 0..4 {
            o.i32(-1);
        }
        for _ in 0..6 {
            o.i8(-1);
        }
        o.int_byte_string("");
        o.i32(t);
        o.i8(0); // at once
        o.u8(0); // shown
        o.u8(0);
        o.i8(-1); // no wah
        o.int_byte_string("");
        o.int_byte_string("");
    }
    // Strings 1 (highest) to 7, as bits from the top.
    let mut notes: Vec<(u8, &super::score::Played)> = e
        .notes
        .iter()
        .filter(|n| n.string < strings)
        .map(|n| (strings - n.string, n))
        .collect();
    notes.sort_by_key(|n| n.0);
    notes.dedup_by_key(|n| n.0);
    let mut bits = 0u8;
    for (s, _) in &notes {
        bits |= 1 << (7 - s);
    }
    o.u8(bits);
    for (_, n) in notes {
        o.u8(0x20);
        if n.tie_in {
            o.u8(2);
            o.i8(0);
        } else {
            o.u8(1);
            o.i8(n.fret as i8);
        }
        o.u8(0);
    }
    o.i16(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_file() {
        let score = super::super::musicxml::tests::sample();
        let b = write(&score);
        assert_eq!(b[0], 24);
        assert_eq!(&b[1..25], b"FICHIER GUITAR PRO v5.10");
        // The title follows the version.
        assert_eq!(&b[31..35], &13i32.to_le_bytes());
        assert_eq!(&b[36..48], b"A & B <Test>");
        if let Ok(p) = std::env::var("CHIPPY_WRITE_SAMPLES") {
            std::fs::write(format!("{p}/sample.gp5"), &b).unwrap();
        }
    }

    #[test]
    fn gives_tracks_channels() {
        let c = channels(6);
        assert_eq!(c[0], (0, 1));
        assert_eq!(c[4], (8, 10));
        assert!(c.iter().all(|&(a, b)| a % 16 != 9 && b % 16 != 9));
        assert_eq!(level(104), 13);
        assert_eq!(level(64), 8);
        assert_eq!(level(0), 0);
    }
}
