//! Dev terminals: a service-mode host refuses them, and Start all / auto-start leave them alone.
//! Its own test binary, because hosts are configured through process-wide environment variables.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use tm_core::ipc;
use tm_core::model::{Project, ShellKind, TerminalSpec};
use tm_core::paths::{ENV_DATA_DIR, ENV_SOCKET, HostMode};
use tm_core::protocol::{ClientMsg, HostMsg, PROTOCOL_VERSION, Status};
use uuid::Uuid;

fn connect(socket: &str) -> (Arc<ipc::Stream>, mpsc::Receiver<HostMsg>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let stream = loop {
        match ipc::connect(socket) {
            Ok(s) => break Arc::new(s),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("could not connect: {e}"),
        }
    };
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
    (stream, rx)
}

fn wait_for(rx: &mpsc::Receiver<HostMsg>, what: &str, mut done: impl FnMut(&HostMsg) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(m) if done(&m) => return,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    panic!("timed out waiting for {what}");
}

fn shell_spec(name: &str, dev: bool) -> TerminalSpec {
    let shell = if cfg!(windows) { ShellKind::Cmd } else { ShellKind::Bash };
    TerminalSpec { name: name.into(), shell, auto_start: true, dev, log: !dev, ..Default::default() }
}

#[test]
fn service_refuses_dev_terminals_and_start_all_skips_them() {
    let dir = std::env::temp_dir().join(format!("tm-dev-test-{}", Uuid::new_v4()));
    let socket = if cfg!(windows) { format!("tm-dev-test-{}", Uuid::new_v4()) } else { dir.join("s.sock").display().to_string() };
    // SAFETY: set before any other thread in this test binary reads the environment.
    unsafe {
        std::env::set_var(ENV_DATA_DIR, &dir);
        std::env::set_var(ENV_SOCKET, &socket);
    }
    // Service mode (paths and rules) without the Service Control Manager.
    std::thread::spawn(|| tm_host::run(HostMode::Service, false).unwrap());
    let (stream, rx) = connect(&socket);
    let send = |m: ClientMsg| ipc::write_frame(&mut &*stream, &m).unwrap();

    // A project with one service terminal and one dev terminal is refused outright.
    let mixed = Project {
        name: "mixed".into(),
        root: dir.clone(),
        terminals: vec![shell_spec("svc", false), shell_spec("claude", true)],
        ..Default::default()
    };
    send(ClientMsg::UpsertProject(mixed));
    wait_for(&rx, "refusal", |m| matches!(m, HostMsg::Error(e) if e.contains("not in the Windows service")));

    // Adding a dev terminal to a service project is refused too.
    let svc = shell_spec("svc", false);
    let svc_id = svc.id;
    let project = Project { name: "p".into(), root: dir.clone(), terminals: vec![svc], ..Default::default() };
    let pid = project.id;
    send(ClientMsg::UpsertProject(project));
    wait_for(&rx, "service terminal running", |m| matches!(m, HostMsg::State(s) if s.terminal == svc_id && s.status == Status::Running));
    send(ClientMsg::UpsertTerminal { project: pid, spec: shell_spec("claude", true), index: None });
    wait_for(&rx, "refusal", |m| matches!(m, HostMsg::Error(e) if e.contains("not in the Windows service")));
    let ws: tm_core::model::Workspace =
        serde_json::from_str(&std::fs::read_to_string(dir.join("workspace.json")).unwrap()).unwrap();
    assert!(ws.projects.iter().all(|p| p.terminals.iter().all(|t| !t.dev)), "no dev terminal was saved");
}
