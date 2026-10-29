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

/// UI colors for the current theme (dark or light), read through functions so every view
/// follows a theme change on the next frame.
pub mod pal {
    use egui::Color32;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub struct Ui {
        pub bg: Color32,
        pub side: Color32,
        pub panel: Color32,
        pub line: Color32,
        pub fg: Color32,
        pub dim: Color32,
        pub accent: Color32,
        pub row_selected: Color32,
        pub ok: Color32,
        pub warn: Color32,
        pub bad: Color32,
        pub idle: Color32,
        pub hover: Color32,
        pub pressed: Color32,
        pub selection: Color32,
    }

    pub const DARK: Ui = Ui {
        bg: Color32::from_rgb(0x15, 0x18, 0x1a),
        side: Color32::from_rgb(0x1b, 0x1f, 0x22),
        panel: Color32::from_rgb(0x20, 0x25, 0x2a),
        line: Color32::from_rgb(0x2d, 0x34, 0x39),
        fg: Color32::from_rgb(0xd7, 0xdd, 0xe0),
        dim: Color32::from_rgb(0x7d, 0x8a, 0x91),
        accent: Color32::from_rgb(0x5f, 0xc7, 0x9a),
        row_selected: Color32::from_rgb(0x26, 0x30, 0x29),
        ok: Color32::from_rgb(0x3f, 0xb6, 0x7e),
        warn: Color32::from_rgb(0xe0, 0xa8, 0x3a),
        bad: Color32::from_rgb(0xe0, 0x60, 0x5a),
        idle: Color32::from_rgb(0x6b, 0x76, 0x7c),
        hover: Color32::from_rgb(0x29, 0x30, 0x36),
        pressed: Color32::from_rgb(0x23, 0x4a, 0x3a),
        selection: Color32::from_rgb(0x2f, 0x6a, 0x52),
    };

    pub const LIGHT: Ui = Ui {
        bg: Color32::from_rgb(0xfa, 0xfa, 0xf8),
        side: Color32::from_rgb(0xee, 0xf0, 0xed),
        panel: Color32::from_rgb(0xff, 0xff, 0xff),
        line: Color32::from_rgb(0xd5, 0xda, 0xd6),
        fg: Color32::from_rgb(0x1f, 0x24, 0x27),
        dim: Color32::from_rgb(0x5f, 0x6b, 0x72),
        accent: Color32::from_rgb(0x1f, 0x8a, 0x5e),
        row_selected: Color32::from_rgb(0xdc, 0xeb, 0xe2),
        ok: Color32::from_rgb(0x23, 0x99, 0x5e),
        warn: Color32::from_rgb(0xb3, 0x76, 0x00),
        bad: Color32::from_rgb(0xc8, 0x42, 0x3b),
        idle: Color32::from_rgb(0x8a, 0x95, 0x9b),
        hover: Color32::from_rgb(0xe4, 0xe8, 0xe5),
        pressed: Color32::from_rgb(0xc9, 0xe3, 0xd5),
        selection: Color32::from_rgb(0xb8, 0xdc, 0xc8),
    };

    static LIGHT_ON: AtomicBool = AtomicBool::new(false);

    /// Switches the palette; returns whether it changed.
    pub fn set_light(light: bool) -> bool {
        LIGHT_ON.swap(light, Ordering::Relaxed) != light
    }

    pub fn is_light() -> bool {
        LIGHT_ON.load(Ordering::Relaxed)
    }

    pub fn current() -> &'static Ui {
        if is_light() { &LIGHT } else { &DARK }
    }

    pub fn bg() -> Color32 { current().bg }
    pub fn side() -> Color32 { current().side }
    pub fn line() -> Color32 { current().line }
    pub fn fg() -> Color32 { current().fg }
    pub fn dim() -> Color32 { current().dim }
    pub fn accent() -> Color32 { current().accent }
    pub fn row_selected() -> Color32 { current().row_selected }
    pub fn ok() -> Color32 { current().ok }
    pub fn warn() -> Color32 { current().warn }
    pub fn bad() -> Color32 { current().bad }
    pub fn idle() -> Color32 { current().idle }
}

pub fn setup(ctx: &egui::Context) {
    load_mono_font(ctx);
    for (theme, p) in [(egui::Theme::Dark, &pal::DARK), (egui::Theme::Light, &pal::LIGHT)] {
        ctx.style_mut_of(theme, |style| {
            let v = &mut style.visuals;
            v.panel_fill = p.bg;
            v.window_fill = p.panel;
            v.window_stroke = Stroke::new(1.0, p.line);
            v.extreme_bg_color = p.bg;
            v.faint_bg_color = p.side;
            v.selection.bg_fill = p.selection;
            v.selection.stroke = Stroke::new(1.0, p.fg);
            v.hyperlink_color = p.accent;
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.fg);
            v.widgets.inactive.weak_bg_fill = p.panel;
            v.widgets.inactive.bg_fill = p.panel;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.line);
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.fg);
            v.widgets.hovered.weak_bg_fill = p.hover;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
            v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.fg);
            v.widgets.active.weak_bg_fill = p.pressed;
            v.widgets.active.fg_stroke = Stroke::new(1.0, p.fg);
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 4.0);
        });
    }
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
    if color == pal::ok() {
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
