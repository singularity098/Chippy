//! Finds the notes a chiptune plays and writes them as a MIDI file, with a
//! summary of each part (and, for SNES tunes, of each instrument sample):
//! `cargo run -p chippy-core --example notes -- PATH OUT.mid [SUBTRACK] [SECONDS]`
use chippy_core::notes::{self, key_name};
use chippy_core::vfs;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: notes PATH OUT.mid [SUBTRACK] [SECONDS]";
    let path = args.next().expect(usage);
    let out = args.next().expect(usage);
    let sub: Option<u32> = args.next().and_then(|s| s.parse().ok());
    let secs: Option<u64> = args.next().and_then(|s| s.parse().ok());
    let loc = match vfs::resolve(path.as_ref())? {
        vfs::Node::Tracks(l) => l,
        n => anyhow::bail!("not a file: {n:?}"),
    };
    let started = std::time::Instant::now();
    let t = notes::transcribe(&loc, sub, secs.map(|s| s * 1000))?;
    println!("{} ({:.2?})", t.system, started.elapsed());
    for p in &t.parts {
        let lo = p.notes.iter().map(|n| n.key).min().unwrap_or(0);
        let hi = p.notes.iter().map(|n| n.key).max().unwrap_or(0);
        let first: Vec<String> = p.notes.iter().take(12).map(|n| key_name(n.key)).collect();
        let end = p.notes.last().map_or(0.0, |n| n.start_ms + n.dur_ms) / 1000.0;
        println!(
            "  {:<34} {:>5} notes to {end:>5.1}s  {}-{}  {} ...",
            p.name,
            p.notes.len(),
            key_name(lo),
            key_name(hi),
            first.join(" ")
        );
    }
    if !t.samples.is_empty() {
        println!("  samples:");
        for s in &t.samples {
            let what = match (s.hz, s.drum_key) {
                (Some(hz), _) => {
                    let k = 69.0 + 12.0 * (hz / 440.0).log2();
                    let r = k.round();
                    format!(
                        "{hz:8.1} Hz = {} {:+3.0} cents (clarity {:.2}) {}",
                        key_name(r as u8),
                        (k - r) * 100.0,
                        s.clarity,
                        notes::gm::program_name(s.program.unwrap_or(0))
                    )
                }
                (None, k) => format!("unpitched: {}", notes::gm::drum_name(k.unwrap_or(0))),
            };
            println!(
                "    #{:<3} {:<22} ${:04X} {:>6} samples{} {:>4} uses  {what}",
                s.srcn,
                s.name,
                s.start,
                s.len,
                if s.looped { " looped" } else { "       " },
                s.uses
            );
        }
    }
    for n in &t.notes {
        println!("  note: {n}");
    }
    std::fs::write(&out, notes::midi::write(&t, &loc.name()))?;
    println!("wrote {out}");
    Ok(())
}
