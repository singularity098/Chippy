//! Decoding any supported track to interleaved stereo `f32` PCM.

pub mod gme;
pub mod midi;
pub mod sym;
pub mod usf;
pub mod vgm;

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
}

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
    for (i, f) in buf.chunks_exact_mut(2).enumerate() {
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
