//! Classifying log lines as errors or warnings.

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const DEFAULT_ERROR_PATTERN: &str =
    r"(?i)\b(error|err|fatal|crit(ical)?|fail(ed|ure)?|unhandled|exception|panic(ked)?)\b|^\s+at \S+\(";
pub const DEFAULT_WARNING_PATTERN: &str = r"(?i)\bwarn(ing)?\b";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
    /// Written by the host itself: started, stopped, exited.
    System,
}

impl Level {
    pub fn code(self) -> char {
        match self {
            Level::Info => 'I',
            Level::Warning => 'W',
            Level::Error => 'E',
            Level::System => 'S',
        }
    }

    pub fn from_code(c: char) -> Level {
        match c {
            'W' => Level::Warning,
            'E' => Level::Error,
            'S' => Level::System,
            _ => Level::Info,
        }
    }
}

pub struct Classifier {
    error: Regex,
    warning: Regex,
}

impl Classifier {
    /// Builds a classifier; invalid patterns fall back to the defaults (and are reported).
    pub fn new(error: &str, warning: &str) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let mut compile = |p: &str, default: &str, what: &str| {
            Regex::new(p).unwrap_or_else(|e| {
                problems.push(format!("Invalid {what} pattern {p:?}: {e}. Using the default."));
                Regex::new(default).expect("default pattern")
            })
        };
        let error = compile(error, DEFAULT_ERROR_PATTERN, "error");
        let warning = compile(warning, DEFAULT_WARNING_PATTERN, "warning");
        (Self { error, warning }, problems)
    }

    pub fn classify(&self, line: &str) -> Level {
        if self.error.is_match(line) {
            Level::Error
        } else if self.warning.is_match(line) {
            Level::Warning
        } else {
            Level::Info
        }
    }
}

impl Default for Classifier {
    fn default() -> Self {
        Self::new(DEFAULT_ERROR_PATTERN, DEFAULT_WARNING_PATTERN).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_formats() {
        let c = Classifier::default();
        for line in [
            "fail: Microsoft.AspNetCore.Server.Kestrel[13]",
            "ERROR 2026-10-29 db timeout",
            "Unhandled exception. System.NullReferenceException: Object reference not set",
            "   at DataMicron.EzSync.Server.Sync.Run() in C:\\src\\Sync.cs:line 42",
            "npm ERR! code ELIFECYCLE",
            "thread 'main' panicked at src/main.rs:3",
        ] {
            assert_eq!(c.classify(line), Level::Error, "{line}");
        }
        for line in ["warn: Microsoft.EntityFrameworkCore[10400]", "WARNING: deprecated option"] {
            assert_eq!(c.classify(line), Level::Warning, "{line}");
        }
        for line in ["info: Now listening on: https://localhost:7241", "errors: 0 found (terror)", "tick 5173"] {
            assert_ne!(c.classify(line), Level::Error, "{line}");
        }
    }

    #[test]
    fn bad_pattern_falls_back() {
        let (c, problems) = Classifier::new("(", DEFAULT_WARNING_PATTERN);
        assert_eq!(problems.len(), 1);
        assert_eq!(c.classify("fatal: boom"), Level::Error);
    }
}
