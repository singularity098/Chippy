//! Raw FFI to Chippy's shim over the WSR player (`shim.cpp`). The core is
//! global: only one song can be open at a time.
use std::os::raw::c_int;

extern "C" {
    pub fn wsr_open(data: *const u8, size: u32, rate: u32) -> c_int;
    pub fn wsr_first_song() -> c_int;
    pub fn wsr_reset(song: u32);
    pub fn wsr_render(out: *mut i16, frames: u32);
    pub fn wsr_close();
}
