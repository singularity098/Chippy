//! Decodes a track for a few seconds and prints its loudness:
//! `cargo run -p chippy-core --example render -- PATH [SUBTRACK] [SECONDS]`
use chippy_core::{open_decoder, vfs, DecodeOptions, TrackRef};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: render PATH [SUBTRACK] [SECONDS]");
    let sub: Option<u32> = args.next().and_then(|s| s.parse().ok());
    let secs: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let loc = match vfs::resolve(path.as_ref())? {
        vfs::Node::Tracks(l) => l,
        n => anyhow::bail!("not a file: {n:?}"),
    };
    let opts = DecodeOptions::default();
    let mut d = open_decoder(&TrackRef { loc, sub }, &opts)?;
    println!(
        "rate {} Hz, length {:?} ms",
        d.sample_rate(),
        d.duration_ms()
    );
    let mut buf = vec![0f32; 4096];
    let want = (d.sample_rate() as f32 * secs) as usize;
    let (mut frames, mut sum, mut peak) = (0usize, 0f64, 0f32);
    let t = std::time::Instant::now();
    while frames < want {
        let n = d.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for s in &buf[..n * 2] {
            sum += (*s as f64) * (*s as f64);
            peak = peak.max(s.abs());
        }
        frames += n;
    }
    let rms = (sum / (frames.max(1) * 2) as f64).sqrt();
    println!(
        "{frames} frames, RMS {rms:.4}, peak {peak:.3}, decoded in {:?} ({:.0}x real time)",
        t.elapsed(),
        frames as f64 / d.sample_rate() as f64 / t.elapsed().as_secs_f64()
    );
    Ok(())
}
