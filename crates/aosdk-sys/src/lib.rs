//! Raw FFI to the Audio Overload SDK (see `third_party/aosdk/ao.h`), plus
//! the `ao_get_lib` hook the engines call to read `_lib` files: it asks the
//! reader installed with [`with_lib_reader`] on the calling thread.
use libz_sys as _;
use std::cell::RefCell;
use std::os::raw::{c_char, c_int, c_void};

/// Engine types, as `ao_identify` numbers them.
pub const TYPE_QSF: u32 = 0;
pub const TYPE_SSF: u32 = 1;
pub const TYPE_PSF: u32 = 2;
pub const TYPE_SPU: u32 = 3;
pub const TYPE_PSF2: u32 = 4;
pub const TYPE_DSF: u32 = 5;

pub const AO_SUCCESS: c_int = 1;
pub const AO_FAIL: c_int = 0;

extern "C" {
    pub fn ao_identify(buffer: *mut c_char) -> c_int;
    pub fn ao_start(kind: u32, path: *const c_char, buffer: *mut u8, size: u32) -> *mut c_void;
    pub fn ao_stop(kind: u32, handle: *mut c_void) -> c_int;
    /// Renders `size` stereo frames of 16-bit samples at 44.1 kHz.
    pub fn ao_decode(kind: u32, handle: *mut c_void, buffer: *mut i16, size: u32) -> c_int;
    fn malloc(n: usize) -> *mut c_void;
}

type Reader = dyn FnMut(&str) -> Option<Vec<u8>>;

thread_local! {
    static READER: RefCell<Option<Box<Reader>>> = RefCell::new(None);
}

/// Runs `f` (an `ao_start`) with `reader` answering the engine's requests
/// for library files by path.
pub fn with_lib_reader<T>(
    reader: impl FnMut(&str) -> Option<Vec<u8>> + 'static,
    f: impl FnOnce() -> T,
) -> T {
    READER.with(|r| *r.borrow_mut() = Some(Box::new(reader)));
    let out = f();
    READER.with(|r| *r.borrow_mut() = None);
    out
}

/// # Safety
/// Called by the engines with a NUL-terminated path and two out pointers.
#[no_mangle]
pub unsafe extern "C" fn ao_get_lib(
    filename: *const c_char,
    buffer: *mut *mut u8,
    length: *mut u64,
) -> c_int {
    let name = std::ffi::CStr::from_ptr(filename)
        .to_string_lossy()
        .into_owned();
    let data = READER.with(|r| r.borrow_mut().as_mut().and_then(|f| f(&name)));
    let Some(data) = data else { return AO_FAIL };
    // The engine frees it with free().
    let p = malloc(data.len().max(1)) as *mut u8;
    if p.is_null() {
        return AO_FAIL;
    }
    std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
    *buffer = p;
    *length = data.len() as u64;
    AO_SUCCESS
}
