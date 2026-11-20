//! UniFFI's bindings generator, at the version chippy-android uses:
//! `cargo run -p uniffi-bindgen -- generate --library <libchippy_android.so>
//! --language kotlin --out-dir <dir>`.

fn main() {
    uniffi::uniffi_bindgen_main()
}
