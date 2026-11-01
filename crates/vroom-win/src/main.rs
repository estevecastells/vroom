//! Vroom for Windows: a tray app with a library window. Like the Mac app it
//! records itself (Windows.Graphics.Capture + Media Foundation's H.264 encoder)
//! and runs `vroom.exe --json` for everything else: library, R2, Loom,
//! transcription. See crates/vroom-win/README.md for what is done and what is not.

// No console window for the app; `cargo run` still prints panics to the terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod cli;
// Also built for tests elsewhere, so the catalog check runs on every system.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod lang;
#[cfg(windows)]
mod prefs;
#[cfg(windows)]
mod recorder;
#[cfg(windows)]
mod tray;

#[cfg(windows)]
fn main() -> eframe::Result {
    let hidden = std::env::args().any(|a| a == "--hidden");
    // Before any window or tray text exists.
    lang::init(prefs::Prefs::load().language.as_deref());
    let icon = tray::icon_rgba(64);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Vroom")
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([640.0, 420.0])
            .with_visible(!hidden)
            .with_icon(eframe::egui::IconData { rgba: icon, width: 64, height: 64 }),
        ..Default::default()
    };
    eframe::run_native("Vroom", options, Box::new(|cc| Ok(Box::new(app::VroomApp::new(cc)))))
}

#[cfg(not(windows))]
fn main() {
    eprintln!("vroom-app es la app de Windows; en macOS usa Vroom.app (carpeta app/).");
    std::process::exit(1);
}
