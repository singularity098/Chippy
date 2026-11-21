//! Sega Master System and Game Gear (SGC): the SN76489 PSG's writes, as
//! Chippy's own emulator runs the tune.

use anyhow::Result;

use super::chips::{Chip, Psg};
use super::Transcription;
use crate::vfs::Location;

pub fn transcribe(loc: &Location, sub: Option<u32>, max_ms: Option<u64>) -> Result<Transcription> {
    // SGC files carry no lengths: the default length, as they play.
    let ms = max_ms.unwrap_or_else(|| {
        let o = crate::decode::DecodeOptions::default();
        o.default_length_ms + o.fade_ms
    });
    let mut psg = Box::new(Psg::new(3_579_545.0));
    let system = crate::decode::sgc::capture(loc, sub, ms, |t, port, v| {
        // Port $06 is the Game Gear's stereo, not a note.
        if port != 0x06 {
            psg.write(t, 0, v);
        }
    })?;
    Ok(Transcription {
        system: system.into(),
        parts: psg.finish(ms as f64),
        notes: vec!["The Master System's FM unit (YM2413) isn't read.".into()],
        ..Default::default()
    })
}
