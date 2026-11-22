//! Chippy's playback engine, free of any UI toolkit: the sound output, the
//! player thread that decodes ahead of it, the spectrum, album art,
//! exporting, and the guitar tab view. The desktop app (`crates/chippy`, Slint) and the Android
//! library (`crates/chippy-android`, Jetpack Compose through UniFFI) both
//! drive it.

pub mod art;
pub mod audio;
pub mod export;
pub mod midimode;
pub mod player;
pub mod spectrum;
pub mod tabs;

/// Lowers the calling thread's priority, for work in the background
/// (reading tags, which can mean running a whole track through its
/// emulator; album art; searching; exporting): the player thread and the
/// window come first, so a busy moment does not starve the sound.
pub fn background_thread() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: plain system calls on this thread; nice values are per thread
    // on Linux.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
}
