#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod claude_runner;
mod config;
mod logger;
mod scheduler;
mod usage_parser;

use eframe::egui;

/// Window/taskbar icon, embedded at compile time (the exe icon itself
/// is set separately via build.rs / winresource).
fn load_app_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon.png");
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let img = img.into_rgba8();
            let (width, height) = img.dimensions();
            egui::IconData {
                rgba: img.into_raw(),
                width,
                height,
            }
        }
        Err(_) => egui::IconData::default(),
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 520.0])
            .with_min_inner_size([320.0, 400.0])
            .with_icon(load_app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Claude Timer Reset",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
