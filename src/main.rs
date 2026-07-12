#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod claude_runner;
mod config;
mod logger;
mod scheduler;
mod usage_parser;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 520.0])
            .with_min_inner_size([320.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Claude Timer Reset",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
