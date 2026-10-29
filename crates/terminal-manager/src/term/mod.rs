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
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use std::cell::RefCell;
use std::sync::mpsc::Sender;
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
}

impl Screen {
    pub fn new(id: Uuid, out: Sender<ClientMsg>) -> Self {
        let size = GridSize { cols: 120, lines: 30, cell_w: 8.0, cell_h: 16.0 };
        Self { id, term: RefCell::new(new_term(size)), parser: Processor::new(), size, out }
    }

    /// Feeds output from the host.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut *self.term.borrow_mut(), bytes);
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
