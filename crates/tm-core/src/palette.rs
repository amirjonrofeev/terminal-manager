//! Terminal colors, shared so the host answers color queries with what the GUI draws.

pub type Rgb = (u8, u8, u8);

pub const BACKGROUND: Rgb = (0x15, 0x18, 0x1a);
pub const FOREGROUND: Rgb = (0xd7, 0xdd, 0xe0);
pub const CURSOR: Rgb = (0xc8, 0xd3, 0xd8);

const ANSI: [Rgb; 16] = [
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
];

// alacritty's `NamedColor` indices above the 256-color palette.
const BACKGROUND_INDEX: usize = 257;
const CURSOR_INDEX: usize = 258;
const DIM_BLACK_INDEX: usize = 259;
const DIM_WHITE_INDEX: usize = 266;
const DIM_FOREGROUND_INDEX: usize = 268;

/// Default color for an alacritty color index (0..=255 palette, 256+ named colors).
pub fn rgb_for_index(index: usize) -> Rgb {
    match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let i = index - 16;
            let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        232..=255 => {
            let v = ((index - 232) * 10 + 8) as u8;
            (v, v, v)
        }
        BACKGROUND_INDEX => BACKGROUND,
        CURSOR_INDEX => CURSOR,
        DIM_FOREGROUND_INDEX => dim(FOREGROUND),
        DIM_BLACK_INDEX..=DIM_WHITE_INDEX => dim(ANSI[index - DIM_BLACK_INDEX]),
        _ => FOREGROUND,
    }
}

fn dim((r, g, b): Rgb) -> Rgb {
    ((r as u16 * 2 / 3) as u8, (g as u16 * 2 / 3) as u8, (b as u16 * 2 / 3) as u8)
}
