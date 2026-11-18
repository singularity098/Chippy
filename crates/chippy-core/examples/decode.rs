//! Decodes a file to the end and reports what came out, to check a decoder
//! works: `cargo run -p chippy-core --example decode -- FILE [SUBTRACK]`
use chippy_core::decode::{open_decoder, DecodeOptions};
use chippy_core::vfs::{Location, TrackRef};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: decode FILE [SUBTRACK]");
    let sub = args.next().map(|s| s.parse()).transpose()?;
    let opts = DecodeOptions::default();
    let t = TrackRef {
        loc: Location::Fs(path.into()),
        sub,
    };
    let mut d = open_decoder(&t, &opts)?;
    let mut buf = vec![0f32; 4096 * 2];
    let (mut frames, mut peak) = (0u64, 0f32);
    loop {
        let n = d.read(&mut buf)?;
        if n == 0 {
            break;
        }
        frames += n as u64;
        peak = buf[..n * 2].iter().fold(peak, |p, s| p.max(s.abs()));
    }
    println!(
        "{} Hz, {:?} ms reported, {} frames decoded ({} ms), peak {:.3}",
        d.sample_rate(),
        d.duration_ms(),
        frames,
        frames * 1000 / d.sample_rate() as u64,
        peak
    );
    Ok(())
}
