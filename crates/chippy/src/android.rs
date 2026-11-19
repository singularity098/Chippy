//! Chippy on Android: start-up inside the NativeActivity, and the bridge to
//! the Java side (`android/app/src/main/java/.../MainActivity.java`), which
//! owns the media session, its notification and the foreground service
//! that keeps the music playing in the background.
//!
//! Rust calls the activity's methods through JNI; the activity sends
//! commands back (media buttons, the back button, storage granted) through
//! `MainActivity.nativeCommand`, which lands in [`command`].

use crate::media::{Playback, Request};
use crossbeam_channel::{Receiver, Sender};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JavaVM;
use std::sync::{Mutex, OnceLock};

pub use slint::android::AndroidApp;

struct Java {
    vm: JavaVM,
    activity: GlobalRef,
}

static JAVA: OnceLock<Java> = OnceLock::new();
/// Where commands from Java go: the media requests of the running app.
static COMMANDS: Mutex<Option<Sender<Request>>> = Mutex::new(None);

/// The library's entry point (called by `android_main`).
pub fn main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("chippy"),
    );
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}");
    }));

    // Settings live in the app's private folder; the file browser starts
    // in shared storage (where Music, Download... are).
    if let Some(dir) = app.internal_data_path() {
        std::env::set_var("CHIPPY_HOME", &dir);
        std::env::set_var("TMPDIR", dir.join("tmp"));
        let _ = std::fs::create_dir_all(dir.join("tmp"));
    }
    std::env::set_var("HOME", "/storage/emulated/0");

    // SAFETY: the pointers come from android-activity and stay valid for
    // the life of the process (the VM) and of the activity.
    let java = unsafe {
        jni::JavaVM::from_raw(app.vm_as_ptr() as *mut _).and_then(|vm| {
            let activity = {
                let env = vm.attach_current_thread_permanently()?;
                let obj = JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject);
                env.new_global_ref(obj)?
            };
            Ok(Java { vm, activity })
        })
    };
    match java {
        Ok(j) => {
            let _ = JAVA.set(j);
        }
        Err(e) => log::error!("no JNI access to the activity: {e}"),
    }

    if let Err(e) = slint::android::init(app) {
        log::error!("cannot start the Android backend: {e}");
        return;
    }
    if let Err(e) = crate::run(crate::StartOptions::default()) {
        log::error!("{e:#}");
    }
    call_activity("onNativeExit", "()V", &[]);
}

/// A command from the Java side.
pub fn command(cmd: &str) {
    let (name, arg) = cmd.split_once(':').unwrap_or((cmd, ""));
    let request = match name {
        "play" => Request::Play,
        "pause" => Request::Pause,
        "toggle" => Request::Toggle,
        "next" => Request::Next,
        "previous" => Request::Previous,
        "stop" => Request::Stop,
        "seek" => match arg.parse() {
            Ok(ms) => Request::SeekTo(ms),
            Err(_) => return,
        },
        "back" => Request::Back,
        "refresh" => Request::Refresh,
        // "folder:<which>:<path>", the path empty when nothing was chosen.
        "folder" => {
            let (which, path) = arg.split_once(':').unwrap_or((arg, ""));
            Request::Folder(
                which.to_string(),
                (!path.is_empty()).then(|| std::path::PathBuf::from(path)),
            )
        }
        _ => {
            log::warn!("unknown command from Java: {cmd}");
            return;
        }
    };
    if let Some(tx) = COMMANDS.lock().unwrap().as_ref() {
        let _ = tx.send(request);
    }
    // Wake the event loop, which may be waiting for its next timer.
    let _ = slint::invoke_from_event_loop(|| {});
}

/// Sends the app to the background (Back with nothing left to close).
pub fn move_to_back() {
    call_activity("moveToBack", "()V", &[]);
}

/// Shows the system's folder picker; the folder chosen comes back as a
/// `folder:<which>:<path>` command. False if it could not be shown.
pub fn pick_folder(which: &str, start: Option<&std::path::Path>) -> bool {
    let Some(java) = JAVA.get() else { return false };
    let Ok(env) = java.vm.attach_current_thread_permanently() else {
        return false;
    };
    let start = start
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (Ok(w), Ok(s)) = (env.new_string(which), env.new_string(start)) else {
        return false;
    };
    call_activity(
        "pickFolder",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        &[JValue::Object(&w), JValue::Object(&s)],
    );
    true
}

fn call_activity(name: &str, sig: &str, args: &[JValue]) {
    let Some(java) = JAVA.get() else { return };
    let result = java
        .vm
        .attach_current_thread_permanently()
        .and_then(|mut env| {
            let r = env.call_method(java.activity.as_obj(), name, sig, args);
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            r.map(|_| ())
        });
    if let Err(e) = result {
        log::warn!("MainActivity.{name} failed: {e}");
    }
}

/// The media session (lock screen, notification, headset buttons), the
/// Android counterpart of the desktop's MPRIS controls.
pub struct Media {
    pub requests: Receiver<Request>,
    shown: Option<(String, String, String, Option<u64>)>,
    playback: Option<Playback>,
}

impl Media {
    pub fn connect() -> Media {
        let (tx, rx) = crossbeam_channel::unbounded();
        *COMMANDS.lock().unwrap() = Some(tx);
        Media {
            requests: rx,
            shown: None,
            playback: None,
        }
    }

    pub fn show(&mut self, title: &str, artist: &str, album: &str, length_ms: Option<u64>) {
        let now = (title.into(), artist.into(), album.into(), length_ms);
        if self.shown.as_ref() == Some(&now) {
            return;
        }
        self.shown = Some(now);
        let Some(java) = JAVA.get() else { return };
        let Ok(env) = java.vm.attach_current_thread_permanently() else {
            return;
        };
        let (Ok(t), Ok(ar), Ok(al)) = (
            env.new_string(title),
            env.new_string(artist),
            env.new_string(album),
        ) else {
            return;
        };
        let length = length_ms.map(|l| l as i64).unwrap_or(-1);
        call_activity(
            "updateMetadata",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;J)V",
            &[
                JValue::Object(&t),
                JValue::Object(&ar),
                JValue::Object(&al),
                JValue::Long(length),
            ],
        );
    }

    pub fn set_playback(&mut self, p: Playback, position_ms: u64) {
        if self.playback == Some(p) {
            return;
        }
        self.playback = Some(p);
        let state = match p {
            Playback::Stopped => 0,
            Playback::Playing => 1,
            Playback::Paused => 2,
        };
        call_activity(
            "updatePlayback",
            "(IJ)V",
            &[JValue::Int(state), JValue::Long(position_ms as i64)],
        );
    }

    /// The cover for the notification and the lock screen (None: none).
    pub fn set_art(&mut self, art: Option<&crate::art::Picture>) {
        let Some(java) = JAVA.get() else { return };
        let Ok(env) = java.vm.attach_current_thread_permanently() else {
            return;
        };
        let (array, w, h) = match art {
            Some(p) => {
                // Android's bitmaps hold premultiplied alpha.
                let mut px = p.rgba.clone();
                for c in px.chunks_exact_mut(4) {
                    let a = c[3] as u16;
                    if a < 255 {
                        for v in &mut c[..3] {
                            *v = ((*v as u16 * a + 127) / 255) as u8;
                        }
                    }
                }
                match env.byte_array_from_slice(&px) {
                    Ok(a) => (JObject::from(a), p.width as i32, p.height as i32),
                    Err(_) => return,
                }
            }
            None => (JObject::null(), 0, 0),
        };
        call_activity(
            "updateArt",
            "([BII)V",
            &[JValue::Object(&array), JValue::Int(w), JValue::Int(h)],
        );
    }

    /// A seek: the session's position has to be told afresh.
    pub fn moved(&mut self) {
        self.playback = None;
    }

    pub fn set_volume(&mut self, _v: f32) {}
}
