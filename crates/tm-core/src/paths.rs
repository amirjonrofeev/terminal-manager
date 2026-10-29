//! Where the host keeps its data and which socket it listens on.
//!
//! A Windows service is machine-wide (`%ProgramData%`, pipe `terminal-manager`). A host started
//! by the GUI for one user is per-user, so it never collides with a service on the same machine.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Overrides the host data directory (tests, portable installs).
pub const ENV_DATA_DIR: &str = "TERMINAL_MANAGER_DATA";
/// Overrides the socket name the GUI connects to and the host listens on.
pub const ENV_SOCKET: &str = "TERMINAL_MANAGER_SOCKET";
/// Overrides the GUI's per-user config folder (`ui.json`, and the v0.1 workspace it imports).
pub const ENV_CONFIG_DIR: &str = "TERMINAL_MANAGER_CONFIG";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostMode {
    /// Installed Windows service; survives logoff and starts at boot.
    Service,
    /// Background process started by the GUI for the current user.
    User,
}

pub fn data_dir(mode: HostMode) -> PathBuf {
    if let Some(dir) = std::env::var_os(ENV_DATA_DIR).filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    match mode {
        HostMode::Service if cfg!(windows) => std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("TerminalManager"),
        _ => local_data_dir().join(if cfg!(windows) { "host" } else { "" }),
    }
}

/// Per-user GUI state (`ui.json`) and the v0.1 `workspace.json`.
pub fn gui_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(ENV_CONFIG_DIR).filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    directories::BaseDirs::new()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("terminal-manager")
}

fn local_data_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("terminal-manager")
}

fn user_name() -> String {
    let name = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "user".into());
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect()
}

/// Socket name for a host mode: a pipe name on Windows, a socket file path on Unix.
pub fn socket_name(mode: HostMode) -> String {
    if let Ok(name) = std::env::var(ENV_SOCKET)
        && !name.is_empty()
    {
        return name;
    }
    if cfg!(windows) {
        match mode {
            HostMode::Service => "terminal-manager".into(),
            HostMode::User => format!("terminal-manager-{}", user_name()),
        }
    } else {
        // $XDG_RUNTIME_DIR is private to the user (0700); fall back to the data dir otherwise.
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| data_dir(mode));
        dir.join("terminal-manager.sock").display().to_string()
    }
}

/// Socket names the GUI tries, in order: an explicit override, the service, then the per-user host.
pub fn client_socket_candidates() -> Vec<(HostMode, String)> {
    if let Ok(name) = std::env::var(ENV_SOCKET)
        && !name.is_empty()
    {
        return vec![(HostMode::User, name)];
    }
    if cfg!(windows) {
        vec![(HostMode::Service, socket_name(HostMode::Service)), (HostMode::User, socket_name(HostMode::User))]
    } else {
        vec![(HostMode::User, socket_name(HostMode::User))]
    }
}
