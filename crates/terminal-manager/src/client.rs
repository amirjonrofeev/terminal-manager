//! Connection to the host. A background thread connects (starting a per-user host if none is
//! running), forwards host messages to the app, and reconnects if the host goes away.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tm_core::ipc::{self, Stream};
use tm_core::paths::{self, HostMode};
use tm_core::protocol::{ClientMsg, HostInfo, HostMsg, PROTOCOL_VERSION};

pub enum ClientEvent {
    Connected(HostInfo),
    Msg(HostMsg),
    /// Lost or couldn't get a connection; the text says why. The client keeps retrying.
    Disconnected(String),
    /// The service is running but this process may not use it (not elevated).
    Denied(String),
}

pub struct HostClient {
    out: Sender<ClientMsg>,
    events: Receiver<ClientEvent>,
}

impl HostClient {
    pub fn start(ctx: egui::Context) -> Self {
        let (out, out_rx) = mpsc::channel();
        let (ev_tx, events) = mpsc::channel();
        let _ = std::thread::Builder::new().name("host-connection".into()).spawn(move || run(out_rx, ev_tx, ctx));
        Self { out, events }
    }

    /// Queues a message; dropped silently while disconnected (the app re-syncs on reconnect).
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.out.send(msg);
    }

    pub fn sender(&self) -> Sender<ClientMsg> {
        self.out.clone()
    }

    pub fn try_recv(&self) -> Option<ClientEvent> {
        self.events.try_recv().ok()
    }
}

fn run(out_rx: Receiver<ClientMsg>, events: Sender<ClientEvent>, ctx: egui::Context) {
    let notify = |e: ClientEvent| {
        let _ = events.send(e);
        ctx.request_repaint();
    };
    let mut last_spawn: Option<Instant> = None;
    let mut last_error = String::new();
    loop {
        match connect_any() {
            Ok(stream) => {
                last_error.clear();
                let reason = session(stream, &out_rx, &notify);
                notify(ClientEvent::Disconnected(reason));
            }
            Err(Failure::Denied(msg)) => {
                if msg != last_error {
                    notify(ClientEvent::Denied(msg.clone()));
                    last_error = msg;
                }
            }
            Err(Failure::NoHost) => {
                // Start a per-user host, at most every few seconds.
                if last_spawn.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
                    last_spawn = Some(Instant::now());
                    if let Err(e) = spawn_host() {
                        let msg = format!("Could not start the background host: {e}");
                        if msg != last_error {
                            notify(ClientEvent::Disconnected(msg.clone()));
                            last_error = msg;
                        }
                    }
                }
            }
        }
        // Discard requests made while disconnected; the app re-sends what matters on reconnect.
        while out_rx.try_recv().is_ok() {}
        std::thread::sleep(Duration::from_millis(250));
    }
}

enum Failure {
    NoHost,
    Denied(String),
}

fn connect_any() -> Result<Stream, Failure> {
    for (mode, socket) in paths::client_socket_candidates() {
        match ipc::connect(&socket) {
            Ok(s) => return Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && mode == HostMode::Service => {
                return Err(Failure::Denied(
                    "The Terminal Manager service is running, but this window isn't allowed to use it. Run Terminal Manager as an administrator.".into(),
                ));
            }
            Err(_) => {}
        }
    }
    Err(Failure::NoHost)
}

/// Runs one connection until it fails; returns the reason.
fn session(stream: Stream, out_rx: &Receiver<ClientMsg>, notify: &impl Fn(ClientEvent)) -> String {
    let stream = Arc::new(stream);
    if let Err(e) = ipc::write_frame(&mut &*stream, &ClientMsg::Hello { version: PROTOCOL_VERSION }) {
        return format!("Lost the connection to the host: {e}");
    }
    let alive = Arc::new(AtomicBool::new(true));
    let (reason_tx, reason_rx) = mpsc::channel::<String>();

    let reader = stream.clone();
    let reader_alive = alive.clone();
    let (msg_tx, msg_rx) = mpsc::channel::<HostMsg>();
    let _ = std::thread::Builder::new().name("host-reader".into()).spawn(move || {
        let reason = loop {
            match ipc::read_frame::<_, HostMsg>(&mut &*reader) {
                Ok(msg) => {
                    if msg_tx.send(msg).is_err() {
                        break "closed".to_string();
                    }
                }
                Err(e) => break format!("Lost the connection to the host: {e}"),
            }
        };
        reader_alive.store(false, Ordering::SeqCst);
        let _ = reason_tx.send(reason);
    });

    loop {
        while let Ok(msg) = msg_rx.try_recv() {
            match msg {
                HostMsg::Welcome(info) => notify(ClientEvent::Connected(info)),
                HostMsg::Error(e) if !alive.load(Ordering::SeqCst) => return e,
                other => notify(ClientEvent::Msg(other)),
            }
        }
        if !alive.load(Ordering::SeqCst) {
            // Deliver anything the reader got before it stopped.
            while let Ok(msg) = msg_rx.try_recv() {
                if let HostMsg::Error(e) = msg {
                    return e;
                }
                notify(ClientEvent::Msg(msg));
            }
            return reason_rx.recv().unwrap_or_else(|_| "Lost the connection to the host.".into());
        }
        match out_rx.recv_timeout(Duration::from_millis(15)) {
            Ok(msg) => {
                if let Err(e) = ipc::write_frame(&mut &*stream, &msg) {
                    return format!("Lost the connection to the host: {e}");
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return "closed".into(),
        }
    }
}

/// Starts this program again elevated (UAC prompt). Returns false if that couldn't be started.
pub fn restart_elevated() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let Ok(exe) = std::env::current_exe() else { return false };
        let path = exe.display().to_string().replace('\'', "''");
        return std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &format!("Start-Process -FilePath '{path}' -Verb RunAs")])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .is_ok_and(|s| s.success());
    }
    #[allow(unreachable_code)]
    false
}

/// Starts `<this exe> --host` detached, so it keeps running after the GUI closes.
fn spawn_host() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--host")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Break away from any job (some launchers kill their job's processes on exit); not
        // every job allows that, so fall back to a plain detached start.
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ())
}
