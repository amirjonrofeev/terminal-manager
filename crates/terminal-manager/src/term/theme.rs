//! Terminal colors for rendering (values shared with the host in `tm_core::palette`).

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use egui::Color32;
use tm_core::palette;

pub const SELECTION: Color32 = Color32::from_rgb(0x2f, 0x4a, 0x5c);
pub const FIND: Color32 = Color32::from_rgb(0x5a, 0x4a, 0x1e);
pub const FIND_CURRENT: Color32 = Color32::from_rgb(0xe0, 0xa8, 0x3a);

fn rgb_for_index(index: usize) -> Rgb {
    let (r, g, b) = palette::rgb_for_index(index);
    Rgb { r, g, b }
}

fn to_color32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

/// Resolves a cell color, honouring colors the program changed via OSC sequences.
pub fn resolve(color: Color, colors: &Colors) -> Color32 {
    let rgb = match color {
        Color::Spec(rgb) => rgb,
        Color::Indexed(i) => colors[i as usize].unwrap_or_else(|| rgb_for_index(i as usize)),
        Color::Named(n) => colors[n].unwrap_or_else(|| rgb_for_index(n as usize)),
    };
    to_color32(rgb)
}

pub fn background(colors: &Colors) -> Color32 {
    resolve(Color::Named(NamedColor::Background), colors)
}
