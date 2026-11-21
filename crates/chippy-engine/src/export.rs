//! Exporting tracks to audio files: WAV, FLAC, MP3 or Opus, one file per
//! track, tagged. Tracks are decoded exactly as they play (looping
//! chiptunes end at their length, with the fade), on a thread of their own.
//! Or to MIDI: the notes, for the chiptunes whose notes can be found.

use anyhow::{anyhow, bail, Context, Result};
use chippy_core::decode::{open_decoder, DecodeOptions, Decoder};
use chippy_core::{notes, TrackMeta, TrackRef};
use crossbeam_channel::Sender;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::player::Resample;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Wav,
    Flac,
    Mp3,
    Opus,
    /// The notes, not the sound (see `chippy_core::notes`).
    Midi,
}

impl Format {
    pub fn from_index(i: u32) -> Format {
        match i {
            0 => Format::Wav,
            2 => Format::Mp3,
            3 => Format::Opus,
            4 => Format::Midi,
            _ => Format::Flac,
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Flac => "flac",
            Format::Mp3 => "mp3",
            Format::Opus => "opus",
            Format::Midi => "mid",
        }
    }
}

pub struct Job {
    pub tracks: Vec<(TrackRef, TrackMeta)>,
    pub folder: PathBuf,
    pub format: Format,
    pub kbps: u32,
    pub opts: DecodeOptions,
}

pub enum Progress {
    /// Starting track `index` of `total`.
    Track {
        index: usize,
        total: usize,
        name: String,
    },
    /// How far through the current track (0..1).
    Fraction(f32),
    Finished {
        written: usize,
        failed: Vec<String>,
        cancelled: bool,
    },
}

/// Longest a track may run, should a decoder never end.
const MAX_SECONDS: u64 = 3 * 60 * 60;

pub fn spawn(job: Job, cancel: Arc<AtomicBool>, tx: Sender<Progress>) {
    std::thread::Builder::new()
        .name("export".into())
        .spawn(move || {
            crate::background_thread();
            run(job, &cancel, &tx)
        })
        .ok();
}

fn run(job: Job, cancel: &AtomicBool, tx: &Sender<Progress>) {
    let total = job.tracks.len();
    let width = total.to_string().len().max(2);
    let (mut written, mut failed) = (0, Vec::new());
    let mut used = std::collections::HashSet::new();
    for (i, (track, meta)) in job.tracks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let name = file_name(i, width, meta, job.format, &mut used);
        let _ = tx.send(Progress::Track {
            index: i,
            total,
            name: meta.title.clone(),
        });
        let path = job.folder.join(&name);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            export_track(track, meta, &path, &job, cancel, tx)
        }))
        .unwrap_or_else(|_| Err(anyhow!("the decoder crashed")));
        match result {
            Ok(()) => written += 1,
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                if !cancel.load(Ordering::Relaxed) {
                    log::warn!("export of {} failed: {e:#}", meta.title);
                    failed.push(format!("{}: {e:#}", meta.title));
                }
            }
        }
    }
    let _ = tx.send(Progress::Finished {
        written,
        failed,
        cancelled: cancel.load(Ordering::Relaxed),
    });
}

/// "07 Title.flac", made safe for file systems and unique in the folder.
fn file_name(
    i: usize,
    width: usize,
    meta: &TrackMeta,
    format: Format,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let title: String = meta
        .title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let title = title.trim().trim_end_matches('.');
    let title: String = title.chars().take(120).collect();
    let base = if title.is_empty() {
        format!("{:0width$}", i + 1)
    } else {
        format!("{:0width$} {title}", i + 1)
    };
    let mut name = format!("{base}.{}", format.extension());
    let mut n = 2;
    while !used.insert(name.to_lowercase()) {
        name = format!("{base} ({n}).{}", format.extension());
        n += 1;
    }
    name
}

/// Decoded audio at the rate an encoder needs.
struct Pcm {
    decoder: Box<dyn Decoder>,
    resample: Option<Resample>,
    rate: u32,
    buf: Vec<f32>,
    out: Vec<f32>,
    out_pos: usize,
    done: bool,
    frames_in: u64,
    expected: Option<u64>,
    in_rate: u32,
}

impl Pcm {
    /// `rate`: the rate wanted, or None for the decoder's own.
    fn open(track: &TrackRef, opts: &DecodeOptions, rate: Option<u32>) -> Result<Pcm> {
        let mut opts = opts.clone();
        if let Some(r) = rate {
            // Emulated formats render at this rate directly.
            opts.sample_rate = r;
        }
        // The WonderSwan core is shared with playback, which may still be
        // letting go of it.
        let mut tries = 0;
        let decoder = loop {
            match open_decoder(track, &opts) {
                Err(e) if tries < 20 && format!("{e}").contains("another WonderSwan") => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                r => break r?,
            }
        };
        let in_rate = decoder.sample_rate();
        let rate = rate.unwrap_or(in_rate);
        let resample = if rate != in_rate {
            Some(Resample::new(in_rate, rate)?)
        } else {
            None
        };
        let expected = decoder.duration_ms().map(|ms| ms * in_rate as u64 / 1000);
        Ok(Pcm {
            decoder,
            resample,
            rate,
            buf: vec![0.0; 4096 * 2],
            out: Vec::new(),
            out_pos: 0,
            done: false,
            frames_in: 0,
            expected,
            in_rate,
        })
    }

    fn fraction(&self) -> f32 {
        match self.expected {
            Some(e) if e > 0 => (self.frames_in as f64 / e as f64).min(1.0) as f32,
            _ => 0.0,
        }
    }

    /// The next stereo samples (interleaved), or None at the end.
    fn next_chunk(&mut self) -> Result<Option<&[f32]>> {
        loop {
            if self.out_pos < self.out.len() {
                let s = &self.out[self.out_pos..];
                self.out_pos = self.out.len();
                return Ok(Some(s));
            }
            if self.done {
                return Ok(None);
            }
            self.out.clear();
            self.out_pos = 0;
            let n = if self.frames_in > MAX_SECONDS * self.in_rate as u64 {
                0
            } else {
                self.decoder.read(&mut self.buf)?
            };
            self.frames_in += n as u64;
            match (&mut self.resample, n) {
                (Some(r), 0) => {
                    r.flush(&mut self.out);
                    self.done = true;
                }
                (None, 0) => self.done = true,
                (Some(r), n) => r.push(&self.buf[..n * 2], &mut self.out),
                (None, n) => self.out.extend_from_slice(&self.buf[..n * 2]),
            }
        }
    }
}

fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn export_track(
    track: &TrackRef,
    meta: &TrackMeta,
    path: &Path,
    job: &Job,
    cancel: &AtomicBool,
    tx: &Sender<Progress>,
) -> Result<()> {
    match job.format {
        Format::Midi => write_midi(track, meta, path),
        _ => export_audio(track, meta, path, job, cancel, tx),
    }
}

fn export_audio(
    track: &TrackRef,
    meta: &TrackMeta,
    path: &Path,
    job: &Job,
    cancel: &AtomicBool,
    tx: &Sender<Progress>,
) -> Result<()> {
    let rate = match job.format {
        Format::Wav | Format::Flac | Format::Midi => None,
        Format::Mp3 => Some(44_100),
        Format::Opus => Some(48_000),
    };
    let mut pcm = Pcm::open(track, &job.opts, rate)?;
    // MP3 takes 48 kHz as is.
    if job.format == Format::Mp3 && pcm.in_rate == 48_000 && pcm.resample.is_some() {
        pcm = Pcm::open(track, &job.opts, Some(48_000))?;
    }
    let mut last = 0.0f32;
    let mut report = |pcm: &Pcm| -> Result<()> {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let f = pcm.fraction();
        if f - last >= 0.01 || f < last {
            last = f;
            let _ = tx.send(Progress::Fraction(f));
        }
        Ok(())
    };
    match job.format {
        Format::Wav => write_wav(&mut pcm, path, &mut report)?,
        Format::Flac => write_flac(&mut pcm, path, meta, &mut report)?,
        Format::Mp3 => write_mp3(&mut pcm, path, job.kbps, &mut report)?,
        Format::Opus => write_opus(&mut pcm, path, job.kbps, meta, &mut report)?,
        Format::Midi => bail!("MIDI is not audio"),
    }
    // FLAC and Opus carry their tags from the start.
    if matches!(job.format, Format::Wav | Format::Mp3) {
        if let Err(e) = write_tags(path, meta) {
            log::warn!("could not tag {}: {e:#}", path.display());
        }
    }
    Ok(())
}

/// The notes of a chiptune (NES, SNES, Genesis) as a MIDI file, over the
/// length it plays for; a MIDI file is copied as it is.
fn write_midi(track: &TrackRef, meta: &TrackMeta, path: &Path) -> Result<()> {
    let bytes = match track.loc.extension().as_str() {
        "mid" | "midi" | "kar" | "smf" => track.loc.read()?.to_vec(),
        _ => {
            let t = notes::transcribe(&track.loc, track.sub, meta.length_ms)?;
            if t.parts.is_empty() {
                bail!("no notes found");
            }
            notes::midi::write(&t, &meta.title)
        }
    };
    std::fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))
}

type Report<'a> = dyn FnMut(&Pcm) -> Result<()> + 'a;

fn write_wav(pcm: &mut Pcm, path: &Path, report: &mut Report) -> Result<()> {
    let file =
        std::fs::File::create(path).with_context(|| format!("cannot write {}", path.display()))?;
    let mut w = BufWriter::new(file);
    let rate = pcm.rate;
    let header = |w: &mut BufWriter<std::fs::File>, data: u32| -> std::io::Result<()> {
        w.write_all(b"RIFF")?;
        w.write_all(&36u32.saturating_add(data).to_le_bytes())?;
        w.write_all(b"WAVEfmt ")?;
        w.write_all(&16u32.to_le_bytes())?;
        w.write_all(&1u16.to_le_bytes())?;
        w.write_all(&2u16.to_le_bytes())?;
        w.write_all(&rate.to_le_bytes())?;
        w.write_all(&(rate * 4).to_le_bytes())?;
        w.write_all(&4u16.to_le_bytes())?;
        w.write_all(&16u16.to_le_bytes())?;
        w.write_all(b"data")?;
        w.write_all(&data.to_le_bytes())
    };
    header(&mut w, 0)?;
    let mut bytes: u64 = 0;
    while let Some(chunk) = pcm.next_chunk()? {
        for &s in chunk {
            w.write_all(&to_i16(s).to_le_bytes())?;
        }
        bytes += chunk.len() as u64 * 2;
        report(pcm)?;
    }
    if bytes > u32::MAX as u64 - 36 {
        bail!("too long for a WAV file");
    }
    w.seek(SeekFrom::Start(0))?;
    header(&mut w, bytes as u32)?;
    w.flush()?;
    Ok(())
}

/// Feeds the decoder to the FLAC encoder block by block.
struct FlacSource<'a, 'b> {
    pcm: &'a mut Pcm,
    report: &'a mut Report<'b>,
    pending: Vec<i32>,
    block: Vec<i32>,
    error: Option<anyhow::Error>,
}

impl flacenc::source::Source for FlacSource<'_, '_> {
    fn channels(&self) -> usize {
        2
    }
    fn bits_per_sample(&self) -> usize {
        16
    }
    fn sample_rate(&self) -> usize {
        self.pcm.rate as usize
    }
    fn read_samples<F: flacenc::source::Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> Result<usize, flacenc::error::SourceError> {
        let want = block_size * 2;
        while self.pending.len() < want && self.error.is_none() {
            match self.pcm.next_chunk() {
                Ok(Some(c)) => self.pending.extend(c.iter().map(|&s| to_i16(s) as i32)),
                Ok(None) => break,
                Err(e) => self.error = Some(e),
            }
            if let Err(e) = (self.report)(self.pcm) {
                self.error = Some(e);
            }
        }
        if self.error.is_some() {
            // Ends the stream; the error is raised afterwards.
            self.pending.clear();
        }
        let n = want.min(self.pending.len());
        self.block.clear();
        self.block.extend(self.pending.drain(..n));
        dest.fill_interleaved(&self.block)?;
        Ok(n / 2)
    }
}

fn write_flac(pcm: &mut Pcm, path: &Path, meta: &TrackMeta, report: &mut Report) -> Result<()> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| anyhow!("FLAC settings: {e:?}"))?;
    let mut source = FlacSource {
        pcm,
        report,
        pending: Vec::new(),
        block: Vec::new(),
        error: None,
    };
    let stream = flacenc::encode_with_fixed_block_size(&config, &mut source, config.block_size)
        .map_err(|e| anyhow!("FLAC encoder: {e:?}"))?;
    if let Some(e) = source.error {
        return Err(e);
    }
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| anyhow!("FLAC encoder: {e:?}"))?;
    let bytes = with_vorbis_comments(sink.as_slice(), &vorbis_comments(meta))?;
    std::fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Adds a VORBIS_COMMENT block after the STREAMINFO block (the only one the
/// encoder writes), so the tags are there from the start.
fn with_vorbis_comments(flac: &[u8], comments: &[(String, String)]) -> Result<Vec<u8>> {
    // "fLaC", then STREAMINFO: a 4-byte header and 34 bytes.
    if flac.len() < 42 || &flac[..4] != b"fLaC" || flac[4] & 0x7f != 0 {
        bail!("unexpected FLAC stream");
    }
    let last = flac[4] & 0x80 != 0;
    let mut body = Vec::new();
    let vendor = format!("Chippy {}", env!("CARGO_PKG_VERSION"));
    body.extend((vendor.len() as u32).to_le_bytes());
    body.extend(vendor.as_bytes());
    body.extend((comments.len() as u32).to_le_bytes());
    for (k, v) in comments {
        let c = format!("{k}={v}");
        body.extend((c.len() as u32).to_le_bytes());
        body.extend(c.as_bytes());
    }
    if body.len() >= 1 << 24 {
        bail!("tags too large");
    }
    let mut out = Vec::with_capacity(flac.len() + body.len() + 4);
    out.extend_from_slice(&flac[..4]);
    out.push(flac[4] & 0x7f);
    out.extend_from_slice(&flac[5..42]);
    out.push(4 | if last { 0x80 } else { 0 });
    out.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    out.extend(body);
    out.extend_from_slice(&flac[42..]);
    Ok(out)
}

fn mp3_bitrate(kbps: u32) -> mp3lame_encoder::Bitrate {
    use mp3lame_encoder::Bitrate as B;
    match kbps {
        0..=96 => B::Kbps96,
        97..=128 => B::Kbps128,
        129..=160 => B::Kbps160,
        161..=192 => B::Kbps192,
        193..=224 => B::Kbps224,
        225..=256 => B::Kbps256,
        _ => B::Kbps320,
    }
}

fn write_mp3(pcm: &mut Pcm, path: &Path, kbps: u32, report: &mut Report) -> Result<()> {
    use mp3lame_encoder::{Builder, FlushNoGap, InterleavedPcm, Quality};
    let err = |e: &dyn std::fmt::Debug| anyhow!("MP3 encoder: {e:?}");
    let mut b = Builder::new().ok_or_else(|| anyhow!("MP3 encoder unavailable"))?;
    b.set_num_channels(2).map_err(|e| err(&e))?;
    b.set_sample_rate(pcm.rate).map_err(|e| err(&e))?;
    b.set_brate(mp3_bitrate(kbps)).map_err(|e| err(&e))?;
    b.set_quality(Quality::Best).map_err(|e| err(&e))?;
    let mut enc = b.build().map_err(|e| err(&e))?;
    let file =
        std::fs::File::create(path).with_context(|| format!("cannot write {}", path.display()))?;
    let mut w = BufWriter::new(file);
    let mut out: Vec<u8> = Vec::new();
    let mut samples: Vec<i16> = Vec::new();
    while let Some(chunk) = pcm.next_chunk()? {
        samples.clear();
        samples.extend(chunk.iter().map(|&s| to_i16(s)));
        out.clear();
        out.reserve(mp3lame_encoder::max_required_buffer_size(samples.len() / 2));
        let n = enc
            .encode(InterleavedPcm(&samples), out.spare_capacity_mut())
            .map_err(|e| err(&e))?;
        unsafe { out.set_len(n) };
        w.write_all(&out)?;
        report(pcm)?;
    }
    out.clear();
    out.reserve(7200);
    let n = enc
        .flush::<FlushNoGap>(out.spare_capacity_mut())
        .map_err(|e| err(&e))?;
    unsafe { out.set_len(n) };
    w.write_all(&out)?;
    w.flush()?;
    Ok(())
}

/// The OpusTags comments of a track.
fn vorbis_comments(meta: &TrackMeta) -> Vec<(String, String)> {
    let mut c = Vec::new();
    let mut add = |k: &str, v: &str| {
        if !v.trim().is_empty() {
            c.push((k.to_string(), v.to_string()));
        }
    };
    add("TITLE", &meta.title);
    add("ARTIST", &meta.artist);
    add("ALBUM", &meta.album);
    add("DATE", &meta.year);
    add("GENRE", &meta.genre);
    add("COMMENT", &meta.comment);
    add("COPYRIGHT", &meta.copyright);
    if let Some(n) = meta.track_no {
        add("TRACKNUMBER", &n.to_string());
    }
    c
}

fn write_opus(
    pcm: &mut Pcm,
    path: &Path,
    kbps: u32,
    meta: &TrackMeta,
    report: &mut Report,
) -> Result<()> {
    use ogg::writing::{PacketWriteEndInfo, PacketWriter};
    const FRAME: usize = 960; // 20 ms at 48 kHz
    let mut enc = opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)
        .map_err(|e| anyhow!("Opus encoder: {e}"))?;
    enc.set_bitrate(opus::Bitrate::Bits(kbps.clamp(32, 256) as i32 * 1000))
        .map_err(|e| anyhow!("Opus encoder: {e}"))?;
    let pre_skip = enc.get_lookahead().unwrap_or(312).max(0) as u16;

    let file =
        std::fs::File::create(path).with_context(|| format!("cannot write {}", path.display()))?;
    let mut w = PacketWriter::new(BufWriter::new(file));
    let serial: u32 = rand_serial();

    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(2);
    head.extend(pre_skip.to_le_bytes());
    head.extend(48_000u32.to_le_bytes());
    head.extend(0i16.to_le_bytes());
    head.push(0);
    w.write_packet(head, serial, PacketWriteEndInfo::EndPage, 0)?;

    let mut tags = b"OpusTags".to_vec();
    let vendor = format!("Chippy {}", env!("CARGO_PKG_VERSION"));
    tags.extend((vendor.len() as u32).to_le_bytes());
    tags.extend(vendor.as_bytes());
    let comments = vorbis_comments(meta);
    tags.extend((comments.len() as u32).to_le_bytes());
    for (k, v) in comments {
        let s = format!("{k}={v}");
        tags.extend((s.len() as u32).to_le_bytes());
        tags.extend(s.as_bytes());
    }
    w.write_packet(tags, serial, PacketWriteEndInfo::EndPage, 0)?;

    let mut pending: Vec<f32> = Vec::new();
    let mut packet = vec![0u8; 4000];
    let mut granule: u64 = pre_skip as u64;
    let mut real_frames: u64 = 0;
    let mut finished = false;
    // Encode whole 20 ms frames; the last is padded with silence, and the
    // granule position of the last page marks where the music ends.
    let mut queued: Option<Vec<u8>> = None;
    while !finished {
        while pending.len() < FRAME * 2 {
            match pcm.next_chunk()? {
                Some(c) => {
                    real_frames += c.len() as u64 / 2;
                    pending.extend_from_slice(c);
                }
                None => {
                    finished = true;
                    break;
                }
            }
            report(pcm)?;
        }
        if finished && pending.is_empty() {
            break;
        }
        pending.resize(pending.len().max(FRAME * 2), 0.0);
        let n = enc
            .encode_float(&pending[..FRAME * 2], &mut packet)
            .map_err(|e| anyhow!("Opus encoder: {e}"))?;
        pending.drain(..FRAME * 2);
        granule += FRAME as u64;
        if let Some(p) = queued.take() {
            w.write_packet(
                p,
                serial,
                PacketWriteEndInfo::NormalPacket,
                granule - FRAME as u64,
            )?;
        }
        queued = Some(packet[..n].to_vec());
        if finished && pending.is_empty() {
            break;
        }
    }
    let end = pre_skip as u64 + real_frames;
    let p = match queued {
        Some(p) => p,
        None => {
            // Not a single frame: one of silence.
            let silence = vec![0f32; FRAME * 2];
            let n = enc
                .encode_float(&silence, &mut packet)
                .map_err(|e| anyhow!("Opus encoder: {e}"))?;
            packet[..n].to_vec()
        }
    };
    w.write_packet(
        p,
        serial,
        PacketWriteEndInfo::EndStream,
        end.max(pre_skip as u64),
    )?;
    w.inner_mut().flush()?;
    Ok(())
}

fn rand_serial() -> u32 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
        .unwrap_or(0x5eed);
    t.wrapping_mul(2_654_435_761)
}

/// Tags WAV, FLAC and MP3 files with lofty.
fn write_tags(path: &Path, meta: &TrackMeta) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::prelude::*;
    use lofty::tag::{ItemKey, Tag};
    let mut file = lofty::read_from_path(path)?;
    let tag_type = file.primary_tag_type();
    let mut tag = Tag::new(tag_type);
    if !meta.title.is_empty() {
        tag.set_title(meta.title.clone());
    }
    if !meta.artist.is_empty() {
        tag.set_artist(meta.artist.clone());
    }
    if !meta.album.is_empty() {
        tag.set_album(meta.album.clone());
    }
    if !meta.genre.is_empty() {
        tag.set_genre(meta.genre.clone());
    }
    if !meta.comment.is_empty() {
        tag.set_comment(meta.comment.clone());
    }
    if let Some(n) = meta.track_no {
        tag.set_track(n);
    }
    if let Ok(y) = meta.year.trim().get(..4).unwrap_or("").parse::<u32>() {
        tag.insert_text(ItemKey::Year, y.to_string());
        tag.insert_text(ItemKey::RecordingDate, meta.year.trim().to_string());
    }
    if !meta.copyright.is_empty() {
        tag.insert_text(ItemKey::CopyrightMessage, meta.copyright.clone());
    }
    file.insert_tag(tag);
    file.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 s, 44.1 kHz stereo sine WAV.
    fn tone(path: &Path) {
        let rate = 44_100u32;
        let mut pcm = Vec::new();
        for i in 0..rate * 2 {
            let s =
                ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 12_000.0) as i16;
            pcm.extend(s.to_le_bytes());
            pcm.extend(s.to_le_bytes());
        }
        let mut f = b"RIFF".to_vec();
        f.extend((36 + pcm.len() as u32).to_le_bytes());
        f.extend(b"WAVEfmt ");
        f.extend(16u32.to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(2u16.to_le_bytes());
        f.extend(rate.to_le_bytes());
        f.extend((rate * 4).to_le_bytes());
        f.extend(4u16.to_le_bytes());
        f.extend(16u16.to_le_bytes());
        f.extend(b"data");
        f.extend((pcm.len() as u32).to_le_bytes());
        f.extend(pcm);
        std::fs::write(path, f).unwrap();
    }

    #[test]
    fn exports_every_format_with_tags() {
        use lofty::prelude::*;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("tone.wav");
        tone(&src);
        let track = TrackRef::file(chippy_core::Location::Fs(src));
        let meta = TrackMeta {
            title: "Tone".into(),
            artist: "Tester".into(),
            album: "Tests".into(),
            track_no: Some(3),
            ..Default::default()
        };
        for format in [Format::Flac, Format::Mp3, Format::Opus, Format::Wav] {
            let out = dir.path().join(format.extension());
            std::fs::create_dir(&out).unwrap();
            let (tx, rx) = crossbeam_channel::unbounded();
            let job = Job {
                tracks: vec![(track.clone(), meta.clone())],
                folder: out.clone(),
                format,
                kbps: 160,
                opts: DecodeOptions::default(),
            };
            run(job, &AtomicBool::new(false), &tx);
            let finished = rx.try_iter().find_map(|p| match p {
                Progress::Finished {
                    written, failed, ..
                } => Some((written, failed)),
                _ => None,
            });
            assert_eq!(finished, Some((1, vec![])), "{format:?}");
            let file = out.join(format!("01 Tone.{}", format.extension()));
            let tagged = lofty::read_from_path(&file).unwrap();
            let secs = tagged.properties().duration().as_secs_f32();
            assert!((secs - 2.0).abs() < 0.1, "{format:?}: {secs} s");
            let tag = tagged
                .primary_tag()
                .or(tagged.first_tag())
                .unwrap_or_else(|| {
                    panic!(
                        "{format:?}: no tags, {:?}",
                        tagged
                            .tags()
                            .iter()
                            .map(|t| t.tag_type())
                            .collect::<Vec<_>>()
                    )
                });
            assert_eq!(tag.title().as_deref(), Some("Tone"), "{format:?}");
            assert_eq!(tag.artist().as_deref(), Some("Tester"), "{format:?}");
            assert_eq!(tag.track(), Some(3), "{format:?}");
        }
    }

    #[test]
    fn names_are_safe_and_unique() {
        let mut used = Default::default();
        let m = TrackMeta {
            title: "A/B: C?".into(),
            ..Default::default()
        };
        assert_eq!(
            file_name(0, 2, &m, Format::Flac, &mut used),
            "01 A_B_ C_.flac"
        );
        let m = TrackMeta {
            title: "a_b_ c_".into(),
            ..Default::default()
        };
        assert_eq!(
            file_name(0, 2, &m, Format::Flac, &mut used),
            "01 a_b_ c_ (2).flac"
        );
    }
}

#[cfg(test)]
mod midi_tests {
    use super::*;

    #[test]
    fn exports_the_notes_of_a_chiptune() {
        let dir = tempfile::tempdir().unwrap();
        let nsf =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/game-music-emu/test.nsf");
        let track = TrackRef::file(chippy_core::Location::Fs(nsf));
        let meta = TrackMeta {
            title: "Test".into(),
            length_ms: Some(10_000),
            ..Default::default()
        };
        let out = dir.path().join("test.mid");
        write_midi(&track, &meta, &out).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"MThd"));
        // The tempo track and the two pulse channels.
        assert_eq!(u16::from_be_bytes([bytes[10], bytes[11]]), 3);

        let wav = dir.path().join("tone.wav");
        std::fs::write(&wav, b"RIFF").unwrap();
        let err = write_midi(
            &TrackRef::file(chippy_core::Location::Fs(wav)),
            &meta,
            &dir.path().join("tone.mid"),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("notes"), "{err}");
    }
}
