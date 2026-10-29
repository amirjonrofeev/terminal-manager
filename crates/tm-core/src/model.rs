//! Saved state owned by the host: everything here is written to `workspace.json`.
//! Per-user UI state (window, selected project and tab) lives in the GUI instead.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const WORKSPACE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Workspace {
    pub version: u32,
    pub projects: Vec<Project>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self { version: WORKSPACE_VERSION, projects: Vec::new() }
    }
}

impl Workspace {
    pub fn project(&self, id: Uuid) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn project_mut(&mut self, id: Uuid) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.id == id)
    }

    /// Finds a terminal and the project it belongs to.
    pub fn terminal(&self, id: Uuid) -> Option<(&Project, &TerminalSpec)> {
        self.projects.iter().find_map(|p| p.terminal(id).map(|t| (p, t)))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub root: PathBuf,
    pub default_shell: ShellKind,
    pub env: Vec<EnvVar>,
    pub terminals: Vec<TerminalSpec>,
    /// Open projects are restored (and their auto-start terminals run) at launch.
    pub open: bool,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            root: PathBuf::new(),
            default_shell: ShellKind::Default,
            env: Vec::new(),
            terminals: Vec::new(),
            open: true,
        }
    }
}

impl Project {
    pub fn terminal(&self, id: Uuid) -> Option<&TerminalSpec> {
        self.terminals.iter().find(|t| t.id == id)
    }

    /// Absolute working dir for a terminal's `cwd` (empty means the project root).
    pub fn resolve_cwd(&self, cwd: &str) -> PathBuf {
        let cwd = cwd.trim();
        if cwd.is_empty() { self.root.clone() } else { self.root.join(cwd) }
    }

    /// The `cwd` value to store for a picked folder: relative when it is inside the project, so
    /// the project can be moved; absolute otherwise.
    pub fn cwd_for(&self, picked: &Path) -> String {
        match picked.strip_prefix(&self.root) {
            Ok(rel) => rel.display().to_string(),
            Err(_) => picked.display().to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TerminalSpec {
    pub id: Uuid,
    pub name: String,
    pub shell: ShellKind,
    /// Typed into the shell after it starts. Empty means a plain shell.
    pub command: String,
    /// Relative to the project root. Empty means the root itself.
    pub cwd: String,
    pub env: Vec<EnvVar>,
    pub auto_start: bool,
    /// A developer's terminal (Claude Code, a shell) shown in the dev panel. Dev terminals run
    /// in the user's own host, never in the Windows service, and are not part of Start all.
    pub dev: bool,
    /// Write this terminal's output to the log files.
    pub log: bool,
}

impl Default for TerminalSpec {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "shell".into(),
            shell: ShellKind::Default,
            command: String::new(),
            cwd: String::new(),
            env: Vec::new(),
            auto_start: true,
            dev: false,
            log: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub enum ShellKind {
    /// The project's default shell, or the system default for a project.
    #[default]
    Default,
    Pwsh,
    PowerShell,
    Cmd,
    Bash,
    Zsh,
    Wsl,
    Custom { program: String, args: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picked_cwd_is_relative_inside_project() {
        let p = Project { root: PathBuf::from("/work/shop"), ..Default::default() };
        assert_eq!(p.cwd_for(Path::new("/work/shop")), "");
        assert_eq!(PathBuf::from(p.cwd_for(Path::new("/work/shop/apps/web"))), PathBuf::from("apps/web"));
        assert_eq!(p.cwd_for(Path::new("/elsewhere/api")), Path::new("/elsewhere/api").display().to_string());
        assert_eq!(p.resolve_cwd("  "), PathBuf::from("/work/shop"));
        assert_eq!(p.resolve_cwd("apps/web"), PathBuf::from("/work/shop/apps/web"));
    }
}
