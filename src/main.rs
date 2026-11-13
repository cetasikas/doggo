// Keep a console for debug logs. Release builds run without a console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod calc;
mod channel;
mod commands;
mod config;
#[allow(dead_code)]
mod desk;
mod dock;
mod drag;
mod editor;
mod files;
mod history;
mod hotkey;
mod indexer;
// Settings also uses package-manager detection and update instructions.
#[allow(dead_code)]
mod install;
mod launch;
mod morph;
mod render;
mod search;
mod startup;
mod surface;
mod system;
mod taskbar;
mod theme;
mod tray;
mod tray_host;
mod util;

fn main() {
    // Windows lets a child inherit its parent's DPI awareness. Explorer (the
    // normal start-up parent) is per-monitor aware, but a terminal or script
    // can be unaware, which would virtualize every window. Ask for the same
    // awareness explicitly so every launch draws in real pixels.
    set_dpi_awareness();
    if taskbar::dispatch_watchdog() {
        return;
    }
    if let Err(e) = app::run() {
        eprintln!("Doggo could not start: {}", e.message());
        util::error_box(&format!("Doggo could not start:\n{}", e.message()));
    }
}

fn set_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
    // Fails harmlessly when the process already inherited awareness.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
