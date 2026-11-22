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
    #[cfg(target_os = "macos")]
    setup_macos_bundle();

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

/// Inside "Try MAN.app", GTK must find its icons, settings schemas and image
/// loaders in the app itself, not in Homebrew.
#[cfg(target_os = "macos")]
fn setup_macos_bundle() {
    let Some(resources) = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("Resources")))
    else {
        return;
    };
    let share = resources.join("share");
    if !share.join("glib-2.0").is_dir() {
        return; // not running from the .app
    }
    // SAFETY: called first thing in main, before any other threads exist.
    unsafe {
        std::env::set_var("XDG_DATA_DIRS", &share);
        std::env::set_var("GSETTINGS_SCHEMA_DIR", share.join("glib-2.0/schemas"));
        let fonts = resources.join("etc/fonts/fonts.conf");
        if fonts.is_file() {
            std::env::set_var("FONTCONFIG_FILE", fonts);
        }
    }
    // The image loader list needs absolute paths, so it is written at startup.
    let loaders = resources.join("lib/gdk-pixbuf-2.0/2.10.0");
    if let Ok(template) = std::fs::read_to_string(loaders.join("loaders.cache.in")) {
        let cache = std::env::temp_dir().join("try-man-pixbuf-loaders.cache");
        let text = template.replace("@RESOURCES@", &resources.to_string_lossy());
        if std::fs::write(&cache, text).is_ok() {
            unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", &cache) };
        }
    }
}
