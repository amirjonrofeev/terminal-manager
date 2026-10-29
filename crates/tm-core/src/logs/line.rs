//! Turns a terminal byte stream into plain text lines for the log.
//!
//! Colors and other escape sequences are dropped. `\r` returns to the start of the line, so a
//! progress bar redrawn in place ends up as its final state rather than a hundred lines.
//! ConPTY repaints the screen with cursor positioning (after a resize or when a viewer
//! attaches); lines repeated during such a repaint are recognised and not logged twice.

use std::collections::{HashSet, VecDeque};
use vte::{Params, Parser, Perform};

/// Lines remembered to recognise a repaint (a screenful or two).
const RECENT_LINES: usize = 300;
/// Longest line kept; anything after this is cut (a runaway line can't eat memory).
const MAX_LINE_CHARS: usize = 16 * 1024;

pub struct LineAssembler {
    parser: Parser,
    state: State,
}

struct State {
    line: Vec<char>,
    col: usize,
    /// True while a repaint is in progress (cursor moved to an absolute position).
    repainting: bool,
    recent: VecDeque<String>,
    recent_set: HashSet<String>,
    done: Vec<String>,
}

impl Default for LineAssembler {
    fn default() -> Self {
        Self {
            parser: Parser::new(),
            state: State {
                line: Vec::new(),
                col: 0,
                repainting: false,
                recent: VecDeque::new(),
                recent_set: HashSet::new(),
                done: Vec::new(),
            },
        }
    }
}

impl LineAssembler {
    /// Feeds raw output; returns the lines completed by it (UTF-8 and escape sequences may be
    /// split across calls).
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.parser.advance(&mut self.state, bytes);
        std::mem::take(&mut self.state.done)
    }

    /// Returns the unfinished last line, if any (on exit).
    pub fn flush(&mut self) -> Option<String> {
        self.state.finish_line();
        self.state.done.pop()
    }
}

impl State {
    fn finish_line(&mut self) {
        let text: String = self.line.iter().collect::<String>().trim_end().to_string();
        self.line.clear();
        self.col = 0;
        if text.trim().is_empty() {
            return;
        }
        if self.repainting {
            // A repaint redraws each screen row on its own, so a wrapped line comes back as
            // pieces: drop exact repeats and pieces of recently logged lines.
            if self.recent_set.contains(&text) || self.recent.iter().any(|r| r.contains(text.as_str())) {
                return;
            }
            // New content: the repaint is over.
            self.repainting = false;
        }
        if self.recent.len() == RECENT_LINES
            && let Some(old) = self.recent.pop_front()
        {
            if !self.recent.contains(&old) {
                self.recent_set.remove(&old);
            }
        }
        self.recent.push_back(text.clone());
        self.recent_set.insert(text.clone());
        self.done.push(text);
    }

    fn put(&mut self, c: char) {
        if self.col >= MAX_LINE_CHARS {
            return;
        }
        if self.col < self.line.len() {
            self.line[self.col] = c;
        } else {
            self.line.resize(self.col, ' ');
            self.line.push(c);
        }
        self.col += 1;
    }

    fn start_repaint(&mut self) {
        self.finish_line();
        self.repainting = true;
    }
}

fn first_param(params: &Params, default: usize) -> usize {
    params.iter().next().and_then(|p| p.first()).map(|&v| v as usize).filter(|&v| v > 0).unwrap_or(default)
}

impl Perform for State {
    fn print(&mut self, c: char) {
        self.put(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.finish_line(),
            b'\r' => self.col = 0,
            b'\t' => {
                let next = (self.col / 8 + 1) * 8;
                while self.col < next {
                    self.put(' ');
                }
            }
            0x08 => self.col = self.col.saturating_sub(1),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        if !intermediates.is_empty() {
            return;
        }
        match action {
            // Cursor left / right / to column.
            'D' => self.col = self.col.saturating_sub(first_param(params, 1)),
            'C' => self.col += first_param(params, 1),
            'G' => self.col = first_param(params, 1) - 1,
            // Erase in line: 0 = to end, 1 = to start, 2 = whole line.
            'K' => match params.iter().next().and_then(|p| p.first()).copied().unwrap_or(0) {
                0 => self.line.truncate(self.col),
                1 => {
                    for c in self.line.iter_mut().take(self.col + 1) {
                        *c = ' ';
                    }
                }
                _ => self.line.clear(),
            },
            // Absolute cursor moves and screen clears mean the screen is being (re)drawn.
            'H' | 'f' | 'd' | 'J' => self.start_repaint(),
            // Cursor up/down: treat like a move to another line.
            'A' | 'B' | 'E' | 'F' => self.finish_line(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(chunks: &[&[u8]]) -> Vec<String> {
        let mut a = LineAssembler::default();
        let mut out: Vec<String> = chunks.iter().flat_map(|c| a.feed(c)).collect();
        out.extend(a.flush());
        out
    }

    #[test]
    fn strips_colors_and_splits_lines() {
        assert_eq!(lines(&[b"\x1b[31mred\x1b[0m text\r\nnext\r\n"]), vec!["red text", "next"]);
    }

    #[test]
    fn carriage_return_overwrites_progress() {
        assert_eq!(lines(&[b"10%\r50%\r100% done\r\n"]), vec!["100% done"]);
        assert_eq!(lines(&[b"downloading 100%\r\x1b[Kdone\r\n"]), vec!["done"]);
    }

    #[test]
    fn handles_splits_across_reads() {
        // An escape sequence and a UTF-8 character split between chunks.
        let s = "caf\u{e9} \x1b[32mok\x1b[0m\r\n".as_bytes();
        let (a, b) = s.split_at(4); // inside the two-byte é
        assert_eq!(lines(&[a, b]), vec!["café ok"]);
        assert_eq!(lines(&[b"x\x1b[3", b"1my\r\n"]), vec!["xy"]);
    }

    #[test]
    fn skips_blank_lines_and_keeps_unfinished_last_line() {
        assert_eq!(lines(&[b"a\r\n\r\n   \r\nlast"]), vec!["a", "last"]);
    }

    #[test]
    fn repaint_does_not_duplicate_lines() {
        let mut a = LineAssembler::default();
        assert_eq!(a.feed(b"one\r\ntwo\r\n"), vec!["one", "two"]);
        // ConPTY redraws the screen after a resize, then new output continues.
        assert_eq!(a.feed(b"\x1b[H\x1b[2Jone\r\ntwo\r\nthree\r\n"), vec!["three"]);
        // Outside a repaint, a repeated line is real output and is kept.
        assert_eq!(a.feed(b"one\r\n"), vec!["one"]);
    }

    #[test]
    fn repaint_of_wrapped_line_is_not_logged_as_pieces() {
        let mut a = LineAssembler::default();
        let long = r"Content root path: W:\DEV\datamicron\IAM.Web\DataMicron.IAM.Web";
        assert_eq!(a.feed(format!("{long}\r\n").as_bytes()), vec![long]);
        // Redrawn at 30 columns: one row per piece, positioned with CUP.
        let rows: String = long
            .as_bytes()
            .chunks(30)
            .enumerate()
            .map(|(i, c)| format!("\x1b[{};1H{}", i + 1, String::from_utf8_lossy(c)))
            .collect();
        assert!(a.feed(format!("\x1b[2J{rows}\x1b[5;1H").as_bytes()).is_empty());
        assert_eq!(a.feed(b"next line\r\n"), vec!["next line"]);
    }

    #[test]
    fn tabs_and_backspace() {
        assert_eq!(lines(&[b"a\tb\r\n", b"abc\x08\x08X\r\n"]), vec!["a       b", "aXc"]);
    }
}
