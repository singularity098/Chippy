//! Raw FFI to Chippy's shim over the WSR player (`shim.cpp`). The core is
//! global: only one song can be open at a time. A second copy of it
//! (`wsrn_*`) is for finding a song's notes while another plays.
use std::os::raw::{c_int, c_void};

/// Called with each sound register write: the CPU clock's timestamp
/// (3.072 MHz), the port, the value; and with port 0x10000 just before the
/// timestamp goes back to 0, its value then being the time run.
pub type WriteHook = unsafe extern "C" fn(user: *mut c_void, ts: u32, port: u32, v: u8);

extern "C" {
    pub fn wsr_open(data: *const u8, size: u32, rate: u32) -> c_int;
    pub fn wsr_first_song() -> c_int;
    pub fn wsr_reset(song: u32);
    pub fn wsr_render(out: *mut i16, frames: u32);
    pub fn wsr_close();

    pub fn wsrn_open(data: *const u8, size: u32, rate: u32) -> c_int;
    pub fn wsrn_reset(song: u32);
    pub fn wsrn_render(out: *mut i16, frames: u32);
    pub fn wsrn_close();
    pub fn wsrn_set_hook(f: Option<WriteHook>, user: *mut c_void);
}
