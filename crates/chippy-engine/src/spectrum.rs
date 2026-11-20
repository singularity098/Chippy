//! The now-playing spectrum: an FFT of the latest output, folded into
//! log-spaced bands, with bars that jump up and settle down and peak caps
//! that hang for a moment before falling.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub const FFT_LEN: usize = 2048;

pub struct Spectrum {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    window_sum: f32,
    work: Vec<Complex<f32>>,
    pub bars: Vec<f32>,
    pub peaks: Vec<f32>,
    hold: Vec<f32>,
    peak_speed: Vec<f32>,
}

impl Spectrum {
    pub fn new(bands: usize) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_LEN);
        let window: Vec<f32> = (0..FFT_LEN)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_LEN - 1) as f32).cos()
            })
            .collect();
        let window_sum = window.iter().sum();
        Spectrum {
            fft,
            window,
            window_sum,
            work: vec![Complex::default(); FFT_LEN],
            bars: vec![0.0; bands],
            peaks: vec![0.0; bands],
            hold: vec![0.0; bands],
            peak_speed: vec![0.0; bands],
        }
    }

    /// Advances by `dt` seconds. `samples` are the latest mono samples
    /// (FFT_LEN of them), or `None` when nothing plays (the bars fall).
    pub fn update(&mut self, samples: Option<&[f32]>, rate: u32, dt: f32) {
        let n = self.bars.len();
        let mut target = vec![0f32; n];
        if let Some(s) = samples.filter(|s| s.len() >= FFT_LEN) {
            for (i, c) in self.work.iter_mut().enumerate() {
                *c = Complex::new(s[i] * self.window[i], 0.0);
            }
            self.fft.process(&mut self.work);
            let bin_hz = rate as f32 / FFT_LEN as f32;
            let lo = 35f32;
            let hi = (rate as f32 / 2.0).min(16_000.0);
            for (b, t) in target.iter_mut().enumerate() {
                let f0 = lo * (hi / lo).powf(b as f32 / n as f32);
                let f1 = lo * (hi / lo).powf((b + 1) as f32 / n as f32);
                let i0 = ((f0 / bin_hz).floor() as usize).max(1);
                let i1 = ((f1 / bin_hz).ceil() as usize).max(i0 + 1).min(FFT_LEN / 2);
                let mag = self.work[i0..i1]
                    .iter()
                    .map(|c| c.norm())
                    .fold(0.0, f32::max);
                let amp = 2.0 * mag / self.window_sum;
                // Tilt up the highs a little: music has less energy there.
                let tilt = 1.0 + 2.5 * (b as f32 / n as f32);
                let db = 20.0 * (amp * tilt).max(1e-9).log10();
                *t = ((db + 66.0) / 60.0).clamp(0.0, 1.0);
            }
        }
        for (i, &t) in target.iter().enumerate() {
            let bar = &mut self.bars[i];
            *bar = if t > *bar {
                *bar + (t - *bar) * 0.65
            } else {
                (*bar - dt * 1.6).max(t)
            };
            if *bar >= self.peaks[i] {
                self.peaks[i] = *bar;
                self.hold[i] = 0.45;
                self.peak_speed[i] = 0.0;
            } else if self.hold[i] > 0.0 {
                self.hold[i] -= dt;
            } else {
                self.peak_speed[i] += dt * 2.4;
                self.peaks[i] = (self.peaks[i] - self.peak_speed[i] * dt).max(*bar);
            }
        }
    }

    pub fn is_still(&self) -> bool {
        self.bars.iter().chain(&self.peaks).all(|&v| v <= 0.001)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_lights_one_region() {
        let mut s = Spectrum::new(32);
        let rate = 48_000;
        let tone: Vec<f32> = (0..FFT_LEN)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / rate as f32).sin() * 0.5)
            .collect();
        for _ in 0..10 {
            s.update(Some(&tone), rate, 1.0 / 60.0);
        }
        let (max_i, _) = s
            .bars
            .iter()
            .enumerate()
            .fold((0, 0.0), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
        assert!(s.bars[max_i] > 0.8);
        assert!(s.bars[0] < 0.3 && s.bars[31] < 0.3);
        for _ in 0..200 {
            s.update(None, rate, 1.0 / 60.0);
        }
        assert!(s.is_still());
    }
}
