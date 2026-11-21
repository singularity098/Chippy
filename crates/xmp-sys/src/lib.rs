//! Raw FFI to Chippy's shim over libxmp (`shim.c`).
use std::os::raw::{c_char, c_int, c_long, c_void};

pub type Context = *mut c_void;

/// One channel's state after a frame (see `cxmp_scan`).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Chan {
    /// The mixer's period * 4096 (13696 is C-0).
    pub period: c_int,
    pub ins: c_int,
    pub smp: c_int,
    pub vol: c_int,
    /// The row's event (on every frame of the row).
    pub ev_note: c_int,
    pub ev_fxt: c_int,
    pub ev_f2t: c_int,
}

pub const SAMPLE_16BIT: c_int = 1;
pub const SAMPLE_LOOP: c_int = 2;
pub const SAMPLE_STEREO: c_int = 1 << 7;

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
    pub fn cxmp_scan(
        c: Context,
        time_ms: *mut c_int,
        pos: *mut c_int,
        row: *mut c_int,
        ch: *mut Chan,
        max: c_int,
    ) -> c_int;
    pub fn cxmp_num_samples(c: Context) -> c_int;
    pub fn cxmp_num_instruments(c: Context) -> c_int;
    pub fn cxmp_instrument_name(c: Context, i: c_int) -> *const c_char;
    pub fn cxmp_sample(
        c: Context,
        i: c_int,
        name: *mut *const c_char,
        data: *mut *const c_void,
        len: *mut c_int,
        lps: *mut c_int,
        lpe: *mut c_int,
        flags: *mut c_int,
    ) -> c_int;
}
