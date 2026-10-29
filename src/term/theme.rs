//! Terminal color palette.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use egui::Color32;

pub const BACKGROUND: Rgb = rgb(0x15, 0x18, 0x1a);
pub const FOREGROUND: Rgb = rgb(0xd7, 0xdd, 0xe0);
pub const CURSOR: Rgb = rgb(0xc8, 0xd3, 0xd8);
pub const SELECTION: Color32 = Color32::from_rgb(0x2f, 0x4a, 0x5c);

const ANSI: [Rgb; 16] = [
    rgb(0x1e, 0x22, 0x25), // black
    rgb(0xe0, 0x60, 0x5a), // red
    rgb(0x5f, 0xc7, 0x9a), // green
    rgb(0xe0, 0xa8, 0x3a), // yellow
    rgb(0x6f, 0xa8, 0xdc), // blue
    rgb(0xc0, 0x8c, 0xd8), // magenta
    rgb(0x5c, 0xc2, 0xc6), // cyan
    rgb(0xc9, 0xd1, 0xd4), // white
    rgb(0x5d, 0x68, 0x6e), // bright black
    rgb(0xf0, 0x7c, 0x76),
    rgb(0x82, 0xdb, 0xb2),
    rgb(0xf2, 0xc4, 0x63),
    rgb(0x92, 0xc2, 0xef),
    rgb(0xd5, 0xa9, 0xe8),
    rgb(0x82, 0xd8, 0xdb),
    rgb(0xf2, 0xf5, 0xf6),
];

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// Default color for an alacritty color index (0..=255 palette, 256+ named colors).
pub fn rgb_for_index(index: usize) -> Rgb {
    match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let i = index - 16;
            let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
            rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        232..=255 => {
            let v = ((index - 232) * 10 + 8) as u8;
            rgb(v, v, v)
        }
        i if i == NamedColor::Background as usize => BACKGROUND,
        i if i == NamedColor::Cursor as usize => CURSOR,
        i if i == NamedColor::DimForeground as usize => dim(FOREGROUND),
        i if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
            dim(ANSI[i - NamedColor::DimBlack as usize])
        }
        _ => FOREGROUND,
    }
}

fn dim(c: Rgb) -> Rgb {
    rgb((c.r as u16 * 2 / 3) as u8, (c.g as u16 * 2 / 3) as u8, (c.b as u16 * 2 / 3) as u8)
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
