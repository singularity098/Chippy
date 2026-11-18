//! Sound output: a cpal stream (or, without a sound device, a clock that
//! consumes samples in real time) playing from a buffer the player thread
//! fills. The callback also keeps the last few thousand samples for the
//! spectrum and the peak levels for the meter.

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
    /// Highest level per channel since last read (after volume).
    pub peak: [f32; 2],
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
            peak: [0.0; 2],
        }
    }

    pub fn buffered_frames(&self) -> usize {
        self.buf.len() / 2
    }

    /// Forgets queued sound and restarts the clock at `ms`.
    pub fn reset(&mut self, ms: u64) {
        self.buf.clear();
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

    /// Plays up to `frames` frames into `out` (stereo), returns frames
    /// taken. `gain` follows the volume smoothly.
    fn render(&mut self, out: &mut [f32], gain: &mut f32) -> usize {
        if self.paused {
            out.fill(0.0);
            return 0;
        }
        let target = self.volume * self.volume;
        let mut taken = 0;
        let mut peak = self.peak;
        for f in out.chunks_exact_mut(2) {
            let (Some(l), Some(r)) = (self.buf.pop_front(), self.buf.pop_front()) else {
                f.fill(0.0);
                continue;
            };
            taken += 1;
            *gain += (target - *gain) * 0.002;
            self.scope[self.scope_pos] = (l + r) * 0.5;
            self.scope_pos = (self.scope_pos + 1) % SCOPE_LEN;
            let (l, r) = (l * *gain, r * *gain);
            peak[0] = peak[0].max(l.abs());
            peak[1] = peak[1].max(r.abs());
            f[0] = l;
            f[1] = r;
        }
        self.peak = peak;
        self.frames_played += taken as u64;
        taken
    }
}

pub struct Output {
    pub shared: Arc<Mutex<Shared>>,
    pub rate: u32,
    pub device_name: String,
    _stream: Option<cpal::Stream>,
}

impl Output {
    /// Opens the default sound device, or the silent clock if there is none.
    pub fn open() -> Output {
        let shared = Arc::new(Mutex::new(Shared::new()));
        match open_device(shared.clone()) {
            Ok((stream, rate, name)) => {
                log::info!("sound output: {name} at {rate} Hz");
                Output {
                    shared,
                    rate,
                    device_name: name,
                    _stream: Some(stream),
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
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, shared)?,
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, shared)?,
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, shared)?,
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, shared)?,
        f => return Err(anyhow!("unsupported sample format {f:?}")),
    };
    stream.play()?;
    Ok((stream, rate, name))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Mutex<Shared>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut stereo: Vec<f32> = Vec::new();
    let mut gain = 0.0f32;
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / channels.max(1);
            stereo.resize(frames * 2, 0.0);
            // Never wait for the player thread: a busy lock is a moment of
            // silence, not a glitch in the whole system's audio.
            match shared.try_lock() {
                Ok(mut s) => {
                    s.render(&mut stereo, &mut gain);
                }
                Err(_) => stereo.fill(0.0),
            }
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
            let mut gain = 0.0;
            let start = std::time::Instant::now();
            let mut played = 0u64;
            loop {
                let due = start.elapsed().as_secs_f64() * rate as f64;
                while (played as f64) < due {
                    shared.lock().unwrap().render(&mut buf, &mut gain);
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
        let mut s = Shared::new();
        s.reset(0);
        s.volume = 1.0;
        s.buf.extend([0.5f32; 8]);
        let mut out = vec![0f32; 12];
        let mut gain = 1.0;
        assert_eq!(s.render(&mut out, &mut gain), 4);
        assert_eq!(s.frames_played, 4);
        assert_eq!(&out[8..], &[0.0; 4]);
        assert_eq!(s.scope_tail(2), vec![0.5, 0.5]);
        s.paused = true;
        s.buf.extend([0.5f32; 2]);
        assert_eq!(s.render(&mut out, &mut gain), 0);
        assert_eq!(s.buffered_frames(), 1);
    }
}
