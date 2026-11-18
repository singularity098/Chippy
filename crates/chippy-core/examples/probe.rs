//! Lists a virtual path the way Chippy's file browser does, and reads the
//! tags of what is playable there: `cargo run -p chippy-core --example probe -- PATH`
use chippy_core::{decode::DecodeOptions, meta, vfs};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: probe PATH");
    let opts = DecodeOptions::default();
    let node = vfs::resolve(path.as_ref())?;
    println!("{node:?}");
    for e in vfs::list(&node, true, &opts)? {
        print!("{:?}\t{}", e.kind, e.name);
        if let Some(t) = &e.track {
            let m = meta::read(t, &opts);
            print!(
                "\t| {} | {} | {} | {:?} ms | {}",
                m.title, m.artist, m.album, m.length_ms, m.system
            );
        }
        println!();
    }
    Ok(())
}
