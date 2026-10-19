//! DataTired — a DataGrip-style database IDE in Rust (phase 1: MongoDB).

mod app;
mod config;
mod dialog;
mod editor;
mod files;
mod grid;
mod icons;
mod modify;
mod mongo;
mod pg;
mod projects;
mod theme;
#[cfg(test)]
mod ui_tests;
mod widgets;

/// Tests that point HOME at a temp dir run one at a time.
#[cfg(test)]
pub static TEST_HOME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Default result page size (DataGrip shows 1-100 for MongoDB collections).
pub const PAGE: u64 = 100;

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DataTired")
            .with_app_id("DataTired")
            .with_icon(std::sync::Arc::new(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).expect("icon png")))
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 560.0])
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        ..Default::default()
    };
    eframe::run_native("DataTired", opts, Box::new(|cc| Ok(Box::new(projects::App::new(cc)))))
}
