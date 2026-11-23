//! Sega Master System and Game Gear (SGC): the SN76489 PSG's writes, as
//! Chippy's own emulator runs the tune, and the FM unit's (YM2413) when the
//! tune writes to it. The FM unit isn't played (nor found by drivers that
//! look for it), so this is only for rips that use it regardless.

use anyhow::Result;

use super::chips::{Chip, Opll, Psg};
use super::Transcription;
use crate::vfs::Location;

pub fn transcribe(loc: &Location, sub: Option<u32>, max_ms: Option<u64>) -> Result<Transcription> {
    // SGC files carry no lengths: the default length, as they play.
    let ms = max_ms.unwrap_or_else(|| {
        let o = crate::decode::DecodeOptions::default();
        o.default_length_ms + o.fade_ms
    });
    let mut psg = Box::new(Psg::new(3_579_545.0));
    let mut fm = Box::new(Opll::new(3_579_545.0));
    let mut latch = 0u8;
    let system = crate::decode::sgc::capture(loc, sub, ms, |t, port, v| match port {
        // Port $06 is the Game Gear's stereo, not a note.
        0x06 => {}
        0xF0 => latch = v,
        0xF1 => fm.write(t, latch as u16, v),
        _ => psg.write(t, 0, v),
    })?;
    let mut parts = psg.finish(ms as f64);
    parts.extend(fm.finish(ms as f64));
    Ok(Transcription {
        system: system.into(),
        parts,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_fm_unit_when_written() {
        let mut f = vec![0u8; 0xa0];
        f[..4].copy_from_slice(b"SGC\x1a");
        f[4] = 1;
        for (at, v) in [
            (0x08, 0x0400u16),
            (0x0a, 0x0400),
            (0x0c, 0x0420),
            (0x0e, 0xdff0),
        ] {
            f[at..at + 2].copy_from_slice(&v.to_le_bytes());
        }
        f[0x20] = 1;
        f[0x21] = 2;
        f[0x23] = 1;
        // init: FM channel 1, fnum 289, block 4, key on; play: ret.
        let mut code = vec![
            0x3e, 0x10, 0xd3, 0xf0, 0x3e, 0x21, 0xd3, 0xf1, 0x3e, 0x20, 0xd3, 0xf0, 0x3e, 0x19,
            0xd3, 0xf1, 0xc9,
        ];
        code.resize(0x20, 0);
        code.push(0xc9);
        f.extend(code);
        let t = crate::notes::transcribe_bytes("t.sgc", &f, None, 1000);
        let fm = crate::notes::part(&t, "FM 1");
        assert_eq!(crate::notes::keys_at(fm), vec![(69, 0)]);
    }
}
