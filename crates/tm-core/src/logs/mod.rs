//! Terminal output logs: plain-text, timestamped, one folder per terminal, one file per day.

pub mod level;
pub mod line;
pub mod writer;

pub use level::{Classifier, Level};
pub use line::LineAssembler;
pub use writer::LogWriter;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Host settings for logging, read from `settings.json` in the host data dir.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct LogSettings {
    /// Days of logs to keep; 0 keeps everything.
    pub retention_days: u32,
    /// A day's file continues in a new part past this size.
    pub max_file_mb: u32,
    /// Regex (case-insensitive by default) marking a line as an error.
    pub error_pattern: String,
    /// Regex marking a line as a warning.
    pub warning_pattern: String,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            retention_days: 30,
            max_file_mb: 200,
            error_pattern: level::DEFAULT_ERROR_PATTERN.into(),
            warning_pattern: level::DEFAULT_WARNING_PATTERN.into(),
        }
    }
}

/// `<data dir>/logs`
pub fn logs_root(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// Folder holding one terminal's daily files (named by id so renames don't split history).
pub fn terminal_dir(logs_root: &Path, terminal: Uuid) -> PathBuf {
    logs_root.join(terminal.to_string())
}

/// `logs/index.json`: which project and terminal each folder belongs to, kept for terminals
/// that were removed too, so their logs stay searchable and readable by people.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct LogIndex {
    pub terminals: BTreeMap<Uuid, IndexEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct IndexEntry {
    pub project: String,
    pub terminal: String,
    pub project_id: Option<Uuid>,
}
