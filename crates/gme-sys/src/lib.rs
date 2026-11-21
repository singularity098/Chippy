//! Raw FFI to Game Music Emu. See `gme.h` in the submodule for documentation.
#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_long, c_short, c_void};

#[repr(C)]
pub struct Music_Emu {
    _private: [u8; 0],
}

pub type gme_err_t = *const c_char;
pub type gme_type_t = *const c_void;

#[repr(C)]
pub struct gme_info_t {
    pub length: c_int,
    pub intro_length: c_int,
    pub loop_length: c_int,
    pub play_length: c_int,
    pub fade_length: c_int,
    pub reserved_i: [c_int; 11],
    pub system: *const c_char,
    pub game: *const c_char,
    pub song: *const c_char,
    pub author: *const c_char,
    pub copyright: *const c_char,
    pub comment: *const c_char,
    pub dumper: *const c_char,
    pub reserved_s: [*const c_char; 9],
}

extern "C" {
    pub fn gme_open_data(
        data: *const c_void,
        size: c_long,
        out: *mut *mut Music_Emu,
        sample_rate: c_int,
    ) -> gme_err_t;
    pub fn gme_track_count(emu: *const Music_Emu) -> c_int;
    pub fn gme_start_track(emu: *mut Music_Emu, index: c_int) -> gme_err_t;
    pub fn gme_play(emu: *mut Music_Emu, count: c_int, out: *mut c_short) -> gme_err_t;
    pub fn gme_delete(emu: *mut Music_Emu);
    pub fn gme_set_fade_msecs(emu: *mut Music_Emu, start_msec: c_int, length_msecs: c_int);
    pub fn gme_set_autoload_playback_limit(emu: *mut Music_Emu, do_autoload_limit: c_int);
    pub fn gme_track_ended(emu: *const Music_Emu) -> c_int;
    pub fn gme_tell(emu: *const Music_Emu) -> c_int;
    pub fn gme_seek(emu: *mut Music_Emu, msec: c_int) -> gme_err_t;
    pub fn gme_track_info(
        emu: *const Music_Emu,
        out: *mut *mut gme_info_t,
        track: c_int,
    ) -> gme_err_t;
    pub fn gme_free_info(info: *mut gme_info_t);
    pub fn gme_ignore_silence(emu: *mut Music_Emu, ignore: c_int);
    pub fn gme_enable_accuracy(emu: *mut Music_Emu, enabled: c_int);
    pub fn gme_type(emu: *const Music_Emu) -> gme_type_t;
    pub fn gme_type_system(t: gme_type_t) -> *const c_char;
    pub fn gme_identify_extension(path_or_ext: *const c_char) -> gme_type_t;
    pub fn gme_load_m3u_data(emu: *mut Music_Emu, data: *const c_void, size: c_long) -> gme_err_t;

    /// Installs (or with `None`, removes) this thread's hook for sound-chip
    /// register writes, see `hooks/config.h`. `kind` is 0 for an APU write
    /// (NES, Game Boy, PC Engine, KSS, SAP), 1 when the play routine is
    /// called, 2 for an S-DSP write (SPC, with `time` in SPC clocks).
    pub fn chippy_gme_set_hook(f: Option<chippy_gme_hook_fn>, user: *mut c_void);
}

pub type chippy_gme_hook_fn =
    unsafe extern "C" fn(user: *mut c_void, kind: c_int, time: c_long, addr: c_int, data: c_int);
