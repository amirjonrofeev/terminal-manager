//! Drives a real host over its real socket: create a project, watch a terminal run and exit,
//! and check a second viewer gets the output replayed.

use std::sync::mpsc;
use std::time::{Duration, Instant};
use tm_core::ipc;
use tm_core::model::{Project, ShellKind, TerminalSpec};
use tm_core::paths::{ENV_DATA_DIR, ENV_SOCKET, HostMode};
use tm_core::protocol::{ClientMsg, HostMsg, PROTOCOL_VERSION, Status};

struct Conn {
    rx: mpsc::Receiver<HostMsg>,
    stream: std::sync::Arc<ipc::Stream>,
}

impl Conn {
    fn open(socket: &str) -> Conn {
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match ipc::connect(socket) {
                Ok(s) => break s,
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => panic!("could not connect: {e}"),
            }
        };
        let stream = std::sync::Arc::new(stream);
        ipc::write_frame(&mut &*stream, &ClientMsg::Hello { version: PROTOCOL_VERSION }).unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = stream.clone();
        std::thread::spawn(move || {
            while let Ok(msg) = ipc::read_frame::<_, HostMsg>(&mut &*reader) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        Conn { rx, stream }
    }

    fn send(&self, msg: ClientMsg) {
        ipc::write_frame(&mut &*self.stream, &msg).unwrap();
    }

    /// Collects messages until `done` returns true, or panics after a timeout.
    fn wait_for(&self, what: &str, mut done: impl FnMut(&HostMsg) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut got = Vec::new();
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.rx.recv_timeout(left) {
                Ok(msg) if done(&msg) => return,
                Ok(msg) => got.push(summary(&msg)),
                Err(_) => break,
            }
        }
        panic!("timed out waiting for {what}; received: {got:#?}");
    }
}

#[test]
fn host_runs_terminals_and_replays_output() {
    let dir = std::env::temp_dir().join(format!("tm-host-test-{}", uuid::Uuid::new_v4()));
    let socket = if cfg!(windows) {
        format!("tm-test-{}", uuid::Uuid::new_v4())
    } else {
        dir.join("host.sock").display().to_string()
    };
    // SAFETY: set before any other thread in this test binary reads the environment.
    unsafe {
        std::env::set_var(ENV_DATA_DIR, &dir);
        std::env::set_var(ENV_SOCKET, &socket);
    }
    std::thread::spawn(|| tm_host::run(HostMode::User, false).unwrap());

    let a = Conn::open(&socket);
    a.wait_for("welcome", |m| matches!(m, HostMsg::Welcome(_)));

    let (shell, command) = if cfg!(windows) {
        (ShellKind::Cmd, "echo hello-from-host & exit 2")
    } else {
        (ShellKind::Bash, "echo hello-from-host; exit 2")
    };
    let spec = TerminalSpec { name: "t".into(), shell, command: command.into(), auto_start: true, ..Default::default() };
    let tid = spec.id;
    let project = Project { name: "p".into(), root: dir.clone(), terminals: vec![spec], ..Default::default() };
    a.send(ClientMsg::Attach(tid)); // ignored: the terminal doesn't exist yet
    a.send(ClientMsg::UpsertProject(project));
    a.send(ClientMsg::Attach(tid));

    // Output and the exit status come from different threads, so accept them in either order.
    let mut seen = Vec::new();
    let mut exited = false;
    a.wait_for("output and exit code 2", |m| {
        match m {
            HostMsg::Output { terminal, bytes } | HostMsg::Replay { terminal, bytes } if *terminal == tid => {
                seen.extend_from_slice(bytes)
            }
            HostMsg::State(s) if s.terminal == tid && s.status == Status::Exited(Some(2)) => exited = true,
            _ => {}
        }
        // The typed command is echoed, then its output follows: two occurrences.
        exited && String::from_utf8_lossy(&seen).matches("hello-from-host").count() >= 2
    });
    assert!(dir.join("workspace.json").exists(), "workspace saved");

    // The run is in today's log: timestamped lines, the output, and the exit marked as an error.
    let log_dir = dir.join("logs").join(tid.to_string());
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        let text = std::fs::read_dir(&log_dir)
            .map(|d| d.flatten().map(|e| std::fs::read_to_string(e.path()).unwrap_or_default()).collect::<String>())
            .unwrap_or_default();
        if text.contains("exited with code 2") || Instant::now() > deadline {
            break text;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let lines: Vec<(&str, &str, &str)> = text
        .lines()
        .map(|l| {
            let mut p = l.splitn(3, '\t');
            (p.next().unwrap(), p.next().unwrap_or(""), p.next().unwrap_or(""))
        })
        .collect();
    assert!(lines.iter().all(|(ts, _, _)| ts.len() >= 29 && ts.contains('T')), "timestamps: {text}");
    assert!(lines.iter().any(|(_, lvl, t)| *lvl == "S" && t.starts_with("started ")), "start line: {text}");
    assert!(lines.iter().any(|(_, lvl, t)| *lvl == "I" && *t == "hello-from-host"), "output line: {text}");
    assert!(lines.iter().any(|(_, lvl, t)| *lvl == "E" && *t == "exited with code 2"), "exit line: {text}");
    assert!(!text.contains('\x1b'), "no escape sequences in the log: {text:?}");
    assert!(dir.join("logs").join("index.json").exists(), "log index");

    // The failed exit is counted for the badge, and MarkSeen clears it.
    a.wait_for("an unseen error", |m| matches!(m, HostMsg::State(s) if s.terminal == tid && s.unseen_errors >= 1));
    a.send(ClientMsg::MarkSeen(tid));
    a.wait_for("errors cleared", |m| matches!(m, HostMsg::State(s) if s.terminal == tid && s.unseen_errors == 0));

    // A second viewer attaching later gets the earlier output replayed.
    let b = Conn::open(&socket);
    b.send(ClientMsg::Attach(tid));
    b.wait_for("replay", |m| {
        matches!(m, HostMsg::Replay { terminal, bytes } if *terminal == tid && String::from_utf8_lossy(bytes).contains("hello-from-host"))
    });
}

fn summary(m: &HostMsg) -> String {
    match m {
        HostMsg::Output { bytes, .. } => format!("Output {:?}", String::from_utf8_lossy(bytes)),
        HostMsg::Replay { bytes, .. } => format!("Replay {:?}", String::from_utf8_lossy(bytes)),
        HostMsg::Workspace(_) => "Workspace".into(),
        other => format!("{other:?}"),
    }
}
