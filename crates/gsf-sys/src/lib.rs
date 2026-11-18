//! Raw FFI to Chippy's shim over mGBA (`shim.c`).
#[repr(C)]
pub struct gsf {
    _p: [u8; 0],
}

extern "C" {
    pub fn gsf_open(rom: *const u8, size: usize, rate: u32) -> *mut gsf;
    pub fn gsf_render(g: *mut gsf, out: *mut i16, frames: u32);
    pub fn gsf_close(g: *mut gsf);
}
