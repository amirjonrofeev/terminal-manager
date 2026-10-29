//! One terminal on the host: its PTY run, its status, and the hub that fans output out to viewers.

use crate::pty_loop::{EventLoop, EventLoopSender, Msg, OutputHook};
use crate::termlog::TermLog;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::Rgb;
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use tm_core::logs::Level;
use tm_core::model::{Project, TerminalSpec};
use tm_core::palette;
use tm_core::protocol::{HostMsg, Status, TerminalState};
use tm_core::shell;
use uuid::Uuid;

/// Raw output kept per terminal and replayed to a viewer when it attaches.
pub const REPLAY_BYTES: usize = 1024 * 1024;
const DEFAULT_SIZE: (u16, u16) = (120, 30);

/// What a PTY run reports back to the host.
pub enum RunEvent {
    Exited(Option<i32>),
    Title(String),
}

/// Fans one terminal's output out to attached clients and keeps the replay buffer.
#[derive(Default)]
pub struct Hub {
    ring: VecDeque<u8>,
    subscribers: HashMap<u64, SyncSender<HostMsg>>,
}

impl Hub {
    /// Appends output; returns the clients whose queue overflowed (they were unsubscribed and
    /// need a fresh replay).
    pub fn push(&mut self, terminal: Uuid, bytes: &[u8]) -> Vec<u64> {
        self.ring.extend(bytes);
        let excess = self.ring.len().saturating_sub(REPLAY_BYTES);
        self.ring.drain(..excess);
        let mut lagging = Vec::new();
        for (&client, tx) in &self.subscribers {
            match tx.try_send(HostMsg::Output { terminal, bytes: bytes.to_vec() }) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => lagging.push(client),
            }
        }
        for c in &lagging {
            self.subscribers.remove(c);
        }
        lagging
    }

    /// Sends the replay and subscribes, atomically with respect to new output.
    pub fn attach(&mut self, terminal: Uuid, client: u64, tx: SyncSender<HostMsg>) {
        let (a, b) = self.ring.as_slices();
        let bytes = [a, b].concat();
        if tx.try_send(HostMsg::Replay { terminal, bytes }).is_ok() {
            self.subscribers.insert(client, tx);
        }
    }

    pub fn detach(&mut self, client: u64) {
        self.subscribers.remove(&client);
    }
}

#[derive(Clone)]
struct Proxy(mpsc::Sender<Event>);

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

struct Run {
    sender: EventLoopSender,
    term: Arc<FairMutex<Term<Proxy>>>,
    pid: Option<u32>,
    killed: bool,
}

impl Run {
    fn kill(&mut self) {
        if self.killed {
            return;
        }
        self.killed = true;
        if let Some(pid) = self.pid {
            kill_tree(pid);
        }
        let _ = self.sender.send(Msg::Shutdown);
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.kill();
    }
}

pub struct HostTerminal {
    pub state: TerminalState,
    pub hub: Arc<Mutex<Hub>>,
    pub log: Arc<Mutex<TermLog>>,
    run: Option<Run>,
    generation: u64,
    size: (u16, u16),
}

impl HostTerminal {
    pub fn new(id: Uuid, log: TermLog) -> Self {
        Self {
            state: TerminalState::new(id),
            hub: Arc::default(),
            log: Arc::new(Mutex::new(log)),
            run: None,
            generation: 0,
            size: DEFAULT_SIZE,
        }
    }

    /// Starts (or restarts) the terminal. `events` receives exit/title events tagged with
    /// this run's generation; `lagging` receives clients that need a fresh replay.
    pub fn start(
        &mut self,
        project: &Project,
        spec: &TerminalSpec,
        events: impl Fn(u64, RunEvent) + Send + 'static,
        lagging: impl Fn(Vec<u64>) + Send + 'static,
    ) {
        let restarted = self.run.is_some() || self.generation > 0;
        self.stop();
        self.generation += 1;
        self.state.title = None;

        let id = self.state.terminal;
        if restarted {
            let stamp = tm_core::time::now_local_display();
            // ConPTY clears the screen when the new console starts, so push the previous run and
            // this banner up into scrollback first; they stay visible by scrolling up.
            let banner = format!(
                "\x1b[0m\x1b[?1049l\x1b[?25h\r\n\x1b[2m── restarted {stamp} ──\x1b[0m{}",
                "\r\n".repeat(self.size.1 as usize)
            );
            let lag = self.hub.lock().unwrap().push(id, banner.as_bytes());
            if !lag.is_empty() {
                lagging(lag);
            }
        }

        let cwd = project.resolve_cwd(&spec.cwd);
        if !cwd.is_dir() {
            self.fail(format!("Folder not found: {}", cwd.display()));
            return;
        }
        let sh = shell::resolve(&spec.shell, &project.default_shell);
        if sh.program.is_empty() {
            self.fail("No shell program set. Edit the terminal and choose a shell.".into());
            return;
        }
        self.log.lock().unwrap().set_enabled(spec.log);
        let described = format!("started {} {} in {}", sh.program, sh.args.join(" "), cwd.display());
        let output = Output { hub: self.hub.clone(), log: self.log.clone() };
        match spawn(id, self.generation, sh.program.clone(), sh.args, cwd, shell::merge_env(&project.env, &spec.env), self.size, output, events, lagging) {
            Ok(run) => {
                self.log_system(Level::System, &described);
                let command = spec.command.trim();
                if !command.is_empty() {
                    self.log.lock().unwrap().expect_echo(command);
                    let _ = run.sender.send(Msg::Input(Cow::Owned(format!("{command}\r").into_bytes())));
                }
                self.run = Some(run);
                self.state.status = Status::Running;
                self.state.started_at_ms = Some(tm_core::time::now_ms());
            }
            Err(e) => self.fail(format!("Could not start {}: {e}", sh.program)),
        }
    }

    fn log_system(&self, level: Level, text: &str) {
        self.log.lock().unwrap().system(level, text);
    }

    fn fail(&mut self, msg: String) {
        log::warn!("terminal {}: {msg}", self.state.terminal);
        self.log_system(Level::Error, &msg);
        self.run = None;
        self.state.status = Status::Failed(msg);
        self.state.started_at_ms = None;
    }

    pub fn stop(&mut self) {
        if let Some(run) = self.run.as_mut() {
            run.kill();
        }
        self.run = None;
        if self.state.status.is_running() {
            self.state.status = Status::Stopped;
            self.log_system(Level::System, "stopped");
        }
        self.state.started_at_ms = None;
    }

    /// Applies an event from a run; returns whether the visible state changed.
    pub fn handle(&mut self, generation: u64, event: RunEvent) -> bool {
        if generation != self.generation {
            return false;
        }
        match event {
            RunEvent::Exited(code) => {
                if !self.state.status.is_running() {
                    return false;
                }
                self.state.status = Status::Exited(code);
                self.state.started_at_ms = None;
                self.run = None;
                match code {
                    Some(0) => self.log_system(Level::System, "exited with code 0"),
                    Some(c) => self.log_system(Level::Error, &format!("exited with code {c}")),
                    None => self.log_system(Level::System, "exited"),
                }
            }
            RunEvent::Title(t) => self.state.title = Some(t),
        }
        true
    }

    pub fn input(&self, bytes: Vec<u8>) {
        if let Some(run) = &self.run
            && !bytes.is_empty()
        {
            let _ = run.sender.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    pub fn resize(&mut self, cols: u16, lines: u16) {
        let size = (cols.max(2), lines.max(1));
        if size == self.size {
            return;
        }
        self.size = size;
        if let Some(run) = &self.run {
            let _ = run.sender.send(Msg::Resize(window_size(size)));
            run.term.lock().resize(TermSize::new(size.0 as usize, size.1 as usize));
        }
    }
}

/// Where a run's output goes: the replay hub (viewers) and the log.
struct Output {
    hub: Arc<Mutex<Hub>>,
    log: Arc<Mutex<TermLog>>,
}

fn window_size((cols, lines): (u16, u16)) -> WindowSize {
    WindowSize { num_lines: lines, num_cols: cols, cell_width: 8, cell_height: 16 }
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    id: Uuid,
    generation: u64,
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    env: Vec<(String, String)>,
    size: (u16, u16),
    output: Output,
    events: impl Fn(u64, RunEvent) + Send + 'static,
    lagging: impl Fn(Vec<u64>) + Send + 'static,
) -> std::io::Result<Run> {
    let opts = tty::Options {
        shell: Some(tty::Shell::new(program, args)),
        working_directory: Some(cwd),
        env: env.into_iter().collect(),
        drain_on_exit: true,
        ..Default::default()
    };
    let pty = tty::new(&opts, window_size(size), 0)?;
    #[cfg(windows)]
    let pid = pty.child_watcher().pid().map(|p| p.get());
    #[cfg(not(windows))]
    let pid = Some(pty.child().id());

    let (tx, rx) = mpsc::channel();
    let proxy = Proxy(tx);
    let config = term::Config { scrolling_history: 1000, ..Default::default() };
    let term = Arc::new(FairMutex::new(Term::new(config, &TermSize::new(size.0 as usize, size.1 as usize), proxy.clone())));

    let on_output: OutputHook = Box::new(move |bytes: &[u8]| {
        let lag = output.hub.lock().unwrap().push(id, bytes);
        if !lag.is_empty() {
            lagging(lag);
        }
        output.log.lock().unwrap().feed(bytes);
    });
    let event_loop = EventLoop::new(term.clone(), proxy, pty, true, on_output)?;
    let sender = event_loop.channel();
    let _ = event_loop.spawn();

    // Answers terminal queries (cursor position, colors, size) even when nobody is viewing,
    // so shells like pwsh never wait on a reply.
    let reply_tx = sender.clone();
    std::thread::Builder::new().name(format!("pty-events-{id}")).spawn(move || {
        let reply = |s: String| {
            let _ = reply_tx.send(Msg::Input(Cow::Owned(s.into_bytes())));
        };
        while let Ok(event) = rx.recv() {
            match event {
                Event::PtyWrite(s) => reply(s),
                Event::TextAreaSizeRequest(fmt) => reply(fmt(window_size(size))),
                Event::ColorRequest(index, fmt) => {
                    let (r, g, b) = palette::rgb_for_index(index);
                    reply(fmt(Rgb { r, g, b }));
                }
                Event::Title(t) => events(generation, RunEvent::Title(t)),
                Event::ChildExit(status) => events(generation, RunEvent::Exited(status.code())),
                Event::Exit => break,
                _ => {}
            }
        }
    })?;

    Ok(Run { sender, term, pid, killed: false })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_keeps_recent_output_and_replays_it() {
        let id = Uuid::new_v4();
        let mut hub = Hub::default();
        hub.push(id, &vec![b'a'; REPLAY_BYTES]);
        hub.push(id, b"tail");
        assert_eq!(hub.ring.len(), REPLAY_BYTES);

        let (tx, rx) = mpsc::sync_channel(4);
        hub.attach(id, 1, tx);
        match rx.try_recv().unwrap() {
            HostMsg::Replay { bytes, .. } => assert!(bytes.ends_with(b"aaaatail")),
            other => panic!("expected replay, got {other:?}"),
        }
        hub.push(id, b"live");
        assert!(matches!(rx.try_recv().unwrap(), HostMsg::Output { bytes, .. } if bytes == b"live"));
    }

    #[test]
    fn lagging_client_is_unsubscribed() {
        let id = Uuid::new_v4();
        let mut hub = Hub::default();
        let (tx, _rx) = mpsc::sync_channel(1);
        hub.attach(id, 7, tx); // the replay fills the queue
        assert_eq!(hub.push(id, b"x"), vec![7]);
        assert!(hub.subscribers.is_empty());
    }
}
