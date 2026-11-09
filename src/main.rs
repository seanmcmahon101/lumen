#![windows_subsystem = "windows"]

mod app;
mod automation;
mod calc;
mod command;
mod convert;
mod desktop;
mod hook;
mod index;
mod library;
mod mail;
mod metrics;
mod preview;
mod settings;
mod storage;
mod system;
mod tray;
mod update;
mod watch;
mod win;

use eframe::egui;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--mail-query") {
        crate::mail::worker();
        return Ok(());
    }
    // An isolated preview can coexist with an installed Lumen during development.
    let isolated = std::env::args().any(|a| a == "--isolated");
    // One instance only. Starting Lumen again opens the one already running.
    if !isolated {
        unsafe {
            let _ = CreateMutexW(None, true, w!("Local\\Lumen.Launcher"));
            if GetLastError() == ERROR_ALREADY_EXISTS {
                if !std::env::args().any(|a| a == "--background") {
                    win::signal_running();
                }
                return Ok(());
            }
        }
        win::listen_for_relaunch();
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lumen")
            .with_decorations(false)
            .with_transparent(false)
            .with_always_on_top()
            .with_taskbar(isolated)
            .with_visible(false)
            .with_inner_size([800.0, 600.0])
            // eframe force-shows the window after its first paint; keep that off-screen.
            .with_position([-32000.0, -32000.0]),
        // Lumen reads the clipboard itself, so a folder copied in File Explorer pastes as its path.
        clipboard_shortcuts: false,
        ..Default::default()
    };
    eframe::run_native("Lumen", options, Box::new(|cc| Ok(Box::new(app::Lumen::new(cc)))))
}
