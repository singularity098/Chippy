//! Raw FFI to libarchive's reading API (see `archive.h` in the submodule).
#![allow(non_camel_case_types)]
use std::os::raw::{c_char, c_int, c_void};

#[repr(C)]
pub struct archive {
    _p: [u8; 0],
}
#[repr(C)]
pub struct archive_entry {
    _p: [u8; 0],
}

pub const ARCHIVE_EOF: c_int = 1;
pub const ARCHIVE_OK: c_int = 0;
pub const ARCHIVE_WARN: c_int = -20;
pub const AE_IFDIR: u32 = 0o040000;
pub const AE_IFREG: u32 = 0o100000;

extern "C" {
    pub fn archive_read_new() -> *mut archive;
    pub fn archive_read_support_format_rar(a: *mut archive) -> c_int;
    pub fn archive_read_support_format_rar5(a: *mut archive) -> c_int;
    pub fn archive_read_open_filename(
        a: *mut archive,
        filename: *const c_char,
        block_size: usize,
    ) -> c_int;
    /// The path as UTF-16, for Windows paths outside the ANSI code page.
    #[cfg(windows)]
    pub fn archive_read_open_filename_w(
        a: *mut archive,
        filename: *const u16,
        block_size: usize,
    ) -> c_int;
    pub fn archive_read_open_memory(a: *mut archive, buf: *const c_void, size: usize) -> c_int;
    pub fn archive_read_next_header(a: *mut archive, entry: *mut *mut archive_entry) -> c_int;
    pub fn archive_read_data(a: *mut archive, buf: *mut c_void, size: usize) -> isize;
    pub fn archive_read_data_skip(a: *mut archive) -> c_int;
    pub fn archive_read_free(a: *mut archive) -> c_int;
    pub fn archive_error_string(a: *mut archive) -> *const c_char;
    pub fn archive_entry_pathname(e: *mut archive_entry) -> *const c_char;
    pub fn archive_entry_pathname_utf8(e: *mut archive_entry) -> *const c_char;
    pub fn archive_entry_size(e: *mut archive_entry) -> i64;
    pub fn archive_entry_filetype(e: *mut archive_entry) -> u32;
}
