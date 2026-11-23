//! MIDI mode: a chiptune whose notes can be found ([`notes::transcribe`])
//! plays as MIDI through the SoundFont, in a [`Style`] of instruments, and
//! back. The chip keeps playing underneath, so turning the mode on or off,
//! or changing the style, crossfades at once, like changing SoundFonts.
//!
//! The mode is one switch for the whole program ([`set`]): it stays on from
//! track to track, and tracks whose notes can't be found play as they are.
//! The notes are worked out in the background when the mode is first
//! wanted for a track (the chip plays meanwhile), and kept ([`cached`]) for
//! the guitar tab too.

use chippy_core::decode::midi::{self, MidiDecoder};
use chippy_core::notes::{self, style::Style, Transcription};
use chippy_core::{formats, Decoder, TrackRef};
use crossbeam_channel::Receiver;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

// ------------------------------------------------------------- the switch

/// 0 off, else the style's index + 1.
static MODE: AtomicU8 = AtomicU8::new(0);
static STATUS: AtomicU8 = AtomicU8::new(Status::Off as u8);
static ERROR: Mutex<String> = Mutex::new(String::new());

/// Plays tracks as MIDI in `style` from now on (including the one
/// playing), or as they are (None).
pub fn set(style: Option<Style>) {
    MODE.store(style.map_or(0, |s| s.index() as u8 + 1), Ordering::Release);
}

pub fn get() -> Option<Style> {
    match MODE.load(Ordering::Acquire) {
        0 => None,
        i => Style::from_index(i as usize - 1),
    }
}

/// What the playing track does about MIDI mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    /// Playing as it is (the mode is off, or nothing plays).
    Off,
    /// Its notes can't be found: it plays as it is.
    Unsupported,
    /// Working out the notes; the chip plays meanwhile.
    Preparing,
    /// Playing (or fading into) MIDI.
    Playing,
    /// The notes or the synthesizer failed: see [`error`].
    Failed,
}

pub fn status() -> Status {
    match STATUS.load(Ordering::Acquire) {
        1 => Status::Unsupported,
        2 => Status::Preparing,
        3 => Status::Playing,
        4 => Status::Failed,
        _ => Status::Off,
    }
}

fn set_status(s: Status) {
    STATUS.store(s as u8, Ordering::Release);
}

/// Why the mode failed for the playing track.
pub fn error() -> String {
    ERROR.lock().unwrap().clone()
}

/// Whether a track's notes can be played as MIDI (MIDI files and tab files
/// themselves already are).
pub fn possible(track: &TrackRef) -> bool {
    let name = track.loc.name();
    notes::supported(&name) && !formats::plays_through_soundfont(&name)
}

/// Called by the player for each track it opens (and with None when it
/// stops): wraps decoders whose notes can be played as MIDI.
pub fn wrap(dec: Box<dyn Decoder>, track: &TrackRef, fade_ms: u64) -> Box<dyn Decoder> {
    if !possible(track) {
        set_status(if get().is_some() {
            Status::Unsupported
        } else {
            Status::Off
        });
        return dec;
    }
    set_status(Status::Off);
    Box::new(Mixer::new(dec, track.clone(), fade_ms))
}

pub fn stopped() {
    set_status(Status::Off);
}

// -------------------------------------------------------- the notes, kept

type Found = Result<Arc<Transcription>, String>;
type Cell = Arc<OnceLock<Found>>;

/// The last few tracks' notes: (track, over how long, the notes).
static CACHE: Mutex<Vec<(TrackRef, Option<u64>, Cell)>> = Mutex::new(Vec::new());
const KEEP: usize = 4;
/// Notes worked out over a length this close to the one asked for will do.
const SLACK_MS: u64 = 1500;

/// `track`'s notes over its first `length_ms`, worked out on this thread
/// unless they already have been (or are being, elsewhere: then this
/// waits for them).
pub fn cached(track: &TrackRef, length_ms: Option<u64>) -> Found {
    let cell = {
        let mut cache = CACHE.lock().unwrap();
        let covers = |had: Option<u64>| match (had, length_ms) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(h), Some(want)) => h + SLACK_MS >= want,
        };
        match cache.iter().position(|(t, l, _)| t == track && covers(*l)) {
            Some(i) => {
                let e = cache.remove(i);
                let cell = e.2.clone();
                cache.push(e);
                cell
            }
            None => {
                let cell: Cell = Arc::default();
                cache.retain(|(t, _, _)| t != track);
                cache.push((track.clone(), length_ms, cell.clone()));
                if cache.len() > KEEP {
                    cache.remove(0);
                }
                cell
            }
        }
    };
    cell.get_or_init(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            notes::transcribe(&track.loc, track.sub, length_ms)
                .map(Arc::new)
                .map_err(|e| format!("{e:#}"))
        }))
        .unwrap_or_else(|_| Err("finding the notes failed".into()))
    })
    .clone()
}

// ------------------------------------------------------------- the mixer

/// How long the mode, or a change of style, crossfades.
const CROSSFADE_S: f64 = 0.6;
/// How quickly the MIDI's loudness follows the chip's (seconds).
const LEVEL_S: f64 = 3.0;

enum NotesState {
    Idle,
    Pending(Receiver<Found>),
    Ready(Arc<Transcription>),
    Failed(String),
}

/// The synthesizer of one style.
struct Synth {
    style: Style,
    dec: MidiDecoder,
    /// It has played to its end (silence from there).
    ended: bool,
}

impl Synth {
    fn render(&mut self, out: &mut [f32], frames: usize) {
        let mut done = 0;
        while !self.ended && done < frames {
            match self.dec.read(&mut out[done * 2..frames * 2]) {
                Ok(0) | Err(_) => self.ended = true,
                Ok(n) => done += n,
            }
        }
        out[done * 2..frames * 2].fill(0.0);
    }

    fn seek(&mut self, ms: u64) {
        self.ended = self.dec.seek(ms).is_err();
    }
}

pub struct Mixer {
    chip: Box<dyn Decoder>,
    track: TrackRef,
    rate: u32,
    length_ms: Option<u64>,
    /// The fade at the end of looping tracks (ms).
    fade_ms: u64,
    /// Frames played from the start of the track.
    frame: u64,
    notes: NotesState,
    /// The style asked for last, and why its synthesizer can't start (no
    /// SoundFont, say): tried again when the style changes.
    want: Option<Style>,
    synth_error: Option<String>,
    /// The SoundFont generation `synth_error` happened with.
    font_generation: u64,
    synth: Option<Synth>,
    /// The previous style, fading out: frames of the fade left.
    old: Option<(Synth, u32)>,
    /// 0 the chip .. 1 the MIDI.
    mix: f32,
    fade_frames: u32,
    /// Running energy of the chip and the MIDI, and the gain bringing the
    /// MIDI to the chip's loudness.
    chip_energy: f64,
    midi_energy: f64,
    measured: u64,
    gain: f32,
    buf: Vec<f32>,
    old_buf: Vec<f32>,
    /// MIDI channels silenced (see [`Decoder::set_muted`]).
    muted: u16,
}

impl Mixer {
    fn new(chip: Box<dyn Decoder>, track: TrackRef, fade_ms: u64) -> Mixer {
        let rate = chip.sample_rate();
        Mixer {
            length_ms: chip.duration_ms(),
            chip,
            track,
            rate,
            fade_ms,
            frame: 0,
            notes: NotesState::Idle,
            want: None,
            synth_error: None,
            font_generation: 0,
            synth: None,
            old: None,
            mix: 0.0,
            fade_frames: (CROSSFADE_S * rate as f64) as u32,
            chip_energy: 0.0,
            midi_energy: 0.0,
            measured: 0,
            gain: 1.0,
            buf: Vec::new(),
            old_buf: Vec::new(),
            muted: 0,
        }
    }

    fn ms(&self) -> u64 {
        self.frame * 1000 / self.rate.max(1) as u64
    }

    /// Moves the notes along: starts working them out, or takes them in.
    fn find_notes(&mut self) {
        match &self.notes {
            NotesState::Idle => {
                let (tx, rx) = crossbeam_channel::bounded(1);
                let (track, length) = (self.track.clone(), self.length_ms);
                let spawned =
                    std::thread::Builder::new()
                        .name("midi-mode".into())
                        .spawn(move || {
                            crate::background_thread();
                            let _ = tx.send(cached(&track, length));
                        });
                self.notes = match spawned {
                    Ok(_) => NotesState::Pending(rx),
                    Err(e) => NotesState::Failed(e.to_string()),
                };
            }
            NotesState::Pending(rx) => match rx.try_recv() {
                Ok(Ok(t)) if t.parts.iter().any(|p| !p.notes.is_empty()) => {
                    self.notes = NotesState::Ready(t)
                }
                Ok(Ok(_)) => self.notes = NotesState::Failed("no notes were found".into()),
                Ok(Err(e)) => self.notes = NotesState::Failed(e),
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(_) => self.notes = NotesState::Failed("finding the notes failed".into()),
            },
            _ => {}
        }
    }

    /// Brings the synthesizer to `style` (crossfading from the one before).
    fn restyle(&mut self, style: Style) {
        let NotesState::Ready(t) = &self.notes else {
            return;
        };
        let (generation, loading) = midi::soundfont_state();
        if loading {
            // Wait for it (the chip plays on meanwhile).
            return;
        }
        let styled = notes::style::restyle(t, style);
        let dec = match MidiDecoder::from_notes(&styled, self.rate) {
            Ok(d) => d,
            Err(e) => {
                self.synth_error = Some(format!("{e:#}"));
                self.font_generation = generation;
                return;
            }
        };
        let mut synth = Synth {
            style,
            dec,
            ended: false,
        };
        synth.dec.set_muted(self.muted);
        synth.seek(self.ms());
        if let Some(prev) = self.synth.replace(synth) {
            if self.mix > 0.0 {
                self.old = Some((prev, self.fade_frames));
            }
        }
    }

    /// Lines the MIDI up with the chip after a seek.
    fn sought(&mut self, ms: u64) {
        self.frame = ms * self.rate as u64 / 1000;
        self.old = None;
        if let Some(s) = &mut self.synth {
            s.seek(ms);
        }
    }

    fn update_status(&self, want: Option<Style>) {
        let failed = match &self.notes {
            NotesState::Failed(e) => Some(e),
            _ => self.synth_error.as_ref(),
        };
        let s = match (&want, failed) {
            (None, _) => Status::Off,
            (Some(_), Some(e)) => {
                let mut err = ERROR.lock().unwrap();
                if *err != *e {
                    *err = e.clone();
                }
                Status::Failed
            }
            (Some(_), None) if self.synth.is_some() => Status::Playing,
            _ => Status::Preparing,
        };
        set_status(s);
    }

    /// Mixes `n` frames of the MIDI into the chip's sound in `out`.
    fn mix_in(&mut self, out: &mut [f32], n: usize, target: f32) {
        self.buf.resize(n * 2, 0.0);
        let mut buf = std::mem::take(&mut self.buf);
        match &mut self.synth {
            Some(s) => s.render(&mut buf, n),
            None => buf.fill(0.0),
        }
        // A change of style: equal-power crossfade from the old one.
        if let Some((old, left)) = &mut self.old {
            self.old_buf.resize(n * 2, 0.0);
            old.render(&mut self.old_buf, n);
            let total = self.fade_frames.max(1) as f32;
            for (i, (f, o)) in buf
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(self.old_buf.as_chunks::<2>().0)
                .enumerate()
            {
                let g = left.saturating_sub(i as u32) as f32 / total;
                let (go, gn) = (
                    (g * std::f32::consts::FRAC_PI_2).sin(),
                    (g * std::f32::consts::FRAC_PI_2).cos(),
                );
                f[0] = f[0] * gn + o[0] * go;
                f[1] = f[1] * gn + o[1] * go;
            }
            *left = left.saturating_sub(n as u32);
            if *left == 0 {
                self.old = None;
            }
        }
        // Follow the chip's loudness.
        let energy = |b: &[f32]| b.iter().map(|&x| (x * x) as f64).sum::<f64>();
        let decay = (-(n as f64) / (LEVEL_S * self.rate as f64)).exp();
        self.chip_energy = self.chip_energy * decay + energy(&out[..n * 2]);
        self.midi_energy = self.midi_energy * decay + energy(&buf);
        self.measured += n as u64;
        if self.measured > self.rate as u64 / 2 && self.midi_energy > 1e-6 {
            let want = ((self.chip_energy / self.midi_energy).sqrt() as f32).clamp(0.4, 4.0);
            self.gain += (want - self.gain) * (n as f32 / self.rate as f32).min(1.0);
        }
        // The fade at the end of a looping track, as the chip does it.
        let end_fade = |frame: u64| -> f32 {
            let Some(len) = self.length_ms else {
                return 1.0;
            };
            let ms = frame * 1000 / self.rate as u64;
            let left = len.saturating_sub(ms);
            (left as f32 / self.fade_ms.max(1) as f32).min(1.0)
        };
        let step = 1.0 / self.fade_frames.max(1) as f32;
        for (i, (f, m)) in out[..n * 2]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(buf.as_chunks::<2>().0)
            .enumerate()
        {
            if self.mix < target {
                self.mix = (self.mix + step).min(target);
            } else if self.mix > target {
                self.mix = (self.mix - step).max(target);
            }
            let a = self.mix * std::f32::consts::FRAC_PI_2;
            let (gc, gm) = (
                a.cos(),
                a.sin() * self.gain * end_fade(self.frame + i as u64),
            );
            f[0] = f[0] * gc + m[0] * gm;
            f[1] = f[1] * gc + m[1] * gm;
        }
        self.buf = buf;
    }
}

impl Decoder for Mixer {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn duration_ms(&self) -> Option<u64> {
        self.chip.duration_ms()
    }

    fn read(&mut self, out: &mut [f32]) -> anyhow::Result<usize> {
        let want = get();
        if want != self.want
            || (self.synth_error.is_some() && midi::soundfont_state().0 != self.font_generation)
        {
            // Another style or SoundFont: worth another try.
            self.want = want;
            self.synth_error = None;
        }
        if want.is_some() {
            self.find_notes();
        }
        if let Some(style) = want.filter(|_| self.synth_error.is_none()) {
            if self.synth.as_ref().map(|s| s.style) != Some(style) {
                self.restyle(style);
            }
        }
        self.update_status(want);
        let target = if want.is_some() && self.synth.is_some() {
            1.0
        } else {
            0.0
        };
        let n = self.chip.read(out)?;
        if n > 0 && (self.mix > 0.0 || target > 0.0) {
            self.mix_in(out, n, target);
        } else if self.mix == 0.0 && want.is_none() {
            // Off and faded out: the synthesizer goes (it would fall
            // behind the chip); a new one starts in the right place.
            self.synth = None;
            self.old = None;
        }
        self.frame += n as u64;
        Ok(n)
    }

    fn seek(&mut self, ms: u64) -> anyhow::Result<()> {
        self.chip.seek(ms)?;
        self.sought(ms);
        Ok(())
    }

    fn seek_step(&mut self, ms: u64) -> anyhow::Result<bool> {
        let done = self.chip.seek_step(ms)?;
        if done {
            self.sought(ms);
        }
        Ok(done)
    }

    fn set_muted(&mut self, channels: u16) {
        self.muted = channels;
        self.chip.set_muted(channels);
        if let Some(s) = &mut self.synth {
            s.dec.set_muted(channels);
        }
        if let Some((s, _)) = &mut self.old {
            s.dec.set_muted(channels);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        set(Some(Style::Piano));
        assert_eq!(get(), Some(Style::Piano));
        set(None);
        assert_eq!(get(), None);
    }
}
