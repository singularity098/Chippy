//! Parts muted (or soloed) in the guitar tab's list of parts: which MIDI
//! channels each track plays without, for as long as the app runs (never
//! saved). The tab view sets them; the player hands them to the playing
//! track's decoder ([`chippy_core::Decoder::set_muted`]), which only
//! decoders playing through the SoundFont act on: MIDI files, tab files,
//! and chiptunes in MIDI mode.

use chippy_core::TrackRef;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static MUTES: Mutex<Option<HashMap<TrackRef, u16>>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Plays `track` without the MIDI channels set in `channels` (a bit each,
/// bit 0 being channel 1).
pub fn set(track: &TrackRef, channels: u16) {
    let mut m = MUTES.lock().unwrap();
    let m = m.get_or_insert_with(HashMap::new);
    let before = m.get(track).copied().unwrap_or(0);
    if channels == 0 {
        m.remove(track);
    } else {
        m.insert(track.clone(), channels);
    }
    if before != channels {
        GENERATION.fetch_add(1, Ordering::AcqRel);
    }
}

/// The MIDI channels `track` plays without.
pub fn get(track: &TrackRef) -> u16 {
    MUTES
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(track).copied())
        .unwrap_or(0)
}

/// Bumped each time something is muted or let back in.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}
