#![windows_subsystem = "windows"]

mod app;
mod calc;
mod hook;
mod index;
mod win;

use eframe::egui;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

fn main() -> eframe::Result {
    // One instance only.
    unsafe {
        let _ = CreateMutexW(None, true, w!("Local\\Lumen.Launcher"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lumen")
            .with_decorations(false)
            .with_transparent(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_visible(false)
            .with_inner_size([800.0, 600.0])
            // eframe force-shows the window after its first paint; keep that off-screen.
            .with_position([-32000.0, -32000.0]),
        ..Default::default()
    };
    eframe::run_native("Lumen", options, Box::new(|cc| Ok(Box::new(app::Lumen::new(cc)))))
}
