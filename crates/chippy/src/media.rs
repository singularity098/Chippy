//! The system's media controls: media keys, and the player widgets of the
//! desktop (MPRIS on Linux, Now Playing on macOS, the media overlay on
//! Windows). Their requests come in
//! on a channel the clock reads; what plays is told back to them. (The
//! Android app has its own media session, in Kotlin.)

#[cfg(not(target_os = "android"))]
pub use desktop::Media;

/// What the system asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    /// Seconds, forwards or back.
    SeekBy(i64),
    SeekTo(u64),
    Volume(f32),
    Raise,
    Quit,
    /// Android's back button or gesture.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    Back,
    /// Storage access was granted (Android): list the folder again.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    Refresh,
    /// The system's folder picker came back (Android): which setting it
    /// was for, and the folder, if one was chosen.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    Folder(String, Option<std::path::PathBuf>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    Stopped,
    Playing,
    Paused,
}

#[cfg(not(target_os = "android"))]
mod desktop {
    use super::{Playback, Request};
    use crossbeam_channel::{Receiver, Sender};
    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig,
    };
    use std::time::Duration;

    pub struct Media {
        controls: Option<MediaControls>,
        pub requests: Receiver<Request>,
        shown: Option<(String, String, String, Option<u64>)>,
        playback: Option<Playback>,
        /// The cover, written to a file for the desktop's player widgets.
        cover: Option<std::path::PathBuf>,
        covers_written: u64,
        /// Windows ties the controls to a window: they are attached once it
        /// is on screen, with this end of the channel.
        #[cfg(windows)]
        pending: Option<Sender<Request>>,
    }

    fn translate(e: MediaControlEvent) -> Option<Request> {
        use souvlaki::SeekDirection::*;
        Some(match e {
            MediaControlEvent::Play => Request::Play,
            MediaControlEvent::Pause => Request::Pause,
            MediaControlEvent::Toggle => Request::Toggle,
            MediaControlEvent::Next => Request::Next,
            MediaControlEvent::Previous => Request::Previous,
            MediaControlEvent::Stop => Request::Stop,
            MediaControlEvent::Seek(Forward) => Request::SeekBy(10),
            MediaControlEvent::Seek(Backward) => Request::SeekBy(-10),
            MediaControlEvent::SeekBy(Forward, d) => Request::SeekBy(d.as_secs() as i64),
            MediaControlEvent::SeekBy(Backward, d) => Request::SeekBy(-(d.as_secs() as i64)),
            MediaControlEvent::SetPosition(p) => Request::SeekTo(p.0.as_millis() as u64),
            MediaControlEvent::SetVolume(v) => Request::Volume(v as f32),
            MediaControlEvent::Raise => Request::Raise,
            MediaControlEvent::Quit => Request::Quit,
            _ => return None,
        })
    }

    impl Media {
        /// Connects to the system's media controls; without them (no session
        /// bus, say) the player works as before.
        pub fn connect() -> Media {
            let (tx, rx) = crossbeam_channel::unbounded();
            #[cfg(not(windows))]
            let controls = Self::attach(tx, None)
                .map_err(|e| log::info!("no system media controls: {e}"))
                .ok();
            #[cfg(windows)]
            let controls = None;
            Media {
                controls,
                requests: rx,
                shown: None,
                playback: None,
                cover: None,
                covers_written: 0,
                #[cfg(windows)]
                pending: Some(tx),
            }
        }

        /// Windows: connects to the media controls for the window, once it
        /// has one (the first call; later ones do nothing).
        #[cfg(windows)]
        pub fn attach_window(&mut self, hwnd: *mut std::ffi::c_void) {
            let Some(tx) = self.pending.take() else {
                return;
            };
            self.controls = Self::attach(tx, Some(hwnd))
                .map_err(|e| log::info!("no system media controls: {e}"))
                .ok();
        }

        fn attach(
            tx: Sender<Request>,
            hwnd: Option<*mut std::ffi::c_void>,
        ) -> Result<MediaControls, String> {
            // Souvlaki's D-Bus thread panics when there is no session bus.
            #[cfg(target_os = "linux")]
            if !session_bus_present() {
                return Err("no D-Bus session bus".into());
            }
            // Each instance its own name, as MPRIS asks (a taken name would
            // fail the connection).
            let dbus_name = format!("chippy.instance{}", std::process::id());
            let config = PlatformConfig {
                display_name: "Chippy",
                dbus_name: &dbus_name,
                hwnd,
            };
            let mut c = MediaControls::new(config).map_err(|e| format!("{e:?}"))?;
            c.attach(move |e| {
                if let Some(r) = translate(e) {
                    let _ = tx.send(r);
                }
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(c)
        }

        /// Tells the system what plays (called whenever it may have changed;
        /// only changes are passed on).
        pub fn show(&mut self, title: &str, artist: &str, album: &str, length_ms: Option<u64>) {
            let Some(c) = self.controls.as_mut() else {
                return;
            };
            let now = (title.into(), artist.into(), album.into(), length_ms);
            if self.shown.as_ref() == Some(&now) {
                return;
            }
            fn none_if_empty(s: &str) -> Option<&str> {
                (!s.is_empty()).then_some(s)
            }
            let cover_url = self
                .cover
                .as_ref()
                .map(|p| format!("file://{}", p.display()));
            let _ = c.set_metadata(MediaMetadata {
                title: none_if_empty(title),
                artist: none_if_empty(artist),
                album: none_if_empty(album),
                duration: length_ms.map(Duration::from_millis),
                cover_url: cover_url.as_deref(),
            });
            self.shown = Some(now);
        }

        /// The current track's cover (None: it has none). Saved as a PNG
        /// under a new name each time, so the widgets notice the change.
        pub fn set_art(&mut self, art: Option<&crate::art::Picture>) {
            if self.controls.is_none() {
                return;
            }
            if let Some(old) = self.cover.take() {
                let _ = std::fs::remove_file(old);
            }
            if let Some(p) = art {
                let dir = std::env::var_os("XDG_RUNTIME_DIR")
                    .map(std::path::PathBuf::from)
                    .filter(|d| d.is_dir())
                    .unwrap_or_else(std::env::temp_dir);
                self.covers_written += 1;
                let path = dir.join(format!(
                    "chippy-cover-{}-{}.png",
                    std::process::id(),
                    self.covers_written
                ));
                match image::save_buffer(
                    &path,
                    &p.rgba,
                    p.width,
                    p.height,
                    image::ExtendedColorType::Rgba8,
                ) {
                    Ok(()) => self.cover = Some(path),
                    Err(e) => log::debug!("cannot save the cover for MPRIS: {e}"),
                }
            }
            // Tell the widgets again, with the new cover.
            if let Some((t, ar, al, len)) = self.shown.take() {
                self.show(&t, &ar, &al, len);
            }
        }

        pub fn set_playback(&mut self, p: Playback, position_ms: u64) {
            let Some(c) = self.controls.as_mut() else {
                return;
            };
            if self.playback == Some(p) {
                return;
            }
            let progress = Some(souvlaki::MediaPosition(Duration::from_millis(position_ms)));
            let _ = c.set_playback(match p {
                Playback::Stopped => MediaPlayback::Stopped,
                Playback::Playing => MediaPlayback::Playing { progress },
                Playback::Paused => MediaPlayback::Paused { progress },
            });
            self.playback = Some(p);
        }

        /// A seek: the system's position has to be told afresh.
        pub fn moved(&mut self) {
            self.playback = None;
        }

        #[cfg(target_os = "linux")]
        pub fn set_volume(&mut self, v: f32) {
            if let Some(c) = self.controls.as_mut() {
                let _ = c.set_volume(v as f64);
            }
        }

        #[cfg(not(target_os = "linux"))]
        pub fn set_volume(&mut self, _v: f32) {}
    }

    impl Drop for Media {
        fn drop(&mut self) {
            if let Some(old) = self.cover.take() {
                let _ = std::fs::remove_file(old);
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn session_bus_present() -> bool {
        if let Some(addr) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") {
            return !addr.is_empty();
        }
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|d| std::path::Path::new(&d).join("bus").exists())
            .unwrap_or(false)
    }
}
