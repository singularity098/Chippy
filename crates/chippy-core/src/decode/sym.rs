//! Common formats through Symphonia (MP3, FLAC, WAV, OGG Vorbis, AAC/M4A,
//! ALAC, AIFF, CAF, MKV/WebM audio; Opus in them through libopus); their
//! tags and embedded pictures through lofty.

use anyhow::{anyhow, Result};
use std::io::Cursor;
use std::sync::Arc;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL, CODEC_TYPE_OPUS};
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

/// Opus, which Symphonia demuxes but cannot decode.
struct OpusCodec {
    dec: opus::Decoder,
    channels: usize,
    /// Samples (per channel) still to drop at the start: the encoder's
    /// look-ahead (OpusHead's pre-skip).
    skip: usize,
    pre_skip: usize,
    buf: Vec<f32>,
}

impl OpusCodec {
    fn new(params: &symphonia::core::codecs::CodecParameters) -> Result<Self> {
        // OpusHead: "OpusHead", version, channels, pre-skip (LE16), ...
        let head = params.extra_data.as_deref().unwrap_or(&[]);
        let (channels, pre_skip) = if head.len() >= 12 && &head[..8] == b"OpusHead" {
            (
                head[9] as usize,
                u16::from_le_bytes([head[10], head[11]]) as usize,
            )
        } else {
            (params.channels.map_or(2, |c| c.count()), 0)
        };
        let ch = match channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            n => anyhow::bail!("Opus with {n} channels is not supported"),
        };
        Ok(OpusCodec {
            dec: opus::Decoder::new(48_000, ch)?,
            channels,
            skip: pre_skip,
            pre_skip,
            buf: vec![0.0; 5760 * 2],
        })
    }

    fn decode(&mut self, data: &[u8], out: &mut Vec<f32>) -> Result<()> {
        let n = self.dec.decode_float(data, &mut self.buf, false)?;
        let drop = self.skip.min(n);
        self.skip -= drop;
        to_stereo(
            &self.buf[drop * self.channels..n * self.channels],
            self.channels,
            out,
        );
        Ok(())
    }

    fn reset(&mut self, at_start: bool) {
        let _ = self.dec.reset_state();
        self.skip = if at_start { self.pre_skip } else { 0 };
    }
}

enum Codec {
    Sym(Box<dyn symphonia::core::codecs::Decoder>),
    Opus(OpusCodec),
}

pub struct SymDecoder {
    format: Box<dyn FormatReader>,
    decoder: Codec,
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
        let (decoder, rate) = if params.codec == CODEC_TYPE_OPUS {
            (Codec::Opus(OpusCodec::new(&params)?), 48_000)
        } else {
            let d = symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?;
            let rate = params
                .sample_rate
                .ok_or_else(|| anyhow!("unknown sample rate"))?;
            (Codec::Sym(d), rate)
        };
        // The length is counted in the track's time base: samples for most
        // formats, but Matroska's timestamp units for WebM.
        let duration = params.n_frames.map(|n| match params.time_base {
            Some(tb) => {
                let t = tb.calc_time(n);
                t.seconds * 1000 + (t.frac * 1000.0) as u64
            }
            None => n * 1000 / rate as u64,
        });
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
                    match &mut self.decoder {
                        Codec::Sym(d) => d.reset(),
                        Codec::Opus(o) => o.reset(false),
                    }
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoder = match &mut self.decoder {
                Codec::Sym(d) => d,
                Codec::Opus(o) => match o.decode(&packet.data, &mut self.pending) {
                    Ok(()) => return Ok(true),
                    Err(e) => {
                        log::debug!("skipping a bad packet: {e}");
                        continue;
                    }
                },
            };
            match decoder.decode(&packet) {
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
        match &mut self.decoder {
            Codec::Sym(d) => d.reset(),
            Codec::Opus(o) => o.reset(ms == 0),
        }
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

/// The lyrics in a file's tags (ID3 USLT, Vorbis LYRICS or
/// UNSYNCEDLYRICS, MP4 ©lyr...), as stored.
pub fn lyrics(loc: &Location) -> Option<String> {
    use lofty::prelude::*;
    use lofty::tag::ItemKey;
    let f = tagged(loc).ok()?;
    f.tags().iter().find_map(|tag| {
        [
            ItemKey::Lyrics,
            ItemKey::Unknown("UNSYNCEDLYRICS".into()),
            ItemKey::Unknown("UNSYNCED LYRICS".into()),
            ItemKey::Unknown("SYNCEDLYRICS".into()),
        ]
        .iter()
        .find_map(|k| tag.get_string(k))
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
    })
}

pub fn meta(loc: &Location) -> Result<TrackMeta> {
    lofty_meta(loc).or_else(|e| symphonia_meta(loc).map_err(|_| e))
}

/// What Symphonia reads of a file lofty does not know (WebM, Matroska).
fn symphonia_meta(loc: &Location) -> Result<TrackMeta> {
    use symphonia::core::meta::StandardTagKey as K;
    let d = SymDecoder::open(loc)?;
    let track = d
        .format
        .tracks()
        .iter()
        .find(|t| t.id == d.track_id)
        .ok_or_else(|| anyhow!("no audio track"))?;
    let codec = symphonia::default::get_codecs()
        .get_codec(track.codec_params.codec)
        .map(|c| c.short_name.to_uppercase())
        .unwrap_or_else(|| "OPUS".into());
    let container = loc.extension().to_uppercase();
    let mut m = TrackMeta {
        length_ms: d.duration,
        sample_rate: Some(d.rate),
        channels: track.codec_params.channels.map(|c| c.count() as u32),
        format: format!("{container} ({codec})"),
        ..Default::default()
    };
    let mut format = d.format;
    if let Some(rev) = format.metadata().current() {
        for tag in rev.tags() {
            let v = tag.value.to_string().trim().to_string();
            // Matroska's tags come by name only.
            let key = tag.std_key.or(match tag.key.to_ascii_uppercase().as_str() {
                "TITLE" => Some(K::TrackTitle),
                "ARTIST" => Some(K::Artist),
                "ALBUM_ARTIST" => Some(K::AlbumArtist),
                "ALBUM" => Some(K::Album),
                "GENRE" => Some(K::Genre),
                "COMMENT" | "DESCRIPTION" => Some(K::Comment),
                "COPYRIGHT" => Some(K::Copyright),
                "DATE" | "DATE_RELEASED" | "DATE_RECORDED" | "YEAR" => Some(K::Date),
                "PART_NUMBER" | "TRACK" | "TRACKNUMBER" => Some(K::TrackNumber),
                _ => None,
            });
            match key {
                Some(K::TrackTitle) => m.title = v,
                Some(K::Artist) => m.artist = v,
                Some(K::AlbumArtist) if m.artist.is_empty() => m.artist = v,
                Some(K::Album) => m.album = v,
                Some(K::Genre) => m.genre = v,
                Some(K::Comment) => m.comment = v,
                Some(K::Copyright) => m.copyright = v,
                Some(K::Date) | Some(K::ReleaseDate) => m.year = v.chars().take(4).collect(),
                Some(K::TrackNumber) => {
                    m.track_no = v.split('/').next().and_then(|n| n.trim().parse().ok())
                }
                _ => {}
            }
        }
    }
    Ok(m)
}

fn lofty_meta(loc: &Location) -> Result<TrackMeta> {
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
