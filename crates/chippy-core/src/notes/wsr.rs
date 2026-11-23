//! Bandai WonderSwan (WSR): the sound chip's four wavetable channels, read
//! from the writes the tune makes to its ports as Mednafen's core plays it.
//! Channel 2 can play samples instead (voice mode), and channel 4 noise:
//! those become drums, or are skipped. Channel 3's hardware sweep moves
//! its pitch without writes, so sweeps aren't followed.

use anyhow::Result;

use super::chips::{part, sound, velocity, Edge};
use super::{Drums, Part, Transcription, Voice};
use crate::vfs::Location;

/// The V30's 3.072 MHz clock over a 32-step wave.
const WAVE_HZ: f64 = 3_072_000.0 / 32.0;

#[derive(Default)]
struct Ws {
    period: [u16; 4],
    volume: [u8; 4],
    control: u8,
    edge: [Edge; 4],
    voices: [Voice; 4],
    drums: Drums,
}

impl Ws {
    fn write(&mut self, t: f64, port: u32, v: u8) {
        match port {
            0x80..=0x87 => {
                let c = ((port - 0x80) / 2) as usize;
                self.period[c] = if port & 1 != 0 {
                    (self.period[c] & 0xFF) | ((v & 7) as u16) << 8
                } else {
                    (self.period[c] & 0x700) | v as u16
                };
                self.update(t, c);
            }
            0x88..=0x8B => {
                let c = (port - 0x88) as usize;
                self.volume[c] = v;
                self.update(t, c);
            }
            0x90 => {
                let started = v & !self.control;
                self.control = v;
                for c in 0..4 {
                    if started & (1 << c) != 0 {
                        // A channel switched on starts a note.
                        self.edge[c].attack(0);
                    }
                    self.update(t, c);
                }
            }
            _ => {}
        }
    }

    fn update(&mut self, t: f64, c: usize) {
        let on = self.control & (1 << c) != 0;
        let samples = c == 1 && self.control & 0x20 != 0;
        let noise = c == 3 && self.control & 0x80 != 0;
        let vol = self.volume[c];
        let level = if on && !samples {
            (vol >> 4).max(vol & 15)
        } else {
            0
        };
        let attack = self.edge[c].attack(level);
        let v = &mut self.voices[c];
        if level == 0 {
            return v.release(t);
        }
        let step_hz = 3_072_000.0 / (2048 - self.period[c] as u32) as f64;
        if noise {
            v.release(t);
            if attack {
                let key = if step_hz > 40_000.0 {
                    42
                } else if step_hz > 8_000.0 {
                    38
                } else {
                    36
                };
                self.drums.hit(t, key, velocity(level, 15));
            }
            return;
        }
        let hz = WAVE_HZ / (2048 - self.period[c] as u32) as f64;
        sound(v, t, hz, attack, velocity(level, 15));
    }

    fn finish(self, end: f64) -> Vec<Part> {
        let mut parts: Vec<Part> = self
            .voices
            .into_iter()
            .enumerate()
            .map(|(i, v)| part(format!("Wave {}", i + 1), 80, v.finish(end)))
            .collect();
        parts.push(Part::drums("Drums (noise)", self.drums.finish()));
        parts
    }
}

pub fn transcribe(loc: &Location, sub: Option<u32>, max_ms: Option<u64>) -> Result<Transcription> {
    let mut ws = Ws::default();
    let mut end = 0.0f64;
    crate::decode::wsr::capture(loc, sub, max_ms, &mut |t, port, v| {
        end = end.max(t);
        ws.write(t, port, v);
    })?;
    let end = max_ms.map_or(end, |m| m as f64);
    Ok(Transcription {
        system: "Bandai WonderSwan".into(),
        parts: ws.finish(end),
        notes: vec!["Samples (channel 2's voice mode) aren't read as notes.".into()],
        ..Default::default()
    })
}
