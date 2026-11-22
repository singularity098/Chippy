//! Playing a tune's notes on other instruments: the same parts, given to a
//! piano, a string section, a rock band... Each pitched part is taken as
//! the lead (the tune), the bass, or a part in between, and each style
//! says what plays each of those, and whether the drums stay.

use super::{instrument, Part, Transcription};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    /// The instruments the notes were found with (each chip channel's
    /// closest General MIDI instrument, or a module's samples').
    Original,
    Piano,
    Strings,
    Rock,
    Jazz,
    Brass,
    MusicBox,
}

/// What a part does in the tune.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Lead,
    Middle,
    Bass,
}

impl Style {
    pub const ALL: [Style; 7] = [
        Style::Original,
        Style::Piano,
        Style::Strings,
        Style::Rock,
        Style::Jazz,
        Style::Brass,
        Style::MusicBox,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Style::Original => "Original instruments",
            Style::Piano => "Piano",
            Style::Strings => "Strings",
            Style::Rock => "Rock band",
            Style::Jazz => "Jazz combo",
            Style::Brass => "Brass band",
            Style::MusicBox => "Music box",
        }
    }

    /// For settings files: stays the same across versions.
    pub fn id(self) -> &'static str {
        match self {
            Style::Original => "original",
            Style::Piano => "piano",
            Style::Strings => "strings",
            Style::Rock => "rock",
            Style::Jazz => "jazz",
            Style::Brass => "brass",
            Style::MusicBox => "music-box",
        }
    }

    pub fn from_id(id: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.id() == id)
    }

    pub fn index(self) -> usize {
        Style::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Option<Style> {
        Style::ALL.get(i).copied()
    }

    /// The General MIDI program for a part of this role, an octave-ish
    /// range below middle C counting as low; None keeps the part's own.
    fn program(self, role: Role, low: bool) -> Option<u8> {
        use Role::*;
        Some(match (self, role) {
            (Style::Original, _) => return None,
            (Style::Piano, _) => 0,              // Acoustic Grand Piano
            (Style::Strings, Lead) => 40,        // Violin
            (Style::Strings, Middle) => 48,      // String Ensemble 1
            (Style::Strings, Bass) if low => 43, // Contrabass
            (Style::Strings, Bass) => 42,        // Cello
            (Style::Rock, Lead) => 29,           // Overdriven Guitar
            (Style::Rock, Middle) => 27,         // Electric Guitar (clean)
            (Style::Rock, Bass) => 34,           // Electric Bass (pick)
            (Style::Jazz, Lead) => 65,           // Alto Sax
            (Style::Jazz, Middle) => 4,          // Electric Piano 1
            (Style::Jazz, Bass) => 32,           // Acoustic Bass
            (Style::Brass, Lead) => 56,          // Trumpet
            (Style::Brass, Middle) => 61,        // Brass Section
            (Style::Brass, Bass) => 58,          // Tuba
            (Style::MusicBox, Bass) => 46,       // Orchestral Harp
            (Style::MusicBox, _) => 10,          // Music Box
        })
    }

    fn keeps_drums(self) -> bool {
        matches!(
            self,
            Style::Original | Style::Rock | Style::Jazz | Style::Brass
        )
    }
}

fn median_key(p: &Part) -> u8 {
    let mut keys: Vec<u8> = p.notes.iter().map(|n| n.key).collect();
    instrument::median(&mut keys).unwrap_or(60)
}

fn named(p: &Part, words: &[&str]) -> bool {
    let n = p.name.to_lowercase();
    words.iter().any(|w| n.contains(w))
}

/// The role of each part (drums get none).
fn roles(t: &Transcription) -> Vec<Option<Role>> {
    let bass = |p: &Part| {
        median_key(p) < 48 || (32..=39).contains(&p.program) || named(p, &["bass", "triangle"])
    };
    let mut roles: Vec<Option<Role>> = t
        .parts
        .iter()
        .map(|p| match () {
            _ if p.drums => None,
            _ if bass(p) => Some(Role::Bass),
            _ => Some(Role::Middle),
        })
        .collect();
    // The lead: of the parts above the bass at least half as busy as the
    // busiest, the highest (on a chip, the first square usually carries
    // the tune, at the top).
    let most = t
        .parts
        .iter()
        .zip(&roles)
        .filter(|(_, r)| **r == Some(Role::Middle))
        .map(|(p, _)| p.notes.len())
        .max()
        .unwrap_or(0);
    let lead = t
        .parts
        .iter()
        .enumerate()
        .filter(|(i, p)| roles[*i] == Some(Role::Middle) && p.notes.len() * 2 >= most)
        .max_by_key(|(i, p)| (median_key(p), std::cmp::Reverse(*i)))
        .map(|(i, _)| i);
    match lead {
        Some(i) => roles[i] = Some(Role::Lead),
        // Only bass parts: the busiest of them carries the tune.
        None => {
            if let Some(i) = (0..t.parts.len())
                .filter(|&i| roles[i].is_some())
                .max_by_key(|&i| t.parts[i].notes.len())
            {
                roles[i] = Some(Role::Lead);
            }
        }
    }
    roles
}

/// The tune's parts given to the instruments of `style`.
pub fn restyle(t: &Transcription, style: Style) -> Transcription {
    if style == Style::Original {
        return t.clone();
    }
    let roles = roles(t);
    let mut out = Transcription {
        system: t.system.clone(),
        notes: t.notes.clone(),
        meter: t.meter.clone(),
        ..Default::default()
    };
    for (p, role) in t.parts.iter().zip(roles) {
        let Some(role) = role else {
            if style.keeps_drums() {
                out.parts.push(p.clone());
            }
            continue;
        };
        let mut p = p.clone();
        if let Some(program) = style.program(role, median_key(&p) < 40) {
            p.program = program;
            p.programs.clear();
        }
        out.parts.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::Note;
    use super::*;

    fn part(name: &str, key: u8, n: usize) -> Part {
        let notes = (0..n)
            .map(|i| Note {
                start_ms: i as f64 * 100.0,
                dur_ms: 90.0,
                key,
                velocity: 100,
                legato: false,
            })
            .collect();
        Part::new(name, 80, notes)
    }

    fn tune() -> Transcription {
        Transcription {
            parts: vec![
                part("Square 1", 72, 40),
                part("Square 2", 67, 50),
                part("Triangle", 40, 30),
                Part::drums("Noise", part("", 38, 10).notes),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn finds_lead_and_bass() {
        let r = roles(&tune());
        assert_eq!(
            r,
            vec![Some(Role::Lead), Some(Role::Middle), Some(Role::Bass), None]
        );
    }

    #[test]
    fn restyles() {
        let t = tune();
        let piano = restyle(&t, Style::Piano);
        assert_eq!(piano.parts.len(), 3, "drums dropped");
        assert!(piano.parts.iter().all(|p| p.program == 0));
        let rock = restyle(&t, Style::Rock);
        let programs: Vec<_> = rock.parts.iter().map(|p| (p.program, p.drums)).collect();
        assert_eq!(
            programs,
            vec![(29, false), (27, false), (34, false), (0, true)]
        );
        let orig = restyle(&t, Style::Original);
        assert_eq!(orig.parts[0].program, 80);
    }

    #[test]
    fn ids_round_trip() {
        for s in Style::ALL {
            assert_eq!(Style::from_id(s.id()), Some(s));
            assert_eq!(Style::from_index(s.index()), Some(s));
        }
    }
}
