//! Commodore 64 (SID): each SID chip's three voices, read from their
//! registers every 8 milliseconds as libsidplayfp plays.
//!
//! A note starts when a voice's gate opens and ends when it closes; its
//! pitch is the frequency register. Voices playing noise are drums, except
//! for the short burst of noise many players start a tone with.

use anyhow::Result;

use super::{hz_to_key, Drums, Part, Transcription, Voice};
use crate::vfs::Location;

/// A noise burst this long at a note's start is part of the note's attack.
const BURST_MS: f64 = 30.0;

#[derive(Default)]
struct SidVoice {
    voice: Voice,
    gate: bool,
    /// A gate opened on noise, at this time: a drum, unless a tone follows
    /// within [`BURST_MS`].
    noise_since: Option<f64>,
    /// The instrument each note started with (ms, program).
    uses: Vec<(f64, u8)>,
}

/// The waveform a control register selects, as an instrument.
fn program(ctrl: u8, key: f64) -> u8 {
    if key < 48.0 {
        return 38; // synth bass
    }
    if ctrl & 0x40 != 0 {
        80 // pulse: square lead
    } else if ctrl & 0x20 != 0 {
        81 // sawtooth
    } else {
        // Triangle: soft, flute-like.
        79
    }
}

fn drum_key(hz: f64) -> u8 {
    if hz < 300.0 {
        36
    } else if hz < 2500.0 {
        38
    } else {
        42
    }
}

pub fn transcribe(loc: &Location, sub: Option<u32>, max_ms: Option<u64>) -> Result<Transcription> {
    let mut voices: Vec<SidVoice> = (0..9).map(|_| SidVoice::default()).collect();
    let mut drums = Drums::default();
    let mut used = 1;
    let mut last_t = 0.0;
    let ms = crate::decode::sid::capture(loc, sub, max_ms, |t, chip, regs, clock| {
        used = used.max(chip + 1);
        last_t = t;
        for v in 0..3 {
            let r = &regs[v * 7..v * 7 + 7];
            let sv = &mut voices[chip * 3 + v];
            let freq = r[0] as u32 | (r[1] as u32) << 8;
            let ctrl = r[4];
            let gate = ctrl & 1 != 0;
            let hz = freq as f64 * clock / 16_777_216.0;
            let noise = ctrl & 0x80 != 0;
            let tonal = ctrl & 0x70 != 0 && ctrl & 0x08 == 0 && !noise;
            // Attack 0-15 and sustain 0-15 are irrelevant to pitch; a voice
            // gated with no waveform, or the test bit, is silent.
            if gate && !sv.gate {
                if noise {
                    sv.voice.release(t);
                    sv.noise_since = Some(t);
                } else if tonal && hz >= 20.0 {
                    let key = hz_to_key(hz);
                    sv.voice.attack(t, key, 100);
                    sv.uses.push((t, program(ctrl, key)));
                }
            } else if gate {
                if let Some(since) = sv.noise_since {
                    if tonal && hz >= 20.0 {
                        if t - since <= BURST_MS {
                            // A tone after a burst of noise: the tone's attack.
                            let key = hz_to_key(hz);
                            sv.voice.attack(since, key, 100);
                            sv.uses.push((since, program(ctrl, key)));
                        } else {
                            drums.hit(since, drum_key(hz), 100);
                            let key = hz_to_key(hz);
                            sv.voice.attack(t, key, 100);
                            sv.uses.push((t, program(ctrl, key)));
                        }
                        sv.noise_since = None;
                    } else if t - since > BURST_MS {
                        drums.hit(since, drum_key(hz), 100);
                        sv.noise_since = None;
                    }
                } else if sv.voice.sounding() && tonal && hz >= 20.0 {
                    sv.voice.pitch(t, hz_to_key(hz));
                } else if sv.voice.sounding() && !tonal {
                    sv.voice.release(t);
                }
            } else if sv.gate {
                sv.voice.release(t);
                if let Some(since) = sv.noise_since.take() {
                    drums.hit(since, drum_key(hz), 100);
                }
            }
            sv.gate = gate;
        }
    })?;
    let end = (ms as f64).max(last_t);
    let mut parts: Vec<Part> = voices
        .into_iter()
        .take(used * 3)
        .enumerate()
        .map(|(i, sv)| {
            let name = if used > 1 {
                format!("SID {} voice {}", i / 3 + 1, i % 3 + 1)
            } else {
                format!("Voice {}", i + 1)
            };
            Part::new(name, 80, sv.voice.finish(end)).with_programs(&sv.uses)
        })
        .collect();
    parts.push(Part::drums("Drums (noise)", drums.finish()));
    Ok(Transcription {
        system: "Commodore 64".into(),
        parts,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    /// A PSID whose play routine gates A4 (pulse) on for 16 frames, off for
    /// 16.
    fn psid() -> Vec<u8> {
        let mut d = vec![0u8; 0x7C];
        d[..4].copy_from_slice(b"PSID");
        d[4..6].copy_from_slice(&2u16.to_be_bytes());
        d[6..8].copy_from_slice(&0x7Cu16.to_be_bytes());
        d[0x0A..0x0C].copy_from_slice(&0x1000u16.to_be_bytes());
        d[0x0C..0x0E].copy_from_slice(&0x1040u16.to_be_bytes());
        d[0x0E..0x10].copy_from_slice(&1u16.to_be_bytes());
        d[0x10..0x12].copy_from_slice(&1u16.to_be_bytes());
        d[0x76..0x78].copy_from_slice(&0x14u16.to_be_bytes()); // PAL, 6581
        let mut code = vec![0u8; 0x60];
        let init: &[u8] = &[
            0xA9, 0x0F, 0x8D, 0x18, 0xD4, // volume
            0xA9, 0x00, 0x8D, 0x05, 0xD4, // attack/decay
            0xA9, 0xF0, 0x8D, 0x06, 0xD4, // sustain/release
            0xA9, 0x08, 0x8D, 0x03, 0xD4, // pulse width
            0xA9, 0x44, 0x8D, 0x00, 0xD4, // frequency $1D44: 440 Hz on PAL
            0xA9, 0x1D, 0x8D, 0x01, 0xD4, //
            0xA9, 0x00, 0x8D, 0x00, 0x11, // frame counter
            0x60,
        ];
        let play: &[u8] = &[
            0xEE, 0x00, 0x11, 0xAD, 0x00, 0x11, 0x29, 0x10, 0xF0, 0x06, // every 16 frames
            0xA9, 0x41, 0x8D, 0x04, 0xD4, 0x60, // pulse, gate on
            0xA9, 0x40, 0x8D, 0x04, 0xD4, 0x60, // gate off
        ];
        code[..init.len()].copy_from_slice(init);
        code[0x40..0x40 + play.len()].copy_from_slice(play);
        d.extend_from_slice(&[0x00, 0x10]);
        d.extend_from_slice(&code);
        d
    }

    #[test]
    fn reads_gated_notes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sid");
        std::fs::write(&path, psid()).unwrap();
        let t = super::transcribe(&crate::vfs::Location::Fs(path), None, Some(3000)).unwrap();
        let v = &t.parts[0];
        assert!(v.notes.len() >= 4, "{:?}", v.notes);
        assert!(v.notes.iter().all(|n| n.key == 69), "{:?}", v.notes);
        // 16 frames of 20 ms.
        assert!((v.notes[1].dur_ms - 320.0).abs() < 25.0, "{:?}", v.notes[1]);
        assert_eq!(v.program, 80);
    }
}
