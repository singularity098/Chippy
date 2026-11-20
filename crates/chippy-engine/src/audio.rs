//! Sound output: a cpal stream (or, without a sound device, a clock that
//! consumes samples in real time) playing from a buffer the player thread
//! fills. The callback also keeps the last few thousand samples for the
//! spectrum.
//!
//! The callback never waits for the lock: it takes a little more than it
//! needs whenever it gets the lock, so when another thread happens to hold
//! it (say, one paused by the system mid-update while the app is busy
//! starting up) it plays from what it has in hand instead of a click of
//! silence.
//!
//! Peaks above full scale (loud tracks at full volume, and the overshoot
//! resampling adds to square waves) are rounded off rather than clipped:
//! clipped, each one is a crackle.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub const SCOPE_LEN: usize = 4096;

pub struct Shared {
    /// Interleaved stereo samples waiting to play.
    pub buf: VecDeque<f32>,
    /// Frames played since the track started (or was last sought).
    pub frames_played: u64,
    /// Where in the track `frames_played` counts from (ms).
    pub base_ms: u64,
    /// The decoder has delivered its last sample.
    pub producer_done: bool,
    pub paused: bool,
    /// 0..1, as set by the slider.
    pub volume: f32,
    /// Bumped for every track loaded, so stale ends are ignored.
    pub generation: u64,
    /// Ring of the latest mono samples (before volume) for the spectrum.
    pub scope: Vec<f32>,
    pub scope_pos: usize,
    /// Times the output ran out of samples mid-track (the decoder fell
    /// behind), each one heard as a click.
    pub underruns: u64,
    /// Bumped by every reset, so the output drops what it holds from before.
    epoch: u64,
    /// Frames taken from `buf` that the output holds but has not played.
    held: usize,
}

impl Shared {
    fn new() -> Self {
        Shared {
            buf: VecDeque::with_capacity(48_000),
            frames_played: 0,
            base_ms: 0,
            producer_done: true,
            paused: false,
            volume: 0.8,
            generation: 0,
            scope: vec![0.0; SCOPE_LEN],
            scope_pos: 0,
            underruns: 0,
            epoch: 0,
            held: 0,
        }
    }

    /// Frames still to play, including those the output holds.
    pub fn buffered_frames(&self) -> usize {
        self.buf.len() / 2 + self.held
    }

    /// Forgets queued sound and restarts the clock at `ms`.
    pub fn reset(&mut self, ms: u64) {
        self.buf.clear();
        self.held = 0;
        self.epoch += 1;
        self.frames_played = 0;
        self.base_ms = ms;
        self.producer_done = false;
    }

    /// The latest `n` scope samples, oldest first.
    pub fn scope_tail(&self, n: usize) -> Vec<f32> {
        let n = n.min(SCOPE_LEN);
        (0..n)
            .map(|i| self.scope[(self.scope_pos + SCOPE_LEN - n + i) % SCOPE_LEN])
            .collect()
    }
}

/// The output's side: samples in hand and the state they play with, so a
/// callback that cannot get the lock still has something to play.
struct Feed {
    /// Interleaved stereo samples taken from `Shared::buf`.
    hand: VecDeque<f32>,
    /// Frames kept in hand beyond what a callback needs.
    reserve: usize,
    epoch: u64,
    gain: f32,
    target: f32,
    paused: bool,
    /// Frames played while the lock was busy, not yet counted.
    unreported: u64,
    /// Sound has come since the last reset (before, an empty buffer is the
    /// decoder starting, not running late).
    flowing: bool,
}

impl Feed {
    fn new(rate: u32) -> Self {
        Feed {
            hand: VecDeque::new(),
            // 50 ms: longer than any lock is held, even by a thread the
            // system has paused for a moment.
            reserve: rate as usize / 20,
            epoch: 0,
            gain: 0.0,
            target: 0.0,
            paused: false,
            unreported: 0,
            flowing: false,
        }
    }

    /// Fills `out` (stereo), returns frames played.
    fn fill(&mut self, shared: &Mutex<Shared>, out: &mut [f32]) -> usize {
        let frames = out.len() / 2;
        let Ok(mut s) = shared.try_lock() else {
            let n = self.play(out, None);
            self.unreported += n as u64;
            return n;
        };
        if s.epoch != self.epoch {
            // Seek, stop or a new track: what is in hand is stale.
            self.epoch = s.epoch;
            self.hand.clear();
            self.unreported = 0;
            self.flowing = false;
        }
        s.frames_played += std::mem::take(&mut self.unreported);
        self.paused = s.paused;
        self.target = s.volume * s.volume;
        let want = (frames + self.reserve) * 2;
        if self.hand.len() < want {
            let n = (want - self.hand.len()).min(s.buf.len() & !1);
            self.hand.extend(s.buf.drain(..n));
        }
        let n = self.play(out, Some(&mut s));
        if n > 0 {
            self.flowing = true;
        }
        if self.flowing && n < frames && !self.paused && !s.producer_done {
            s.underruns += 1;
        }
        s.frames_played += n as u64;
        s.held = self.hand.len() / 2;
        n
    }

    fn play(&mut self, out: &mut [f32], mut s: Option<&mut Shared>) -> usize {
        if self.paused {
            out.fill(0.0);
            return 0;
        }
        let mut taken = 0;
        for f in out.as_chunks_mut::<2>().0 {
            let (Some(l), Some(r)) = (self.hand.pop_front(), self.hand.pop_front()) else {
                f.fill(0.0);
                continue;
            };
            taken += 1;
            self.gain += (self.target - self.gain) * 0.002;
            if let Some(s) = s.as_deref_mut() {
                s.scope[s.scope_pos] = (l + r) * 0.5;
                s.scope_pos = (s.scope_pos + 1) % SCOPE_LEN;
            }
            let (l, r) = (soft_clip(l * self.gain), soft_clip(r * self.gain));
            f[0] = l;
            f[1] = r;
        }
        taken
    }
}

/// Leaves samples up to the knee alone and bends louder ones smoothly
/// towards full scale, which they never reach.
#[inline]
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let bent = KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh();
    bent.copysign(x)
}

pub struct Output {
    pub shared: Arc<Mutex<Shared>>,
    pub rate: u32,
    pub device_name: String,
    /// The stream lives on a thread of its own (cpal's streams cannot move
    /// between threads, and the Android engine is shared by several); it
    /// stops when this is dropped.
    _stream: Option<crossbeam_channel::Sender<()>>,
}

impl Output {
    /// Opens the default sound device, or the silent clock if there is none.
    pub fn open() -> Output {
        let shared = Arc::new(Mutex::new(Shared::new()));
        let (opened_tx, opened) = crossbeam_channel::bounded(1);
        let (keep, stop) = crossbeam_channel::bounded::<()>(0);
        let for_device = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("sound-output".into())
            .spawn(move || match open_device(for_device) {
                Ok((stream, rate, name)) => {
                    let _ = opened_tx.send(Ok((rate, name)));
                    // Until the output is dropped.
                    let _ = stop.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = opened_tx.send(Err(e));
                }
            });
        let result = match spawned {
            Ok(_) => opened
                .recv()
                .unwrap_or_else(|_| Err(anyhow!("the sound output thread ended"))),
            Err(e) => Err(e.into()),
        };
        match result {
            Ok((rate, name)) => {
                log::info!("sound output: {name} at {rate} Hz");
                Output {
                    shared,
                    rate,
                    device_name: name,
                    _stream: Some(keep),
                }
            }
            Err(e) => {
                log::warn!("no sound output ({e:#}); playing silently");
                null_clock(shared.clone(), 48_000);
                Output {
                    shared,
                    rate: 48_000,
                    device_name: "none (silent)".into(),
                    _stream: None,
                }
            }
        }
    }
}

fn open_device(shared: Arc<Mutex<Shared>>) -> Result<(cpal::Stream, u32, String)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no output device"))?;
    let name = device.name().unwrap_or_else(|_| "default".into());
    let supported = device.default_output_config()?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let rate = config.sample_rate.0;
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, shared, rate)?,
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, shared, rate)?,
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, shared, rate)?,
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, shared, rate)?,
        f => return Err(anyhow!("unsupported sample format {f:?}")),
    };
    stream.play()?;
    Ok((stream, rate, name))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Mutex<Shared>>,
    rate: u32,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut stereo: Vec<f32> = Vec::new();
    let mut feed = Feed::new(rate);
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / channels.max(1);
            stereo.resize(frames * 2, 0.0);
            // Never waits for the lock (see the top of this file).
            feed.fill(&shared, &mut stereo);
            for (i, f) in data.chunks_mut(channels.max(1)).enumerate() {
                let (l, r) = (stereo[2 * i], stereo[2 * i + 1]);
                match f.len() {
                    1 => f[0] = T::from_sample((l + r) * 0.5),
                    _ => {
                        f[0] = T::from_sample(l);
                        f[1] = T::from_sample(r);
                        for x in &mut f[2..] {
                            *x = T::from_sample(0.0);
                        }
                    }
                }
            }
        },
        |e| log::error!("sound output error: {e}"),
        None,
    )?;
    Ok(stream)
}

/// Consumes samples in real time without a device, so playback (and the
/// spectrum) behave the same on a machine with no sound card.
fn null_clock(shared: Arc<Mutex<Shared>>, rate: u32) {
    std::thread::Builder::new()
        .name("silent-output".into())
        .spawn(move || {
            let block = (rate / 100) as usize;
            let mut buf = vec![0f32; block * 2];
            let mut feed = Feed::new(rate);
            let start = std::time::Instant::now();
            let mut played = 0u64;
            loop {
                let due = start.elapsed().as_secs_f64() * rate as f64;
                while (played as f64) < due {
                    feed.fill(&shared, &mut buf);
                    played += block as u64;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        })
        .expect("thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_counts() {
        let shared = Mutex::new(Shared::new());
        let mut feed = Feed::new(0);
        feed.gain = 1.0;
        {
            let mut s = shared.lock().unwrap();
            s.reset(0);
            s.volume = 1.0;
            s.buf.extend([0.5f32; 8]);
        }
        let mut out = vec![0f32; 12];
        assert_eq!(feed.fill(&shared, &mut out), 4);
        {
            let s = shared.lock().unwrap();
            assert_eq!(s.frames_played, 4);
            assert_eq!(s.scope_tail(2), vec![0.5, 0.5]);
            assert_eq!(s.buffered_frames(), 0);
        }
        assert_eq!(&out[8..], &[0.0; 4]);
        {
            let mut s = shared.lock().unwrap();
            s.paused = true;
            s.buf.extend([0.5f32; 2]);
        }
        assert_eq!(feed.fill(&shared, &mut out), 0);
        assert_eq!(shared.lock().unwrap().buffered_frames(), 1);
    }

    #[test]
    fn plays_through_a_busy_lock() {
        let shared = Mutex::new(Shared::new());
        let mut feed = Feed::new(48_000);
        {
            let mut s = shared.lock().unwrap();
            s.reset(0);
            s.volume = 1.0;
            s.buf.extend(std::iter::repeat_n(0.5f32, 48_000));
        }
        let mut out = vec![0f32; 512];
        assert_eq!(feed.fill(&shared, &mut out), 256);
        // Another thread holds the lock: the output plays what it has.
        let guard = shared.lock().unwrap();
        assert_eq!(feed.fill(&shared, &mut out), 256);
        assert!(out.iter().all(|&x| x > 0.0));
        drop(guard);
        // Counted once the lock is free again.
        feed.fill(&shared, &mut out);
        let s = shared.lock().unwrap();
        assert_eq!(s.frames_played, 768);
        assert_eq!(s.buffered_frames(), 24_000 - 768);
    }

    #[test]
    fn rounds_off_peaks() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.9), -0.9);
        for x in [0.95f32, 1.0, 1.2, 3.0, 100.0] {
            let y = soft_clip(x);
            assert!(y > 0.9 && y <= 1.0, "{x} -> {y}");
            assert_eq!(soft_clip(-x), -y);
        }
        // Smooth at the knee and rising all the way.
        assert!((soft_clip(0.9001) - 0.9001).abs() < 1e-5);
        assert!(soft_clip(1.2) > soft_clip(1.1));
    }

    #[test]
    fn counts_running_dry_mid_track() {
        let shared = Mutex::new(Shared::new());
        let mut feed = Feed::new(0);
        shared.lock().unwrap().reset(0);
        let mut out = vec![0f32; 8];
        // The decoder has not started: not late.
        feed.fill(&shared, &mut out);
        assert_eq!(shared.lock().unwrap().underruns, 0);
        shared.lock().unwrap().buf.extend([0.5f32; 4]);
        feed.fill(&shared, &mut out);
        assert_eq!(shared.lock().unwrap().underruns, 1);
        // The end of the track is not running dry.
        shared.lock().unwrap().producer_done = true;
        feed.fill(&shared, &mut out);
        assert_eq!(shared.lock().unwrap().underruns, 1);
    }

    #[test]
    fn drops_what_it_holds_on_reset() {
        let shared = Mutex::new(Shared::new());
        let mut feed = Feed::new(48_000);
        shared.lock().unwrap().buf.extend([0.5f32; 4_000]);
        let mut out = vec![0f32; 64];
        feed.fill(&shared, &mut out);
        shared.lock().unwrap().reset(1_000);
        assert_eq!(feed.fill(&shared, &mut out), 0);
        assert_eq!(shared.lock().unwrap().buffered_frames(), 0);
    }
}
