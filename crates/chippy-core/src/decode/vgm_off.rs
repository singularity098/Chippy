//! vgmstream left out (chippy-core built without its `vgmstream`
//! feature, as for Tabby, which only reads notes): its formats aren't
//! opened, and nothing else changes.

use anyhow::{bail, Result};

use super::{DecodeOptions, Decoder, SubTrack};
use crate::meta::TrackMeta;
use crate::vfs::Location;

pub fn extensions() -> Vec<String> {
    Vec::new()
}

pub enum VgmDecoder {}

impl VgmDecoder {
    pub fn open(_loc: &Location, _sub: Option<u32>, _opts: &DecodeOptions) -> Result<Self> {
        bail!("built without vgmstream")
    }
}

impl Decoder for VgmDecoder {
    fn sample_rate(&self) -> u32 {
        match *self {}
    }
    fn duration_ms(&self) -> Option<u64> {
        match *self {}
    }
    fn read(&mut self, _out: &mut [f32]) -> Result<usize> {
        match *self {}
    }
    fn seek(&mut self, _ms: u64) -> Result<()> {
        match *self {}
    }
}

pub fn track_list(_loc: &Location, _opts: &DecodeOptions) -> Result<Vec<SubTrack>> {
    bail!("built without vgmstream")
}

pub fn meta(_loc: &Location, _sub: Option<u32>, _opts: &DecodeOptions) -> Result<TrackMeta> {
    bail!("built without vgmstream")
}
