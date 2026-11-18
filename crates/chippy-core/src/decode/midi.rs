//! MIDI files played through a SoundFont (SF2) with RustySynth.
//!
//! The SoundFont is chosen for the whole program ([`set_soundfont`]) and
//! may change while a song plays: the playing decoder notices, builds a
//! synthesizer on the new font, brings it to the same state (programs,
//! controllers, held notes) and crossfades to it.

use anyhow::{bail, Context, Result};
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use super::{DecodeOptions, Decoder};
use crate::meta::TrackMeta;
use crate::vfs::Location;

// ------------------------------------------------------------ the SoundFont

struct Font {
    generation: u64,
    font: Option<Arc<SoundFont>>,
}

static FONT: RwLock<Font> = RwLock::new(Font {
    generation: 0,
    font: None,
});
static FONT_GENERATION: AtomicU64 = AtomicU64::new(0);
static FONT_LOADING: AtomicBool = AtomicBool::new(false);

/// Loads an SF2 file (this can take a moment for a large one).
pub fn load_soundfont(path: &std::path::Path) -> Result<Arc<SoundFont>> {
    let f = std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut r = std::io::BufReader::with_capacity(1 << 20, f);
    let font = SoundFont::new(&mut r)
        .map_err(|e| anyhow::anyhow!("{} is not a usable SoundFont: {e:?}", path.display()))?;
    Ok(Arc::new(font))
}

/// The SoundFont MIDI plays with from now on, including the song playing.
pub fn set_soundfont(font: Option<Arc<SoundFont>>) {
    let mut f = FONT.write().unwrap();
    f.generation += 1;
    f.font = font;
    FONT_GENERATION.store(f.generation, Ordering::Release);
    FONT_LOADING.store(false, Ordering::Release);
}

/// Says a SoundFont is being loaded: a MIDI file opened meanwhile waits
/// for it rather than failing.
pub fn soundfont_loading() {
    FONT_LOADING.store(true, Ordering::Release);
}

fn current_font() -> (u64, Option<Arc<SoundFont>>) {
    let f = FONT.read().unwrap();
    (f.generation, f.font.clone())
}

// ----------------------------------------------------------- the MIDI file

/// A channel message, at its time in seconds.
#[derive(Debug, Clone, Copy)]
struct Ev {
    time: f64,
    status: u8,
    d1: u8,
    d2: u8,
}

impl Ev {
    fn channel(&self) -> i32 {
        (self.status & 0x0f) as i32
    }
    fn command(&self) -> i32 {
        (self.status & 0xf0) as i32
    }
    fn is_note(&self) -> bool {
        matches!(self.status & 0xf0, 0x80 | 0x90)
    }
}

#[derive(Debug, Default)]
struct Song {
    events: Vec<Ev>,
    /// Seconds to the last event.
    length: f64,
    title: String,
    copyright: String,
    texts: Vec<String>,
}

fn varlen(d: &[u8], p: &mut usize) -> Result<u32> {
    let mut v = 0u32;
    for _ in 0..4 {
        let Some(&b) = d.get(*p) else {
            bail!("MIDI data ends early")
        };
        *p += 1;
        v = (v << 7) | (b & 0x7f) as u32;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    bail!("bad variable-length number")
}

fn text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        // Most older files are Latin-1 (or Shift-JIS, which we cannot tell).
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
    .trim_matches(|c: char| c.is_whitespace() || c == '\0')
    .to_string()
}

fn parse(data: &[u8]) -> Result<Song> {
    let mut d = data;
    // RIFF MIDI (.rmi): the SMF is the "data" chunk.
    if d.len() > 20 && &d[0..4] == b"RIFF" && &d[8..12] == b"RMID" {
        let mut p = 12;
        loop {
            if p + 8 > d.len() {
                bail!("no MIDI data in the RIFF file");
            }
            let len = u32::from_le_bytes(d[p + 4..p + 8].try_into().unwrap()) as usize;
            if &d[p..p + 4] == b"data" {
                d = &d[p + 8..(p + 8 + len).min(d.len())];
                break;
            }
            p += 8 + len + (len & 1);
        }
    }
    let start = d
        .windows(4)
        .take(4096)
        .position(|w| w == b"MThd")
        .context("not a MIDI file")?;
    let d = &d[start..];
    if d.len() < 14 {
        bail!("not a MIDI file");
    }
    let hlen = u32::from_be_bytes(d[4..8].try_into().unwrap()) as usize;
    let ntracks = u16::from_be_bytes([d[10], d[11]]) as usize;
    let division = i16::from_be_bytes([d[12], d[13]]);
    // (tick, order, event); tempo changes as (tick, order, µs per beat).
    let mut raw: Vec<(u64, usize, Ev)> = Vec::new();
    let mut tempos: Vec<(u64, u32)> = Vec::new();
    let mut song = Song::default();
    let mut p = 8 + hlen;
    let mut order = 0usize;
    for track_no in 0..ntracks {
        if p + 8 > d.len() {
            break;
        }
        let len = u32::from_be_bytes(d[p + 4..p + 8].try_into().unwrap()) as usize;
        let body_start = p + 8;
        let body_end = (body_start + len).min(d.len());
        let is_track = &d[p..p + 4] == b"MTrk";
        p = body_end;
        if !is_track {
            continue;
        }
        let t = &d[body_start..body_end];
        let mut q = 0;
        let mut tick = 0u64;
        let mut running = 0u8;
        while q < t.len() {
            let Ok(delta) = varlen(t, &mut q) else { break };
            tick += delta as u64;
            let Some(&b) = t.get(q) else { break };
            let status = if b & 0x80 != 0 {
                q += 1;
                b
            } else {
                running
            };
            match status {
                0xff => {
                    let Some(&kind) = t.get(q) else { break };
                    q += 1;
                    let Ok(l) = varlen(t, &mut q) else { break };
                    let l = l as usize;
                    let body = &t[q.min(t.len())..(q + l).min(t.len())];
                    q += l;
                    match kind {
                        0x51 if body.len() == 3 => {
                            let us =
                                (body[0] as u32) << 16 | (body[1] as u32) << 8 | body[2] as u32;
                            tempos.push((tick, us));
                        }
                        0x03 if track_no == 0 && song.title.is_empty() => song.title = text(body),
                        0x02 if song.copyright.is_empty() => song.copyright = text(body),
                        0x01 if song.texts.len() < 8 => {
                            let s = text(body);
                            // Karaoke lyrics are text events too: skip those.
                            if !s.is_empty() && !s.starts_with(['@', '/', '\\']) {
                                song.texts.push(s);
                            }
                        }
                        0x2f => break,
                        _ => {}
                    }
                }
                0xf0 | 0xf7 => {
                    let Ok(l) = varlen(t, &mut q) else { break };
                    q += l as usize;
                }
                0x80..=0xef => {
                    running = status;
                    let two = !matches!(status & 0xf0, 0xc0 | 0xd0);
                    let d1 = *t.get(q).unwrap_or(&0) & 0x7f;
                    let d2 = if two {
                        *t.get(q + 1).unwrap_or(&0) & 0x7f
                    } else {
                        0
                    };
                    q += if two { 2 } else { 1 };
                    raw.push((
                        tick,
                        order,
                        Ev {
                            time: 0.0,
                            status,
                            d1,
                            d2,
                        },
                    ));
                    order += 1;
                }
                // System common/real-time messages: no data we use.
                _ => {}
            }
        }
    }
    if ntracks == 0 {
        bail!("the MIDI file has no tracks");
    }
    raw.sort_by_key(|&(tick, order, _)| (tick, order));
    tempos.sort_by_key(|&(tick, _)| tick);
    // Ticks to seconds.
    let seconds_per_tick = |us_per_beat: u32| -> f64 {
        if division < 0 {
            // SMPTE: frames per second × ticks per frame.
            let fps = match -(division >> 8) {
                29 => 29.97,
                f => f as f64,
            };
            1.0 / (fps * (division & 0xff) as f64)
        } else {
            us_per_beat as f64 / 1e6 / division.max(1) as f64
        }
    };
    let mut ti = 0;
    let (mut seg_tick, mut seg_time, mut spt) = (0u64, 0f64, seconds_per_tick(500_000));
    for (tick, _, mut ev) in raw {
        while ti < tempos.len() && tempos[ti].0 <= tick {
            seg_time += (tempos[ti].0 - seg_tick) as f64 * spt;
            seg_tick = tempos[ti].0;
            spt = seconds_per_tick(tempos[ti].1);
            ti += 1;
        }
        ev.time = seg_time + (tick - seg_tick) as f64 * spt;
        song.events.push(ev);
    }
    song.length = song.events.last().map(|e| e.time).unwrap_or(0.0);
    Ok(song)
}

// ---------------------------------------------------------------- playing

/// Time after the last event for notes to ring out.
const TAIL_S: f64 = 1.5;
/// How long a SoundFont change crossfades.
const CROSSFADE_S: f64 = 0.25;

pub struct MidiDecoder {
    song: Arc<Song>,
    rate: u32,
    synth: Synthesizer,
    font_generation: u64,
    /// The synthesizer of the previous SoundFont, fading out, and how many
    /// frames of the fade are left.
    old: Option<(Synthesizer, u32)>,
    fade_frames: u32,
    /// The next event to play.
    next: usize,
    frame: u64,
    end_frame: u64,
    /// Held notes: velocity per channel and key (0 when up).
    held: Box<[[u8; 128]; 16]>,
    left: Vec<f32>,
    right: Vec<f32>,
    old_left: Vec<f32>,
    old_right: Vec<f32>,
}

fn new_synth(font: &Arc<SoundFont>, rate: u32) -> Result<Synthesizer> {
    let mut s = SynthesizerSettings::new(rate as i32);
    s.maximum_polyphony = 128;
    Synthesizer::new(font, &s).map_err(|e| anyhow::anyhow!("cannot start the synthesizer: {e:?}"))
}

fn wait_for_font() -> (u64, Option<Arc<SoundFont>>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let f = current_font();
        if f.1.is_some() || !FONT_LOADING.load(Ordering::Acquire) || Instant::now() > deadline {
            return f;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn send(s: &mut Synthesizer, e: &Ev) {
    s.process_midi_message(e.channel(), e.command(), e.d1 as i32, e.d2 as i32);
}

impl MidiDecoder {
    pub fn open(loc: &Location, opts: &DecodeOptions) -> Result<Self> {
        let song = Arc::new(parse(&loc.read()?)?);
        let (generation, font) = wait_for_font();
        let Some(font) = font else {
            bail!("no SoundFont: choose a folder of SF2 files in Settings, under MIDI");
        };
        let rate = opts.sample_rate;
        let end_frame = ((song.length + TAIL_S) * rate as f64) as u64;
        let block = 64;
        Ok(MidiDecoder {
            synth: new_synth(&font, rate)?,
            font_generation: generation,
            old: None,
            fade_frames: (CROSSFADE_S * rate as f64) as u32,
            song,
            rate,
            next: 0,
            frame: 0,
            end_frame,
            held: Box::new([[0; 128]; 16]),
            left: vec![0.0; block],
            right: vec![0.0; block],
            old_left: vec![0.0; block],
            old_right: vec![0.0; block],
        })
    }

    fn event_frame(&self, e: &Ev) -> u64 {
        (e.time * self.rate as f64).round() as u64
    }

    /// Brings `synth` to the state of the song just before event `upto`:
    /// programs, controllers and pitch bends, without the notes.
    fn replay_state(song: &Song, synth: &mut Synthesizer, upto: usize) {
        for e in &song.events[..upto] {
            if !e.is_note() {
                send(synth, e);
            }
        }
    }

    fn sound_held(held: &[[u8; 128]; 16], synth: &mut Synthesizer) {
        for (ch, keys) in held.iter().enumerate() {
            for (key, &vel) in keys.iter().enumerate() {
                // Drums would come in again as fresh hits: leave them.
                if vel > 0 && ch != 9 {
                    synth.note_on(ch as i32, key as i32, vel as i32);
                }
            }
        }
    }

    /// Moves to the SoundFont chosen since, crossfading from the old one.
    fn switch_font(&mut self) {
        let (generation, font) = current_font();
        self.font_generation = generation;
        let Some(font) = font else { return };
        let Ok(mut synth) = new_synth(&font, self.rate) else {
            return;
        };
        Self::replay_state(&self.song, &mut synth, self.next);
        Self::sound_held(&self.held, &mut synth);
        let old = std::mem::replace(&mut self.synth, synth);
        self.old = Some((old, self.fade_frames));
    }

    fn play_event(&mut self, i: usize) {
        let e = self.song.events[i];
        match e.status & 0xf0 {
            0x90 if e.d2 > 0 => self.held[e.channel() as usize][e.d1 as usize] = e.d2,
            0x80 | 0x90 => self.held[e.channel() as usize][e.d1 as usize] = 0,
            0xb0 if e.d1 == 120 || e.d1 == 123 => self.held[e.channel() as usize] = [0; 128],
            _ => {}
        }
        send(&mut self.synth, &e);
        if let Some((old, _)) = &mut self.old {
            send(old, &e);
        }
    }

    /// Renders `n` frames (at most a block) into `out`.
    fn render(&mut self, out: &mut [f32], n: usize) {
        let (l, r) = (&mut self.left[..n], &mut self.right[..n]);
        self.synth.render(l, r);
        match &mut self.old {
            None => {
                for (i, f) in out.chunks_exact_mut(2).take(n).enumerate() {
                    f[0] = l[i];
                    f[1] = r[i];
                }
            }
            Some((old, left)) => {
                let (ol, or) = (&mut self.old_left[..n], &mut self.old_right[..n]);
                old.render(ol, or);
                let total = self.fade_frames.max(1) as f32;
                for (i, f) in out.chunks_exact_mut(2).take(n).enumerate() {
                    let g = (left.saturating_sub(i as u32)) as f32 / total;
                    // Equal-power crossfade.
                    let (go, gn) = (
                        (g * std::f32::consts::FRAC_PI_2).sin(),
                        (g * std::f32::consts::FRAC_PI_2).cos(),
                    );
                    f[0] = l[i] * gn + ol[i] * go;
                    f[1] = r[i] * gn + or[i] * go;
                }
                *left = left.saturating_sub(n as u32);
                if *left == 0 {
                    self.old = None;
                }
            }
        }
    }
}

impl Decoder for MidiDecoder {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn duration_ms(&self) -> Option<u64> {
        Some(self.end_frame * 1000 / self.rate as u64)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        if FONT_GENERATION.load(Ordering::Acquire) != self.font_generation {
            self.switch_font();
        }
        let want = (out.len() / 2).min((self.end_frame.saturating_sub(self.frame)) as usize);
        let mut done = 0;
        while done < want {
            while self.next < self.song.events.len()
                && self.event_frame(&self.song.events[self.next]) <= self.frame
            {
                self.play_event(self.next);
                self.next += 1;
            }
            let until_event = self
                .song
                .events
                .get(self.next)
                .map(|e| self.event_frame(e) - self.frame)
                .unwrap_or(u64::MAX);
            let n = (want - done)
                .min(self.left.len())
                .min(until_event.max(1) as usize);
            self.render(&mut out[done * 2..], n);
            done += n;
            self.frame += n as u64;
        }
        Ok(done)
    }

    fn seek(&mut self, ms: u64) -> Result<()> {
        let target = ms * self.rate as u64 / 1000;
        self.synth.reset();
        self.old = None;
        *self.held = [[0; 128]; 16];
        let upto = self
            .song
            .events
            .partition_point(|e| self.event_frame(e) < target);
        Self::replay_state(&self.song, &mut self.synth, upto);
        // Notes held across the new place sound again.
        for i in 0..upto {
            let e = self.song.events[i];
            match e.status & 0xf0 {
                0x90 if e.d2 > 0 => self.held[e.channel() as usize][e.d1 as usize] = e.d2,
                0x80 | 0x90 => self.held[e.channel() as usize][e.d1 as usize] = 0,
                _ => {}
            }
        }
        Self::sound_held(&self.held, &mut self.synth);
        self.next = upto;
        self.frame = target.min(self.end_frame);
        Ok(())
    }
}

pub fn meta(loc: &Location) -> Result<TrackMeta> {
    let song = parse(&loc.read()?)?;
    let mut m = TrackMeta {
        length_ms: Some(((song.length + TAIL_S) * 1000.0) as u64),
        format: "MIDI".into(),
        channels: Some(2),
        copyright: song.copyright.clone(),
        comment: song.texts.join("\n"),
        ..Default::default()
    };
    m.title = song.title;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-track file: tempo 120 bpm, name, two quarter notes.
    pub fn tiny_midi() -> Vec<u8> {
        let mut trk = Vec::new();
        trk.extend([0x00, 0xff, 0x03, 4]);
        trk.extend(b"Tune");
        trk.extend([0x00, 0xff, 0x51, 3, 0x07, 0xa1, 0x20]); // 500000 µs
        trk.extend([0x00, 0xc0, 0]); // program 0
        trk.extend([0x00, 0x90, 60, 100]);
        trk.extend([0x60, 0x80, 60, 0]); // 96 ticks later
        trk.extend([0x00, 0x90, 64, 100]);
        trk.extend([0x60, 64, 0]); // running status, velocity 0
        trk.extend([0x00, 0xff, 0x2f, 0]);
        let mut f = b"MThd".to_vec();
        f.extend(6u32.to_be_bytes());
        f.extend(0u16.to_be_bytes());
        f.extend(1u16.to_be_bytes());
        f.extend(96u16.to_be_bytes());
        f.extend(b"MTrk");
        f.extend((trk.len() as u32).to_be_bytes());
        f.extend(trk);
        f
    }

    #[test]
    fn parses_events_and_tempo() {
        let s = parse(&tiny_midi()).unwrap();
        assert_eq!(s.title, "Tune");
        assert_eq!(s.events.len(), 5);
        assert!((s.length - 1.0).abs() < 1e-9, "{}", s.length);
        assert!((s.events[2].time - 0.5).abs() < 1e-9);
        // Running status kept the note-on command.
        assert_eq!(s.events[4].status, 0x90);
        assert_eq!(s.events[4].d2, 0);
    }

    #[test]
    fn reads_rmi() {
        let smf = tiny_midi();
        let mut f = b"RIFF".to_vec();
        f.extend(((smf.len() + 12) as u32).to_le_bytes());
        f.extend(b"RMIDdata");
        f.extend((smf.len() as u32).to_le_bytes());
        f.extend(&smf);
        assert_eq!(parse(&f).unwrap().events.len(), 5);
    }

    #[test]
    fn plays_and_switches_soundfont() {
        let sf = std::path::Path::new("/usr/share/sounds/sf2/TimGM6mb.sf2");
        if !sf.exists() {
            eprintln!("no test SoundFont; skipped");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.mid");
        std::fs::write(&p, tiny_midi()).unwrap();
        set_soundfont(Some(load_soundfont(sf).unwrap()));
        let loc = Location::Fs(p);
        let mut d = MidiDecoder::open(&loc, &DecodeOptions::default()).unwrap();
        assert_eq!(d.duration_ms(), Some(2500));
        let mut buf = vec![0f32; 4800];
        let energy = |b: &[f32]| b.iter().map(|s| s * s).sum::<f32>();
        assert_eq!(d.read(&mut buf).unwrap(), 2400);
        assert!(energy(&buf) > 0.0, "silence");
        // A new font while the note sounds: the crossfade keeps it going.
        set_soundfont(Some(load_soundfont(sf).unwrap()));
        let mut total = 0;
        loop {
            let n = d.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            assert!(buf[..n * 2].iter().all(|s| s.is_finite()));
            total += n;
        }
        assert_eq!(total, 120_000 - 2400);
        d.seek(600).unwrap();
        assert_eq!(d.read(&mut buf).unwrap(), 2400);
        assert!(energy(&buf) > 0.0, "silence after seeking into a note");
    }
}
