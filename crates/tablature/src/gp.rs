//! Reading Guitar Pro 3, 4 and 5 files (`.gp3`, `.gp4`, `.gp5`): the song's
//! details, each track's tuning, capo and instrument, and every note on its
//! string and fret, through the bars with their repeats and tempo changes.
//!
//! What Chippy has no use for (chord diagrams, bends, page layout, the RSE
//! sound) is read past. The layout is the one [`crate::gp5`] writes, and
//! that Guitar Pro, TuxGuitar, PyGuitarPro and alphaTab agree on.

use crate::song::{build, err, tuplet, Measure, Result, Song, Tempo, Track, Written, DRUM_CHANNEL};

struct R<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        match self.d.get(self.p..self.p + n) {
            Some(b) => {
                self.p += n;
                Ok(b)
            }
            None => err(format!("the file ends early (at byte {})", self.p)),
        }
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    /// A count that can't sensibly be more than `max`.
    fn count(&mut self, max: i32, what: &str) -> Result<usize> {
        let n = self.i32()?;
        if !(0..=max).contains(&n) {
            return err(format!("not a Guitar Pro file ({n} {what})"));
        }
        Ok(n as usize)
    }
    fn text(b: &[u8]) -> String {
        // Guitar Pro writes Windows' 8-bit encoding; Latin-1 is close.
        b.iter()
            .map(|&c| c as char)
            .collect::<String>()
            .trim_matches(|c: char| c == '\0' || c.is_whitespace())
            .to_string()
    }
    /// A length byte, then the text in a field `size` long.
    fn byte_string(&mut self, size: usize) -> Result<String> {
        let n = self.u8()? as usize;
        let b = self.take(size)?;
        Ok(Self::text(&b[..n.min(size)]))
    }
    /// The length (plus one) as 32 bits, then as a byte, then the text.
    fn int_byte_string(&mut self) -> Result<String> {
        let total = self.i32()?;
        if !(0..=1 << 20).contains(&total) {
            return err("not a Guitar Pro file (bad text)");
        }
        if total == 0 {
            return Ok(String::new());
        }
        let n = self.u8()? as usize;
        let b = self.take(total as usize - 1)?;
        Ok(Self::text(&b[..n.min(b.len())]))
    }
    /// The length as 32 bits, then the text.
    fn int_string(&mut self) -> Result<String> {
        let n = self.count(1 << 20, "bytes of text")?;
        Ok(Self::text(self.take(n)?))
    }
}

/// Which Guitar Pro the file is from: (3, 0), (4, 6), (5, 0), (5, 10)...
fn version(s: &str) -> Result<(u8, u8)> {
    if !s.starts_with("FICHIER GUITAR PRO") {
        return err("not a Guitar Pro 3 to 5 file");
    }
    let v = s.rsplit(['v', 'L']).next().unwrap_or("");
    let (major, minor) = v.split_once('.').unwrap_or((v, "0"));
    match (major.trim().parse::<u8>(), minor.trim().parse::<u8>()) {
        (Ok(m @ 3..=5), Ok(n)) => Ok((m, n)),
        _ => err(format!("unknown Guitar Pro version ({s})")),
    }
}

/// A note's loudness from its dynamic mark (1 ppp to 8 fff; 6 is forte).
fn velocity(dynamic: i8) -> u8 {
    (15 + 16 * (dynamic.clamp(1, 8) as i32 - 1)).min(127) as u8
}

/// How much quieter a ghost note is played.
const GHOST: f64 = 0.55;

struct Reader<'a> {
    r: R<'a>,
    major: u8,
    /// Guitar Pro 5.1 or later.
    v51: bool,
}

impl Reader<'_> {
    fn v5(&self) -> bool {
        self.major == 5
    }

    fn bend(&mut self) -> Result<()> {
        self.r.i8()?;
        self.r.i32()?;
        let n = self.r.count(1000, "bend points")?;
        self.r.skip(n * 9)
    }

    fn chord(&mut self) -> Result<()> {
        let new_format = self.r.u8()? != 0;
        match self.major {
            3 if new_format => self.r.skip(124),
            5 => self.r.skip(106),
            4 if new_format => self.r.skip(106),
            _ => {
                self.r.int_byte_string()?;
                if self.r.i32()? != 0 {
                    self.r.skip(6 * 4)?;
                }
                Ok(())
            }
        }
    }

    fn beat_effects(&mut self) -> Result<()> {
        if self.major == 3 {
            let f = self.r.u8()?;
            if f & 0x20 != 0 {
                self.r.i8()?;
                self.r.i32()?;
            }
            if f & 0x40 != 0 {
                self.r.skip(2)?;
            }
            return Ok(());
        }
        let f1 = self.r.u8()?;
        let f2 = self.r.u8()?;
        if f1 & 0x20 != 0 {
            self.r.i8()?;
        }
        if f2 & 0x04 != 0 {
            self.bend()?;
        }
        if f1 & 0x40 != 0 {
            self.r.skip(2)?;
        }
        if f2 & 0x02 != 0 {
            self.r.i8()?;
        }
        Ok(())
    }

    /// A mix table change: the tempo it sets, if it does.
    fn mix_table(&mut self) -> Result<Option<i32>> {
        self.r.i8()?; // instrument
        if self.v5() {
            self.r.skip(16)?; // RSE instrument
        }
        let mut levels = [0i8; 6];
        for l in &mut levels {
            *l = self.r.i8()?;
        }
        if self.v5() {
            self.r.int_byte_string()?; // tempo name
        }
        let tempo = self.r.i32()?;
        for l in levels {
            if l >= 0 {
                self.r.i8()?;
            }
        }
        if tempo >= 0 {
            self.r.i8()?;
            if self.v51 {
                self.r.u8()?;
            }
        }
        if self.major >= 4 {
            self.r.u8()?; // which tracks it applies to
        }
        if self.v5() {
            self.r.i8()?; // wah
        }
        if self.v51 {
            self.r.int_byte_string()?;
            self.r.int_byte_string()?;
        }
        Ok((tempo > 0).then_some(tempo))
    }

    fn note_effects(&mut self) -> Result<()> {
        if self.major == 3 {
            let f = self.r.u8()?;
            if f & 0x01 != 0 {
                self.bend()?;
            }
            if f & 0x10 != 0 {
                self.r.skip(4)?; // grace note
            }
            return Ok(());
        }
        let f1 = self.r.u8()?;
        let f2 = self.r.u8()?;
        if f1 & 0x01 != 0 {
            self.bend()?;
        }
        if f1 & 0x10 != 0 {
            self.r.skip(if self.v5() { 5 } else { 4 })?; // grace note
        }
        if f2 & 0x04 != 0 {
            self.r.u8()?; // tremolo picking
        }
        if f2 & 0x08 != 0 {
            self.r.u8()?; // slide
        }
        if f2 & 0x10 != 0 {
            // Harmonic.
            let kind = self.r.u8()?;
            if self.v5() {
                match kind {
                    2 => self.r.skip(3)?,
                    3 => self.r.skip(1)?,
                    _ => {}
                }
            }
        }
        if f2 & 0x20 != 0 {
            self.r.skip(2)?; // trill
        }
        Ok(())
    }

    /// A note: (kind (1 plain, 2 tied, 3 dead), fret, velocity, ghost).
    fn note(&mut self) -> Result<(u8, u8, u8, bool)> {
        let flags = self.r.u8()?;
        let ghost = flags & 0x04 != 0;
        let mut kind = 1;
        let mut vel = velocity(6);
        let mut fret = 0;
        if flags & 0x20 != 0 {
            kind = self.r.u8()?;
        }
        if !self.v5() && flags & 0x01 != 0 {
            self.r.skip(2)?; // its own duration
        }
        if flags & 0x10 != 0 {
            vel = velocity(self.r.i8()?);
        }
        if flags & 0x20 != 0 {
            fret = self.r.i8()?.max(0) as u8;
        }
        if flags & 0x80 != 0 {
            self.r.skip(2)?; // fingering
        }
        if self.v5() {
            if flags & 0x01 != 0 {
                self.r.skip(8)?;
            }
            self.r.u8()?;
        }
        if flags & 0x08 != 0 {
            self.note_effects()?;
        }
        Ok((kind, fret, vel, ghost))
    }
}

struct TrackHead {
    track: Track,
    /// Open strings, highest first, as the file lists them.
    high_first: Vec<u8>,
}

/// Reads a Guitar Pro 3, 4 or 5 file.
pub fn read(data: &[u8]) -> Result<Song> {
    let mut r = R { d: data, p: 0 };
    let v = r.byte_string(30)?;
    let (major, minor) = version(&v)?;
    let mut g = Reader {
        r,
        major,
        v51: major == 5 && minor >= 10,
    };
    let v5 = g.v5();
    let mut song = Song {
        format: format!("Guitar Pro {major}"),
        ..Default::default()
    };
    let r = &mut g.r;
    song.title = r.int_byte_string()?;
    song.subtitle = r.int_byte_string()?;
    song.artist = r.int_byte_string()?;
    song.album = r.int_byte_string()?;
    song.words = r.int_byte_string()?;
    if v5 {
        song.music = r.int_byte_string()?;
    } else {
        song.music = song.words.clone();
    }
    song.copyright = r.int_byte_string()?;
    song.tab_by = r.int_byte_string()?;
    let instructions = r.int_byte_string()?;
    if !instructions.is_empty() {
        song.comments.push(instructions);
    }
    let notice = r.count(1000, "lines of notice")?;
    let mut lines = Vec::new();
    for _ in 0..notice {
        lines.push(r.int_byte_string()?);
    }
    let notice = lines.join("\n").trim().to_string();
    if !notice.is_empty() {
        song.comments.push(notice);
    }
    if !v5 {
        r.u8()?; // triplet feel
    }
    if major >= 4 {
        // Lyrics.
        r.i32()?;
        for _ in 0..5 {
            r.i32()?;
            r.int_string()?;
        }
    }
    if g.v51 {
        r.skip(4 + 4 + 11)?; // master effect
    }
    if v5 {
        r.skip(7 * 4 + 2)?; // page setup
        for _ in 0..10 {
            r.int_byte_string()?;
        }
        r.int_byte_string()?; // tempo name
    }
    let bpm = r.i32()?;
    if g.v51 {
        r.u8()?;
    }
    match major {
        5 => {
            r.i8()?;
            r.i32()?;
        }
        4 => {
            r.i32()?;
            r.i8()?;
        }
        _ => {
            r.i32()?;
        }
    }
    // The 64 MIDI channels: the program each plays.
    let mut programs = [0u8; 64];
    for p in &mut programs {
        *p = r.i32()?.clamp(0, 127) as u8;
        r.skip(8)?;
    }
    if v5 {
        r.skip(19 * 2 + 4)?; // directions, reverb
    }
    let n_measures = r.count(100_000, "bars")?;
    let n_tracks = r.count(128, "tracks")?;

    // Bar headers.
    let mut measures: Vec<Measure> = Vec::with_capacity(n_measures);
    for i in 0..n_measures {
        if v5 && i > 0 {
            r.u8()?;
        }
        let flags = r.u8()?;
        let prev = measures.last();
        let mut m = Measure::new(prev.map_or((4, 4), |p| p.sig));
        if flags & 0x01 != 0 {
            m.sig.0 = r.i8()?.max(1) as u8;
        }
        if flags & 0x02 != 0 {
            m.sig.1 = r.i8()?.max(1) as u8;
        }
        m.repeat_open = flags & 0x04 != 0;
        if flags & 0x08 != 0 {
            let n = r.i8()? as i32 - if v5 { 1 } else { 0 };
            m.repeat_close = n.clamp(1, 32) as u32;
        }
        if !v5 && flags & 0x10 != 0 {
            // The ending's number: this one and those before it not yet
            // taken since the repeat started.
            let n = r.u8()?.min(31) as u32;
            let mut taken = 0;
            for p in measures.iter().rev() {
                taken |= p.endings;
                if p.repeat_open {
                    break;
                }
            }
            m.endings = ((1u32 << n) - 1) & !taken;
        }
        if flags & 0x20 != 0 {
            r.int_byte_string()?; // marker
            r.skip(4)?;
        }
        if flags & 0x40 != 0 {
            r.skip(2)?; // key
        }
        if v5 {
            if flags & 0x03 != 0 {
                r.skip(4)?; // beams
            }
            if flags & 0x10 != 0 {
                m.endings = r.u8()? as u32;
            } else {
                r.u8()?;
            }
            r.u8()?; // triplet feel
        }
        measures.push(m);
    }

    // Tracks.
    let mut tracks: Vec<TrackHead> = Vec::with_capacity(n_tracks);
    for i in 0..n_tracks {
        if v5 && (i == 0 || !g.v51) {
            r.u8()?;
        }
        let flags = r.u8()?;
        let name = r.byte_string(40)?;
        let n = r.count(7, "strings")?;
        let mut tuning = [0u8; 7];
        for t in &mut tuning {
            *t = r.i32()?.clamp(0, 127) as u8;
        }
        let port = r.i32()?;
        let channel = r.i32()?;
        r.i32()?; // effects channel
        r.i32()?; // frets
        let capo = r.i32()?.clamp(0, 24) as u8;
        r.skip(4)?; // colour
        if v5 {
            r.skip(2 + 1 + 1 + 1 + 3 * 4 + 12)?;
            r.skip(if g.v51 { 16 } else { 15 })?; // RSE instrument
            if g.v51 {
                r.skip(4)?;
                r.int_byte_string()?;
                r.int_byte_string()?;
            }
        }
        let ch = ((port - 1).clamp(0, 3) * 16 + (channel - 1).clamp(0, 15)) as usize;
        let drums = flags & 0x01 != 0 || ch % 16 == DRUM_CHANNEL as usize;
        let high_first = tuning[..n].to_vec();
        let mut strings = high_first.clone();
        strings.reverse();
        tracks.push(TrackHead {
            track: Track {
                name: if name.is_empty() {
                    format!("Track {}", i + 1)
                } else {
                    name
                },
                program: programs[ch],
                drums,
                strings,
                capo,
            },
            high_first,
        });
    }
    if v5 {
        r.skip(if g.v51 { 1 } else { 2 })?;
    }

    // The bars, each track's in turn.
    let mut notes: Vec<Written> = Vec::new();
    let mut tempos: Vec<Tempo> = Vec::new();
    let voices = if v5 { 2 } else { 1 };
    for mi in 0..n_measures {
        for (ti, th) in tracks.iter().enumerate() {
            let n_strings = th.high_first.len();
            for _ in 0..voices {
                let beats = g.r.count(10_000, "beats in a bar")?;
                let mut at = 0.0;
                for _ in 0..beats {
                    let flags = g.r.u8()?;
                    if flags & 0x40 != 0 {
                        g.r.u8()?; // empty or rest
                    }
                    let d = g.r.i8()?.clamp(-2, 6) as i32;
                    let mut q = 2f64.powi(-d);
                    if flags & 0x01 != 0 {
                        q *= 1.5;
                    }
                    if flags & 0x20 != 0 {
                        q *= tuplet(g.r.i32()?.clamp(1, 64) as u32);
                    }
                    if flags & 0x02 != 0 {
                        g.chord()?;
                    }
                    if flags & 0x04 != 0 {
                        g.r.int_byte_string()?;
                    }
                    if flags & 0x08 != 0 {
                        g.beat_effects()?;
                    }
                    if flags & 0x10 != 0 {
                        if let Some(bpm) = g.mix_table()? {
                            tempos.push(Tempo {
                                measure: mi,
                                at,
                                bpm: bpm as f64,
                            });
                        }
                    }
                    let bits = g.r.u8()?;
                    for s in 1..=7u8 {
                        if bits & (1 << (7 - s)) == 0 {
                            continue;
                        }
                        let (kind, fret, vel, ghost) = g.note()?;
                        let s = s as usize;
                        if kind == 3 || s > n_strings {
                            continue; // dead (muted) notes have no pitch
                        }
                        let vel = if ghost {
                            (vel as f64 * GHOST) as u8
                        } else {
                            vel
                        };
                        let (key, string) = if th.track.drums {
                            (fret, None)
                        } else {
                            let open = th.high_first[s - 1] as u32 + th.track.capo as u32;
                            (
                                (open + fret as u32).min(127) as u8,
                                Some((n_strings - s) as u8),
                            )
                        };
                        notes.push(Written {
                            track: ti,
                            measure: mi,
                            at,
                            quarters: q,
                            key,
                            velocity: vel,
                            string,
                            tie: kind == 2,
                        });
                    }
                    if v5 {
                        let f2 = g.r.i16()?;
                        if f2 & 0x0800 != 0 {
                            g.r.u8()?;
                        }
                    }
                    at += q;
                }
            }
            if v5 {
                g.r.u8()?; // line break
            }
        }
    }
    let tracks: Vec<Track> = tracks.into_iter().map(|t| t.track).collect();
    Ok(build(song, &measures, &tracks, &notes, &tempos, bpm as f64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meter::{beats_in, Bar, Meter};
    use crate::score::{build as score, PartTab};
    use crate::tab::{self, TUNINGS};
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

    /// Four bars of 4/4 at 120 bpm.
    fn meter() -> Meter {
        let bars = (0..4)
            .map(|b| Bar {
                start_ms: b as f64 * 2000.0,
                beats: (0..beats_in((4, 4)))
                    .map(|k| b as f64 * 2000.0 + k as f64 * 500.0)
                    .collect(),
                sig: (4, 4),
            })
            .collect();
        Meter {
            bars,
            end_ms: 8000.0,
            exact: true,
        }
    }

    #[test]
    fn reads_what_chippy_writes() {
        let lead: Vec<Note> = [64, 67, 69, 71, 72, 74, 76, 79]
            .iter()
            .enumerate()
            .map(|(i, &k)| note(i as f64 * 1000.0, k, 500.0))
            .collect();
        let bass: Vec<Note> = (0..4)
            .map(|i| note(i as f64 * 2000.0, 40 + i as u8 * 2, 1900.0))
            .collect();
        let lt = tab::arrange(&lead, &TUNINGS[0]);
        let bt = tab::arrange(&bass, &TUNINGS[5]);
        let s = score(
            "Round Trip",
            &meter(),
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
        );
        let song = read(&crate::gp5::write(&s)).unwrap();
        assert_eq!(song.title, "Round Trip");
        assert_eq!(song.format, "Guitar Pro 5");
        assert_eq!(song.parts.len(), 2);
        assert_eq!(song.meter.bars.len(), 4);
        assert!((song.meter.end_ms - 8000.0).abs() < 1.0);

        let p = &song.parts[0];
        assert_eq!((p.name.as_str(), p.program), ("Lead", 29));
        assert_eq!(p.strings, TUNINGS[0].strings.to_vec());
        let keys: Vec<u8> = p.notes.iter().map(|n| n.key).collect();
        assert_eq!(keys, lead.iter().map(|n| n.key).collect::<Vec<_>>());
        for (n, w) in p.notes.iter().zip(&lead) {
            assert!((n.start_ms - w.start_ms).abs() < 1.0, "{n:?}");
        }
        // On the strings the tab put them on.
        let strings: Vec<Option<u8>> = lt.notes.iter().map(|n| Some(n.string)).collect();
        assert_eq!(p.written, strings);
        let t = tab::written(
            &p.notes,
            &p.written,
            &tab::Tuning::new("", p.strings.clone()),
        )
        .unwrap();
        let frets: Vec<u8> = t.notes.iter().map(|n| n.fret).collect();
        assert_eq!(frets, lt.notes.iter().map(|n| n.fret).collect::<Vec<_>>());

        let b = &song.parts[1];
        assert_eq!(b.strings, TUNINGS[5].strings.to_vec());
        assert_eq!(b.notes.len(), 4);
        // Held most of the bar (rounded to sixteenths).
        assert!(
            (1800.0..=2000.0).contains(&b.notes[0].dur_ms),
            "{:?}",
            b.notes[0]
        );
    }

    /// Writes a Guitar Pro 3 or 4 file, as the format says.
    struct W(Vec<u8>);

    impl W {
        fn u8(&mut self, v: u8) -> &mut Self {
            self.0.push(v);
            self
        }
        fn i32(&mut self, v: i32) -> &mut Self {
            self.0.extend(v.to_le_bytes());
            self
        }
        fn byte_string(&mut self, s: &str, size: usize) -> &mut Self {
            self.u8(s.len() as u8);
            self.0.extend(s.bytes());
            self.0.resize(self.0.len() + size - s.len(), 0);
            self
        }
        fn int_byte_string(&mut self, s: &str) -> &mut Self {
            self.i32(s.len() as i32 + 1).u8(s.len() as u8);
            self.0.extend(s.bytes());
            self
        }
    }

    /// A Guitar Pro 3 or 4 file: a guitar (capo 2) and drums, three bars
    /// of 3/4 at 100 bpm, the second repeated, with a tie, a chord, a
    /// ghost note, a dead note and a change to 200 bpm.
    fn gp34(major: u8) -> Vec<u8> {
        let mut w = W(Vec::new());
        w.byte_string(&format!("FICHIER GUITAR PRO v{major}.00"), 30);
        for s in [
            "Song", "Sub", "Band", "Record", "Words", "(c)", "Tabber", "Play it",
        ] {
            w.int_byte_string(s);
        }
        w.i32(1).int_byte_string("A note");
        w.u8(0); // triplet feel
        if major == 4 {
            w.i32(0);
            for _ in 0..5 {
                w.i32(0).i32(0);
            }
        }
        w.i32(100).i32(0);
        if major == 4 {
            w.u8(0);
        }
        for c in 0..64 {
            w.i32(if c == 0 { 27 } else { 0 });
            w.0.extend([13, 8, 0, 0, 0, 0, 0, 0]);
        }
        w.i32(3).i32(2);
        // Bars: 3/4; one repeated once; one more.
        w.u8(0x03).u8(3).u8(4);
        w.u8(0x04 | 0x08).u8(1);
        w.u8(0x00);
        // Tracks: a guitar on channel 1, capo 2; drums on channel 10.
        w.u8(0).byte_string("Gtr", 40).i32(6);
        for k in [64, 59, 55, 50, 45, 40, 0] {
            w.i32(k);
        }
        w.i32(1).i32(1).i32(2).i32(24).i32(2).i32(0);
        w.u8(1).byte_string("Kit", 40).i32(6);
        for k in [64, 59, 55, 50, 45, 40, 0] {
            w.i32(k);
        }
        w.i32(1).i32(10).i32(10).i32(24).i32(0).i32(0);
        let effects_note = |w: &mut W| {
            // Note effects: a hammer-on (GP3: one byte; GP4: two).
            if major == 3 {
                w.u8(0x02);
            } else {
                w.u8(0x02).u8(0);
            }
        };
        // Bar 1, guitar: a half note, fret 3 on the top string, tied into
        // a quarter; then nothing more.
        w.i32(2);
        w.u8(0x00).u8(0xFF).u8(1 << 6); // half note, top string
        w.u8(0x20 | 0x08).u8(1).u8(3);
        effects_note(&mut w);
        w.u8(0x00).u8(0).u8(1 << 6);
        w.u8(0x20).u8(2).u8(3);
        // Bar 1, drums: a quarter note kick, a dotted half rest.
        w.i32(2);
        w.u8(0x00).u8(0).u8(1 << 1);
        w.u8(0x20).u8(1).u8(36);
        w.u8(0x41).u8(2).u8(0xFF).u8(0);
        // Bar 2, guitar: a chord (strings 5 and 6, frets 2 and 0) as a
        // quarter with a tempo change, a ghost note, a dead note.
        w.i32(3);
        w.u8(0x10).u8(0);
        // Mix table: only the tempo.
        w.u8(0xFF);
        for _ in 0..6 {
            w.u8(0xFF);
        }
        w.i32(200).u8(0);
        if major == 4 {
            w.u8(0);
        }
        w.u8((1 << 2) | (1 << 1));
        w.u8(0x20).u8(1).u8(2);
        w.u8(0x20).u8(1).u8(0);
        w.u8(0x00).u8(0).u8(1 << 6);
        w.u8(0x20 | 0x04 | 0x10).u8(1).u8(4).u8(5);
        w.u8(0x00).u8(0).u8(1 << 6);
        w.u8(0x20).u8(3).u8(0);
        // Bar 2, drums: empty.
        w.i32(0);
        // Bar 3, guitar: a whole-bar rest (dotted half); drums empty.
        w.i32(1);
        w.u8(0x41).u8(2).u8(0xFF).u8(0);
        w.i32(0);
        w.0
    }

    #[test]
    fn reads_guitar_pro_3_and_4() {
        for major in [3, 4] {
            let song = read(&gp34(major)).unwrap();
            assert_eq!(song.format, format!("Guitar Pro {major}"));
            assert_eq!(
                (
                    song.title.as_str(),
                    song.artist.as_str(),
                    song.album.as_str()
                ),
                ("Song", "Band", "Record")
            );
            assert_eq!(song.tab_by, "Tabber");
            assert_eq!(song.comments, vec!["Play it".to_string(), "A note".into()]);
            // 1, 2, 2 (repeated), 3.
            let starts: Vec<f64> = song.meter.bars.iter().map(|b| b.start_ms).collect();
            // 100 bpm: 1800 ms a bar; 200 bpm from bar 2: 900.
            assert_eq!(starts, vec![0.0, 1800.0, 2700.0, 3600.0]);
            assert!(song.meter.bars.iter().all(|b| b.sig == (3, 4)));

            let g = &song.parts[0];
            assert_eq!((g.name.as_str(), g.program, g.capo), ("Gtr", 27, 2));
            assert_eq!(g.strings, vec![42, 47, 52, 57, 61, 66]);
            let got: Vec<(f64, f64, u8, Option<u8>)> = g
                .notes
                .iter()
                .zip(&g.written)
                .map(|(n, s)| (n.start_ms, n.dur_ms, n.key, *s))
                .collect();
            assert_eq!(
                got,
                vec![
                    // Fret 3 on the top string, capo 2: held 1.2 s + 0.6 s.
                    (0.0, 1800.0, 69, Some(5)),
                    // The chord, twice (the bar repeats).
                    (1800.0, 300.0, 49, Some(1)),
                    (1800.0, 300.0, 42, Some(0)),
                    (2100.0, 300.0, 71, Some(5)),
                    (2700.0, 300.0, 49, Some(1)),
                    (2700.0, 300.0, 42, Some(0)),
                    (3000.0, 300.0, 71, Some(5)),
                ]
            );
            // The ghost note is played softer.
            assert!(g.notes[3].velocity < g.notes[1].velocity);

            let d = &song.parts[1];
            assert!(d.drums);
            assert!(d.written.is_empty() && d.strings.is_empty());
            assert_eq!(d.notes.len(), 1);
            assert_eq!(d.notes[0].key, 36);
        }
    }

    #[test]
    fn knows_versions() {
        assert_eq!(version("FICHIER GUITAR PRO v3.00").unwrap(), (3, 0));
        assert_eq!(version("FICHIER GUITAR PRO L4.06").unwrap(), (4, 6));
        assert_eq!(version("FICHIER GUITAR PRO v5.10").unwrap(), (5, 10));
        assert!(version("CLIPBOARD GUITAR PRO 5").is_err());
        assert!(read(b"not a tab file at all, just some words").is_err());
        assert!(read(&gp34(4)[..200]).is_err());
    }
}
