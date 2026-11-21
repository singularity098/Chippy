//! The app's JavaVM and Context, for the libraries that call into Java
//! through `ndk-context`: cpal's Oboe backend asks `AudioTrack` for buffer
//! sizes while opening the sound output, and panics when no context was
//! given (the engine then plays silently). A NativeActivity sets it up by
//! itself; the Compose app hands it over with `NativeContext.init` before
//! the engine starts.

use jni::objects::JObject;
use jni::JNIEnv;
use std::sync::Once;

static INIT: Once = Once::new();

#[no_mangle]
pub extern "system" fn Java_io_github_singularity098_chippy_NativeContext_init(
    env: JNIEnv,
    _this: JObject,
    context: JObject,
) {
    INIT.call_once(|| {
        let vm = match env.get_java_vm() {
            Ok(vm) => vm,
            Err(e) => return log::error!("no JavaVM for the sound output: {e}"),
        };
        // Kept for the life of the process.
        let context = match env.new_global_ref(context) {
            Ok(g) => g,
            Err(e) => return log::error!("no Context for the sound output: {e}"),
        };
        let raw = context.as_obj().as_raw();
        std::mem::forget(context);
        // SAFETY: both pointers stay valid for the life of the process, and
        // `INIT` makes this the only call.
        unsafe {
            ndk_context::initialize_android_context(vm.get_java_vm_pointer().cast(), raw.cast())
        };
    });
}
