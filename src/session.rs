//! Runtime state for a terminal: the live PTY and its status. Never saved.

use crate::model::{Project, TerminalSpec};
use crate::shell;
use crate::term::{SpawnOptions, TermEvent, TerminalBackend};
use std::sync::mpsc::Sender;
use std::time::Instant;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
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

/// Key for PTY events: which terminal and which run of it, so events from a killed run are ignored.
pub type EventKey = (Uuid, u64);

pub struct Session {
    pub status: Status,
    pub started_at: Option<Instant>,
    pub title: Option<String>,
    pub backend: Option<TerminalBackend>,
    generation: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self { status: Status::NotStarted, started_at: None, title: None, backend: None, generation: 0 }
    }
}

impl Session {
    pub fn start(
        &mut self,
        project: &Project,
        spec: &TerminalSpec,
        events: &Sender<(EventKey, TermEvent)>,
        ctx: &egui::Context,
    ) {
        self.stop();
        self.generation += 1;
        self.title = None;

        let cwd = if spec.cwd.trim().is_empty() { project.root.clone() } else { project.root.join(spec.cwd.trim()) };
        if !cwd.is_dir() {
            self.status = Status::Failed(format!("Folder not found: {}", cwd.display()));
            self.backend = None;
            return;
        }
        let sh = shell::resolve(&spec.shell, &project.default_shell);
        if sh.program.is_empty() {
            self.status = Status::Failed("No shell program set. Edit the terminal and choose a shell.".into());
            self.backend = None;
            return;
        }
        let opts = SpawnOptions {
            program: sh.program.clone(),
            args: sh.args,
            cwd,
            env: shell::merge_env(&project.env, &spec.env),
        };
        match TerminalBackend::spawn(opts, (spec.id, self.generation), events.clone(), ctx.clone()) {
            Ok(backend) => {
                let command = spec.command.trim();
                if !command.is_empty() {
                    backend.write(format!("{command}\r").into_bytes());
                }
                self.backend = Some(backend);
                self.status = Status::Running;
                self.started_at = Some(Instant::now());
            }
            Err(e) => {
                self.backend = None;
                self.status = Status::Failed(format!("Could not start {}: {e}", sh.program));
            }
        }
    }

    /// Kills the process but keeps the last screen visible.
    pub fn stop(&mut self) {
        if let Some(backend) = self.backend.as_mut() {
            backend.kill();
        }
        if self.status.is_running() {
            self.status = Status::Stopped;
        }
        self.started_at = None;
    }

    pub fn handle(&mut self, generation: u64, event: TermEvent) {
        if generation != self.generation {
            return;
        }
        match event {
            TermEvent::Exited(code) => {
                if self.status.is_running() {
                    self.status = Status::Exited(code);
                    self.started_at = None;
                }
            }
            TermEvent::Title(t) => self.title = Some(t),
        }
    }

    pub fn uptime(&self) -> Option<String> {
        let secs = self.started_at?.elapsed().as_secs();
        Some(match secs {
            0..60 => format!("{secs}s"),
            60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
            _ => format!("{}h {:02}m", secs / 3600, (secs / 60) % 60),
        })
    }
}
