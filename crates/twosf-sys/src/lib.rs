//! Raw FFI to Chippy's shim over vio2sf (`shim.c`). It renders 44.1 kHz.
use libz_sys as _;
use std::os::raw::c_int;

pub const RATE: u32 = 44_100;

#[repr(C)]
pub struct NDS_state {
    _p: [u8; 0],
}

extern "C" {
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
