#![cfg_attr(windows, windows_subsystem = "windows")]

mod net;
mod release;
mod system;
mod vm;
mod vnc;
mod window;
#[cfg(test)]
mod selftest;

use adw::prelude::*;
use gtk::glib;

const APP_ID: &str = "io.github.mancorporation.TryMan";

fn main() -> glib::ExitCode {
    setup_bundle();

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        if let Some(existing) = app.active_window() {
            existing.present();
            return;
        }
        let tryman = window::TryMan::new(app);
        tryman.present();

        // Make sure QEMU never keeps running after the app has quit.
        app.connect_shutdown(move |_| tryman.shutdown());
    });

    app.run()
}

/// In the packaged app (macOS "Try MAN.app", the Windows folder or the Linux
/// AppImage), GTK must find its icons, settings schemas and image loaders
/// inside the app itself.
fn setup_bundle() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(parent) = exe.parent().and_then(|dir| dir.parent()) else { return };
    // macOS: Try MAN.app/Contents/MacOS/try-man -> Contents/Resources
    // Windows: Try MAN\bin\try-man.exe -> Try MAN
    // AppImage: usr/bin/try-man -> usr (its start script sets up the rest)
    let root = if cfg!(target_os = "macos") { parent.join("Resources") } else { parent.to_path_buf() };
    let share = root.join("share");
    if !share.join("glib-2.0").is_dir() {
        return; // running from a development build
    }
    // The AppImage's GTK start script forces GTK's old built-in theme, which
    // replaces libadwaita's design. libadwaita does light/dark by itself.
    // SAFETY: called first thing in main, before any other threads exist.
    #[cfg(target_os = "linux")]
    if std::env::var_os("APPDIR").is_some() {
        unsafe { std::env::remove_var("GTK_THEME") };
    }
    // SAFETY: as above.
    #[cfg(any(target_os = "macos", windows))]
    unsafe {
        std::env::set_var("XDG_DATA_DIRS", &share);
        std::env::set_var("GSETTINGS_SCHEMA_DIR", share.join("glib-2.0/schemas"));
        let fonts = root.join("etc/fonts/fonts.conf");
        if fonts.is_file() {
            std::env::set_var("FONTCONFIG_FILE", fonts);
        }
    }
    // The image loader list needs absolute paths (and the app may live
    // anywhere, an AppImage even in a new temporary folder each time), so it
    // is written at startup from a template.
    let loaders = root.join("lib/gdk-pixbuf-2.0/2.10.0");
    if let Ok(template) = std::fs::read_to_string(loaders.join("loaders.cache.in")) {
        let cache = std::env::temp_dir().join("try-man-pixbuf-loaders.cache");
        let root_text = root.to_string_lossy().replace('\\', "/");
        if std::fs::write(&cache, template.replace("@RESOURCES@", &root_text)).is_ok() {
            unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", &cache) };
        }
    }
}
