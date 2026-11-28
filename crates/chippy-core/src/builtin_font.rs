//! The built-in SoundFont: a small General MIDI SoundFont (SF2) made in
//! code, so MIDI can be heard before any SoundFont is chosen. Its
//! sounds are synthesised here (a Karplus–Strong plucked string for the
//! guitars, bass, keys and plucked instruments; looped waveforms for the
//! sustained and overdriven ones; a drum kit, the metronome's woodblocks
//! among it), so it has no samples from anywhere else and no licence of
//! its own. Every GM program has a preset (each family on the sound
//! nearest it), and the kit is bank 128.
//!
//! (The same file is in Tabby, `crates/tabby-core/src/builtin_font.rs`.)

/// The name it is listed under, among the SoundFonts found.
pub const NAME: &str = "Built-in";

/// The sample rate the sounds are made at.
const RATE: u32 = 44100;

/// The SF2 file's bytes.
pub fn sf2() -> Vec<u8> {
    Font::build().write()
}

// --------------------------------------------------------------- sounds

/// A small deterministic noise source (xorshift), so the font is the same
/// every time.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn secs(s: f32) -> usize {
    (s * RATE as f32) as usize
}

/// A plucked string (Karplus–Strong) with a delay of `n` samples (its
/// period is `n + 0.5`), `len` long; `soft` (0–1) how much the pluck is
/// low-passed (a warmer, rounder string).
fn pluck(n: usize, len: usize, soft: f32, seed: u32) -> Vec<f32> {
    let mut noise = Noise(seed);
    let mut line: Vec<f32> = (0..n).map(|_| noise.next()).collect();
    // Soften the pluck: a few passes of a one-pole low-pass, and the mean
    // taken out (no DC).
    for _ in 0..(1 + (soft * 6.0) as usize) {
        let mut y = line[n - 1];
        for v in line.iter_mut() {
            y += (*v - y) * (1.0 - soft * 0.8);
            *v = y;
        }
    }
    let mean = line.iter().sum::<f32>() / n as f32;
    line.iter_mut().for_each(|v| *v -= mean);
    let mut out = Vec::with_capacity(len);
    let mut i = 0;
    let mut prev = line[n - 1];
    while out.len() < len {
        let cur = line[i];
        let next = 0.5 * (cur + prev) * 0.9995;
        out.push(cur);
        prev = cur;
        line[i] = next;
        i = (i + 1) % n;
    }
    out
}

/// One period (`n` samples) of a waveform from its harmonics' amplitudes,
/// repeated `times`.
fn wave(n: usize, harmonics: &[f32], times: usize) -> Vec<f32> {
    let cycle: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            harmonics
                .iter()
                .enumerate()
                .map(|(h, a)| a * (t * (h + 1) as f32).sin())
                .sum()
        })
        .collect();
    cycle.iter().copied().cycle().take(n * times).collect()
}

/// A sine sweeping from `from` to `to` Hz (exponentially, over `sweep`
/// seconds), decaying over `decay` seconds, `len` seconds long.
fn sweep(from: f32, to: f32, sweep: f32, decay: f32, len: f32) -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..secs(len))
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let f = to + (from - to) * (-t / sweep).exp();
            phase += f / RATE as f32 * std::f32::consts::TAU;
            phase.sin() * (-t / decay).exp()
        })
        .collect()
}

/// Noise, high-passed by `bright` (0 none, near 1 only the hiss), decaying
/// over `decay` seconds, `len` seconds long.
fn hiss(bright: f32, decay: f32, len: f32, seed: u32) -> Vec<f32> {
    let mut noise = Noise(seed);
    let mut low = 0.0f32;
    (0..secs(len))
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let x = noise.next();
            low += (x - low) * (1.0 - bright);
            (x - low) * (-t / decay).exp()
        })
        .collect()
}

/// Square waves at these frequencies together (a cymbal's or cowbell's
/// metallic clang), decaying over `decay` seconds.
fn clang(freqs: &[f32], decay: f32, len: f32) -> Vec<f32> {
    (0..secs(len))
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let s: f32 = freqs
                .iter()
                .map(|f| if (t * f).fract() < 0.5 { 1.0 } else { -1.0 })
                .sum();
            s / freqs.len() as f32 * (-t / decay).exp()
        })
        .collect()
}

fn mix(a: &[f32], b: &[f32], gb: f32) -> Vec<f32> {
    (0..a.len().max(b.len()))
        .map(|i| a.get(i).copied().unwrap_or(0.0) + gb * b.get(i).copied().unwrap_or(0.0))
        .collect()
}

/// A short fade in (to avoid a click) and out at the end.
fn fade(mut s: Vec<f32>, fade_in: usize, fade_out: usize) -> Vec<f32> {
    let n = s.len();
    for (i, v) in s.iter_mut().take(fade_in).enumerate() {
        *v *= i as f32 / fade_in as f32;
    }
    for (i, v) in s.iter_mut().rev().take(fade_out.min(n)).enumerate() {
        *v *= i as f32 / fade_out as f32;
    }
    s
}

// ------------------------------------------------------------ the font

/// SF2 generator numbers.
mod gen {
    pub const FILTER_FC: u16 = 8;
    pub const PAN: u16 = 17;
    pub const ATTACK: u16 = 34;
    pub const DECAY: u16 = 36;
    pub const SUSTAIN: u16 = 37;
    pub const RELEASE: u16 = 38;
    pub const KEY_TO_DECAY: u16 = 40;
    pub const INSTRUMENT: u16 = 41;
    pub const KEY_RANGE: u16 = 43;
    pub const ATTENUATION: u16 = 48;
    pub const SAMPLE: u16 = 53;
    pub const SAMPLE_MODES: u16 = 54;
    pub const SCALE_TUNING: u16 = 56;
    pub const EXCLUSIVE: u16 = 57;
    pub const ROOT_KEY: u16 = 58;
}

/// Seconds as SF2 timecents.
fn tc(s: f32) -> i16 {
    (1200.0 * s.log2()).round() as i16
}

/// A frequency as SF2 absolute cents (for the filter).
fn cents(hz: f32) -> i16 {
    (1200.0 * (hz / 8.176).log2()).round() as i16
}

struct Sample {
    name: &'static str,
    data: Vec<f32>,
    /// The loop, from here to the end (looped samples only).
    loop_from: Option<usize>,
    /// The pitch it sounds at, as a MIDI key, and the cents to correct it by.
    key: u8,
    cents: i8,
}

/// A zone: the keys it covers and its generators (without the key range
/// and the sample, put in on writing).
struct Zone {
    keys: (u8, u8),
    sample: usize,
    gens: Vec<(u16, i16)>,
}

/// Drum keys, the sample they play and their own generators.
type KitKeys = (std::ops::RangeInclusive<u8>, usize, Vec<(u16, i16)>);

struct Instrument {
    name: &'static str,
    zones: Vec<Zone>,
}

struct Preset {
    name: String,
    bank: u16,
    program: u16,
    instrument: usize,
}

struct Font {
    samples: Vec<Sample>,
    instruments: Vec<Instrument>,
    presets: Vec<Preset>,
}

/// The MIDI key and cents of a period of `p` samples.
fn pitch(p: f32) -> (u8, i8) {
    let midi = 69.0 + 12.0 * (RATE as f32 / p / 440.0).log2();
    let key = midi.round();
    (key as u8, ((midi - key) * 100.0).round() as i8)
}

impl Font {
    fn build() -> Font {
        let mut samples = Vec::new();
        let mut add = |s: Sample| {
            samples.push(s);
            samples.len() - 1
        };
        let looped = |name, data: Vec<f32>, period: f32, loop_len: usize| {
            let (key, cents) = pitch(period);
            let n = data.len();
            // (The correction the synthesiser applies, against how sharp it is.)
            Sample {
                name,
                data,
                loop_from: Some(n - loop_len),
                key,
                cents: -cents,
            }
        };
        let shot = |name, data: Vec<f32>, key| Sample {
            name,
            data: fade(data, 8, 64),
            loop_from: None,
            key,
            cents: 0,
        };

        // Plucked strings: their last two periods (2n + 1 samples, a whole
        // number) looped, as the string rings on; the volume envelope
        // decays it.
        let guitar = add(looped(
            "Guitar",
            pluck(200, secs(1.0) + 401, 0.15, 7),
            200.5,
            401,
        ));
        let bass = add(looped(
            "Bass",
            pluck(400, secs(1.2) + 801, 0.55, 11),
            400.5,
            801,
        ));
        let keys = add(looped(
            "Keys",
            pluck(200, secs(0.6) + 401, 0.0, 23),
            200.5,
            401,
        ));
        // Sustained tones: a period repeated, all of it looped.
        let soft = add(looped(
            "Soft",
            wave(200, &[1.0, 0.45, 0.3, 0.18, 0.12, 0.08, 0.05, 0.03], 8),
            200.0,
            1600,
        ));
        let drive = {
            // A bright saw, clipped.
            let h: Vec<f32> = (1..40).map(|k| 1.0 / k as f32).collect();
            let w: Vec<f32> = wave(200, &h, 8).iter().map(|x| (x * 2.5).tanh()).collect();
            add(looped("Drive", w, 200.0, 1600))
        };

        // The drums, at their own pitch (one-shot).
        let kick = add(shot("Kick", sweep(130.0, 48.0, 0.04, 0.22, 0.5), 36));
        let snare = add(shot(
            "Snare",
            mix(
                &hiss(0.3, 0.12, 0.35, 3),
                &sweep(240.0, 180.0, 0.02, 0.06, 0.2),
                0.8,
            ),
            38,
        ));
        let stick = add(shot(
            "Stick",
            mix(
                &sweep(1800.0, 1500.0, 0.01, 0.012, 0.06),
                &hiss(0.6, 0.01, 0.06, 5),
                0.5,
            ),
            37,
        ));
        let clap = add(shot(
            "Clap",
            {
                let mut c = hiss(0.5, 0.09, 0.3, 9);
                // Three quick hits before the clap itself.
                for (i, v) in c.iter_mut().enumerate().take(secs(0.03)) {
                    if (i / secs(0.01)) % 2 == 1 {
                        *v *= 0.2;
                    }
                }
                c
            },
            39,
        ));
        let hat = add(shot("Hi-hat", hiss(0.93, 0.03, 0.15, 13), 42));
        let open_hat = add(shot(
            "Open hi-hat",
            mix(
                &hiss(0.93, 0.3, 0.9, 17),
                &clang(&[3150.0, 4870.0, 6340.0], 0.25, 0.9),
                0.25,
            ),
            46,
        ));
        let tom = add(shot(
            "Tom",
            mix(
                &sweep(170.0, 120.0, 0.08, 0.28, 0.7),
                &hiss(0.2, 0.03, 0.1, 19),
                0.3,
            ),
            45,
        ));
        let crash = add(shot(
            "Crash",
            mix(
                &hiss(0.85, 0.7, 2.2, 29),
                &clang(&[540.0, 812.0, 1347.0, 2213.0, 3511.0], 0.6, 2.2),
                0.35,
            ),
            49,
        ));
        let ride = add(shot(
            "Ride",
            mix(
                &clang(&[612.0, 1033.0, 2447.0, 3769.0], 0.9, 2.0),
                &hiss(0.9, 0.08, 0.3, 31),
                0.5,
            ),
            51,
        ));
        let cowbell = add(shot("Cowbell", clang(&[540.0, 800.0], 0.12, 0.4), 56));
        let block_hi = add(shot(
            "Woodblock high",
            sweep(1250.0, 1150.0, 0.005, 0.025, 0.12),
            76,
        ));
        let block_lo = add(shot(
            "Woodblock low",
            sweep(900.0, 830.0, 0.005, 0.03, 0.14),
            77,
        ));
        let shaker = add(shot("Shaker", hiss(0.8, 0.04, 0.15, 37), 70));

        use gen::*;
        let one = |sample, gens: Vec<(u16, i16)>| {
            vec![Zone {
                keys: (0, 127),
                sample,
                gens,
            }]
        };
        let silent = 1440; // sustain: down to nothing
        let instruments = vec![
            Instrument {
                name: "Guitar",
                zones: one(
                    guitar,
                    vec![
                        (DECAY, tc(7.0)),
                        (KEY_TO_DECAY, 25),
                        (SUSTAIN, silent),
                        (RELEASE, tc(0.2)),
                        (SAMPLE_MODES, 1),
                    ],
                ),
            },
            Instrument {
                name: "Muted guitar",
                zones: one(
                    guitar,
                    vec![
                        (FILTER_FC, cents(3500.0)),
                        (DECAY, tc(3.0)),
                        (SUSTAIN, silent),
                        (RELEASE, tc(0.06)),
                        (SAMPLE_MODES, 1),
                    ],
                ),
            },
            Instrument {
                name: "Bass",
                zones: one(
                    bass,
                    vec![
                        (DECAY, tc(8.0)),
                        (KEY_TO_DECAY, 20),
                        (SUSTAIN, silent),
                        (RELEASE, tc(0.12)),
                        (SAMPLE_MODES, 1),
                    ],
                ),
            },
            Instrument {
                name: "Keys",
                zones: one(
                    keys,
                    vec![
                        (DECAY, tc(6.0)),
                        (KEY_TO_DECAY, 30),
                        (SUSTAIN, silent),
                        (RELEASE, tc(0.35)),
                        (SAMPLE_MODES, 1),
                        (ATTENUATION, 20),
                    ],
                ),
            },
            Instrument {
                name: "Organ",
                zones: one(
                    soft,
                    vec![
                        (ATTACK, tc(0.01)),
                        (RELEASE, tc(0.12)),
                        (SAMPLE_MODES, 1),
                        (ATTENUATION, 270),
                    ],
                ),
            },
            Instrument {
                name: "Strings",
                zones: one(
                    soft,
                    vec![
                        (ATTACK, tc(0.18)),
                        (RELEASE, tc(0.5)),
                        (FILTER_FC, cents(3500.0)),
                        (SAMPLE_MODES, 1),
                        (ATTENUATION, 260),
                    ],
                ),
            },
            Instrument {
                name: "Overdrive",
                zones: one(
                    drive,
                    vec![
                        (ATTACK, tc(0.004)),
                        (DECAY, tc(6.0)),
                        (SUSTAIN, 80),
                        (RELEASE, tc(0.12)),
                        (FILTER_FC, cents(4200.0)),
                        (SAMPLE_MODES, 1),
                        (ATTENUATION, 260),
                    ],
                ),
            },
            Instrument {
                name: "Drums",
                zones: Self::kit(&[
                    (35..=36, kick, vec![(ATTENUATION, 210)]),
                    (37..=37, stick, vec![]),
                    (38..=38, snare, vec![]),
                    (39..=39, clap, vec![]),
                    (40..=40, snare, vec![(ROOT_KEY, 39)]),
                    (
                        42..=42,
                        hat,
                        vec![(EXCLUSIVE, 1), (PAN, 150), (ATTENUATION, 400)],
                    ),
                    (
                        44..=44,
                        hat,
                        vec![
                            (EXCLUSIVE, 1),
                            (ROOT_KEY, 43),
                            (PAN, 150),
                            (ATTENUATION, 400),
                        ],
                    ),
                    (
                        46..=46,
                        open_hat,
                        vec![(EXCLUSIVE, 1), (PAN, 150), (ATTENUATION, 500)],
                    ),
                    (41..=41, tom, vec![(PAN, -200), (ATTENUATION, 170)]),
                    (43..=43, tom, vec![(PAN, -120), (ATTENUATION, 170)]),
                    (45..=45, tom, vec![(PAN, -40), (ATTENUATION, 170)]),
                    (47..=47, tom, vec![(PAN, 40), (ATTENUATION, 170)]),
                    (48..=48, tom, vec![(PAN, 120), (ATTENUATION, 170)]),
                    (50..=50, tom, vec![(PAN, 200), (ATTENUATION, 170)]),
                    (49..=49, crash, vec![(PAN, -150), (ATTENUATION, 270)]),
                    (
                        57..=57,
                        crash,
                        vec![(ROOT_KEY, 47), (PAN, 150), (ATTENUATION, 270)],
                    ),
                    (52..=52, crash, vec![(ROOT_KEY, 46), (ATTENUATION, 270)]),
                    (55..=55, crash, vec![(ROOT_KEY, 44), (ATTENUATION, 350)]),
                    (51..=51, ride, vec![(PAN, 200), (ATTENUATION, 370)]),
                    (
                        59..=59,
                        ride,
                        vec![(ROOT_KEY, 50), (PAN, 200), (ATTENUATION, 370)],
                    ),
                    (
                        53..=53,
                        ride,
                        vec![(ROOT_KEY, 47), (PAN, 200), (ATTENUATION, 330)],
                    ),
                    (56..=56, cowbell, vec![]),
                    (76..=76, block_hi, vec![(ATTENUATION, 120)]),
                    (77..=77, block_lo, vec![(ATTENUATION, 120)]),
                    (75..=75, block_hi, vec![(ROOT_KEY, 70)]),
                    (60..=74, shaker, vec![(SCALE_TUNING, 30)]),
                    (78..=87, shaker, vec![(SCALE_TUNING, 30)]),
                    (27..=34, stick, vec![(SCALE_TUNING, 50)]),
                    (54..=54, shaker, vec![]),
                    (58..=58, cowbell, vec![(ROOT_KEY, 50)]),
                ]),
            },
        ];

        // Each program on the sound nearest it.
        let family = |p: u16| match p {
            0..=15 => 3,             // pianos, chromatic percussion
            16..=23 => 4,            // organs
            28 => 1,                 // muted guitar
            29 | 30 => 6,            // overdriven, distortion
            24..=31 => 0,            // guitars
            32..=39 => 2,            // basses
            40..=55 | 88..=103 => 5, // strings, ensembles, pads, effects
            56..=79 => 4,            // brass, reeds, pipes
            80..=87 => 6,            // synth leads
            104..=111 => 0,          // plucked ethnic instruments
            112..=119 => 3,          // percussive
            _ => 5,
        };
        let mut presets: Vec<Preset> = (0..128)
            .map(|p| Preset {
                name: format!("{} {p}", instruments[family(p)].name),
                bank: 0,
                program: p,
                instrument: family(p),
            })
            .collect();
        presets.push(Preset {
            name: "Drums".into(),
            bank: 128,
            program: 0,
            instrument: 7,
        });
        Font {
            samples,
            instruments,
            presets,
        }
    }

    /// The drum kit's zones: each at the sample's own pitch (key tracking
    /// off unless asked for), rooted on its key.
    fn kit(map: &[KitKeys]) -> Vec<Zone> {
        map.iter()
            .map(|(keys, sample, extra)| {
                let mut gens = vec![(gen::SAMPLE_MODES, 0)];
                if !extra.iter().any(|g| g.0 == gen::SCALE_TUNING) {
                    gens.push((gen::SCALE_TUNING, 0));
                }
                gens.extend(extra.iter().copied());
                Zone {
                    keys: (*keys.start(), *keys.end()),
                    sample: *sample,
                    gens,
                }
            })
            .collect()
    }

    fn write(&self) -> Vec<u8> {
        // The samples, each with 46 zero points after it (as SF2 asks).
        let mut smpl: Vec<u8> = Vec::new();
        let mut shdr: Vec<u8> = Vec::new();
        let mut at = 0u32;
        for s in &self.samples {
            let peak = s.data.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
            let gain = 0.9 / peak * 32767.0;
            for v in &s.data {
                smpl.extend(((v * gain).round() as i16).to_le_bytes());
            }
            smpl.extend([0u8; 46 * 2]);
            let start = at;
            let end = at + s.data.len() as u32;
            let (ls, le) = match s.loop_from {
                Some(f) => (start + f as u32, end),
                None => (start, end),
            };
            name20(&mut shdr, s.name);
            for v in [start, end, ls, le, RATE] {
                shdr.extend(v.to_le_bytes());
            }
            shdr.push(s.key);
            shdr.push(s.cents as u8);
            shdr.extend(0u16.to_le_bytes()); // link
            shdr.extend(1u16.to_le_bytes()); // mono
            at = end + 46;
        }
        name20(&mut shdr, "EOS");
        shdr.extend([0u8; 26]);

        let gen = |out: &mut Vec<u8>, op: u16, amount: i16| {
            out.extend(op.to_le_bytes());
            out.extend(amount.to_le_bytes());
        };
        let bag = |out: &mut Vec<u8>, g: usize| {
            out.extend((g as u16).to_le_bytes());
            out.extend(0u16.to_le_bytes());
        };

        let (mut inst, mut ibag, mut igen) = (Vec::new(), Vec::new(), Vec::new());
        let (mut nbag, mut ngen) = (0usize, 0usize);
        for i in &self.instruments {
            name20(&mut inst, i.name);
            inst.extend((nbag as u16).to_le_bytes());
            for z in &i.zones {
                bag(&mut ibag, ngen);
                nbag += 1;
                // The key range first, the sample last.
                igen.extend(gen::KEY_RANGE.to_le_bytes());
                igen.extend([z.keys.0, z.keys.1]);
                for &(op, amount) in &z.gens {
                    gen(&mut igen, op, amount);
                }
                gen(&mut igen, gen::SAMPLE, z.sample as i16);
                ngen += z.gens.len() + 2;
            }
        }
        name20(&mut inst, "EOI");
        inst.extend((nbag as u16).to_le_bytes());
        bag(&mut ibag, ngen);
        igen.extend([0u8; 4]);

        let (mut phdr, mut pbag, mut pgen) = (Vec::new(), Vec::new(), Vec::new());
        for (n, p) in self.presets.iter().enumerate() {
            name20(&mut phdr, &p.name);
            phdr.extend(p.program.to_le_bytes());
            phdr.extend(p.bank.to_le_bytes());
            phdr.extend((n as u16).to_le_bytes());
            phdr.extend([0u8; 12]);
            bag(&mut pbag, n);
            gen(&mut pgen, gen::INSTRUMENT, p.instrument as i16);
        }
        name20(&mut phdr, "EOP");
        phdr.extend([0u8; 4]);
        phdr.extend((self.presets.len() as u16).to_le_bytes());
        phdr.extend([0u8; 12]);
        bag(&mut pbag, self.presets.len());
        pgen.extend([0u8; 4]);

        let mut ifil = Vec::new();
        ifil.extend(2u16.to_le_bytes());
        ifil.extend(1u16.to_le_bytes());
        let info = list(
            b"INFO",
            &[
                chunk(b"ifil", &ifil),
                chunk(b"isng", &zstr("EMU8000")),
                chunk(b"INAM", &zstr("Built-in")),
            ],
        );
        let sdta = list(b"sdta", &[chunk(b"smpl", &smpl)]);
        let pdta = list(
            b"pdta",
            &[
                chunk(b"phdr", &phdr),
                chunk(b"pbag", &pbag),
                chunk(b"pmod", &[0u8; 10]),
                chunk(b"pgen", &pgen),
                chunk(b"inst", &inst),
                chunk(b"ibag", &ibag),
                chunk(b"imod", &[0u8; 10]),
                chunk(b"igen", &igen),
                chunk(b"shdr", &shdr),
            ],
        );
        let mut body = b"sfbk".to_vec();
        body.extend(info);
        body.extend(sdta);
        body.extend(pdta);
        chunk(b"RIFF", &body)
    }
}

/// A name in a 20-byte field.
fn name20(out: &mut Vec<u8>, s: &str) {
    let mut b = [0u8; 20];
    for (d, c) in b.iter_mut().zip(s.bytes().take(19)) {
        *d = c;
    }
    out.extend(b);
}

/// A zero-terminated string, padded to an even length.
fn zstr(s: &str) -> Vec<u8> {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    if b.len() % 2 == 1 {
        b.push(0);
    }
    b
}

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn list(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut body = kind.to_vec();
    for c in chunks {
        body.extend(c);
    }
    chunk(b"LIST", &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
    use std::sync::Arc;

    fn level(font: &Arc<SoundFont>, channel: i32, program: i32, key: i32, ms: usize) -> f32 {
        let st = SynthesizerSettings::new(RATE as i32);
        let mut s = Synthesizer::new(font, &st).unwrap();
        s.process_midi_message(channel, 0xc0, program, 0);
        s.note_on(channel, key, 100);
        let n = RATE as usize * ms / 1000;
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.render(&mut l, &mut r);
        (l.iter().chain(&r).map(|v| v * v).sum::<f32>() / (2 * n) as f32).sqrt()
    }

    #[test]
    fn loads_and_sounds() {
        let bytes = sf2();
        let font = Arc::new(SoundFont::new(&mut &bytes[..]).expect("a usable SoundFont"));
        assert_eq!(font.get_presets().len(), 129);
        // Every family, and the drums (the metronome's woodblocks too), are
        // heard, at levels not far apart.
        let mut levels = Vec::new();
        for program in [0, 16, 25, 28, 30, 33, 48, 81, 105] {
            levels.push((
                format!("program {program}"),
                level(&font, 0, program, 52, 400),
            ));
        }
        for key in [36, 38, 42, 46, 49, 51, 76, 77] {
            levels.push((format!("drum {key}"), level(&font, 9, 0, key, 150)));
        }
        for (what, l) in &levels {
            eprintln!("{what}: {l:.4}");
            assert!(*l > 0.002, "{what} is silent ({l})");
            assert!(*l < 0.6, "{what} is too loud ({l})");
        }
    }

    /// The levels against a real GM SoundFont's, to balance them by
    /// (`cargo test compare_levels -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn compare_levels() {
        let bytes = sf2();
        let ours = Arc::new(SoundFont::new(&mut &bytes[..]).unwrap());
        let mut f = std::fs::File::open("/usr/share/sounds/sf2/TimGM6mb.sf2").unwrap();
        let theirs = Arc::new(SoundFont::new(&mut f).unwrap());
        for (ch, program, key) in [
            (0, 0, 60),
            (0, 16, 60),
            (0, 25, 52),
            (0, 27, 64),
            (0, 28, 52),
            (0, 30, 52),
            (0, 33, 40),
            (0, 48, 60),
            (0, 56, 60),
            (0, 81, 60),
            (0, 105, 60),
            (9, 0, 36),
            (9, 0, 38),
            (9, 0, 42),
            (9, 0, 46),
            (9, 0, 49),
            (9, 0, 51),
            (9, 0, 45),
            (9, 0, 76),
            (9, 0, 77),
        ] {
            let ms = if ch == 9 { 150 } else { 600 };
            let (a, b) = (
                level(&ours, ch, program, key, ms),
                level(&theirs, ch, program, key, ms),
            );
            eprintln!(
                "{ch} {program} {key}: ours {a:.4} theirs {b:.4} ({:+.1} dB)",
                20.0 * (a / b).log10()
            );
        }
    }

    #[test]
    fn is_in_tune() {
        // The guitar's A (key 57) at its pitch, 220 Hz, once the string's
        // ring has settled.
        let bytes = sf2();
        let font = Arc::new(SoundFont::new(&mut &bytes[..]).unwrap());
        let st = SynthesizerSettings::new(RATE as i32);
        let mut s = Synthesizer::new(&font, &st).unwrap();
        s.process_midi_message(0, 0xc0, 25, 0);
        s.note_on(0, 57, 100);
        let n = RATE as usize;
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        s.render(&mut l, &mut r);
        // The period: the lag the ring best matches itself at.
        let w = &l[n / 2..];
        let lag = (100..400)
            .max_by(|&a, &b| {
                let c = |d: usize| (0..4000).map(|i| w[i] * w[i + d]).sum::<f32>();
                c(a).total_cmp(&c(b))
            })
            .unwrap();
        let hz = RATE as f32 / lag as f32;
        assert!((hz - 220.0).abs() < 2.0, "A3 heard at about {hz} Hz");
    }
}
