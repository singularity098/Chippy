//! Nintendo DS (2SF): the sound hardware's 16 channels, as vio2sf plays the
//! tune. A channel plays a sample (8- or 16-bit PCM, or IMA-ADPCM) at a
//! rate set by its timer, so the notes' pitches come from each sample's
//! measured pitch ([`super::sampled`]); channels 8-13 can instead play
//! square waves (exact pitches), and 14-15 noise (drums).

use anyhow::Result;
use twosf_sys::NoteEvent;

use super::chips::{part, sound, velocity as level_velocity};
use super::sampled::{sample_id, velocity, Pcm, Sampler};
use super::{Drums, Part, Transcription, Voice};
use crate::vfs::Location;

/// The rate a channel's timer counts at: the ARM7's 33.5 MHz over 2.
const TIMER_HZ: f64 = 33_513_982.0 / 2.0;
/// The rate samples are measured at (a ratio of 1).
const NOMINAL: f64 = 32_768.0;
/// The longest a sample is decoded to: 3 s at 32 kHz.
const MAX_SAMPLES: usize = 98_304;

fn rate(timer: u32) -> f64 {
    TIMER_HZ / (0x10000 - timer.min(0xFFFF)) as f64
}

/// Decodes a sample: (PCM, loop start), from `format` 0 (PCM8), 1
/// (PCM16) or 2 (IMA-ADPCM), `loopstart` and `length` in words.
pub(crate) fn decode(
    data: &[u8],
    format: i32,
    loopstart: u32,
    length: u32,
    looped: bool,
) -> (Vec<f32>, Option<usize>) {
    let total = (((loopstart + length) as usize) * 4).min(data.len());
    let loop_byte = (loopstart as usize * 4).min(total);
    let d = &data[..total];
    let (pcm, loop_at): (Vec<f32>, usize) = match format {
        0 => (
            d.iter().map(|&b| b as i8 as f32 / 128.0).collect(),
            loop_byte,
        ),
        1 => (
            d.as_chunks::<2>()
                .0
                .iter()
                .map(|&c| i16::from_le_bytes(c) as f32 / 32768.0)
                .collect(),
            loop_byte / 2,
        ),
        _ => {
            const STEPS: [i32; 89] = [
                7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50,
                55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279,
                307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282,
                1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871,
                5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818,
                18500, 20350, 22385, 24623, 27086, 29794, 32767,
            ];
            const INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
            if d.len() < 4 {
                return (Vec::new(), None);
            }
            let mut s = i16::from_le_bytes([d[0], d[1]]) as i32;
            let mut idx = (d[2] & 0x7F).min(88) as i32;
            let mut out = Vec::with_capacity((d.len() - 4) * 2);
            for &b in &d[4..] {
                for nib in [b & 15, b >> 4] {
                    let step = STEPS[idx as usize];
                    let mut diff = step >> 3;
                    if nib & 1 != 0 {
                        diff += step >> 2;
                    }
                    if nib & 2 != 0 {
                        diff += step >> 1;
                    }
                    if nib & 4 != 0 {
                        diff += step;
                    }
                    s = if nib & 8 != 0 { s - diff } else { s + diff }.clamp(-32768, 32767);
                    idx = (idx + INDEX[(nib & 7) as usize]).clamp(0, 88);
                    out.push(s as f32 / 32768.0);
                }
            }
            (out, loop_byte.saturating_sub(4) * 2)
        }
    };
    let mut pcm = pcm;
    pcm.truncate(MAX_SAMPLES);
    let loop_start = (looped && loop_at < pcm.len()).then_some(loop_at);
    (pcm, loop_start)
}

struct Nds {
    sampler: Sampler,
    /// When each channel's one-shot sample runs out.
    ends: [Option<f64>; 16],
    psg: [Voice; 6],
    drums: Drums,
}

impl Nds {
    fn new() -> Nds {
        Nds {
            sampler: Sampler::new(16),
            ends: [None; 16],
            psg: Default::default(),
            drums: Drums::default(),
        }
    }

    fn expire(&mut self, t: f64) {
        for c in 0..16 {
            if let Some(e) = self.ends[c] {
                if e <= t {
                    self.sampler.key_off(e, c);
                    self.ends[c] = None;
                }
            }
        }
    }

    fn event(&mut self, t: f64, e: &NoteEvent, data: &[u8]) {
        self.expire(t);
        let c = (e.channel as usize).min(15);
        let level = (e.vol as u32 >> e.datashift.clamp(0, 4)) as f64;
        let psg = e.format == 3 && (8..14).contains(&c);
        match e.kind {
            0 if e.format == 3 => {
                self.sampler.key_off(t, c);
                if psg {
                    // An 8-step square wave.
                    let v = &mut self.psg[c - 8];
                    let vel = level_velocity(level as u8, 127);
                    sound(v, t, rate(e.timer) / 8.0, true, vel);
                } else if c >= 14 && level > 0.0 {
                    let r = rate(e.timer);
                    let key = if r > 20_000.0 {
                        42
                    } else if r > 5_000.0 {
                        38
                    } else {
                        36
                    };
                    self.drums.hit(t, key, velocity(level, 127.0));
                }
            }
            0 => {
                if (8..14).contains(&c) {
                    self.psg[c - 8].release(t);
                }
                if data.is_empty() || level == 0.0 {
                    return self.sampler.key_off(t, c);
                }
                let looped = e.repeat == 1;
                let id = sample_id(0, data) ^ (data.as_ptr() as u64).rotate_left(17);
                let s = self.sampler.sample(id, || {
                    let (pcm, loop_start) = decode(data, e.format, e.loopstart, e.length, looped);
                    Pcm {
                        pcm,
                        loop_start,
                        rate: NOMINAL,
                    }
                });
                let r = rate(e.timer);
                self.sampler
                    .key_on(t, c, s, r / NOMINAL, velocity(level, 127.0));
                self.ends[c] = None;
                if !looped {
                    // How long it lasts: its samples at this rate.
                    let samples = match e.format {
                        0 => (e.loopstart + e.length) * 4,
                        1 => (e.loopstart + e.length) * 2,
                        _ => ((e.loopstart + e.length) * 4).saturating_sub(4) * 2,
                    };
                    self.ends[c] = Some(t + samples as f64 * 1000.0 / r);
                }
            }
            1 => {
                if psg {
                    let v = &mut self.psg[c - 8];
                    if v.sounding() {
                        v.pitch(t, super::hz_to_key(rate(e.timer) / 8.0));
                    }
                } else {
                    self.sampler.pitch(t, c, rate(e.timer) / NOMINAL);
                }
            }
            _ => {
                self.sampler.key_off(t, c);
                self.ends[c] = None;
                if (8..14).contains(&c) {
                    self.psg[c - 8].release(t);
                }
            }
        }
    }

    fn finish(mut self, end: f64) -> Transcription {
        self.expire(end);
        let (mut parts, samples) = self.sampler.finish("Channel", end);
        let drums = parts.pop();
        for (i, v) in self.psg.into_iter().enumerate() {
            parts.push(part(format!("PSG {}", i + 9), 80, v.finish(end)));
        }
        parts.extend(drums);
        parts.push(Part::drums("Drums (noise)", self.drums.finish()));
        Transcription {
            system: "Nintendo DS".into(),
            parts,
            samples,
            notes: vec![
                "DS pitches depend on each sample's measured root note: a part playing an \
                 octave (or a fifth) off means its sample was misjudged. Instruments are \
                 guessed from how each sample sounds."
                    .into(),
            ],
            meter: None,
        }
    }
}

pub fn transcribe(loc: &Location, max_ms: Option<u64>) -> Result<Transcription> {
    let mut nds = Nds::new();
    let end = crate::decode::twosf::capture(loc, max_ms, &mut |t, e, data| nds.event(t, e, data))?;
    Ok(nds.finish(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(
        kind: i32,
        channel: i32,
        format: i32,
        timer: u32,
        data: &[u8],
        len_words: u32,
        repeat: i32,
    ) -> NoteEvent {
        NoteEvent {
            kind,
            channel,
            format,
            vol: 127,
            datashift: 0,
            duty: 0,
            repeat,
            timer,
            loopstart: 0,
            length: len_words,
            data: data.as_ptr(),
            data_size: data.len() as u32,
            sample: 0,
        }
    }

    #[test]
    fn sampled_channels_and_one_shots() {
        // A looped 16-bit tone of 64 samples a cycle: 512 Hz at 32768 Hz.
        let tone: Vec<u8> = (0..64 * 64)
            .flat_map(|i| {
                (((i as f64 / 64.0 * std::f64::consts::TAU).sin() * 20000.0) as i16).to_le_bytes()
            })
            .collect();
        let words = tone.len() as u32 / 4;
        let mut n = Nds::new();
        // At a timer giving 32768 Hz: the sample's own pitch, 512 Hz.
        let t32k = 0x10000 - (TIMER_HZ / 32768.0).round() as u32;
        n.event(0.0, &ev(0, 0, 1, t32k, &tone, words, 1), &tone);
        n.event(500.0, &ev(2, 0, 1, t32k, &[], 0, 1), &[]);
        // Played once (not looped) at twice the rate: an octave up, and
        // over when its 4096 samples run out (62.5 ms).
        let t64k = 0x10000 - (TIMER_HZ / 65536.0).round() as u32;
        n.event(600.0, &ev(0, 0, 1, t64k, &tone, words, 2), &tone);
        let t = n.finish(1000.0);
        let notes = &t.parts[0].notes;
        let got: Vec<_> = notes.iter().map(|n| (n.key, n.start_ms.round())).collect();
        // 512 Hz is C5 +38 cents.
        assert_eq!(got, vec![(72, 0.0), (84, 600.0)]);
        assert!((notes[1].dur_ms - 62.5).abs() < 0.5, "{:?}", notes[1]);
    }

    #[test]
    fn decodes_ima_adpcm() {
        // Header: sample 0, index 0; nibbles of +7 then -7 alternately.
        let mut d = vec![0, 0, 0, 0];
        d.extend([0x07u8; 8]);
        let (pcm, _) = decode(&d, 2, 0, 3, false);
        assert_eq!(pcm.len(), 16);
        assert!(pcm[0] > 0.0);
    }
}
