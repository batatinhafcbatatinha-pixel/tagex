#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod db;
mod i18n;
mod model;
mod scanner;
mod smb;
mod theme;
mod thumbs;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tagex")
            .with_inner_size([1180.0, 680.0])
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Tagex", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
