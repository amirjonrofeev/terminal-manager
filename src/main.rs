#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod model;
mod persist;
mod session;
mod shell;
mod term;
mod ui;

fn main() -> eframe::Result {
    term::setup_env();
    let store_path = persist::workspace_path();
    let loaded = persist::load(&store_path);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Terminal Manager")
        .with_inner_size([1200.0, 760.0])
        .with_min_inner_size([640.0, 400.0]);
    if let Some(w) = loaded.workspace.window {
        viewport = viewport.with_inner_size([w.width, w.height]).with_maximized(w.maximized);
        if let (Some(x), Some(y)) = (w.x, w.y) {
            viewport = viewport.with_position([x, y]);
        }
    }

    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Terminal Manager",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, store_path, loaded)))),
    )
}
