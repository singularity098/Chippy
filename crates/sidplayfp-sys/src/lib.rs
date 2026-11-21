//! Raw FFI to Chippy's C wrapper over libsidplayfp (`wrapper.cpp`).
use std::os::raw::{c_char, c_int};

#[repr(C)]
pub struct csid {
    _p: [u8; 0],
}

extern "C" {
    pub fn csid_open(data: *const u8, len: u32, err: *mut c_char, errlen: usize) -> *mut csid;
    pub fn csid_close(s: *mut csid);
    pub fn csid_songs(s: *mut csid) -> u32;
    pub fn csid_start_song(s: *mut csid) -> u32;
    pub fn csid_info(s: *mut csid, i: u32) -> *const c_char;
    pub fn csid_sid_model(s: *mut csid) -> c_int;
    pub fn csid_clock(s: *mut csid) -> c_int;
    pub fn csid_sid_chips(s: *mut csid) -> u32;
    pub fn csid_md5(s: *mut csid, out: *mut c_char);
    pub fn csid_start(s: *mut csid, song: u32, rate: u32, err: *mut c_char, errlen: usize)
        -> c_int;
    pub fn csid_play(s: *mut csid, buf: *mut i16, frames: u32) -> u32;
    /// The registers last written to SID chip `sid`: fills 32 bytes;
    /// returns 0 if there is no such chip.
    pub fn csid_regs(s: *mut csid, sid: u32, out: *mut u8) -> c_int;
}
