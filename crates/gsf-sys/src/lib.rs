//! Raw FFI to Chippy's shim over mGBA (`shim.c`).
#[repr(C)]
pub struct gsf {
    _p: [u8; 0],
}

extern "C" {
    pub fn gsf_open(rom: *const u8, size: usize, rate: u32) -> *mut gsf;
    pub fn gsf_render(g: *mut gsf, out: *mut i16, frames: u32);
    pub fn gsf_close(g: *mut gsf);

    /// Installs (or with `None`, removes) the hook for writes to the sound
    /// registers.
    pub fn gsf_set_notes_hook(g: *mut gsf, f: Option<NotesHook>, user: *mut std::os::raw::c_void);
    /// Runs one video frame, throwing its sound away.
    pub fn gsf_run_frame(g: *mut gsf);
    /// The CPU's cycles (16.78 MHz) since it started, in 32 bits (they wrap).
    pub fn gsf_cycles(g: *mut gsf) -> u64;
    pub fn gsf_read8(g: *mut gsf, address: u32) -> u32;
    pub fn gsf_read32(g: *mut gsf, address: u32) -> u32;
}

/// (user, cycles (as `gsf_cycles`), I/O register offset ($60-$89), value,
/// bytes (1, 2)).
pub type NotesHook = unsafe extern "C" fn(
    user: *mut std::os::raw::c_void,
    cycles: u64,
    address: u32,
    value: u32,
    bytes: std::os::raw::c_int,
);
