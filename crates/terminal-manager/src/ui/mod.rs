//! Look and feel shared by the UI modules.

pub mod dev;
pub mod dialogs;
pub mod kind;
pub mod logs;
pub mod main_area;
pub mod sidebar;

use egui::{Color32, FontData, FontDefinitions, FontFamily, Stroke};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x15, 0x18, 0x1a);
pub const SIDE: Color32 = Color32::from_rgb(0x1b, 0x1f, 0x22);
pub const PANEL: Color32 = Color32::from_rgb(0x20, 0x25, 0x2a);
pub const LINE: Color32 = Color32::from_rgb(0x2d, 0x34, 0x39);
pub const FG: Color32 = Color32::from_rgb(0xd7, 0xdd, 0xe0);
pub const DIM: Color32 = Color32::from_rgb(0x7d, 0x8a, 0x91);
pub const ACCENT: Color32 = Color32::from_rgb(0x5f, 0xc7, 0x9a);
pub const ROW_SELECTED: Color32 = Color32::from_rgb(0x26, 0x30, 0x29);
pub const OK: Color32 = Color32::from_rgb(0x3f, 0xb6, 0x7e);
pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xa8, 0x3a);
pub const BAD: Color32 = Color32::from_rgb(0xe0, 0x60, 0x5a);
pub const IDLE: Color32 = Color32::from_rgb(0x6b, 0x76, 0x7c);

pub fn setup(ctx: &egui::Context) {
    load_mono_font(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, LINE);
        v.extreme_bg_color = BG;
        v.faint_bg_color = SIDE;
        v.selection.bg_fill = Color32::from_rgb(0x2f, 0x6a, 0x52);
        v.selection.stroke = Stroke::new(1.0, FG);
        v.hyperlink_color = ACCENT;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, FG);
        v.widgets.inactive.weak_bg_fill = PANEL;
        v.widgets.inactive.bg_fill = PANEL;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, FG);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x29, 0x30, 0x36);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(0x23, 0x4a, 0x3a);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
    });
}

/// Terminal font: an installed Nerd Font Mono if there is one (prompt glyphs), then a system
/// monospace font, then egui's bundled one.
fn load_mono_font(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut chain: Vec<PathBuf> = nerd_font().into_iter().collect();
    let system: &[&str] = if cfg!(windows) {
        &[r"C:\Windows\Fonts\CascadiaMono.ttf", r"C:\Windows\Fonts\consola.ttf"]
    } else {
        &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
        ]
    };
    chain.extend(system.iter().map(PathBuf::from).filter(|p| p.is_file()).take(1));

    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    for (i, path) in chain.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(path) {
            let name = format!("mono-{i}");
            fonts.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
            mono.insert(i, name);
        }
    }
    ctx.set_fonts(fonts);
}

fn nerd_font() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
        dirs.push(r"C:\Windows\Fonts".into());
    } else {
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&home).join(".local/share/fonts"));
            dirs.push(PathBuf::from(&home).join(".fonts"));
        }
        dirs.push("/usr/share/fonts".into());
    }
    dirs.iter().find_map(|d| find_file(d, &|name| name.ends_with("NerdFontMono-Regular.ttf"), 3))
}

fn find_file(dir: &Path, pred: &dyn Fn(&str) -> bool, depth: u32) -> Option<PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    entries
        .iter()
        .map(|e| e.path())
        .find(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(pred))
        .or_else(|| {
            (depth > 0).then(|| {
                entries.iter().map(|e| e.path()).filter(|p| p.is_dir()).find_map(|p| find_file(&p, pred, depth - 1))
            })?
        })
}

/// Small filled status dot.
pub fn dot(painter: &egui::Painter, center: egui::Pos2, color: Color32) {
    if color == OK {
        painter.circle_filled(center, 6.0, color.gamma_multiply(0.18));
    }
    painter.circle_filled(center, 4.0, color);
}

/// Width of a count pill drawn by [`pill`].
pub fn pill_width(text: &str) -> f32 {
    (text.len() as f32 * 7.0 + 10.0).max(18.0)
}

/// A small rounded count badge whose left edge is at `left_center`.
pub fn pill(painter: &egui::Painter, left_center: egui::Pos2, text: &str, color: Color32) {
    let rect = egui::Rect::from_min_size(
        egui::pos2(left_center.x, left_center.y - 8.0),
        egui::vec2(pill_width(text), 16.0),
    );
    painter.rect_filled(rect, egui::CornerRadius::same(8), color.gamma_multiply(0.25));
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::monospace(11.0), color);
}
