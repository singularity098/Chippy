//! Notes from sound chips that play samples (the PlayStation's SPUs, the
//! Nintendo DS): a voice is keyed on with a sample and a playback rate,
//! which is a ratio, not a pitch. As for the SNES ([`super::spc`]), each
//! sample's own pitch is measured once, and gives the notes' pitches;
//! samples without a clear pitch play as drums.

use std::collections::HashMap;

use super::{hz_to_key, instrument, Drums, Part, SampleInfo, Voice};

/// A sample, decoded.
pub(crate) struct Pcm {
    pub pcm: Vec<f32>,
    pub loop_start: Option<usize>,
    /// The rate it plays at when the voice's rate ratio is 1.
    pub rate: f64,
}

struct Sample {
    info: SampleInfo,
    features: instrument::Features,
    keys: Vec<u8>,
}

/// The voices of a sample-playing chip, turned into notes.
pub(crate) struct Sampler {
    by_id: HashMap<u64, usize>,
    list: Vec<Sample>,
    voices: Vec<Voice>,
    /// The root key of the sample each voice plays, while it sounds.
    playing: Vec<Option<f64>>,
    /// Each voice's key-ons: (ms, sample).
    uses: Vec<Vec<(f64, usize)>>,
    /// Unpitched hits: (ms, sample, or None for the chip's noise, velocity).
    hits: Vec<(f64, Option<usize>, u8)>,
}

impl Sampler {
    pub fn new(voices: usize) -> Sampler {
        Sampler {
            by_id: HashMap::new(),
            list: Vec::new(),
            voices: (0..voices).map(|_| Voice::default()).collect(),
            playing: vec![None; voices],
            uses: vec![Vec::new(); voices],
            hits: Vec::new(),
        }
    }

    /// The sample known as `id` (by its address and contents, say),
    /// decoded and measured by `decode` the first time.
    pub fn sample(&mut self, id: u64, decode: impl FnOnce() -> Pcm) -> usize {
        if let Some(&i) = self.by_id.get(&id) {
            return i;
        }
        let s = decode();
        let p = instrument::pitch(&s.pcm, s.loop_start, s.rate);
        let hz = p.as_ref().map(|p| p.hz);
        let features = instrument::measure(&s.pcm, s.loop_start, s.rate, hz);
        self.list.push(Sample {
            info: SampleInfo {
                srcn: self.list.len().min(255) as u8,
                start: id as u16,
                len: s.pcm.len(),
                looped: s.loop_start.is_some(),
                hz,
                clarity: p.as_ref().map_or(0.0, |p| p.clarity),
                ..Default::default()
            },
            features,
            keys: Vec::new(),
        });
        let i = self.list.len() - 1;
        self.by_id.insert(id, i);
        i
    }

    /// Voice `v` starts `sample` at `ratio` times its rate.
    pub fn key_on(&mut self, t: f64, v: usize, sample: usize, ratio: f64, velocity: u8) {
        let s = &mut self.list[sample];
        s.info.uses += 1;
        match s.info.hz {
            Some(hz) if ratio > 0.0 => {
                let base = hz_to_key(hz);
                let key = base + 12.0 * ratio.log2();
                s.keys.push(key.round().clamp(0.0, 127.0) as u8);
                self.voices[v].attack(t, key, velocity);
                self.playing[v] = Some(base);
                self.uses[v].push((t, sample));
            }
            _ => {
                self.key_off(t, v);
                self.hits.push((t, Some(sample), velocity));
            }
        }
    }

    /// Voice `v` starts the chip's noise: a drum.
    pub fn noise(&mut self, t: f64, v: usize, velocity: u8) {
        self.key_off(t, v);
        self.hits.push((t, None, velocity));
    }

    pub fn key_off(&mut self, t: f64, v: usize) {
        self.voices[v].release(t);
        self.playing[v] = None;
    }

    /// Voice `v`'s rate is now `ratio` times its sample's.
    pub fn pitch(&mut self, t: f64, v: usize, ratio: f64) {
        if let Some(base) = self.playing[v] {
            if ratio > 0.0 {
                self.voices[v].pitch(t, base + 12.0 * ratio.log2());
            }
        }
    }

    pub fn sounding(&self, v: usize) -> bool {
        self.playing[v].is_some()
    }

    /// The parts (voices named `name` 1, 2...) and the samples.
    pub fn finish(mut self, name: &str, end: f64) -> (Vec<Part>, Vec<SampleInfo>) {
        for s in &mut self.list {
            let median = instrument::median(&mut s.keys);
            match instrument::classify("", &s.features, s.info.hz.is_some(), median) {
                instrument::Kind::Melodic(p) => s.info.program = Some(p),
                instrument::Kind::Drum(k) => s.info.drum_key = Some(k),
            }
        }
        let list = &self.list;
        let mut parts: Vec<Part> = self
            .voices
            .into_iter()
            .zip(self.uses)
            .enumerate()
            .map(|(i, (v, uses))| {
                let progs: Vec<(f64, u8)> = uses
                    .iter()
                    .map(|&(t, s)| (t, list[s].info.program.unwrap_or(0)))
                    .collect();
                let mut p = Part::new("", 0, v.finish(end)).with_programs(&progs);
                p.name = format!("{name} {} · {}", i + 1, super::gm::program_name(p.program));
                p
            })
            .collect();
        let mut drums = Drums::default();
        for (t, s, vel) in self.hits {
            let key = match s {
                Some(s) => list[s].info.drum_key.unwrap_or(38),
                None => 42,
            };
            drums.hit(t, key, vel);
        }
        parts.push(Part::drums("Drums (unpitched samples)", drums.finish()));
        (parts, self.list.into_iter().map(|s| s.info).collect())
    }
}

/// A sample's identity: where it starts and a hash of its first bytes (a
/// driver may load other samples to the same place later).
pub(crate) fn sample_id(addr: usize, mem: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ addr as u64;
    for &b in mem.iter().skip(addr).take(64) {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// A velocity from a level out of `max`.
pub(crate) fn velocity(level: f64, max: f64) -> u8 {
    (40.0 + (level / max).clamp(0.0, 1.0) * 87.0) as u8
}
