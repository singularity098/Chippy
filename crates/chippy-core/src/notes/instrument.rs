//! Which General MIDI instrument a sampled sound is closest to.
//!
//! Sample-based music (SNES, tracker modules) holds recordings, not
//! instrument names, so the instrument is guessed from what can be measured:
//!
//! * the name, when the musician gave one ("Slap Bass", "strings2"), which
//!   is the surest guide;
//! * the envelope: how fast the sound starts, and whether it holds (a looped
//!   sample, or the SNES's ADSR settings) or dies away like a plucked string;
//! * the tone: how its energy spreads over the harmonics (a flute is nearly
//!   a sine, a clarinet has mostly odd harmonics, brass is bright);
//! * the range it is played in (low parts are basses).
//!
//! Unpitched samples become drums, told apart by their brightness and length.

/// What [`classify`] decided a sample is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A General MIDI program.
    Melodic(u8),
    /// A General MIDI percussion key.
    Drum(u8),
}

/// How a sound's loudness moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Envelope {
    /// From the start to (nearly) full loudness.
    pub attack_ms: f64,
    /// It holds while the note is held (rather than dying away).
    pub sustained: bool,
    /// When not sustained: how long it takes to fade to a tenth.
    pub decay_ms: f64,
}

/// What was measured of one sample.
#[derive(Clone, Debug, Default)]
pub struct Features {
    pub envelope: Option<Envelope>,
    /// The relative strength of harmonics 1.. (amplitudes, the largest 1).
    pub harmonics: Vec<f64>,
    /// The loop is a short single-cycle waveform (a synth, not a recording).
    pub synthetic: bool,
    /// Unpitched only: a rough brightness in Hz (from zero crossings).
    pub noise_hz: f64,
    /// Unpitched only: how long it sounds, ms.
    pub length_ms: f64,
}

/// A sound's fundamental, in Hz at `rate`, with how sure the measure is
/// (0..1), or `None` when it has no clear pitch.
pub struct Pitch {
    pub hz: f64,
    pub clarity: f64,
}

/// What sounds while a note is held: the loop, repeated (so it starts with
/// the loop); or, for a one-shot sample, what follows its attack.
pub fn held(pcm: &[f32], loop_start: Option<usize>, want: usize) -> Vec<f32> {
    match loop_start {
        Some(ls) if ls < pcm.len() && pcm.len() - ls >= 8 => {
            let lp = &pcm[ls..];
            let mut v = Vec::with_capacity(want + lp.len());
            while v.len() < want {
                v.extend_from_slice(lp);
            }
            v
        }
        _ => {
            let skip = pcm.len() / 10;
            pcm[skip..].to_vec()
        }
    }
}

/// Measures a sample's fundamental frequency (with YIN). Unpitched sounds
/// (drums, noise) give `None`.
pub fn pitch(pcm: &[f32], loop_start: Option<usize>, rate: f64) -> Option<Pitch> {
    let sig = held(pcm, loop_start, 8192);
    let max_lag = (rate / 25.0) as usize;
    let min_lag = ((rate / 4000.0) as usize).max(2);
    let max_lag = max_lag.min(sig.len() / 2);
    if max_lag <= min_lag + 2 {
        return None;
    }
    let window = (sig.len() - max_lag).min(4096);
    let energy: f32 = sig[..window].iter().map(|x| x * x).sum::<f32>() / window as f32;
    if energy < 1e-7 {
        return None;
    }
    // Cumulative mean normalised difference.
    let mut d = vec![1.0f64; max_lag + 1];
    let mut running = 0.0f64;
    for (lag, dv) in d.iter_mut().enumerate().skip(1) {
        let sum: f64 = (0..window)
            .map(|i| {
                let x = (sig[i] - sig[i + lag]) as f64;
                x * x
            })
            .sum();
        running += sum;
        *dv = if running > 0.0 {
            sum * lag as f64 / running
        } else {
            1.0
        };
    }
    // The first dip under the threshold, else the deepest.
    let mut best = None;
    let mut lag = min_lag;
    while lag < max_lag {
        if d[lag] < 0.2 {
            while lag + 1 < max_lag && d[lag + 1] < d[lag] {
                lag += 1;
            }
            best = Some(lag);
            break;
        }
        lag += 1;
    }
    let lag = best.unwrap_or_else(|| {
        (min_lag..max_lag)
            .min_by(|&a, &b| d[a].total_cmp(&d[b]))
            .unwrap()
    });
    let clarity = 1.0 - d[lag];
    if clarity < 0.6 {
        return None;
    }
    // Between samples: fit a parabola through the dip.
    let (a, b, c) = (d[lag - 1], d[lag], d[lag + 1]);
    let den = a - 2.0 * b + c;
    let frac = if den.abs() > 1e-12 {
        (0.5 * (a - c) / den).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    Some(Pitch {
        hz: rate / (lag as f64 + frac),
        clarity,
    })
}

/// The loudness (RMS) of each 5 ms of a one-shot sample.
fn rms_frames(pcm: &[f32], rate: f64) -> (Vec<f64>, f64) {
    let step = ((rate * 0.005) as usize).max(1);
    let frames: Vec<f64> = pcm
        .chunks(step)
        .map(|c| (c.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / c.len() as f64).sqrt())
        .collect();
    (frames, step as f64 * 1000.0 / rate)
}

/// The envelope as the sample itself plays it: a looped sample holds; a
/// one-shot one is measured.
pub fn sample_envelope(pcm: &[f32], loop_start: Option<usize>, rate: f64) -> Envelope {
    let (frames, ms) = rms_frames(pcm, rate);
    let peak = frames.iter().cloned().fold(0.0, f64::max);
    if peak <= 0.0 {
        return Envelope {
            attack_ms: 0.0,
            sustained: false,
            decay_ms: 0.0,
        };
    }
    let at = frames.iter().position(|&f| f >= 0.9 * peak).unwrap_or(0);
    let attack_ms = at as f64 * ms;
    let total_ms = frames.len() as f64 * ms;
    if loop_start.is_some() {
        return Envelope {
            attack_ms,
            sustained: true,
            decay_ms: f64::INFINITY,
        };
    }
    let after = frames[at..]
        .iter()
        .position(|&f| f < 0.1 * peak)
        .map(|i| i as f64 * ms)
        .unwrap_or(total_ms - attack_ms);
    Envelope {
        attack_ms,
        // A long, steady one-shot (an organ chord recorded whole) holds.
        sustained: after > 2500.0,
        decay_ms: after,
    }
}

/// The strengths of a pitched sound's harmonics (up to 12, the largest 1),
/// by correlating its held part with each harmonic's sine and cosine.
pub fn harmonics(pcm: &[f32], loop_start: Option<usize>, rate: f64, hz: f64) -> Vec<f64> {
    let sig = held(pcm, loop_start, 4096);
    // A whole number of periods, so each harmonic is measured cleanly.
    let period = rate / hz;
    let periods = ((sig.len().min(4096) as f64 / period).floor()).max(1.0);
    let n = ((periods * period).round() as usize).min(sig.len());
    if n < 8 {
        return Vec::new();
    }
    let mut amps: Vec<f64> = (1..=12)
        .take_while(|&k| (k as f64) * hz < rate / 2.0)
        .map(|k| {
            let w = std::f64::consts::TAU * k as f64 * hz / rate;
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &x) in sig[..n].iter().enumerate() {
                let p = w * i as f64;
                re += x as f64 * p.cos();
                im += x as f64 * p.sin();
            }
            (re * re + im * im).sqrt()
        })
        .collect();
    let max = amps.iter().cloned().fold(0.0, f64::max);
    if max > 0.0 {
        amps.iter_mut().for_each(|a| *a /= max);
    }
    amps
}

/// Everything measured of a sample, given its pitch (`None`: unpitched).
pub fn measure(pcm: &[f32], loop_start: Option<usize>, rate: f64, hz: Option<f64>) -> Features {
    let envelope = Some(sample_envelope(pcm, loop_start, rate));
    let loop_len = loop_start.map(|ls| pcm.len().saturating_sub(ls));
    match hz {
        Some(hz) => Features {
            envelope,
            harmonics: harmonics(pcm, loop_start, rate, hz),
            // One or two cycles of a waveform, looped: a chip-style synth.
            synthetic: loop_len.is_some_and(|l| l > 0 && (l as f64) <= 2.5 * rate / hz)
                && pcm.len() <= loop_len.unwrap_or(0) * 4,
            ..Default::default()
        },
        None => {
            let sig = &pcm[..pcm.len().min((rate * 0.25) as usize)];
            let crossings = sig
                .windows(2)
                .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
                .count();
            let (frames, ms) = rms_frames(pcm, rate);
            let peak = frames.iter().cloned().fold(0.0, f64::max);
            let last = frames.iter().rposition(|&f| f > 0.03 * peak).unwrap_or(0);
            Features {
                envelope,
                noise_hz: crossings as f64 * rate / 2.0 / sig.len().max(1) as f64,
                length_ms: if loop_start.is_some() {
                    f64::INFINITY
                } else {
                    (last + 1) as f64 * ms
                },
                ..Default::default()
            }
        }
    }
}

/// Words in instrument names, and what they mean. Checked in order: the
/// first match wins, so the more specific words come first.
const NAMES: &[(&str, Kind)] = &[
    // Drums.
    ("kick", Kind::Drum(36)),
    ("bassdrum", Kind::Drum(36)),
    ("bass drum", Kind::Drum(36)),
    ("bdrum", Kind::Drum(36)),
    ("bd", Kind::Drum(36)),
    ("sd", Kind::Drum(38)),
    ("hh", Kind::Drum(42)),
    ("snare", Kind::Drum(38)),
    ("snr", Kind::Drum(38)),
    ("rimshot", Kind::Drum(37)),
    ("rim", Kind::Drum(37)),
    ("clap", Kind::Drum(39)),
    ("open hat", Kind::Drum(46)),
    ("openhat", Kind::Drum(46)),
    ("ohat", Kind::Drum(46)),
    ("hihat", Kind::Drum(42)),
    ("hi-hat", Kind::Drum(42)),
    ("hi hat", Kind::Drum(42)),
    ("hat", Kind::Drum(42)),
    ("crash", Kind::Drum(49)),
    ("splash", Kind::Drum(55)),
    ("china", Kind::Drum(52)),
    ("ride", Kind::Drum(51)),
    ("cymbal", Kind::Drum(49)),
    ("cymb", Kind::Drum(49)),
    ("tom", Kind::Drum(45)),
    ("cowbell", Kind::Drum(56)),
    ("tambourine", Kind::Drum(54)),
    ("tamb", Kind::Drum(54)),
    ("conga", Kind::Drum(63)),
    ("bongo", Kind::Drum(60)),
    ("timbale", Kind::Drum(65)),
    ("shaker", Kind::Drum(70)),
    ("maraca", Kind::Drum(70)),
    ("cabasa", Kind::Drum(69)),
    ("woodblock", Kind::Drum(76)),
    ("triangle", Kind::Drum(81)),
    ("guiro", Kind::Drum(73)),
    ("whistle", Kind::Drum(71)),
    ("drum", Kind::Drum(38)),
    ("perc", Kind::Drum(38)),
    // Basses.
    ("slap", Kind::Melodic(36)),
    ("fretless", Kind::Melodic(35)),
    ("synbass", Kind::Melodic(38)),
    ("synth bass", Kind::Melodic(38)),
    ("synthbass", Kind::Melodic(38)),
    ("acoustic bass", Kind::Melodic(32)),
    ("upright", Kind::Melodic(32)),
    ("contrabass", Kind::Melodic(43)),
    ("double bass", Kind::Melodic(32)),
    ("bass", Kind::Melodic(33)),
    // Guitars.
    ("distortion", Kind::Melodic(30)),
    ("distort", Kind::Melodic(30)),
    ("dist", Kind::Melodic(30)),
    ("overdrive", Kind::Melodic(29)),
    ("od gtr", Kind::Melodic(29)),
    ("power", Kind::Melodic(30)),
    ("mute", Kind::Melodic(28)),
    ("harmonics", Kind::Melodic(31)),
    ("nylon", Kind::Melodic(24)),
    ("classical", Kind::Melodic(24)),
    ("steel", Kind::Melodic(25)),
    ("acoustic", Kind::Melodic(25)),
    ("jazz", Kind::Melodic(26)),
    ("clean", Kind::Melodic(27)),
    ("elec gt", Kind::Melodic(27)),
    ("guitar", Kind::Melodic(25)),
    ("gtr", Kind::Melodic(25)),
    ("guit", Kind::Melodic(25)),
    ("banjo", Kind::Melodic(105)),
    ("sitar", Kind::Melodic(104)),
    ("koto", Kind::Melodic(107)),
    ("shamisen", Kind::Melodic(106)),
    ("harp", Kind::Melodic(46)),
    // Keys.
    ("e.piano", Kind::Melodic(4)),
    ("epiano", Kind::Melodic(4)),
    ("rhodes", Kind::Melodic(4)),
    ("e piano", Kind::Melodic(4)),
    ("harpsi", Kind::Melodic(6)),
    ("clav", Kind::Melodic(7)),
    ("piano", Kind::Melodic(0)),
    ("pno", Kind::Melodic(0)),
    ("celesta", Kind::Melodic(8)),
    ("glock", Kind::Melodic(9)),
    ("music box", Kind::Melodic(10)),
    ("musicbox", Kind::Melodic(10)),
    ("vibra", Kind::Melodic(11)),
    ("marimba", Kind::Melodic(12)),
    ("xylo", Kind::Melodic(13)),
    ("tubular", Kind::Melodic(14)),
    ("chime", Kind::Melodic(14)),
    ("bell", Kind::Melodic(14)),
    ("dulcimer", Kind::Melodic(15)),
    ("church", Kind::Melodic(19)),
    ("pipe", Kind::Melodic(19)),
    ("accordion", Kind::Melodic(21)),
    ("accord", Kind::Melodic(21)),
    ("harmonica", Kind::Melodic(22)),
    ("organ", Kind::Melodic(16)),
    ("orgn", Kind::Melodic(16)),
    // Strings.
    ("pizz", Kind::Melodic(45)),
    ("tremolo", Kind::Melodic(44)),
    ("violin", Kind::Melodic(40)),
    ("fiddle", Kind::Melodic(40)),
    ("viola", Kind::Melodic(41)),
    ("cello", Kind::Melodic(42)),
    ("orch hit", Kind::Melodic(55)),
    ("orchestra hit", Kind::Melodic(55)),
    ("hit", Kind::Melodic(55)),
    ("timpani", Kind::Melodic(47)),
    ("timp", Kind::Melodic(47)),
    ("syn str", Kind::Melodic(50)),
    ("synstr", Kind::Melodic(50)),
    ("string", Kind::Melodic(48)),
    ("strng", Kind::Melodic(48)),
    ("str", Kind::Melodic(48)),
    ("orch", Kind::Melodic(48)),
    // Voices.
    ("choir", Kind::Melodic(52)),
    ("aah", Kind::Melodic(52)),
    ("ahh", Kind::Melodic(52)),
    ("ooh", Kind::Melodic(53)),
    ("vox", Kind::Melodic(54)),
    ("voice", Kind::Melodic(54)),
    ("vocal", Kind::Melodic(54)),
    // Brass.
    ("trumpet", Kind::Melodic(56)),
    ("trump", Kind::Melodic(56)),
    ("tpt", Kind::Melodic(56)),
    ("trombone", Kind::Melodic(57)),
    ("tuba", Kind::Melodic(58)),
    ("horn", Kind::Melodic(60)),
    ("synbrass", Kind::Melodic(62)),
    ("synth brass", Kind::Melodic(62)),
    ("brass", Kind::Melodic(61)),
    ("brs", Kind::Melodic(61)),
    // Reeds and pipes.
    ("soprano sax", Kind::Melodic(64)),
    ("alto", Kind::Melodic(65)),
    ("tenor", Kind::Melodic(66)),
    ("bari", Kind::Melodic(67)),
    ("sax", Kind::Melodic(65)),
    ("oboe", Kind::Melodic(68)),
    ("english horn", Kind::Melodic(69)),
    ("bassoon", Kind::Melodic(70)),
    ("clarinet", Kind::Melodic(71)),
    ("clari", Kind::Melodic(71)),
    ("piccolo", Kind::Melodic(72)),
    ("flute", Kind::Melodic(73)),
    ("flut", Kind::Melodic(73)),
    ("recorder", Kind::Melodic(74)),
    ("pan flute", Kind::Melodic(75)),
    ("panflute", Kind::Melodic(75)),
    ("bottle", Kind::Melodic(76)),
    ("shakuhachi", Kind::Melodic(77)),
    ("ocarina", Kind::Melodic(79)),
    ("bagpipe", Kind::Melodic(109)),
    ("steel drum", Kind::Melodic(114)),
    ("steeldrum", Kind::Melodic(114)),
    ("kalimba", Kind::Melodic(108)),
    // Synths.
    ("square", Kind::Melodic(80)),
    ("sqr", Kind::Melodic(80)),
    ("pulse", Kind::Melodic(80)),
    ("saw", Kind::Melodic(81)),
    ("lead", Kind::Melodic(81)),
    ("pad", Kind::Melodic(89)),
    ("sweep", Kind::Melodic(95)),
    ("syn", Kind::Melodic(81)),
];

/// What an instrument's name says it is.
pub fn by_name(name: &str) -> Option<Kind> {
    let lower = name.to_lowercase();
    // Short words only count as whole words, give or take a number or a
    // plural ("str2", "toms"): "str" in "strings" counts, "hat" in "what"
    // doesn't.
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    NAMES.iter().find_map(|&(word, kind)| {
        let hit = if word.len() <= 4 && !word.contains(' ') {
            words.iter().any(|w| {
                w.strip_prefix(word).is_some_and(|rest| {
                    rest.is_empty() || rest == "s" || rest.chars().all(|c| c.is_ascii_digit())
                })
            })
        } else {
            lower.contains(word)
        };
        hit.then_some(kind)
    })
}

/// The drum an unpitched sound is most like.
pub fn drum_for(f: &Features) -> u8 {
    let hz = f.noise_hz;
    let len = f.length_ms;
    if hz < 450.0 {
        if len > 400.0 {
            45 // a low tom
        } else {
            36 // a kick
        }
    } else if hz < 1500.0 {
        if len > 500.0 {
            48 // a tom
        } else {
            38 // a snare
        }
    } else if hz < 4500.0 {
        if len > 900.0 {
            49
        } else if len < 80.0 {
            39 // a clap
        } else {
            38
        }
    } else if len < 150.0 {
        42 // a closed hi-hat
    } else if len < 700.0 {
        46 // an open hi-hat
    } else {
        49 // a crash
    }
}

/// The melodic instrument a pitched sound is most like, played around
/// `median_key` (a MIDI note, `None` if not known).
pub fn program_for(f: &Features, median_key: Option<u8>) -> u8 {
    let key = median_key.unwrap_or(64) as f64;
    let env = f.envelope.unwrap_or(Envelope {
        attack_ms: 5.0,
        sustained: true,
        decay_ms: f64::INFINITY,
    });
    let h = &f.harmonics;
    let power: f64 = h.iter().map(|a| a * a).sum::<f64>().max(1e-12);
    // How much of the sound is the fundamental, and where its energy sits
    // (in harmonic numbers: 1 is a sine).
    let pure = h.first().map_or(1.0, |a| a * a) / power;
    let brightness: f64 = h
        .iter()
        .enumerate()
        .map(|(i, a)| (i + 1) as f64 * a * a)
        .sum::<f64>()
        / power;
    let even: f64 = h.iter().skip(1).step_by(2).map(|a| a * a).sum::<f64>();
    let odd: f64 = h.iter().skip(2).step_by(2).map(|a| a * a).sum::<f64>();
    let hollow = odd > 0.05 * power && even < 0.25 * odd;

    // Low parts: basses.
    if key < 47.0 {
        return if f.synthetic || (env.sustained && brightness > 2.6) {
            38 // synth bass
        } else if !env.sustained && env.decay_ms < 700.0 && brightness > 2.2 {
            34 // picked bass
        } else if pure > 0.8 && env.sustained {
            32 // acoustic bass
        } else {
            33 // finger bass
        };
    }
    if f.synthetic {
        return if pure > 0.85 || hollow {
            80 // square lead (and the plainest, for near-sines)
        } else if brightness > 3.0 {
            81 // saw lead
        } else {
            87 // bass + lead
        };
    }
    if !env.sustained {
        // Struck and plucked sounds.
        if env.decay_ms < 300.0 {
            return if pure > 0.75 {
                if key >= 76.0 {
                    13 // xylophone
                } else {
                    12 // marimba
                }
            } else if brightness > 3.5 {
                45 // pizzicato strings
            } else {
                24 // nylon guitar
            };
        }
        if env.decay_ms < 1500.0 {
            return if pure > 0.75 {
                if key >= 80.0 {
                    9 // glockenspiel
                } else {
                    11 // vibraphone
                }
            } else if key >= 79.0 && brightness < 2.5 {
                46 // harp
            } else if brightness > 3.0 {
                25 // steel guitar
            } else {
                24 // nylon guitar
            };
        }
        return if pure > 0.75 {
            if key >= 84.0 {
                10 // music box
            } else {
                4 // electric piano
            }
        } else if brightness > 4.5 {
            6 // harpsichord
        } else if key >= 84.0 {
            14 // bells
        } else {
            0 // piano
        };
    }
    // Held sounds.
    if env.attack_ms > 100.0 {
        return if pure > 0.8 {
            89 // warm pad
        } else if brightness < 2.0 {
            52 // choir
        } else if env.attack_ms > 300.0 {
            49 // slow strings
        } else {
            48 // strings
        };
    }
    if hollow {
        return 71; // clarinet
    }
    if pure > 0.8 {
        return if key >= 84.0 {
            72 // piccolo
        } else if key < 60.0 {
            74 // recorder
        } else {
            73 // flute
        };
    }
    if brightness > 4.0 {
        return if key < 55.0 {
            57 // trombone
        } else if key >= 70.0 {
            56 // trumpet
        } else {
            61 // brass section
        };
    }
    if brightness > 2.8 {
        return if key >= 70.0 {
            40 // violin
        } else {
            48 // strings
        };
    }
    if key >= 66.0 {
        68 // oboe
    } else {
        16 // organ
    }
}

/// Decides what a sample is: by its name first, then by measuring it.
/// `pitched` says whether a pitch was found.
pub fn classify(name: &str, f: &Features, pitched: bool, median_key: Option<u8>) -> Kind {
    match by_name(name) {
        // A sample named as a drum but played as a tune is a tom or the like;
        // a drum's name wins only for unpitched sounds or one-key parts.
        Some(Kind::Drum(k)) if !pitched => Kind::Drum(k),
        Some(Kind::Melodic(p)) if pitched => Kind::Melodic(p),
        Some(Kind::Drum(k)) => Kind::Drum(k),
        Some(Kind::Melodic(p)) => Kind::Melodic(p),
        None if pitched => Kind::Melodic(program_for(f, median_key)),
        None => Kind::Drum(drum_for(f)),
    }
}

/// The SNES DSP's envelope rates: samples (at 32 kHz) between steps, by
/// rate number (0 never steps).
const SNES_RATES: [u32; 32] = [
    0, 2048, 1536, 1280, 1024, 768, 640, 512, 384, 320, 256, 192, 160, 128, 96, 80, 64, 48, 40, 32,
    24, 20, 16, 12, 10, 8, 6, 5, 4, 3, 2, 1,
];

/// The envelope a SNES voice's ADSR1/ADSR2/GAIN registers give its notes.
pub fn snes_envelope(adsr1: u8, adsr2: u8, gain: u8) -> Envelope {
    let ms = |rate: usize, steps: f64| match SNES_RATES[rate.min(31)] {
        0 => f64::INFINITY,
        r => steps * r as f64 / 32.0,
    };
    if adsr1 & 0x80 != 0 {
        let ar = (adsr1 & 0x0F) as usize;
        let dr = ((adsr1 >> 4) & 7) as usize;
        let sl = (adsr2 >> 5) as f64;
        let sr = (adsr2 & 0x1F) as usize;
        // Attack: 32 steps of 1/64 (the fastest rate jumps straight up).
        let attack_ms = if ar == 15 { 0.0 } else { ms(ar * 2 + 1, 32.0) };
        // Exponential decays: about 590 steps to fall to a tenth.
        let level = (sl + 1.0) / 8.0;
        let decay_to_sustain = ms(dr * 2 + 16, 256.0 * (1.0 / level).ln());
        let sustain_ms = ms(sr, 590.0);
        // Sustaining at a low level after a quick fall plays like a pluck.
        let sustained = sustain_ms > 2500.0 && level >= 0.4;
        let decay_ms = if level < 0.1 {
            decay_to_sustain
        } else {
            decay_to_sustain + if level < 0.4 { 0.0 } else { sustain_ms }
        };
        Envelope {
            attack_ms,
            sustained,
            decay_ms: if sustained {
                f64::INFINITY
            } else {
                decay_ms.min(sustain_ms + decay_to_sustain)
            },
        }
    } else if gain & 0x80 == 0 {
        // A fixed volume.
        Envelope {
            attack_ms: 0.0,
            sustained: true,
            decay_ms: f64::INFINITY,
        }
    } else {
        let rate = (gain & 0x1F) as usize;
        match (gain >> 5) & 3 {
            // Linear or exponential decrease.
            0 => Envelope {
                attack_ms: 0.0,
                sustained: false,
                decay_ms: ms(rate, 58.0),
            },
            1 => Envelope {
                attack_ms: 0.0,
                sustained: false,
                decay_ms: ms(rate, 590.0),
            },
            // Linear or bent increase: a swell.
            _ => Envelope {
                attack_ms: ms(rate, 64.0),
                sustained: true,
                decay_ms: f64::INFINITY,
            },
        }
    }
}

/// The median of some keys.
pub fn median(keys: &mut [u8]) -> Option<u8> {
    if keys.is_empty() {
        return None;
    }
    keys.sort_unstable();
    Some(keys[keys.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 32_000.0;

    fn wave(hz: f64, len: usize, harmonics: &[f64], decay: Option<f64>) -> Vec<f32> {
        (0..len)
            .map(|i| {
                let t = i as f64 / RATE;
                let env = decay.map_or(1.0, |d| (-t / d).exp());
                let s: f64 = harmonics
                    .iter()
                    .enumerate()
                    .map(|(k, a)| a * (std::f64::consts::TAU * hz * (k + 1) as f64 * t).sin())
                    .sum();
                (s * env * 0.5) as f32
            })
            .collect()
    }

    fn kind_of(pcm: &[f32], looped: bool, key: u8) -> Kind {
        let ls = looped.then_some(0);
        let p = pitch(pcm, ls, RATE);
        let f = measure(pcm, ls, RATE, p.as_ref().map(|p| p.hz));
        classify("", &f, p.is_some(), Some(key))
    }

    #[test]
    fn tells_instruments_apart() {
        // Period of 64 samples so the loop is seamless.
        let hz = RATE / 64.0;
        let sine = wave(hz, 4096, &[1.0], None);
        assert_eq!(kind_of(&sine, true, 72), Kind::Melodic(73), "flute");
        let square = wave(
            hz,
            4096,
            &[1.0, 0.0, 0.33, 0.0, 0.2, 0.0, 0.14, 0.0, 0.11],
            None,
        );
        assert_eq!(kind_of(&square, true, 64), Kind::Melodic(71), "clarinet");
        let pluck = wave(hz, 16000, &[1.0, 0.5, 0.3, 0.2, 0.1], Some(0.15));
        assert!(
            matches!(kind_of(&pluck, false, 60), Kind::Melodic(24 | 25)),
            "a pluck: {:?}",
            kind_of(&pluck, false, 60)
        );
        assert!(matches!(kind_of(&pluck, false, 36), Kind::Melodic(32..=39)));
    }

    #[test]
    fn tells_drums_apart() {
        let mut x = 1u32;
        let mut noise = |n: usize, decay: f64| -> Vec<f32> {
            (0..n)
                .map(|i| {
                    x = x.wrapping_mul(1103515245).wrapping_add(12345);
                    let r = ((x >> 16) as f32 / 32768.0) - 1.0;
                    r * (-(i as f64) / RATE / decay).exp() as f32
                })
                .collect()
        };
        let hat = noise(3000, 0.02);
        assert_eq!(kind_of(&hat, false, 60), Kind::Drum(42));
        // A low thump: a decaying 60 Hz sine with a noisy start.
        let kick: Vec<f32> = (0..6000)
            .map(|i| {
                let t = i as f64 / RATE;
                ((std::f64::consts::TAU * 60.0 * t * (1.0 - t)).sin() * (-t / 0.08).exp()) as f32
            })
            .collect();
        let f = measure(&kick, None, RATE, None);
        assert_eq!(drum_for(&f), 36);
    }

    #[test]
    fn names() {
        assert_eq!(by_name("Slap Bass 2"), Some(Kind::Melodic(36)));
        assert_eq!(by_name("strings"), Some(Kind::Melodic(48)));
        assert_eq!(by_name("BD"), Some(Kind::Drum(36)));
        assert_eq!(by_name("bdsm"), None);
        assert_eq!(by_name("kick.wav"), Some(Kind::Drum(36)));
        assert_eq!(by_name("what a tune"), None);
        assert_eq!(by_name("hihat 1"), Some(Kind::Drum(42)));
        assert_eq!(by_name("dist.gtr"), Some(Kind::Melodic(30)));
        assert_eq!(by_name("Construct"), None);
    }

    #[test]
    fn snes_envelopes() {
        // Instant attack, fall to 1/8 then slow fade: a pluck.
        let e = snes_envelope(0x8F, 0x0A, 0);
        assert!(!e.sustained && e.attack_ms == 0.0, "{e:?}");
        // Slow attack, full sustain: a pad.
        let e = snes_envelope(0x84, 0xE0, 0);
        assert!(e.sustained && e.attack_ms > 100.0, "{e:?}");
        assert!(snes_envelope(0, 0, 0x7F).sustained);
    }
}
