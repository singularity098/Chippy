//! Raw FFI to Chippy's shim over vio2sf (`shim.c`). It renders 44.1 kHz.
use libz_sys as _;
use std::os::raw::{c_int, c_void};

pub const RATE: u32 = 44_100;

#[repr(C)]
pub struct NDS_state {
    _p: [u8; 0],
}

/// What a sound channel did (see `twosf_note_event` in vio2sf's state.h).
#[repr(C)]
#[derive(Debug)]
pub struct NoteEvent {
    /// 0: key on, 1: a new timer (pitch), 2: stopped by a write.
    pub kind: c_int,
    pub channel: c_int,
    /// 0: PCM8, 1: PCM16, 2: IMA-ADPCM, 3: PSG (square, or noise).
    pub format: c_int,
    pub vol: c_int,
    pub datashift: c_int,
    pub duty: c_int,
    pub repeat: c_int,
    pub timer: u32,
    /// In 32-bit words.
    pub loopstart: u32,
    pub length: u32,
    /// The sample (key on, formats 0-2), with `data_size` bytes readable.
    pub data: *const u8,
    pub data_size: u32,
    /// Output samples mixed before it.
    pub sample: u64,
}

pub type NotesHook = unsafe extern "C" fn(user: *mut c_void, e: *const NoteEvent);

extern "C" {
    /// Installs (or with `None`, removes) the hook, and starts its count of
    /// samples from 0.
    pub fn twosf_set_notes_hook(s: *mut NDS_state, f: Option<NotesHook>, user: *mut c_void);
    #[allow(clippy::too_many_arguments)]
    pub fn twosf_open(
        rom: *const u8,
        rom_size: u32,
        state: *const u8,
        state_size: u32,
        initial_frames: c_int,
        sync_type: c_int,
        arm7_clockdown: c_int,
        arm9_clockdown: c_int,
        interpolation: c_int,
    ) -> *mut NDS_state;
    pub fn twosf_render(s: *mut NDS_state, out: *mut i16, frames: u32);
    pub fn twosf_close(s: *mut NDS_state);
}
