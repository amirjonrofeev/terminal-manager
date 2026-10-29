//! Terminal colors for rendering (values shared with the host in `tm_core::palette`).

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use egui::Color32;
use tm_core::palette;

fn light() -> bool {
    crate::ui::pal::is_light()
}

pub fn selection() -> Color32 {
    if light() { Color32::from_rgb(0xb4, 0xd5, 0xf0) } else { Color32::from_rgb(0x2f, 0x4a, 0x5c) }
}

pub fn find() -> Color32 {
    if light() { Color32::from_rgb(0xf6, 0xe2, 0xa6) } else { Color32::from_rgb(0x5a, 0x4a, 0x1e) }
}

pub fn find_current() -> Color32 {
    if light() { Color32::from_rgb(0xe6, 0xa1, 0x17) } else { Color32::from_rgb(0xe0, 0xa8, 0x3a) }
}

fn rgb_for_index(index: usize) -> Rgb {
    let (r, g, b) = palette::rgb_for_index(palette::for_theme(light()), index);
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

pub fn foreground(colors: &Colors) -> Color32 {
    resolve(Color::Named(NamedColor::Foreground), colors)
}

fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// A text color for `bg`: the theme's text color if it reads well there, else near-black or
/// near-white depending on how light `bg` is.
pub fn readable_on(bg: Color32, preferred: Color32) -> Color32 {
    if !indistinguishable(preferred, bg) && contrast(preferred, bg) >= 3.0 {
        preferred
    } else if luminance(bg) > 0.4 {
        Color32::from_rgb(0x24, 0x29, 0x2e)
    } else {
        Color32::from_rgb(0xe8, 0xec, 0xee)
    }
}

/// `a` moved `t` (0..1) of the way toward `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Text that would be unreadable on its background (contrast ratio under 1.5:1).
pub fn indistinguishable(fg: Color32, bg: Color32) -> bool {
    contrast(fg, bg) < 1.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_on_light_and_black_on_dark_are_indistinguishable() {
        let white = Color32::WHITE;
        let light_bg = Color32::from_rgb(0xfa, 0xfa, 0xf8);
        let dark_bg = Color32::from_rgb(0x15, 0x18, 0x1a);
        assert!(indistinguishable(white, light_bg));
        assert!(indistinguishable(Color32::from_rgb(0x1e, 0x22, 0x25), dark_bg));
        assert!(!indistinguishable(Color32::from_rgb(0x24, 0x29, 0x2e), light_bg));
        assert!(!indistinguishable(Color32::from_rgb(0xe0, 0x60, 0x5a), dark_bg));
        // White text on a near-white cell background (a prompt segment) becomes dark.
        let fixed = readable_on(Color32::from_rgb(0xf0, 0xf0, 0xf0), Color32::WHITE);
        assert!(contrast(fixed, Color32::from_rgb(0xf0, 0xf0, 0xf0)) > 7.0);
    }
}
