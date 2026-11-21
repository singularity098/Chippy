//! The SNES's BRR samples, and the pitch each was recorded at.

/// A decoded sample: `pcm` from its start, and where its loop begins (if
/// it loops) as an index into `pcm`.
pub struct Sample {
    pub pcm: Vec<f32>,
    pub loop_start: Option<usize>,
}

/// Decodes the BRR sample at `start` in the 64 KiB of SPC RAM, looping back
/// to `loop_addr` if its last block says so.
pub fn decode(ram: &[u8], start: u16, loop_addr: u16) -> Sample {
    let mut pcm = Vec::new();
    let (mut p1, mut p2) = (0f32, 0f32);
    let mut addr = start as usize;
    let mut loop_start = None;
    // A sample can't be longer than RAM; a broken one may never end.
    for _ in 0..(0x10000 / 9) {
        if addr + 9 > ram.len() {
            break;
        }
        let header = ram[addr];
        let shift = header >> 4;
        let filter = (header >> 2) & 3;
        for i in 0..16 {
            let byte = ram[addr + 1 + i / 2];
            let nibble = if i % 2 == 0 { byte >> 4 } else { byte & 15 };
            let n = ((nibble as i8) << 4 >> 4) as f32;
            let mut s = if shift <= 12 {
                n * (1 << shift) as f32 / 2.0
            } else if n < 0.0 {
                -2048.0
            } else {
                0.0
            };
            s += match filter {
                0 => 0.0,
                1 => p1 * 15.0 / 16.0,
                2 => p1 * 61.0 / 32.0 - p2 * 15.0 / 16.0,
                _ => p1 * 115.0 / 64.0 - p2 * 13.0 / 16.0,
            };
            s = s.clamp(-32768.0, 32767.0);
            p2 = p1;
            p1 = s;
            pcm.push(s / 32768.0);
        }
        let end = header & 1 != 0;
        let looped = header & 2 != 0;
        addr += 9;
        if end {
            if looped && loop_addr >= start {
                let offset = (loop_addr - start) as usize;
                if offset.is_multiple_of(9) && offset / 9 * 16 < pcm.len() {
                    loop_start = Some(offset / 9 * 16);
                }
            }
            break;
        }
    }
    Sample { pcm, loop_start }
}

pub const RATE: f64 = 32_000.0;

/// The pitch a sample plays at when the DSP plays it at 32 kHz (pitch
/// register $1000); `None` if it has no clear pitch (drums, noise).
pub fn pitch(s: &Sample) -> Option<super::instrument::Pitch> {
    super::instrument::pitch(&s.pcm, s.loop_start, RATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes PCM as BRR (filter 0 only: plenty for tests).
    fn encode(pcm: &[i16], looped: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let blocks = pcm.chunks(16).count();
        for (b, chunk) in pcm.chunks(16).enumerate() {
            let peak = chunk.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0);
            let mut shift = 0u8;
            while shift < 12 && peak * 2 > 7 << shift {
                shift += 1;
            }
            let mut header = shift << 4;
            if b + 1 == blocks {
                header |= 1 | if looped { 2 } else { 0 };
            }
            out.push(header);
            let nib = |s: i16| -> u8 {
                let v = ((s as i32 * 2) >> shift).clamp(-8, 7);
                (v as u8) & 15
            };
            for pair in chunk.chunks(2) {
                out.push(nib(pair[0]) << 4 | nib(*pair.get(1).unwrap_or(&0)));
            }
        }
        out
    }

    fn tone(hz: f64, len: usize) -> Vec<i16> {
        (0..len)
            .map(|i| ((i as f64 * hz / RATE * std::f64::consts::TAU).sin() * 12000.0) as i16)
            .collect()
    }

    #[test]
    fn measures_a_looped_tone() {
        // 32 periods of 250 Hz (128 samples each) in a loop.
        let brr = encode(&tone(250.0, 4096), true);
        let mut ram = vec![0u8; 0x10000];
        ram[0x1000..0x1000 + brr.len()].copy_from_slice(&brr);
        let s = decode(&ram, 0x1000, 0x1000);
        assert_eq!(s.pcm.len(), 4096);
        assert_eq!(s.loop_start, Some(0));
        let p = pitch(&s).expect("a pitch");
        assert!((p.hz - 250.0).abs() < 2.0, "{}", p.hz);
    }

    #[test]
    fn measures_a_one_shot_tone() {
        let brr = encode(&tone(523.25, 6000), false);
        let mut ram = vec![0u8; 0x10000];
        ram[..brr.len()].copy_from_slice(&brr);
        let s = decode(&ram, 0, 0);
        assert_eq!(s.loop_start, None);
        let p = pitch(&s).expect("a pitch");
        assert!(
            (super::super::hz_to_key(p.hz) - 72.0).abs() < 0.2,
            "{}",
            p.hz
        );
    }

    #[test]
    fn noise_has_no_pitch() {
        let mut x = 12345u32;
        let pcm: Vec<i16> = (0..6000)
            .map(|_| {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                ((x >> 16) as i16) / 3
            })
            .collect();
        let brr = encode(&pcm, false);
        let mut ram = vec![0u8; 0x10000];
        ram[..brr.len()].copy_from_slice(&brr);
        assert!(pitch(&decode(&ram, 0, 0)).is_none());
    }
}
