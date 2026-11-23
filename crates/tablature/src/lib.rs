//! Guitar tablature and the music it is made from: notes, the beats and
//! bars they fall in, fingering on a fretted instrument, and scores written
//! out as MusicXML or Guitar Pro 5.
//!
//! Nothing here knows where the notes came from (a chiptune's sound chip, a
//! MIDI file, or one day a tab file read in) or how they sound: it has no
//! audio and no interface, so a player and a tab editor can share it.

pub mod gp5;
pub mod meter;
pub mod musicxml;
pub mod score;
pub mod tab;

/// One note, in milliseconds from the start of the track.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub start_ms: f64,
    pub dur_ms: f64,
    /// MIDI note number (60 is middle C).
    pub key: u8,
    pub velocity: u8,
    /// Reached by a pitch change without a new attack (a slide or legato).
    pub legato: bool,
}

/// One line of music: an instrument's part (a chip channel, a MIDI
/// channel, a guitar track), or the drums.
#[derive(Clone, Debug, Default)]
pub struct Part {
    pub name: String,
    /// Unpitched: keys are General MIDI percussion.
    pub drums: bool,
    /// A General MIDI program to play the part with.
    pub program: u8,
    /// Changes of program later on (ms, program), in time order: a channel
    /// that switches samples switches instrument.
    pub programs: Vec<(f64, u8)>,
    pub notes: Vec<Note>,
}

impl Part {
    pub fn new(name: impl Into<String>, program: u8, notes: Vec<Note>) -> Part {
        Part {
            name: name.into(),
            program,
            notes,
            ..Default::default()
        }
    }

    pub fn drums(name: impl Into<String>, notes: Vec<Note>) -> Part {
        Part {
            name: name.into(),
            drums: true,
            notes,
            ..Default::default()
        }
    }

    /// Sets the programs from the instrument each note started with
    /// ((ms, program) in time order): the first is the part's own, and each
    /// change after it a program change.
    pub fn with_programs(mut self, uses: &[(f64, u8)]) -> Part {
        self.programs.clear();
        let mut cur = None;
        for &(t, p) in uses {
            match cur {
                None => self.program = p,
                Some(c) if c != p => self.programs.push((t, p)),
                _ => {}
            }
            cur = Some(p);
        }
        self
    }

    /// The program sounding at `ms`.
    pub fn program_at(&self, ms: f64) -> u8 {
        self.programs
            .iter()
            .take_while(|(t, _)| *t <= ms)
            .last()
            .map_or(self.program, |&(_, p)| p)
    }
}

/// A MIDI note number's name, with its octave: 60 is `C4`.
pub fn key_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[key as usize % 12], key as i32 / 12 - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keys() {
        assert_eq!(key_name(60), "C4");
        assert_eq!(key_name(69), "A4");
    }
}
