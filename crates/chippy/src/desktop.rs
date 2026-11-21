//! Fitting in on the desktop: which window system to use, the desktop entry
//! that lets a Wayland compositor show Chippy's icon, and the window's icon
//! on Windows.

/// The id the window gives the window manager (Wayland app id, X11
/// WM_CLASS); desktop entries and taskbars match it to `chippy.desktop`.
#[cfg_attr(windows, allow(dead_code))]
pub const APP_ID: &str = "chippy";

/// Picks the winit backend: through XWayland on a Wayland desktop, unless
/// native Wayland is asked for (in Settings, or `CHIPPY_WAYLAND=1`).
pub fn select_backend(native_wayland: bool) {
    let native_wayland = match std::env::var("CHIPPY_WAYLAND").as_deref() {
        Ok("1") | Ok("true") => true,
        Ok("0") | Ok("false") => false,
        _ => native_wayland,
    };
    // Only Linux desktops have a choice to make.
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "android"))))]
    let _ = native_wayland;
    #[allow(unused_mut)]
    let mut selector = slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(window_attributes);
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
    {
        let on_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let has_x11 = std::env::var_os("DISPLAY").is_some();
        if on_wayland && has_x11 && !native_wayland {
            use slint::winit_030::winit::platform::x11::EventLoopBuilderExtX11;
            let mut b = slint::winit_030::winit::event_loop::EventLoop::<
                slint::winit_030::SlintEvent,
            >::with_user_event();
            b.with_x11();
            selector = selector.with_winit_event_loop_builder(b);
            log::info!("using XWayland (file drops and the window icon work there)");
        } else if on_wayland {
            install_desktop_entry();
        }
    }
    if let Err(e) = selector.select() {
        log::warn!("could not set up the window system as asked: {e}");
    }
}

fn window_attributes(
    a: slint::winit_030::winit::window::WindowAttributes,
) -> slint::winit_030::winit::window::WindowAttributes {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
    {
        use slint::winit_030::winit::platform::{wayland, x11};
        let a = wayland::WindowAttributesExtWayland::with_name(a, APP_ID, APP_ID);
        x11::WindowAttributesExtX11::with_name(a, APP_ID, "Chippy")
    }
    // The icon built into chippy.exe (by build.rs), for the title bar,
    // Alt+Tab and the taskbar.
    #[cfg(windows)]
    {
        use slint::winit_030::winit::platform::windows::{
            IconExtWindows, WindowAttributesExtWindows,
        };
        use slint::winit_030::winit::window::Icon;
        let big = Icon::from_resource(1, None).ok();
        let small = Icon::from_resource(1, Some((16, 16).into())).ok();
        a.with_window_icon(small).with_taskbar_icon(big)
    }
    #[cfg(not(any(
        windows,
        all(unix, not(target_os = "macos"), not(target_os = "android"))
    )))]
    a
}

/// On Wayland a window has no icon of its own: the compositor shows the one
/// of the desktop entry matching its app id. When no `chippy.desktop` is
/// installed (an AppImage, or `cargo run`), one is written to the user's
/// applications folder, with the icon beside it in the icon theme.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
fn install_desktop_entry() {
    use std::path::PathBuf;
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    let Some(data_home) = data_home else { return };
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    let installed = std::iter::once(data_home.clone())
        .chain(
            data_dirs
                .split(':')
                .filter(|d| !d.is_empty())
                .map(PathBuf::from),
        )
        .any(|d| d.join("applications/chippy.desktop").is_file());
    if installed {
        return;
    }
    // An AppImage is run through $APPIMAGE; the binary itself would be gone
    // once it exits.
    let exe = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok());
    let Some(exe) = exe else { return };
    let icon_dir = data_home.join("icons/hicolor/scalable/apps");
    let apps = data_home.join("applications");
    let entry = include_str!("../../../packaging/linux/chippy.desktop")
        .replace("Exec=chippy %f", &format!("Exec=\"{}\" %f", exe.display()));
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&icon_dir)?;
        std::fs::write(
            icon_dir.join("chippy.svg"),
            include_bytes!("../../../assets/chippy-icon.svg"),
        )?;
        std::fs::create_dir_all(&apps)?;
        std::fs::write(apps.join("chippy.desktop"), entry)
    })();
    match result {
        Ok(()) => log::info!("installed {}", apps.join("chippy.desktop").display()),
        Err(e) => log::warn!("could not install a desktop entry: {e}"),
    }
}
