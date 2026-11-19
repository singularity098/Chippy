//! Decodes a track to a 16-bit WAV file, to listen to or inspect a decoder's
//! output: `cargo run -p chippy-core --example towav -- PATH OUT.wav [SUBTRACK] [SECONDS]`
use chippy_core::{open_decoder, vfs, DecodeOptions, TrackRef};
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: towav PATH OUT.wav [SUBTRACK] [SECONDS]";
    let path = args.next().expect(usage);
    let out = args.next().expect(usage);
    let sub: Option<u32> = args.next().and_then(|s| s.parse().ok());
    let secs: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let loc = match vfs::resolve(path.as_ref())? {
        vfs::Node::Tracks(l) => l,
        n => anyhow::bail!("not a file: {n:?}"),
    };
    let mut d = open_decoder(&TrackRef { loc, sub }, &DecodeOptions::default())?;
    let rate = d.sample_rate();
    let want = (rate as f32 * secs) as usize;
    let mut pcm: Vec<u8> = Vec::new();
    let mut buf = vec![0f32; 8192];
    let mut frames = 0;
    while frames < want {
        let n = d.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for s in &buf[..n * 2] {
            pcm.extend(((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        frames += n;
    }
    let mut f = std::fs::File::create(out)?;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + pcm.len() as u32).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&(pcm.len() as u32).to_le_bytes())?;
    f.write_all(&pcm)?;
    println!("{frames} frames at {rate} Hz");
    Ok(())
}
