//! Loading album art off the UI thread: find it, decode it, and scale it
//! down to what the window can show.

use chippy_core::vfs::Location;
use crossbeam_channel::Sender;

pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// A small copy for the system's media controls (the notification and
    /// lock screen on Android, MPRIS on Linux).
    pub small: Option<Box<Picture>>,
}

/// Longest side kept; larger covers are scaled down (sharply).
const MAX_SIDE: u32 = 1400;
/// Longest side of the copy for the media controls.
const SMALL_SIDE: u32 = 512;

pub fn decode(data: &[u8]) -> Option<Picture> {
    let img = image::load_from_memory(data)
        .map_err(|e| log::debug!("art not decodable: {e}"))
        .ok()?;
    let small = img.thumbnail(SMALL_SIDE, SMALL_SIDE).to_rgba8();
    let small = Picture {
        width: small.width(),
        height: small.height(),
        rgba: small.into_raw(),
        small: None,
    };
    let img = if img.width().max(img.height()) > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Some(Picture {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
        small: Some(Box::new(small)),
    })
}

/// Finds and decodes the art for `loc` in the background; sends
/// `(generation, picture)` when done (None when there is none).
pub fn load(
    loc: Location,
    prefer_embedded: bool,
    generation: u64,
    tx: Sender<(u64, Option<Picture>)>,
) {
    std::thread::Builder::new()
        .name("art".into())
        .spawn(move || {
            let pic = chippy_core::art::find(&loc, prefer_embedded).and_then(|a| decode(&a.data));
            let _ = tx.send((generation, pic));
        })
        .ok();
}
