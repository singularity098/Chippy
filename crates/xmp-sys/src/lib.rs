//! Raw FFI to Chippy's shim over libxmp (`shim.c`).
use std::os::raw::{c_char, c_int, c_long, c_void};

pub type Context = *mut c_void;

extern "C" {
    pub fn cxmp_open(data: *const c_void, size: c_long, rate: c_int) -> Context;
    pub fn cxmp_info(
        c: Context,
        name: *mut *const c_char,
        kind: *mut *const c_char,
        comment: *mut *const c_char,
        length_ms: *mut c_int,
        channels: *mut c_int,
    );
    pub fn cxmp_render(c: Context, out: *mut i16, frames: c_int) -> c_int;
    pub fn cxmp_seek(c: Context, ms: c_int);
    pub fn cxmp_close(c: Context, started: c_int);
}
