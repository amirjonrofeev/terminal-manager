//! Maps egui key presses to the bytes a terminal expects.

use alacritty_terminal::term::TermMode;
use egui::{Key, Modifiers};

/// Bytes for a key press, or `None` when the key produces text via `Event::Text` instead.
pub fn key_bytes(key: Key, mods: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4)
    let m = 1 + mods.shift as u8 + 2 * mods.alt as u8 + 4 * mods.ctrl as u8;

    let csi_letter = |c: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let ss3 = |c: char| -> Vec<u8> {
        if m > 1 { format!("\x1b[1;{m}{c}").into_bytes() } else { format!("\x1bO{c}").into_bytes() }
    };

    let bytes = match key {
        Key::Enter => if mods.alt { b"\x1b\r".to_vec() } else { b"\r".to_vec() },
        Key::Backspace => match (mods.ctrl, mods.alt) {
            (true, _) => vec![0x08],
            (false, true) => b"\x1b\x7f".to_vec(),
            _ => vec![0x7f],
        },
        Key::Tab => if mods.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() },
        Key::Escape => b"\x1b".to_vec(),
        Key::ArrowUp => csi_letter('A'),
        Key::ArrowDown => csi_letter('B'),
        Key::ArrowRight => csi_letter('C'),
        Key::ArrowLeft => csi_letter('D'),
        Key::Home => csi_letter('H'),
        Key::End => csi_letter('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => ss3('P'),
        Key::F2 => ss3('Q'),
        Key::F3 => ss3('R'),
        Key::F4 => ss3('S'),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        _ if mods.ctrl && !mods.shift => return ctrl_char(key).map(|b| {
            if mods.alt { vec![0x1b, b] } else { vec![b] }
        }),
        _ if mods.alt && !mods.ctrl => {
            // Alt+letter sends ESC prefix; egui doesn't emit Text for it reliably.
            let c = letter(key)?;
            let c = if mods.shift { c.to_ascii_uppercase() } else { c };
            vec![0x1b, c as u8]
        }
        _ => return None,
    };
    Some(bytes)
}

fn letter(key: Key) -> Option<char> {
    let name = key.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// Control character for Ctrl+key.
fn ctrl_char(key: Key) -> Option<u8> {
    if let Some(c) = letter(key) {
        return Some(c as u8 - b'a' + 1);
    }
    Some(match key {
        Key::Space | Key::Num2 => 0x00,
        Key::OpenBracket | Key::Num3 => 0x1b,
        Key::Backslash | Key::Num4 => 0x1c,
        Key::CloseBracket | Key::Num5 => 0x1d,
        Key::Num6 => 0x1e,
        Key::Minus | Key::Slash | Key::Num7 => 0x1f,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_respect_app_cursor_and_modifiers() {
        assert_eq!(key_bytes(Key::ArrowUp, Modifiers::NONE, TermMode::empty()).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes(Key::ArrowUp, Modifiers::NONE, TermMode::APP_CURSOR).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes(Key::ArrowRight, Modifiers::CTRL, TermMode::empty()).unwrap(), b"\x1b[1;5C");
    }

    #[test]
    fn ctrl_letters() {
        assert_eq!(key_bytes(Key::D, Modifiers::CTRL, TermMode::empty()).unwrap(), vec![4]);
        assert_eq!(key_bytes(Key::Z, Modifiers::CTRL, TermMode::empty()).unwrap(), vec![26]);
        assert!(key_bytes(Key::D, Modifiers::NONE, TermMode::empty()).is_none());
    }
}
