//! What a part is played on, at a glance: a microphone for vocals, a
//! headstock for a guitar or a bass, sticks on a snare for the drums, a
//! square, saw, triangle or sine wave for a chip's channels... Worked out
//! from the part's name (chip channels and tab file tracks are named for
//! what they are) and its General MIDI instrument. The icons are line
//! drawings on a 24 × 24 grid, as SVG path data (stroked, and filled),
//! the same as the app's other icons, so both the desktop and Android
//! draw them.

use chippy_core::notes::Part;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Vocals,
    ElectricGuitar,
    AcousticGuitar,
    Bass,
    Drums,
    Piano,
    Organ,
    Strings,
    Brass,
    Reed,
    Flute,
    Mallets,
    SquareWave,
    SawWave,
    TriangleWave,
    SineWave,
    Sample,
    Pad,
    Effects,
    Other,
}

/// Whether `name` has any of `words` in it, as whole words (lower case).
fn has_word(name: &str, words: &[&str]) -> bool {
    name.split(|c: char| !c.is_alphanumeric())
        .any(|w| words.contains(&w))
}

/// "Voice", "Lead voice": a singer; but not "Voice 3", a chip's channel.
fn sung_voice(name: &str) -> bool {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().enumerate().any(|(i, w)| {
        *w == "voice"
            && !words
                .get(i + 1)
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
    })
}

fn has(name: &str, parts: &[&str]) -> bool {
    parts.iter().any(|p| name.contains(p))
}

/// What `p` is played on.
pub fn kind_of(p: &Part) -> Kind {
    if p.drums {
        return Kind::Drums;
    }
    // The name first: chip channels say which waveform they are, and tab
    // files' tracks what they are ("Lead Vocals", "Bass", "Acoustic").
    let name = p.name.to_lowercase();
    // A merged part ("Pulse 1 + Pulse 2"), or "Voice 3 · Flute": the part
    // itself, before what it is played with.
    let own = name.split(" · ").next().unwrap_or(&name);
    if has(own, &["vocal", "vox", "singer", "choir", "lyric"])
        || has_word(own, &["voc", "sing"])
        || sung_voice(own)
    {
        return Kind::Vocals;
    }
    if has(own, &["drum", "perc", "kit", "noise"]) {
        return Kind::Drums;
    }
    if has(own, &["pulse", "square"]) || has_word(own, &["psg", "ay", "sn76489", "duty"]) {
        return Kind::SquareWave;
    }
    if has(own, &["triangle"]) {
        return Kind::TriangleWave;
    }
    if has(own, &["saw"]) {
        return Kind::SawWave;
    }
    if has(own, &["sine"]) || has_word(own, &["fm", "opl", "opn", "opm", "vrc7", "ym2612"]) {
        return Kind::SineWave;
    }
    if has(own, &["dmc", "dpcm", "pcm", "sample", "adpcm"]) {
        return Kind::Sample;
    }
    if has(own, &["bass"]) {
        return Kind::Bass;
    }
    if has(
        own,
        &["acoustic", "nylon", "classical", "steel gtr", "folk"],
    ) {
        return Kind::AcousticGuitar;
    }
    if has(
        own,
        &["guitar", "gtr", "dist", "overdrive", "lead gt", "rhythm gt"],
    ) {
        return Kind::ElectricGuitar;
    }
    if has(own, &["piano", "keys", "rhodes"]) {
        return Kind::Piano;
    }
    if has(own, &["organ"]) {
        return Kind::Organ;
    }
    if has(own, &["violin", "viola", "cello", "string"]) {
        return Kind::Strings;
    }
    if has(own, &["trumpet", "trombone", "horn", "brass", "tuba"]) {
        return Kind::Brass;
    }
    if has(own, &["sax", "clarinet", "oboe", "bassoon"]) {
        return Kind::Reed;
    }
    if has(own, &["flute", "piccolo", "recorder", "whistle", "ocarina"]) {
        return Kind::Flute;
    }
    // Then the General MIDI instrument it plays with.
    kind_of_program(p.program)
}

/// What General MIDI program `program` is.
pub fn kind_of_program(program: u8) -> Kind {
    match program & 0x7F {
        0..=7 => Kind::Piano,
        8..=15 => Kind::Mallets,
        16..=23 => Kind::Organ,
        24..=25 => Kind::AcousticGuitar,
        26..=31 => Kind::ElectricGuitar,
        32..=39 => Kind::Bass,
        40..=51 => Kind::Strings,
        52..=54 => Kind::Vocals,
        55 => Kind::Effects,
        56..=63 => Kind::Brass,
        64..=71 => Kind::Reed,
        72..=79 => Kind::Flute,
        80 => Kind::SquareWave,
        81 => Kind::SawWave,
        82 => Kind::Flute,
        83 => Kind::SquareWave,
        84 => Kind::ElectricGuitar,
        85 => Kind::Vocals,
        86..=87 => Kind::SawWave,
        88..=95 => Kind::Pad,
        96..=103 => Kind::Effects,
        104 | 106 | 107 => Kind::AcousticGuitar,
        105 => Kind::AcousticGuitar,
        108 => Kind::Mallets,
        109..=111 => Kind::Reed,
        112..=119 => Kind::Drums,
        _ => Kind::Effects,
    }
}

impl Kind {
    /// What it is, for a tooltip or a screen reader.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Vocals => "Vocals",
            Kind::ElectricGuitar => "Electric guitar",
            Kind::AcousticGuitar => "Acoustic guitar",
            Kind::Bass => "Bass",
            Kind::Drums => "Drums",
            Kind::Piano => "Piano",
            Kind::Organ => "Organ",
            Kind::Strings => "Strings",
            Kind::Brass => "Brass",
            Kind::Reed => "Reed",
            Kind::Flute => "Flute",
            Kind::Mallets => "Mallets",
            Kind::SquareWave => "Square wave",
            Kind::SawWave => "Saw wave",
            Kind::TriangleWave => "Triangle wave",
            Kind::SineWave => "Sine wave (FM)",
            Kind::Sample => "Samples",
            Kind::Pad => "Synth pad",
            Kind::Effects => "Effects",
            Kind::Other => "Instrument",
        }
    }

    /// The icon: (stroked path, filled path), on a 24 × 24 grid.
    pub fn icon(self) -> (&'static str, &'static str) {
        match self {
            // A microphone on a stand.
            Kind::Vocals => (
                "M 8.8 6 A 3.2 3.2 0 0 1 15.2 6 L 15.2 9.6 A 3.2 3.2 0 0 1 8.8 9.6 Z M 5.6 10 A 6.4 6.4 0 0 0 18.4 10 M 12 16.4 L 12 20.5 M 8.5 20.5 L 15.5 20.5 M 8.8 7.2 L 11 7.2 M 8.8 9.2 L 11 9.2",
                "",
            ),
            // A Stratocaster's headstock: six tuner posts in a row down one
            // side of it, the end flaring out.
            Kind::ElectricGuitar => (
                "M 10 22 L 10 14.5 C 9.3 11.5 8.3 7 8.9 4.2 C 9.3 2.3 12.1 1.5 13.7 2.9 C 14.9 4 14.7 5.9 16.1 7.9 C 17.2 9.5 17 11.7 15.1 12.7 C 14.3 13.1 14 13.6 14 14.5 L 14 22 M 11.3 16.2 L 11.3 22 M 12.7 16.2 L 12.7 22",
                "M 11.95 4.4 A 0.85 0.85 0 1 1 10.25 4.4 A 0.85 0.85 0 1 1 11.95 4.4 Z M 11.85 6.3 A 0.85 0.85 0 1 1 10.15 6.3 A 0.85 0.85 0 1 1 11.85 6.3 Z M 11.95 8.2 A 0.85 0.85 0 1 1 10.25 8.2 A 0.85 0.85 0 1 1 11.95 8.2 Z M 12.15 10.1 A 0.85 0.85 0 1 1 10.45 10.1 A 0.85 0.85 0 1 1 12.15 10.1 Z M 12.45 12 A 0.85 0.85 0 1 1 10.75 12 A 0.85 0.85 0 1 1 12.45 12 Z M 12.85 13.9 A 0.85 0.85 0 1 1 11.15 13.9 A 0.85 0.85 0 1 1 12.85 13.9 Z",
            ),
            // An acoustic guitar's headstock: three tuners a side.
            Kind::AcousticGuitar => (
                "M 9.8 22 L 9.8 13.5 L 8.2 3.5 C 8.1 2.8 8.6 2.2 9.3 2.2 L 14.7 2.2 C 15.4 2.2 15.9 2.8 15.8 3.5 L 14.2 13.5 L 14.2 22 M 9.8 13.5 L 14.2 13.5 M 5 5 L 8.5 5 M 5.3 8 L 8.9 8 M 5.6 11 L 9.4 11 M 15.5 5 L 19 5 M 15.1 8 L 18.7 8 M 14.6 11 L 18.4 11 M 11.3 15 L 11.3 22 M 12.7 15 L 12.7 22",
                "M 6.1 5 A 1.15 1.15 0 1 1 3.8 5 A 1.15 1.15 0 1 1 6.1 5 Z M 6.4 8 A 1.15 1.15 0 1 1 4.1 8 A 1.15 1.15 0 1 1 6.4 8 Z M 6.7 11 A 1.15 1.15 0 1 1 4.4 11 A 1.15 1.15 0 1 1 6.7 11 Z M 20.2 5 A 1.15 1.15 0 1 1 17.9 5 A 1.15 1.15 0 1 1 20.2 5 Z M 19.9 8 A 1.15 1.15 0 1 1 17.6 8 A 1.15 1.15 0 1 1 19.9 8 Z M 19.6 11 A 1.15 1.15 0 1 1 17.3 11 A 1.15 1.15 0 1 1 19.6 11 Z",
            ),
            // A bass's headstock: four big tuners in a row, longer.
            Kind::Bass => (
                "M 10.2 22 L 10.2 15 C 9.6 12 9 6 9.6 3.2 C 10 1.6 12.4 1.4 13.4 2.8 C 14.4 4.4 14.2 7 15.4 9.4 C 16.2 11 15.8 12.8 14.6 13.6 C 14.1 14 13.8 14.5 13.8 15 L 13.8 22 M 5.6 4.4 L 9.5 4.4 M 5.6 7.6 L 9.4 7.6 M 5.8 10.8 L 9.6 10.8 M 6 14 L 10.1 14 M 12 16 L 12 22",
                "M 7 4.4 A 1.6 1.6 0 1 1 3.8 4.4 A 1.6 1.6 0 1 1 7 4.4 Z M 7 7.6 A 1.6 1.6 0 1 1 3.8 7.6 A 1.6 1.6 0 1 1 7 7.6 Z M 7.2 10.8 A 1.6 1.6 0 1 1 4 10.8 A 1.6 1.6 0 1 1 7.2 10.8 Z M 7.4 14 A 1.6 1.6 0 1 1 4.2 14 A 1.6 1.6 0 1 1 7.4 14 Z",
            ),
            // Two sticks hitting a snare drum.
            Kind::Drums => (
                "M 19 13 A 7 2.3 0 1 1 5 13 A 7 2.3 0 1 1 19 13 Z M 5 13 L 5 18.2 A 7 2.3 0 0 0 19 18.2 L 19 13 M 8 15 L 8 20.2 M 12 15.3 L 12 20.5 M 16 15 L 16 20.2 M 3.5 3.5 L 10.5 11.6 M 20.5 3.5 L 13.5 11.6",
                "",
            ),
            // A keyboard.
            Kind::Piano => (
                "M 4.5 5 L 19.5 5 A 1.5 1.5 0 0 1 21 6.5 L 21 17.5 A 1.5 1.5 0 0 1 19.5 19 L 4.5 19 A 1.5 1.5 0 0 1 3 17.5 L 3 6.5 A 1.5 1.5 0 0 1 4.5 5 Z M 12 13 L 12 19 M 7.5 13 L 7.5 19 M 16.5 13 L 16.5 19 M 6 5 L 6 13 L 9 13 L 9 5 M 15 5 L 15 13 L 18 13 L 18 5 M 10.5 5 L 10.5 13 L 13.5 13 L 13.5 5",
                "",
            ),
            // Organ pipes.
            Kind::Organ => (
                "M 3 20.5 L 21 20.5 M 4.5 20.5 L 4.5 11.5 L 7.5 11.5 L 7.5 20.5 M 7.5 20.5 L 7.5 7.5 L 10.5 7.5 L 10.5 20.5 M 10.5 20.5 L 10.5 3.5 L 13.5 3.5 L 13.5 20.5 M 13.5 20.5 L 13.5 7.5 L 16.5 7.5 L 16.5 20.5 M 16.5 20.5 L 16.5 11.5 L 19.5 11.5 L 19.5 20.5 M 5.2 17 L 6.8 17 M 8.2 17 L 9.8 17 M 11.2 17 L 12.8 17 M 14.2 17 L 15.8 17 M 17.2 17 L 18.8 17",
                "",
            ),
            // A violin, its bow across it.
            Kind::Strings => (
                "M 12 9 C 9.4 9 8.4 10.2 8.4 11.8 C 8.4 13.2 9.8 13.8 9.8 15 C 9.8 16 7.4 16.6 7.4 18.6 C 7.4 20.6 9.6 21.6 12 21.6 C 14.4 21.6 16.6 20.6 16.6 18.6 C 16.6 16.6 14.2 16 14.2 15 C 14.2 13.8 15.6 13.2 15.6 11.8 C 15.6 10.2 14.6 9 12 9 Z M 12 9 L 12 3.6 M 12 3.6 A 1.2 1.2 0 1 1 13.2 2.4 M 10.6 18 L 13.4 18 M 3.5 20 L 20.5 6",
                "",
            ),
            // A trumpet.
            Kind::Brass => (
                "M 2.5 10.5 L 15 10.5 M 2.5 13.5 L 15 13.5 M 2.5 9.6 L 2.5 14.4 M 15 10.5 C 17 10.5 19 9 21.5 7 L 21.5 17 C 19 15 17 13.5 15 13.5 M 7.5 10.5 L 7.5 6.5 M 10 10.5 L 10 6.5 M 12.5 10.5 L 12.5 6.5 M 6.6 6.5 L 8.4 6.5 M 9.1 6.5 L 10.9 6.5 M 11.6 6.5 L 13.4 6.5 M 6 13.5 C 6 17.5 13.5 17.5 13.5 13.5",
                "",
            ),
            // A saxophone.
            Kind::Reed => (
                "M 5 3 L 8.4 4 L 10.6 4.6 L 10.6 14.6 C 10.6 19 12.2 21 15 21 C 17.8 21 19 19 19 16.4 L 19 12.5 M 17.2 12.5 L 20.8 12.5 M 10.6 4.6 C 11.2 3.6 12.6 3.6 13 4.8 L 13 15",
                "M 14.2 7 A 1 1 0 1 1 12.2 7 A 1 1 0 1 1 14.2 7 Z M 14.2 10 A 1 1 0 1 1 12.2 10 A 1 1 0 1 1 14.2 10 Z M 14.2 13 A 1 1 0 1 1 12.2 13 A 1 1 0 1 1 14.2 13 Z",
            ),
            // A flute, held across.
            Kind::Flute => (
                "M 3 17.6 L 17.6 3 L 21 6.4 L 6.4 21 Z",
                "M 15.6 7.6 A 0.9 0.9 0 1 1 13.8 7.6 A 0.9 0.9 0 1 1 15.6 7.6 Z M 13.1 10.1 A 0.9 0.9 0 1 1 11.3 10.1 A 0.9 0.9 0 1 1 13.1 10.1 Z M 10.6 12.6 A 0.9 0.9 0 1 1 8.8 12.6 A 0.9 0.9 0 1 1 10.6 12.6 Z M 8.1 15.1 A 0.9 0.9 0 1 1 6.3 15.1 A 0.9 0.9 0 1 1 8.1 15.1 Z",
            ),
            // A xylophone's bars, and a mallet.
            Kind::Mallets => (
                "M 4 5 L 4 19 M 7.5 6 L 7.5 18 M 11 7 L 11 17 M 14.5 8 L 14.5 16 M 13 21.5 L 19 9",
                "M 21.4 7.6 A 1.9 1.9 0 1 1 17.6 7.6 A 1.9 1.9 0 1 1 21.4 7.6 Z",
            ),
            Kind::SquareWave => (
                "M 2.5 12 L 2.5 7 L 7.25 7 L 7.25 17 L 12 17 L 12 7 L 16.75 7 L 16.75 17 L 21.5 17 L 21.5 12",
                "",
            ),
            Kind::SawWave => (
                "M 2.5 17 L 8.8 7 L 8.8 17 L 15.2 7 L 15.2 17 L 21.5 7 L 21.5 12",
                "",
            ),
            Kind::TriangleWave => (
                "M 2.5 12 L 4.9 7 L 9.6 17 L 14.4 7 L 19.1 17 L 21.5 12",
                "",
            ),
            Kind::SineWave => (
                "M 2.5 12 C 4.1 5 5.7 5 7.25 12 C 8.8 19 10.4 19 12 12 C 13.6 5 15.2 5 16.75 12 C 18.3 19 19.9 19 21.5 12",
                "",
            ),
            // A recorded waveform.
            Kind::Sample => (
                "M 3 11.5 L 3 12.5 M 5.5 9 L 5.5 15 M 8 6 L 8 18 M 10.5 9.5 L 10.5 14.5 M 13 4 L 13 20 M 15.5 8 L 15.5 16 M 18 10 L 18 14 M 20.5 11.5 L 20.5 12.5",
                "",
            ),
            // Soft waves, one over another.
            Kind::Pad => (
                "M 3 8 C 6 5 9 11 12 8 C 15 5 18 11 21 8 M 3 12.5 C 6 9.5 9 15.5 12 12.5 C 15 9.5 18 15.5 21 12.5 M 3 17 C 6 14 9 20 12 17 C 15 14 18 20 21 17",
                "",
            ),
            // Sparkles.
            Kind::Effects => (
                "M 10 3.5 L 11.6 9.4 L 17.5 11 L 11.6 12.6 L 10 18.5 L 8.4 12.6 L 2.5 11 L 8.4 9.4 Z M 18 14.5 L 18.7 16.8 L 21 17.5 L 18.7 18.2 L 18 20.5 L 17.3 18.2 L 15 17.5 L 17.3 16.8 Z",
                "",
            ),
            // A note.
            Kind::Other => (
                "M 9 18 L 9 5.6 L 19 3.6 L 19 16",
                "M 9 18 A 2.6 2.2 0 1 1 3.8 18 A 2.6 2.2 0 1 1 9 18 Z M 19 16 A 2.6 2.2 0 1 1 13.8 16 A 2.6 2.2 0 1 1 19 16 Z",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, program: u8) -> Kind {
        kind_of(&Part::new(name, program, Vec::new()))
    }

    #[test]
    fn kinds() {
        assert_eq!(named("Lead Vocals", 52), Kind::Vocals);
        assert_eq!(named("Pulse 1", 38), Kind::SquareWave);
        assert_eq!(named("Triangle", 33), Kind::TriangleWave);
        assert_eq!(named("VRC6 Saw", 81), Kind::SawWave);
        assert_eq!(named("FM 3", 81), Kind::SineWave);
        assert_eq!(named("Bass", 33), Kind::Bass);
        assert_eq!(named("Acoustic", 25), Kind::AcousticGuitar);
        assert_eq!(named("Rhythm Gtr", 30), Kind::ElectricGuitar);
        assert_eq!(named("Channel 3", 25), Kind::AcousticGuitar);
        assert_eq!(named("Channel 2", 29), Kind::ElectricGuitar);
        assert_eq!(named("Channel 1", 0), Kind::Piano);
        // A SNES voice is no singer.
        assert_eq!(named("Voice 3 · Flute", 73), Kind::Flute);
        assert_eq!(named("Voice 2", 0), Kind::Piano);
        assert_eq!(
            kind_of(&Part::drums("Drums (noise)", Vec::new())),
            Kind::Drums
        );
    }
}
