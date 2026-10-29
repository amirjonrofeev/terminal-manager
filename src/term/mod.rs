//! A terminal backed by `alacritty_terminal`: PTY spawn, VT state, resize, selection and kill.
//! Rendering and input live in `view.rs`.
//!
//! The rendering and input approach is adapted from egui_term (MIT, github.com/Harzu/egui_term).

mod keys;
pub mod theme;
mod view;

pub use view::TerminalView;

use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, Term, TermMode};
use alacritty_terminal::tty;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

pub struct SpawnOptions {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Notifications for the app, tagged with the caller's key.
#[derive(Debug, Clone)]
pub enum TermEvent {
    Exited(Option<i32>),
    Title(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridSize {
    pub cols: usize,
    pub lines: usize,
    pub cell_w: f32,
    pub cell_h: f32,
}

impl GridSize {
    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.lines as u16,
            num_cols: self.cols as u16,
            cell_width: self.cell_w as u16,
            cell_height: self.cell_h as u16,
        }
    }
}

#[derive(Clone)]
pub struct EventProxy(mpsc::Sender<Event>);

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

pub struct TerminalBackend {
    pub(crate) term: Arc<FairMutex<Term<EventProxy>>>,
    notifier: Notifier,
    size: Arc<Mutex<GridSize>>,
    pid: Option<u32>,
    shut_down: bool,
}

/// Sets TERM/COLORTERM for child processes. Call once at startup.
pub fn setup_env() {
    tty::setup_env();
}

impl TerminalBackend {
    pub fn spawn<K: Clone + Send + 'static>(
        opts: SpawnOptions,
        key: K,
        events: Sender<(K, TermEvent)>,
        ctx: egui::Context,
    ) -> std::io::Result<Self> {
        let grid = GridSize { cols: 100, lines: 30, cell_w: 8.0, cell_h: 16.0 };
        let pty_opts = tty::Options {
            shell: Some(tty::Shell::new(opts.program, opts.args)),
            working_directory: Some(opts.cwd),
            env: opts.env.into_iter().collect::<HashMap<_, _>>(),
            drain_on_exit: true,
            ..Default::default()
        };
        let pty = tty::new(&pty_opts, grid.window_size(), 0)?;
        #[cfg(windows)]
        let pid = pty.child_watcher().pid().map(|p| p.get());
        #[cfg(not(windows))]
        let pid = Some(pty.child().id());

        let (tx, rx) = mpsc::channel();
        let proxy = EventProxy(tx);
        let config = term::Config { scrolling_history: 10_000, ..Default::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &TermSize::new(grid.cols, grid.lines), proxy.clone())));
        let event_loop = EventLoop::new(term.clone(), proxy, pty, true, false)?;
        let loop_tx = event_loop.channel();
        let _ = event_loop.spawn();

        let size = Arc::new(Mutex::new(grid));
        let thread_size = size.clone();
        let reply_tx = loop_tx.clone();
        std::thread::Builder::new().name("pty-events".into()).spawn(move || {
            let reply = |s: String| {
                let _ = reply_tx.send(Msg::Input(Cow::Owned(s.into_bytes())));
            };
            while let Ok(event) = rx.recv() {
                match event {
                    Event::PtyWrite(s) => reply(s),
                    Event::TextAreaSizeRequest(fmt) => reply(fmt(thread_size.lock().unwrap().window_size())),
                    Event::ColorRequest(index, fmt) => reply(fmt(theme::rgb_for_index(index))),
                    Event::ClipboardStore(_, text) => ctx.copy_text(text),
                    Event::Title(t) => {
                        let _ = events.send((key.clone(), TermEvent::Title(t)));
                    }
                    Event::ChildExit(status) => {
                        let _ = events.send((key.clone(), TermEvent::Exited(status.code())));
                    }
                    Event::Exit => {
                        ctx.request_repaint();
                        break;
                    }
                    _ => {}
                }
                ctx.request_repaint();
            }
        })?;

        Ok(Self { term, notifier: Notifier(loop_tx), size, pid, shut_down: false })
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    pub(crate) fn grid_size(&self) -> GridSize {
        *self.size.lock().unwrap()
    }

    pub(crate) fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    /// Resizes the grid to fit `width` x `height` pixels of cells.
    pub(crate) fn resize(&mut self, width: f32, height: f32, cell_w: f32, cell_h: f32) {
        let cols = ((width / cell_w).floor() as usize).max(2);
        let lines = ((height / cell_h).floor() as usize).max(1);
        let new = GridSize { cols, lines, cell_w, cell_h };
        let mut size = self.size.lock().unwrap();
        if *size == new {
            return;
        }
        *size = new;
        self.notifier.on_resize(new.window_size());
        self.term.lock().resize(TermSize::new(cols, lines));
    }

    pub(crate) fn scroll(&self, lines: i32) {
        let mut term = self.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let seq: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
            let bytes = seq.repeat(lines.unsigned_abs() as usize);
            drop(term);
            self.write(bytes);
        } else {
            term.scroll_display(Scroll::Delta(lines));
        }
    }

    pub(crate) fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    /// Converts a position relative to the grid's top-left into a grid point and side.
    pub(crate) fn point_at(&self, x: f32, y: f32) -> (Point, Side) {
        let s = self.grid_size();
        let col = ((x / s.cell_w).max(0.0) as usize).min(s.cols - 1);
        let line = ((y / s.cell_h).max(0.0) as usize).min(s.lines - 1);
        let offset = self.term.lock().grid().display_offset() as i32;
        let side = if (x / s.cell_w).fract() > 0.5 { Side::Right } else { Side::Left };
        (Point::new(Line(line as i32 - offset), Column(col)), side)
    }

    pub(crate) fn start_selection(&self, ty: SelectionType, x: f32, y: f32) {
        let (point, side) = self.point_at(x, y);
        self.term.lock().selection = Some(Selection::new(ty, point, side));
    }

    pub(crate) fn update_selection(&self, x: f32, y: f32) {
        let (point, side) = self.point_at(x, y);
        if let Some(sel) = self.term.lock().selection.as_mut() {
            sel.update(point, side);
        }
    }

    pub(crate) fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    pub(crate) fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    pub(crate) fn paste(&self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.mode().contains(TermMode::BRACKETED_PASTE) {
            let clean = text.replace('\x1b', "");
            self.write(format!("\x1b[200~{clean}\x1b[201~").into_bytes());
        } else {
            self.write(text.into_bytes());
        }
        self.scroll_to_bottom();
    }

    /// Stops the shell and everything it started.
    pub fn kill(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        if let Some(pid) = self.pid {
            kill_tree(pid);
        }
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

impl Drop for TerminalBackend {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(windows)]
fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(windows))]
fn kill_tree(pid: u32) {
    // The shell leads its own session/process group, so signal the whole group.
    let _ = std::process::Command::new("kill")
        .args(["-HUP", "--", &format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
