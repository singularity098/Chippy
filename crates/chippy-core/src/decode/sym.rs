//! Common formats through Symphonia (MP3, FLAC, WAV, OGG Vorbis, AAC/M4A,
//! ALAC, AIFF, CAF, MKV/WebM audio); their tags and embedded pictures
//! through lofty.

use anyhow::{anyhow, Result};
use std::io::Cursor;
use std::sync::Arc;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use super::{to_stereo, Decoder};
use crate::meta::TrackMeta;
use crate::vfs::Location;

/// Shared bytes Symphonia can read like a file.
struct Bytes(Arc<Vec<u8>>);
impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

fn source(loc: &Location) -> Result<Box<dyn MediaSource>> {
    Ok(match loc {
        Location::Fs(p) => Box::new(std::fs::File::open(p)?),
        Location::Archive { .. } => Box::new(Cursor::new(Bytes(loc.read()?))),
    })
}

pub struct SymDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    rate: u32,
    duration: Option<u64>,
    pending: Vec<f32>,
    /// Reused between packets while the format stays the same.
    sample_buf: Option<(SampleBuffer<f32>, symphonia::core::audio::SignalSpec, u64)>,
    done: bool,
}

impl SymDecoder {
    pub fn open(loc: &Location) -> Result<Self> {
        let mss = MediaSourceStream::new(source(loc)?, Default::default());
        let mut hint = Hint::new();
        hint.with_extension(&loc.extension());
        let probed = symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| anyhow!("no audio track"))?;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?;
        let rate = params
            .sample_rate
            .ok_or_else(|| anyhow!("unknown sample rate"))?;
        let duration = params.n_frames.map(|n| n * 1000 / rate as u64);
        Ok(SymDecoder {
            track_id: track.id,
            format,
            decoder,
            rate,
            duration,
            pending: Vec::new(),
            sample_buf: None,
            done: false,
        })
    }

    /// Decodes the next packet into `pending`; false at the end.
    fn decode_next(&mut self) -> Result<bool> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(false)
                }
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    let spec = *buf.spec();
                    let frames = buf.capacity() as u64;
                    let fits =
                        matches!(&self.sample_buf, Some((_, s, f)) if *s == spec && *f >= frames);
                    if !fits {
                        self.sample_buf = Some((SampleBuffer::new(frames, spec), spec, frames));
                    }
                    let (sb, _, _) = self.sample_buf.as_mut().unwrap();
                    sb.copy_interleaved_ref(buf);
                    to_stereo(sb.samples(), spec.channels.count(), &mut self.pending);
                    return Ok(true);
                }
                // A damaged packet: skip it.
                Err(SymError::DecodeError(e)) => {
                    log::debug!("skipping a bad packet: {e}");
                    continue;
                }
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(false)
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl Decoder for SymDecoder {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn duration_ms(&self) -> Option<u64> {
        self.duration
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        while self.pending.len() < out.len() && !self.done {
            if !self.decode_next()? {
                self.done = true;
            }
        }
        let n = self.pending.len().min(out.len()) & !1;
        out[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n / 2)
    }
    fn seek(&mut self, ms: u64) -> Result<()> {
        self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::new(ms / 1000, (ms % 1000) as f64 / 1000.0),
                track_id: Some(self.track_id),
            },
        )?;
        self.decoder.reset();
        self.pending.clear();
        self.done = false;
        Ok(())
    }
}

// ------------------------------------------------------------- tags (lofty)

fn tagged(loc: &Location) -> Result<lofty::file::TaggedFile> {
    use lofty::probe::Probe;
    Ok(match loc {
        Location::Fs(p) => Probe::open(p)?.guess_file_type()?.read()?,
        Location::Archive { .. } => {
            let data = loc.read()?;
            Probe::new(Cursor::new(&data[..]))
                .guess_file_type()?
                .read()?
        }
    })
}

pub fn meta(loc: &Location) -> Result<TrackMeta> {
    use lofty::prelude::*;
    use lofty::tag::ItemKey;
    let f = tagged(loc)?;
    let props = f.properties();
    let mut m = TrackMeta {
        length_ms: Some(props.duration().as_millis() as u64).filter(|&d| d > 0),
        sample_rate: props.sample_rate(),
        channels: props.channels().map(u32::from),
        bitrate_kbps: props.audio_bitrate(),
        format: format!("{:?}", f.file_type())
            .to_uppercase()
            .replace("MPEG", "MP3"),
        ..Default::default()
    };
    if let Some(tag) = f.primary_tag().or_else(|| f.first_tag()) {
        let s = |v: Option<std::borrow::Cow<'_, str>>| {
            v.map(|v| v.trim().to_string()).unwrap_or_default()
        };
        m.title = s(tag.title());
        m.artist = s(tag.artist());
        m.album = s(tag.album());
        m.genre = s(tag.genre());
        m.comment = s(tag.comment());
        m.track_no = tag.track();
        m.year = [
            ItemKey::Year,
            ItemKey::RecordingDate,
            ItemKey::OriginalReleaseDate,
        ]
        .iter()
        .find_map(|k| tag.get_string(k))
        .map(|y| y.chars().take(4).collect())
        .unwrap_or_default();
        m.copyright = tag
            .get_string(&ItemKey::CopyrightMessage)
            .unwrap_or("")
            .to_string();
        if m.artist.is_empty() {
            m.artist = tag
                .get_string(&ItemKey::AlbumArtist)
                .unwrap_or("")
                .to_string();
        }
    }
    Ok(m)
}

/// The picture embedded in a file's tags: the front cover if marked, else
/// the first one.
pub fn embedded_picture(loc: &Location) -> Option<Vec<u8>> {
    use lofty::picture::PictureType;
    use lofty::prelude::*;
    let f = tagged(loc).ok()?;
    let pics: Vec<_> = f.tags().iter().flat_map(|t| t.pictures().iter()).collect();
    pics.iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pics.first())
        .map(|p| p.data().to_vec())
}
