//! Messages between the GUI (client) and the host.

use crate::model::{Project, TerminalSpec, Workspace};
use crate::paths::HostMode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Bump on any change to these types; mismatched peers refuse to talk.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Status {
    NotStarted,
    Running,
    Exited(Option<i32>),
    Stopped,
    Failed(String),
}

impl Status {
    pub fn is_running(&self) -> bool {
        matches!(self, Status::Running)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Status::Failed(_) | Status::Exited(Some(1..)) | Status::Exited(Some(..=-1)))
    }

    pub fn label(&self) -> String {
        match self {
            Status::NotStarted => "Not started".into(),
            Status::Running => "Running".into(),
            Status::Exited(Some(code)) => format!("Exited (code {code})"),
            Status::Exited(None) => "Exited".into(),
            Status::Stopped => "Stopped".into(),
            Status::Failed(_) => "Failed to start".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TerminalState {
    pub terminal: Uuid,
    pub status: Status,
    /// Unix time in milliseconds when the current run started.
    pub started_at_ms: Option<i64>,
    pub title: Option<String>,
}

impl TerminalState {
    pub fn new(terminal: Uuid) -> Self {
        Self { terminal, status: Status::NotStarted, started_at_ms: None, title: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HostInfo {
    pub mode: HostMode,
    pub pid: u32,
    pub version: String,
    pub data_dir: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientMsg {
    Hello { version: u32 },
    /// Adds a project (with its terminals, auto-starting them if open) or updates an existing
    /// project's settings. Updates leave the existing terminal list alone.
    UpsertProject(Project),
    RemoveProject(Uuid),
    SetProjectOpen { project: Uuid, open: bool },
    /// Adds or replaces a terminal. `index` places a new terminal (e.g. after the one duplicated).
    UpsertTerminal { project: Uuid, spec: TerminalSpec, index: Option<usize> },
    RemoveTerminal { project: Uuid, terminal: Uuid },
    /// Starts, or restarts when already running.
    Start(Uuid),
    Stop(Uuid),
    StartAll(Uuid),
    StopAll(Uuid),
    /// Receive a replay of recent output, then live output, for a terminal.
    Attach(Uuid),
    Detach(Uuid),
    Input { terminal: Uuid, bytes: Vec<u8> },
    Resize { terminal: Uuid, cols: u16, lines: u16 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum HostMsg {
    Welcome(HostInfo),
    /// The full saved state; sent after connecting and after every change.
    Workspace(Workspace),
    State(TerminalState),
    /// Recent output for a freshly attached terminal; the client should reset its screen first.
    Replay { terminal: Uuid, bytes: Vec<u8> },
    Output { terminal: Uuid, bytes: Vec<u8> },
    /// A request could not be carried out.
    Error(String),
}
