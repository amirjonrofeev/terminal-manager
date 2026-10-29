//! Terminal Manager host: owns the terminal processes and keeps them running while GUIs come
//! and go. Runs as a per-user background process (started by the GUI) or, on servers, as a
//! Windows service.

mod host;
mod logger;
// Vendored from alacritty and kept close to upstream, unused parts included.
#[allow(dead_code)]
mod pty_loop;
mod server;
#[cfg(windows)]
pub mod service;
mod terminal;

use serde::{Deserialize, Serialize};
use std::io;
use std::sync::Arc;
use tm_core::model::Workspace;
use tm_core::paths::{self, HostMode};
use tm_core::protocol::HostInfo;
use tm_core::{ipc, persist};

pub use host::Host;

/// Service settings written by `tm-host install` (`service.json` in the data dir).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ServiceConfig {
    /// SIDs, besides SYSTEM, Administrators and the service account, allowed to connect.
    pub allow: Vec<String>,
}

/// Who may connect to the pipe, as SDDL. Controlling the host means running commands as its
/// account, so a service admits only SYSTEM, elevated Administrators, the service account
/// (the pipe's owner) and accounts given with `install --allow`; a per-user host admits only
/// its own user.
fn pipe_sddl(mode: HostMode, data_dir: &std::path::Path) -> String {
    match mode {
        HostMode::Service => {
            let config = persist::load::<ServiceConfig>(&data_dir.join("service.json")).value;
            let extra: String = config.allow.iter().map(|sid| format!("(A;;GA;;;{sid})")).collect();
            format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW){extra}")
        }
        HostMode::User => "D:P(A;;GA;;;SY)(A;;GA;;;OW)".into(),
    }
}

/// Starts a host: listens on the mode's socket and serves clients on a background thread.
/// Fails if another host already owns the socket.
pub fn start(mode: HostMode, echo_log: bool) -> io::Result<Arc<Host>> {
    let data_dir = paths::data_dir(mode);
    let socket = paths::socket_name(mode);
    logger::init(&data_dir, echo_log);

    if ipc::connect(&socket).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("a host is already running on {socket}")));
    }
    let listener = ipc::listen(&socket, Some(&pipe_sddl(mode, &data_dir)))?;
    alacritty_terminal::tty::setup_env();

    let ws = load_workspace(mode, &data_dir);
    let info = HostInfo {
        mode,
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").into(),
        data_dir: data_dir.display().to_string(),
    };
    log::info!("host {} started ({mode:?}) on {socket}, data in {}", info.version, data_dir.display());
    let host = Host::new(info, data_dir, ws);
    let serving = host.clone();
    std::thread::Builder::new().name("accept".into()).spawn(move || server::serve(serving, listener))?;
    Ok(host)
}

/// Runs a host until the process is killed.
pub fn run(mode: HostMode, echo_log: bool) -> io::Result<()> {
    let _host = start(mode, echo_log)?;
    loop {
        std::thread::park();
    }
}

/// Loads the host workspace. A per-user host with no workspace yet imports the one the v0.1
/// GUI kept in the user's config folder; never when the data dir is overridden (tests,
/// portable installs), so those never pick up and start the user's real terminals.
fn load_workspace(mode: HostMode, data_dir: &std::path::Path) -> Workspace {
    let path = data_dir.join("workspace.json");
    let overridden = std::env::var_os(paths::ENV_DATA_DIR).is_some_and(|d| !d.is_empty());
    if mode == HostMode::User && !overridden && !path.exists() {
        let legacy = paths::gui_config_dir().join("workspace.json");
        if legacy.exists() {
            let loaded = persist::load::<Workspace>(&legacy);
            if loaded.warning.is_none() {
                log::info!("imported {}", legacy.display());
                let _ = persist::save(&path, &loaded.value);
                return loaded.value;
            }
        }
    }
    let loaded = persist::load::<Workspace>(&path);
    if let Some(w) = &loaded.warning {
        log::warn!("{w}");
    }
    loaded.value
}
