//! Client-side terminal screen: an alacritty `Term` fed with output from the host. Input,
//! resizes and pastes go back to the host, which owns the real PTY.
//! Rendering and input handling live in `view.rs`.
//!
//! The rendering and input approach is adapted from egui_term (MIT, github.com/Harzu/egui_term).

mod keys;
pub mod theme;
mod view;

pub use view::TerminalView;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Direction;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use std::cell::RefCell;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use tm_core::protocol::ClientMsg;
use uuid::Uuid;

/// Scrollback kept in the GUI. Older output lives in the host's logs.
const SCROLLBACK: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridSize {
    pub cols: usize,
    pub lines: usize,
    pub cell_w: f32,
    pub cell_h: f32,
}

pub struct Screen {
    id: Uuid,
    pub(crate) term: RefCell<Term<VoidListener>>,
    parser: Processor,
    size: GridSize,
    out: Sender<ClientMsg>,
    pub find: Find,
}

/// Find-in-terminal state (Ctrl+F) over the screen and its scrollback.
#[derive(Default)]
pub struct Find {
    query: String,
    case_sensitive: bool,
    matches: Vec<Match>,
    /// Index into `matches` of the selected match.
    pub current: Option<usize>,
    /// New output arrived since the matches were computed.
    stale: bool,
    computed_at: Option<Instant>,
}

impl Find {
    pub fn count(&self) -> usize {
        self.matches.len()
    }
}

impl Screen {
    pub fn new(id: Uuid, out: Sender<ClientMsg>) -> Self {
        let size = GridSize { cols: 120, lines: 30, cell_w: 8.0, cell_h: 16.0 };
        Self { id, term: RefCell::new(new_term(size)), parser: Processor::new(), size, out, find: Find::default() }
    }

    /// Feeds output from the host.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut *self.term.borrow_mut(), bytes);
        self.find.stale = !self.find.query.is_empty();
    }

    /// Sets the text to find; selects the newest match and scrolls to it.
    pub fn find_set(&mut self, query: &str, case_sensitive: bool) {
        if query == self.find.query && case_sensitive == self.find.case_sensitive {
            return;
        }
        self.find.query = query.to_string();
        self.find.case_sensitive = case_sensitive;
        self.find_compute();
        self.find.current = self.find.matches.len().checked_sub(1);
        self.find_reveal();
    }

    pub fn find_clear(&mut self) {
        self.find = Find::default();
    }

    /// Moves to the next (towards newer output) or previous match.
    pub fn find_step(&mut self, forward: bool) {
        self.find_refresh(true);
        let n = self.find.matches.len();
        if n == 0 {
            return;
        }
        self.find.current = Some(match self.find.current {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => n - 1,
        });
        self.find_reveal();
    }

    /// Recomputes matches after new output, at most a few times a second.
    pub fn find_refresh(&mut self, now: bool) {
        let due = self.find.computed_at.is_none_or(|t| t.elapsed() > Duration::from_millis(300));
        if self.find.stale && (now || due) {
            let selected = self.find.current.and_then(|i| self.find.matches.get(i)).map(|m| *m.start());
            self.find_compute();
            // Keep the same match selected if it is still there (lines shift as output scrolls).
            self.find.current = selected
                .and_then(|p| self.find.matches.iter().position(|m| *m.start() == p))
                .or(self.find.matches.len().checked_sub(1));
        }
    }

    fn find_compute(&mut self) {
        self.find.stale = false;
        self.find.computed_at = Some(Instant::now());
        self.find.matches.clear();
        if self.find.query.is_empty() {
            return;
        }
        let flags = if self.find.case_sensitive { "(?-i)" } else { "(?i)" };
        let Ok(mut regex) = RegexSearch::new(&format!("{flags}{}", regex_escape(&self.find.query))) else { return };
        let term = self.term.borrow();
        let start = Point::new(term.grid().topmost_line(), Column(0));
        let end = Point::new(term.grid().bottommost_line(), term.grid().last_column());
        self.find.matches = RegexIter::new(start, end, Direction::Right, &term, &mut regex).take(10_000).collect();
    }

    fn find_reveal(&mut self) {
        if let Some(m) = self.find.current.and_then(|i| self.find.matches.get(i)) {
            let point = *m.start();
            self.term.borrow_mut().scroll_to_point(point);
        }
    }

    /// Matches in the current viewport, with whether each is the selected one.
    pub(crate) fn find_visible(&self) -> Vec<(Match, bool)> {
        if self.find.matches.is_empty() {
            return Vec::new();
        }
        let term = self.term.borrow();
        let offset = term.grid().display_offset() as i32;
        let top = Line(-offset);
        let bottom = Line(-offset + term.screen_lines() as i32 - 1);
        self.find
            .matches
            .iter()
            .enumerate()
            .filter(|(_, m)| m.end().line >= top && m.start().line <= bottom)
            .map(|(i, m)| (m.clone(), Some(i) == self.find.current))
            .collect()
    }

    /// Clears everything before a replay.
    pub fn reset(&mut self) {
        *self.term.borrow_mut() = new_term(self.size);
        self.parser = Processor::new();
    }

    /// Sends keyboard input to the process.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            let _ = self.out.send(ClientMsg::Input { terminal: self.id, bytes });
        }
    }

    pub(crate) fn mode(&self) -> TermMode {
        *self.term.borrow().mode()
    }

    /// Resizes the grid to fit `width` x `height` pixels of cells and tells the host.
    pub(crate) fn resize(&mut self, width: f32, height: f32, cell_w: f32, cell_h: f32) {
        let cols = ((width / cell_w).floor() as usize).max(2);
        let lines = ((height / cell_h).floor() as usize).max(1);
        let new = GridSize { cols, lines, cell_w, cell_h };
        if self.size == new {
            return;
        }
        let grid_changed = (self.size.cols, self.size.lines) != (cols, lines);
        self.size = new;
        if grid_changed {
            self.term.borrow_mut().resize(TermSize::new(cols, lines));
            let _ = self.out.send(ClientMsg::Resize { terminal: self.id, cols: cols as u16, lines: lines as u16 });
        }
    }

    /// Tells the host this view's size again (after reconnecting, the host may have another size).
    pub fn announce_size(&self) {
        let _ = self.out.send(ClientMsg::Resize {
            terminal: self.id,
            cols: self.size.cols as u16,
            lines: self.size.lines as u16,
        });
    }

    pub(crate) fn scroll(&self, lines: i32) {
        let mut term = self.term.borrow_mut();
        if term.mode().contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let seq: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
            drop(term);
            self.write(seq.repeat(lines.unsigned_abs() as usize));
        } else {
            term.scroll_display(Scroll::Delta(lines));
        }
    }

    pub(crate) fn scroll_to_bottom(&self) {
        self.term.borrow_mut().scroll_display(Scroll::Bottom);
    }

    /// Converts a position relative to the grid's top-left into a grid point and side.
    pub(crate) fn point_at(&self, x: f32, y: f32) -> (Point, Side) {
        let s = self.size;
        let col = ((x / s.cell_w).max(0.0) as usize).min(s.cols - 1);
        let line = ((y / s.cell_h).max(0.0) as usize).min(s.lines - 1);
        let offset = self.term.borrow().grid().display_offset() as i32;
        let side = if (x / s.cell_w).fract() > 0.5 { Side::Right } else { Side::Left };
        (Point::new(Line(line as i32 - offset), Column(col)), side)
    }

    pub(crate) fn start_selection(&self, ty: SelectionType, x: f32, y: f32) {
        let (point, side) = self.point_at(x, y);
        self.term.borrow_mut().selection = Some(Selection::new(ty, point, side));
    }

    pub(crate) fn update_selection(&self, x: f32, y: f32) {
        let (point, side) = self.point_at(x, y);
        if let Some(sel) = self.term.borrow_mut().selection.as_mut() {
            sel.update(point, side);
        }
    }

    pub(crate) fn clear_selection(&self) {
        self.term.borrow_mut().selection = None;
    }

    pub(crate) fn selection_text(&self) -> Option<String> {
        self.term.borrow().selection_to_string().filter(|s| !s.is_empty())
    }

    pub(crate) fn paste(&self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.mode().contains(TermMode::BRACKETED_PASTE) {
            let clean = text.replace('\x1b', "");
            self.write(format!("\x1b[200~{clean}\x1b[201~"));
        } else {
            self.write(text);
        }
        self.scroll_to_bottom();
    }
}

fn new_term(size: GridSize) -> Term<VoidListener> {
    let config = term::Config { scrolling_history: SCROLLBACK, ..Default::default() };
    Term::new(config, &TermSize::new(size.cols, size.lines), VoidListener)
}

/// Escapes regex metacharacters so find matches the text literally.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        let (tx, _rx) = std::sync::mpsc::channel();
        Screen::new(Uuid::new_v4(), tx)
    }

    #[test]
    fn find_counts_steps_and_follows_new_output() {
        let mut s = screen();
        for i in 0..200 {
            s.feed(format!("line {i} {}\r\n", if i % 50 == 0 { "ERROR timeout" } else { "ok" }).as_bytes());
        }
        s.find_set("error", false);
        assert_eq!(s.find.count(), 4);
        assert_eq!(s.find.current, Some(3), "starts at the newest match");
        s.find_step(false);
        assert_eq!(s.find.current, Some(2));
        s.find_step(true);
        s.find_step(true);
        assert_eq!(s.find.current, Some(0), "wraps around");

        s.find_set("error", true);
        assert_eq!(s.find.count(), 0, "case-sensitive");
        s.find_set("ERROR", true);
        assert_eq!(s.find.count(), 4);

        // Regex metacharacters are literal; new output is picked up.
        s.find_set("(x)", false);
        assert_eq!(s.find.count(), 0);
        s.feed(b"call (x) failed\r\n");
        s.find_refresh(true);
        assert_eq!(s.find.count(), 1);
        s.find_clear();
        assert!(s.find_visible().is_empty());
    }
}
