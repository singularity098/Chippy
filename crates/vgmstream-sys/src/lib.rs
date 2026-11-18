//! Raw FFI to libvgmstream's public API (`src/libvgmstream.h`, API 1.1).
#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_void};

pub const LIBVGMSTREAM_SFMT_PCM16: c_int = 1;
pub const LIBVGMSTREAM_SFMT_PCM24: c_int = 2;
pub const LIBVGMSTREAM_SFMT_PCM32: c_int = 3;
pub const LIBVGMSTREAM_SFMT_FLOAT: c_int = 4;

#[repr(C)]
pub struct libvgmstream_format_t {
    pub channels: c_int,
    pub sample_rate: c_int,
    pub sample_format: c_int,
    pub sample_size: c_int,
    pub channel_layout: u32,
    pub subsong_index: c_int,
    pub subsong_count: c_int,
    pub input_channels: c_int,
    pub stream_samples: i64,
    pub loop_start: i64,
    pub loop_end: i64,
    pub loop_flag: bool,
    pub play_forever: bool,
    pub play_samples: i64,
    pub stream_bitrate: c_int,
    pub codec_name: [c_char; 128],
    pub layout_name: [c_char; 128],
    pub meta_name: [c_char; 128],
    pub stream_name: [c_char; 256],
    pub format_id: c_int,
}

#[repr(C)]
pub struct libvgmstream_decoder_t {
    pub buf: *mut c_void,
    pub buf_samples: c_int,
    pub buf_bytes: c_int,
    pub done: bool,
}

#[repr(C)]
pub struct libvgmstream_t {
    pub priv_: *mut c_void,
    pub format: *const libvgmstream_format_t,
    pub decoder: *mut libvgmstream_decoder_t,
}

#[repr(C)]
#[derive(Default)]
pub struct libvgmstream_config_t {
    pub disable_config_override: bool,
    pub allow_play_forever: bool,
    pub play_forever: bool,
    pub ignore_loop: bool,
    pub force_loop: bool,
    pub really_force_loop: bool,
    pub ignore_fade: bool,
    pub loop_count: f64,
    pub fade_time: f64,
    pub fade_delay: f64,
    pub stereo_track: c_int,
    pub auto_downmix_channels: c_int,
    pub force_sfmt: c_int,
}

#[repr(C)]
pub struct libstreamfile_t {
    pub user_data: *mut c_void,
    pub read: Option<unsafe extern "C" fn(*mut c_void, *mut u8, i64, c_int) -> c_int>,
    pub get_size: Option<unsafe extern "C" fn(*mut c_void) -> i64>,
    pub get_name: Option<unsafe extern "C" fn(*mut c_void) -> *const c_char>,
    pub open: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut libstreamfile_t>,
    pub close: Option<unsafe extern "C" fn(*mut libstreamfile_t)>,
}

#[repr(C)]
pub struct libvgmstream_title_t {
    pub force_title: bool,
    pub subsong_range: bool,
    pub remove_extension: bool,
    pub remove_archive: bool,
    pub filename: *const c_char,
}

pub const LIBVGMSTREAM_LOG_LEVEL_NONE: c_int = 100;

extern "C" {
    pub fn libvgmstream_get_version() -> u32;
    pub fn libvgmstream_init() -> *mut libvgmstream_t;
    pub fn libvgmstream_free(lib: *mut libvgmstream_t);
    pub fn libvgmstream_setup(lib: *mut libvgmstream_t, cfg: *mut libvgmstream_config_t);
    pub fn libvgmstream_open_stream(
        lib: *mut libvgmstream_t,
        libsf: *mut libstreamfile_t,
        subsong: c_int,
    ) -> c_int;
    pub fn libvgmstream_close_stream(lib: *mut libvgmstream_t);
    pub fn libvgmstream_render(lib: *mut libvgmstream_t) -> c_int;
    pub fn libvgmstream_get_play_position(lib: *mut libvgmstream_t) -> i64;
    pub fn libvgmstream_seek(lib: *mut libvgmstream_t, sample: i64);
    pub fn libvgmstream_set_log(
        level: c_int,
        callback: Option<unsafe extern "C" fn(c_int, *const c_char)>,
    );
    pub fn libvgmstream_get_extensions(size: *mut c_int) -> *mut *const c_char;
    pub fn libvgmstream_get_common_extensions(size: *mut c_int) -> *mut *const c_char;
    pub fn libvgmstream_get_title(
        lib: *mut libvgmstream_t,
        cfg: *mut libvgmstream_title_t,
        buf: *mut c_char,
        buf_len: c_int,
    ) -> c_int;
    pub fn libstreamfile_close(libsf: *mut libstreamfile_t);
    pub fn libstreamfile_open_from_stdio(filename: *const c_char) -> *mut libstreamfile_t;
    pub fn libstreamfile_open_buffered(ext: *mut libstreamfile_t) -> *mut libstreamfile_t;
}
