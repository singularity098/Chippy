//! The player thread: opens tracks, decodes ahead of the output, and
//! resamples to the device's rate.

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::audio::Shared;
use chippy_core::{open_decoder, DecodeOptions, Decoder, TrackRef};

pub enum Cmd {
    Load {
        track: TrackRef,
        generation: u64,
        start_ms: u64,
    },
    Seek {
        ms: u64,
    },
    Stop,
    Options(DecodeOptions),
}

pub enum Event {
    Started {
        generation: u64,
        duration_ms: Option<u64>,
    },
    Failed {
        generation: u64,
        error: String,
    },
}

pub struct Player {
    tx: Sender<Cmd>,
    pub events: Receiver<Event>,
}

impl Player {
    pub fn spawn(shared: Arc<Mutex<Shared>>, rate: u32, opts: DecodeOptions) -> Player {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (etx, events) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("player".into())
            .spawn(move || run(rx, etx, shared, rate, opts))
            .expect("thread");
        Player { tx, events }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

/// Decode this far ahead of the output.
const AHEAD_MS: u64 = 400;
const CHUNK: usize = 1024;

struct Playing {
    dec: Box<dyn Decoder>,
    resampler: Option<Resample>,
    done: bool,
}

fn run(
    rx: Receiver<Cmd>,
    events: Sender<Event>,
    shared: Arc<Mutex<Shared>>,
    rate: u32,
    mut opts: DecodeOptions,
) {
    let mut playing: Option<Playing> = None;
    let mut buf = vec![0f32; CHUNK * 2];
    let mut out = Vec::with_capacity(CHUNK * 4);
    let ahead = (rate as u64 * AHEAD_MS / 1000) as usize;
    loop {
        let busy = playing.as_ref().is_some_and(|p| !p.done)
            && shared.lock().unwrap().buffered_frames() < ahead;
        let cmd = if busy {
            rx.try_recv().ok()
        } else {
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(c) => Some(c),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                Err(_) => return,
            }
        };
        match cmd {
            Some(Cmd::Load {
                track,
                generation,
                start_ms,
            }) => {
                playing = None;
                {
                    let mut s = shared.lock().unwrap();
                    s.reset(start_ms);
                    s.generation = generation;
                }
                match open(&track, &opts, rate, start_ms) {
                    Ok(p) => {
                        let _ = events.send(Event::Started {
                            generation,
                            duration_ms: p.dec.duration_ms(),
                        });
                        playing = Some(p);
                    }
                    Err(e) => {
                        log::warn!("cannot play {}: {e:#}", track.loc.vpath().display());
                        shared.lock().unwrap().producer_done = true;
                        let _ = events.send(Event::Failed {
                            generation,
                            error: format!("{e:#}"),
                        });
                    }
                }
            }
            Some(Cmd::Seek { ms }) => {
                if let Some(p) = &mut playing {
                    if let Err(e) = p.dec.seek(ms) {
                        log::warn!("seek failed: {e:#}");
                    }
                    if let Some(r) = &mut p.resampler {
                        r.reset();
                    }
                    p.done = false;
                    shared.lock().unwrap().reset(ms);
                }
            }
            Some(Cmd::Stop) => {
                playing = None;
                let mut s = shared.lock().unwrap();
                s.reset(0);
                s.producer_done = true;
            }
            Some(Cmd::Options(o)) => opts = o,
            None => {}
        }
        // Decode one chunk.
        let Some(p) = &mut playing else { continue };
        if p.done || shared.lock().unwrap().buffered_frames() >= ahead {
            continue;
        }
        out.clear();
        let n = match p.dec.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                log::warn!("decoding stopped: {e:#}");
                0
            }
        };
        match &mut p.resampler {
            Some(r) if n > 0 => r.push(&buf[..n * 2], &mut out),
            Some(r) => r.flush(&mut out),
            None => out.extend_from_slice(&buf[..n * 2]),
        }
        let mut s = shared.lock().unwrap();
        s.buf.extend(out.iter().copied());
        if n == 0 {
            p.done = true;
            s.producer_done = true;
        }
    }
}

fn open(track: &TrackRef, opts: &DecodeOptions, rate: u32, start_ms: u64) -> Result<Playing> {
    let mut dec = open_decoder(track, opts)?;
    if start_ms > 0 {
        dec.seek(start_ms)?;
    }
    let resampler = (dec.sample_rate() != rate)
        .then(|| Resample::new(dec.sample_rate(), rate))
        .transpose()?;
    Ok(Playing {
        dec,
        resampler,
        done: false,
    })
}

/// Streaming stereo resampler (windowed sinc).
struct Resample {
    r: SincFixedIn<f32>,
    input: [Vec<f32>; 2],
}

impl Resample {
    fn new(from: u32, to: u32) -> Result<Self> {
        let params = SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        };
        let r = SincFixedIn::new(to as f64 / from as f64, 1.0, params, CHUNK, 2)?;
        Ok(Resample {
            r,
            input: [Vec::with_capacity(CHUNK * 2), Vec::with_capacity(CHUNK * 2)],
        })
    }

    fn reset(&mut self) {
        self.r.reset();
        self.input[0].clear();
        self.input[1].clear();
    }

    fn push(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        for f in interleaved.chunks_exact(2) {
            self.input[0].push(f[0]);
            self.input[1].push(f[1]);
        }
        while self.input[0].len() >= self.r.input_frames_next() {
            let need = self.r.input_frames_next();
            let chunk = [&self.input[0][..need], &self.input[1][..need]];
            if let Ok(res) = self.r.process(&chunk, None) {
                for (l, r) in res[0].iter().zip(&res[1]) {
                    out.push(*l);
                    out.push(*r);
                }
            }
            self.input[0].drain(..need);
            self.input[1].drain(..need);
        }
    }

    /// Pushes out what is left (padded with silence).
    fn flush(&mut self, out: &mut Vec<f32>) {
        if self.input[0].is_empty() {
            return;
        }
        let real = self.input[0].len();
        let need = self.r.input_frames_next();
        let ratio = self.r.output_frames_next() as f64 / need as f64;
        self.input[0].resize(need, 0.0);
        self.input[1].resize(need, 0.0);
        let before = out.len();
        self.push(&[], out);
        let keep = before + 2 * ((real as f64 * ratio).ceil() as usize);
        out.truncate(keep.min(out.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_roughly_to_ratio() {
        let mut r = Resample::new(44_100, 48_000).unwrap();
        let mut out = Vec::new();
        let input = vec![0.25f32; 44_100 * 2];
        for c in input.chunks(CHUNK * 2) {
            r.push(c, &mut out);
        }
        r.flush(&mut out);
        let frames = out.len() / 2;
        assert!((frames as i64 - 48_000).abs() < 1200, "{frames} frames");
    }
}
