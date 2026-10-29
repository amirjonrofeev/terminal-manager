//! Terminal Manager host: owns the terminal processes and keeps them running while GUIs come
//! and go. Runs as a per-user background process (started by the GUI) or, on servers, as a
//! Windows service.

mod host;
mod logger;
// Vendored from alacritty and kept close to upstream, unused parts included.
#[allow(dead_code)]
mod pty_loop;
mod server;
mod terminal;

use std::io;
use tm_core::model::Workspace;
use tm_core::paths::{self, HostMode};
use tm_core::protocol::HostInfo;
use tm_core::{ipc, persist};

pub use host::Host;

/// Runs a host until the process is killed. Returns an error if it cannot listen, for example
/// because another host already owns the socket.
pub fn run(mode: HostMode, echo_log: bool) -> io::Result<()> {
    let data_dir = paths::data_dir(mode);
    let socket = paths::socket_name(mode);
    logger::init(&data_dir, echo_log);

    if ipc::connect(&socket).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("a host is already running on {socket}")));
    }
    let listener = ipc::listen(&socket)?;
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
    server::serve(host, listener);
    Ok(())
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
