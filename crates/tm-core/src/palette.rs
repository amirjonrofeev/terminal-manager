//! Terminal colors, shared so the host answers color queries with what the GUI draws.

pub type Rgb = (u8, u8, u8);

/// Default colors of a terminal theme.
pub struct Palette {
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Rgb,
    pub ansi: [Rgb; 16],
}

pub const DARK: Palette = Palette {
    background: (0x15, 0x18, 0x1a),
    foreground: (0xd7, 0xdd, 0xe0),
    cursor: (0xc8, 0xd3, 0xd8),
    ansi: [
        (0x1e, 0x22, 0x25), // black
        (0xe0, 0x60, 0x5a), // red
        (0x5f, 0xc7, 0x9a), // green
        (0xe0, 0xa8, 0x3a), // yellow
        (0x6f, 0xa8, 0xdc), // blue
        (0xc0, 0x8c, 0xd8), // magenta
        (0x5c, 0xc2, 0xc6), // cyan
        (0xc9, 0xd1, 0xd4), // white
        (0x5d, 0x68, 0x6e), // bright black
        (0xf0, 0x7c, 0x76),
        (0x82, 0xdb, 0xb2),
        (0xf2, 0xc4, 0x63),
        (0x92, 0xc2, 0xef),
        (0xd5, 0xa9, 0xe8),
        (0x82, 0xd8, 0xdb),
        (0xf2, 0xf5, 0xf6),
    ],
};

/// Light theme. "White" and "bright white" are greys/near-black here: programs written for dark
/// terminals print in white, and that must stay readable on a light background.
pub const LIGHT: Palette = Palette {
    background: (0xfa, 0xfa, 0xf8),
    foreground: (0x24, 0x29, 0x2e),
    cursor: (0x3a, 0x42, 0x48),
    ansi: [
        (0x24, 0x29, 0x2e), // black
        (0xc4, 0x3b, 0x34), // red
        (0x2a, 0x8a, 0x5c), // green
        (0x9a, 0x6b, 0x00), // yellow
        (0x1f, 0x6f, 0xb5), // blue
        (0x8e, 0x44, 0xad), // magenta
        (0x0e, 0x7f, 0x8a), // cyan
        (0x6a, 0x73, 0x7d), // white
        (0x8b, 0x94, 0x9e), // bright black
        (0xd7, 0x4c, 0x44),
        (0x34, 0x9e, 0x6b),
        (0xb0, 0x80, 0x00),
        (0x2b, 0x82, 0xcc),
        (0xa2, 0x55, 0xc2),
        (0x12, 0x94, 0xa0),
        (0x30, 0x36, 0x3d),
    ],
};

pub fn for_theme(light: bool) -> &'static Palette {
    if light { &LIGHT } else { &DARK }
}

// alacritty's `NamedColor` indices above the 256-color palette.
const BACKGROUND_INDEX: usize = 257;
const CURSOR_INDEX: usize = 258;
const DIM_BLACK_INDEX: usize = 259;
const DIM_WHITE_INDEX: usize = 266;
const DIM_FOREGROUND_INDEX: usize = 268;

/// Default color for an alacritty color index (0..=255 palette, 256+ named colors).
pub fn rgb_for_index(p: &Palette, index: usize) -> Rgb {
    match index {
        0..=15 => p.ansi[index],
        16..=231 => {
            let i = index - 16;
            let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        232..=255 => {
            let v = ((index - 232) * 10 + 8) as u8;
            (v, v, v)
        }
        BACKGROUND_INDEX => p.background,
        CURSOR_INDEX => p.cursor,
        DIM_FOREGROUND_INDEX => dim(p, p.foreground),
        DIM_BLACK_INDEX..=DIM_WHITE_INDEX => dim(p, p.ansi[index - DIM_BLACK_INDEX]),
        _ => p.foreground,
    }
}

/// "Dim" moves a color toward the background.
fn dim(p: &Palette, (r, g, b): Rgb) -> Rgb {
    let (br, bg, bb) = p.background;
    let mix = |c: u8, b: u8| ((c as u16 * 2 + b as u16) / 3) as u8;
    (mix(r, br), mix(g, bg), mix(b, bb))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG-style relative luminance contrast, enough to catch unreadable colors.
    fn contrast(a: Rgb, b: Rgb) -> f32 {
        let lum = |(r, g, b): Rgb| {
            let c = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * c(r) + 0.7152 * c(g) + 0.0722 * c(b)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_colors_are_readable_on_their_background() {
        for p in [&DARK, &LIGHT] {
            assert!(contrast(p.foreground, p.background) >= 7.0);
            // Every ANSI color except "black" on dark / the background-like one must be legible.
            for (i, c) in p.ansi.iter().enumerate() {
                if std::ptr::eq(p, &DARK) && (i == 0 || i == 8) {
                    continue; // black and bright black are meant to be dark on dark
                }
                assert!(contrast(*c, p.background) >= 2.8, "ansi {i} {c:?} on {:?}", p.background);
            }
        }
    }
}
