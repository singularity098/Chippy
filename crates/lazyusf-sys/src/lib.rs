//! Raw FFI to lazyusf2. See `usf/usf.h` in the submodule for documentation.
// Links zlib, which lazyusf2 uses for adler32.
use libz_sys as _;
use std::os::raw::{c_char, c_int, c_void};

extern "C" {
    pub fn usf_get_state_size() -> usize;
    pub fn usf_clear(state: *mut c_void);
    pub fn usf_set_compare(state: *mut c_void, enable: c_int);
    pub fn usf_set_fifo_full(state: *mut c_void, enable: c_int);
    pub fn usf_set_hle_audio(state: *mut c_void, enable: c_int);
    pub fn usf_upload_section(state: *mut c_void, data: *const u8, size: usize) -> c_int;
    pub fn usf_render(
        state: *mut c_void,
        buffer: *mut i16,
        count: usize,
        sample_rate: *mut i32,
    ) -> *const c_char;
    pub fn usf_render_resampled(
        state: *mut c_void,
        buffer: *mut i16,
        count: usize,
        sample_rate: i32,
    ) -> *const c_char;
    pub fn usf_restart(state: *mut c_void);
    pub fn usf_shutdown(state: *mut c_void);
}
