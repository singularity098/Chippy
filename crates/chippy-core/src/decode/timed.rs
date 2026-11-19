//! Emulators that play a song forever: [`Timed`] stops them at the song's
//! length with a fade, and seeks by restarting and running ahead.

use anyhow::Result;

use super::{apply_fade, Decoder, SEEK_STEP_MS};

/// An emulator rendering 16-bit stereo.
pub trait Engine: Send {
    fn rate(&self) -> u32;
    /// Fills `out` (interleaved stereo).
    fn render(&mut self, out: &mut [i16]) -> Result<()>;
    /// Back to the start of the song.
    fn restart(&mut self) -> Result<()>;
    /// Runs ahead `frames` frames, throwing the sound away.
    fn skip(&mut self, frames: usize, scratch: &mut Vec<i16>) -> Result<()> {
        let mut left = frames;
        while left > 0 {
            let n = left.min(4096);
            scratch.resize(n * 2, 0);
            self.render(scratch)?;
            left -= n;
        }
        Ok(())
    }
}

pub struct Timed<E: Engine> {
    pub engine: E,
    pos: u64,
    fade_start: u64,
    end: u64,
    duration_ms: u64,
    buf: Vec<i16>,
}

impl<E: Engine> Timed<E> {
    /// Plays `engine` for `length_ms`, then fades over `fade_ms`.
    pub fn new(engine: E, length_ms: u64, fade_ms: u64) -> Self {
        let rate = engine.rate() as u64;
        let f = |ms: u64| ms * rate / 1000;
        Timed {
            engine,
            pos: 0,
            fade_start: f(length_ms),
            end: f(length_ms + fade_ms),
            duration_ms: length_ms + fade_ms,
            buf: Vec::new(),
        }
    }
}

impl<E: Engine> Decoder for Timed<E> {
    fn sample_rate(&self) -> u32 {
        self.engine.rate()
    }
    fn duration_ms(&self) -> Option<u64> {
        Some(self.duration_ms)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        if self.pos >= self.end {
            return Ok(0);
        }
        let frames = ((out.len() / 2) as u64).min(self.end - self.pos) as usize;
        self.buf.resize(frames * 2, 0);
        self.engine.render(&mut self.buf)?;
        for (o, &s) in out.iter_mut().zip(&self.buf) {
            *o = s as f32 / 32768.0;
        }
        apply_fade(&mut out[..frames * 2], self.pos, self.fade_start, self.end);
        self.pos += frames as u64;
        Ok(frames)
    }
    fn seek(&mut self, ms: u64) -> Result<()> {
        while !self.seek_step(ms)? {}
        Ok(())
    }
    fn seek_step(&mut self, ms: u64) -> Result<bool> {
        let rate = self.engine.rate() as u64;
        let target = (ms * rate / 1000).min(self.end);
        if target < self.pos {
            self.engine.restart()?;
            self.pos = 0;
        }
        let n = (target - self.pos).min(rate * SEEK_STEP_MS / 1000);
        self.engine.skip(n as usize, &mut self.buf)?;
        self.pos += n;
        Ok(self.pos == target)
    }
}
