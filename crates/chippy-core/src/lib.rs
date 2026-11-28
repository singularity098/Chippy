//! Chippy's engine, free of any UI: where music files live (folders,
//! archives, and files holding several tracks), decoding them to PCM,
//! reading their tags and finding their album art, the playlist, and the
//! settings.

pub mod archive;
pub mod art;
pub mod builtin_font;
pub mod decode;
pub mod formats;
pub mod lyrics;
pub mod meta;
pub mod notes;
pub mod playlist;
pub mod psf;
pub mod settings;
pub mod vfs;

pub use decode::{open_decoder, DecodeOptions, Decoder};
pub use meta::TrackMeta;
pub use vfs::{Entry, EntryKind, Location, Node, TrackRef};
