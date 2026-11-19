//! Decoding any supported track to interleaved stereo `f32` PCM.

pub mod ao;
pub mod gme;
pub mod gsf;
pub mod midi;
pub mod sgc;
pub mod sid;
pub mod sym;
pub mod timed;
pub mod twosf;
pub mod usf;
pub mod vgm;
pub mod wsr;
pub mod xmp;

use anyhow::Result;

use crate::formats::{kind_of, Codec, FileKind};
use crate::meta::TrackMeta;
use crate::vfs::{Location, TrackRef};

/// How tracks that loop forever (most chiptunes) are played.
#[derive(Debug, Clone)]
pub struct DecodeOptions {
    /// The rate emulated formats render at (the output device's rate).
    pub sample_rate: u32,
    /// Length of tracks that do not say how long they are.
    pub default_length_ms: u64,
    /// Fade-out at the end of looping tracks.
    pub fade_ms: u64,
    /// How many times vgmstream plays a looping section.
    pub loops: f64,
    /// USF: high-level audio emulation (much faster, rarely less accurate).
    pub usf_hle: bool,
    /// GME: more accurate (slower) sound emulation.
    pub gme_accuracy: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions {
            sample_rate: 48_000,
            default_length_ms: 150_000,
            fade_ms: 8_000,
            loops: 2.0,
            usf_hle: true,
            gme_accuracy: true,
        }
    }
}

/// A track being decoded.
pub trait Decoder: Send {
    /// The rate of the samples `read` returns.
    fn sample_rate(&self) -> u32;
    /// How long the track plays (including any fade), when known.
    fn duration_ms(&self) -> Option<u64>;
    /// Fills `out` with interleaved stereo frames; returns how many frames
    /// were written (0 at the end of the track).
    fn read(&mut self, out: &mut [f32]) -> Result<usize>;
    fn seek(&mut self, ms: u64) -> Result<()>;
    /// Moves part of the way to `ms` and says whether it got there.
    /// Emulators can only seek by playing up to the place (from the start,
    /// to go back), which takes seconds on a phone: they go at most
    /// [`SEEK_STEP_MS`] of the song per call, so the player can stop in
    /// between for a newer request. The others seek all at once.
    fn seek_step(&mut self, ms: u64) -> Result<bool> {
        self.seek(ms)?;
        Ok(true)
    }
}

/// How much of the song a [`Decoder::seek_step`] plays through at most.
pub const SEEK_STEP_MS: u64 = 250;

/// A track inside a multi-track file.
#[derive(Debug, Clone)]
pub struct SubTrack {
    pub index: u32,
    pub title: String,
    pub length_ms: Option<u64>,
}

fn codec_of(loc: &Location) -> Option<Codec> {
    match kind_of(&loc.name()) {
        FileKind::Audio(c) => Some(c),
        _ => None,
    }
}

/// Opens a track with the decoder its extension calls for; common formats
/// that Symphonia refuses get a second try with vgmstream, and the other way
/// round.
pub fn open_decoder(t: &TrackRef, opts: &DecodeOptions) -> Result<Box<dyn Decoder>> {
    match codec_of(&t.loc) {
        Some(Codec::Gme) => Ok(Box::new(gme::GmeDecoder::open(
            &t.loc,
            t.sub.unwrap_or(0),
            opts,
        )?)),
        Some(Codec::Usf) => Ok(Box::new(usf::UsfDecoder::open(&t.loc, opts)?)),
        Some(Codec::Midi) => Ok(Box::new(midi::MidiDecoder::open(&t.loc, opts)?)),
        Some(Codec::Ao) => Ok(Box::new(ao::open(&t.loc, opts)?)),
        Some(Codec::Sid) => Ok(Box::new(sid::open(&t.loc, t.sub, opts)?)),
        Some(Codec::Sgc) => Ok(Box::new(sgc::open(&t.loc, t.sub, opts)?)),
        Some(Codec::Gsf) => Ok(Box::new(gsf::open(&t.loc, opts)?)),
        Some(Codec::Twosf) => Ok(Box::new(twosf::open(&t.loc, opts)?)),
        Some(Codec::Wsr) => Ok(Box::new(wsr::open(&t.loc, t.sub, opts)?)),
        Some(Codec::Module) => Ok(Box::new(xmp::XmpDecoder::open(&t.loc, opts)?)),
        Some(Codec::Common) => match sym::SymDecoder::open(&t.loc) {
            Ok(d) => Ok(Box::new(d)),
            Err(e) => vgm::VgmDecoder::open(&t.loc, t.sub, opts)
                .map(|d| Box::new(d) as Box<dyn Decoder>)
                .map_err(|_| e),
        },
        Some(Codec::Vgmstream) | None => match vgm::VgmDecoder::open(&t.loc, t.sub, opts) {
            Ok(d) => Ok(Box::new(d)),
            Err(e) => sym::SymDecoder::open(&t.loc)
                .map(|d| Box::new(d) as Box<dyn Decoder>)
                .map_err(|_| e),
        },
    }
}

/// The tracks inside a file. Files with a single track list one.
pub fn track_list(loc: &Location, opts: &DecodeOptions) -> Result<Vec<SubTrack>> {
    match codec_of(loc) {
        Some(Codec::Gme) => gme::track_list(loc, opts),
        Some(Codec::Vgmstream) => vgm::track_list(loc, opts),
        Some(Codec::Sid) => sid::track_list(loc, opts),
        Some(Codec::Sgc) => sgc::track_list(loc, opts),
        Some(Codec::Wsr) => wsr::track_list(loc, opts),
        _ => Ok(vec![SubTrack {
            index: 0,
            title: loc.name(),
            length_ms: None,
        }]),
    }
}

/// Reads a track's tags and properties with the decoder's own knowledge.
pub fn read_meta(t: &TrackRef, opts: &DecodeOptions) -> Result<TrackMeta> {
    match codec_of(&t.loc) {
        Some(Codec::Gme) => gme::meta(&t.loc, t.sub.unwrap_or(0), opts),
        Some(Codec::Usf) => usf::meta(&t.loc, opts),
        Some(Codec::Midi) => midi::meta(&t.loc),
        Some(Codec::Ao) => ao::meta(&t.loc, opts),
        Some(Codec::Sid) => sid::meta(&t.loc, t.sub, opts),
        Some(Codec::Sgc) => sgc::meta(&t.loc, t.sub, opts),
        Some(Codec::Gsf) => gsf::meta(&t.loc, opts),
        Some(Codec::Twosf) => twosf::meta(&t.loc, opts),
        Some(Codec::Wsr) => wsr::meta(&t.loc, t.sub, opts),
        Some(Codec::Module) => xmp::meta(&t.loc),
        Some(Codec::Common) => sym::meta(&t.loc).or_else(|_| vgm::meta(&t.loc, t.sub, opts)),
        Some(Codec::Vgmstream) | None => vgm::meta(&t.loc, t.sub, opts),
    }
}

/// Mixes interleaved `channels`-channel samples down (or up) to stereo,
/// appending to `out`.
pub fn to_stereo(src: &[f32], channels: usize, out: &mut Vec<f32>) {
    match channels {
        0 => {}
        1 => {
            for &s in src {
                out.push(s);
                out.push(s);
            }
        }
        2 => out.extend_from_slice(src),
        n => {
            // Odd channels lean left, even right, centre-ish ones both.
            for f in src.chunks_exact(n) {
                let (mut l, mut r) = (f[0], f[1]);
                for (i, &s) in f.iter().enumerate().skip(2) {
                    if i % 2 == 0 {
                        l += s * 0.7;
                    } else {
                        r += s * 0.7;
                    }
                }
                let g = 1.0 / (1.0 + 0.7 * ((n - 2) as f32 / 2.0));
                out.push(l * g);
                out.push(r * g);
            }
        }
    }
}

/// Applies a linear fade to stereo frames: `pos` is the frame index of the
/// first frame, fading from `start` to `end` (frame indices).
pub fn apply_fade(buf: &mut [f32], pos: u64, start: u64, end: u64) {
    if end <= start {
        return;
    }
    for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        let p = pos + i as u64;
        if p >= start {
            let g = if p >= end {
                0.0
            } else {
                1.0 - (p - start) as f32 / (end - start) as f32
            };
            f[0] *= g;
            f[1] *= g;
        }
    }
}

/// Helpers for the decoders' tests.
#[cfg(test)]
pub(crate) mod testing {
    use std::io::Write;

    pub fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// A PSF-family file: version `v`, the program section, tags.
    pub fn psf(v: u8, program: &[u8], tags: &str) -> Vec<u8> {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(program).unwrap();
        let z = z.finish().unwrap();
        let mut h = flate2::Crc::new();
        h.update(&z);
        let mut f = b"PSF".to_vec();
        f.push(v);
        f.extend(0u32.to_le_bytes());
        f.extend((z.len() as u32).to_le_bytes());
        f.extend(h.sum().to_le_bytes());
        f.extend(z);
        f.extend(b"[TAG]");
        f.extend(tags.as_bytes());
        f
    }

    /// The frequency (Hz) of the left channel's rising zero crossings.
    pub fn pitch(frames: &[f32], rate: u32) -> f64 {
        let left: Vec<f32> = frames.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        let mean = left.iter().sum::<f32>() / left.len() as f32;
        let n = left
            .windows(2)
            .filter(|w| w[0] < mean && w[1] >= mean)
            .count();
        n as f64 / left.len() as f64 * rate as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_mixing() {
        let mut out = Vec::new();
        to_stereo(&[0.5, -0.5], 1, &mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
        out.clear();
        to_stereo(&[0.1, 0.2, 0.0, 0.0, 0.0, 0.0], 6, &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn fading() {
        let mut b = vec![1.0; 8];
        apply_fade(&mut b, 0, 2, 4);
        assert_eq!(b, vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.5, 0.5]);
    }
}
