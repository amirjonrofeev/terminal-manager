#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod client;
mod term;
mod ui;

use tm_core::paths::{self, HostMode};
use tm_core::persist;

fn main() -> eframe::Result {
    // `--host`: run as the per-user background host. The GUI starts itself this way when no
    // host is running, so one executable is all a developer needs.
    if std::env::args().nth(1).as_deref() == Some("--host") {
        if let Err(e) = tm_host::run(HostMode::User, false) {
            eprintln!("terminal-manager --host: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let ui_path = paths::gui_config_dir().join("ui.json");
    let ui_state = persist::load::<app::UiState>(&ui_path);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Terminal Manager")
        .with_inner_size([1200.0, 760.0])
        .with_min_inner_size([640.0, 400.0]);
    if let Some(w) = ui_state.value.window {
        viewport = viewport.with_inner_size([w.width, w.height]).with_maximized(w.maximized);
        if let (Some(x), Some(y)) = (w.x, w.y) {
            viewport = viewport.with_position([x, y]);
        }
    }

    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Terminal Manager",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, ui_path, ui_state)))),
    )
}
