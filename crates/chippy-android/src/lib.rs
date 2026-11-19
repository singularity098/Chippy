//! The library Android's NativeActivity loads: its entry point, and the
//! native methods the Java side (MainActivity) calls back into.
#![cfg(target_os = "android")]

use jni::objects::{JClass, JString};
use jni::JNIEnv;

#[no_mangle]
fn android_main(app: chippy::android::AndroidApp) {
    chippy::android::main(app);
}

/// MainActivity.nativeCommand: a media button, the notification's
/// controls, a permission granted, the back button...
#[no_mangle]
pub extern "system" fn Java_io_github_singularity098_chippy_MainActivity_nativeCommand(
    mut env: JNIEnv,
    _class: JClass,
    command: JString,
) {
    if let Ok(s) = env.get_string(&command) {
        chippy::android::command(&String::from(s));
    }
}
