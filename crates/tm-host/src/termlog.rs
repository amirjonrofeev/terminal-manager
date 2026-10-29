//! One terminal's log on the host: assembles output into lines, classifies them, writes the
//! daily files, and counts errors and warnings for the GUI's badges.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tm_core::logs::{Classifier, Level, LineAssembler, LogSettings, LogWriter};

#[derive(Default)]
pub struct Counts {
    pub errors: AtomicU32,
    pub warnings: AtomicU32,
    /// Set when the counts changed and the GUI hasn't been told yet.
    pub changed: AtomicBool,
}

impl Counts {
    fn bump(&self, level: Level) {
        let counter = match level {
            Level::Error => &self.errors,
            Level::Warning => &self.warnings,
            _ => return,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        self.changed.store(true, Ordering::Release);
    }

    pub fn reset(&self) {
        self.errors.store(0, Ordering::Relaxed);
        self.warnings.store(0, Ordering::Relaxed);
        self.changed.store(true, Ordering::Release);
    }
}

pub struct TermLog {
    assembler: LineAssembler,
    writer: LogWriter,
    classifier: Arc<Classifier>,
    pub counts: Arc<Counts>,
    failed: bool,
    /// The command the host typed into the shell, while its echo may still arrive.
    echo: Option<(String, Instant)>,
    /// Off for terminals whose output shouldn't be logged (dev terminals).
    enabled: bool,
}

/// How long after start the shell's echo of the typed command is expected.
const ECHO_WINDOW: Duration = Duration::from_secs(15);

impl TermLog {
    pub fn new(dir: PathBuf, settings: &LogSettings, classifier: Arc<Classifier>) -> Self {
        let max_bytes = settings.max_file_mb as u64 * 1024 * 1024;
        Self {
            assembler: LineAssembler::default(),
            writer: LogWriter::new(dir, max_bytes, settings.retention_days),
            classifier,
            counts: Arc::default(),
            failed: false,
            echo: None,
            enabled: true,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// The host typed `command` into the shell. Shells echo it, often several times while
    /// redrawing the prompt; log it once as `$ command` instead of as output (it may contain
    /// words like "error" that would count as errors).
    pub fn expect_echo(&mut self, command: &str) {
        let command = command.trim();
        if !command.is_empty() {
            self.system(Level::System, &format!("$ {command}"));
            self.echo = Some((command.to_string(), Instant::now()));
        }
    }

    fn is_echo(&mut self, line: &str) -> bool {
        let Some((command, since)) = &self.echo else { return false };
        if since.elapsed() > ECHO_WINDOW {
            self.echo = None;
            return false;
        }
        // Redraws can be cut off at the window edge, so match a leading part of the command.
        let probe: String = command.chars().take(24).collect();
        line.contains(&probe)
    }

    /// Logs the complete lines in a chunk of output.
    pub fn feed(&mut self, bytes: &[u8]) {
        if !self.enabled {
            return;
        }
        let lines = self.assembler.feed(bytes);
        if lines.is_empty() {
            return;
        }
        let now = chrono::Local::now();
        for line in lines {
            if self.is_echo(&line) {
                continue;
            }
            let level = self.classifier.classify(&line);
            self.counts.bump(level);
            self.write(now, level, &line);
        }
        self.flush();
    }

    /// Writes any unfinished line (the process ended mid-line).
    pub fn finish(&mut self) {
        if let Some(line) = self.assembler.flush() {
            let level = self.classifier.classify(&line);
            self.counts.bump(level);
            self.write(chrono::Local::now(), level, &line);
        }
        self.flush();
    }

    /// A line from the host itself (started, exited, stopped). Errors count toward the badge.
    pub fn system(&mut self, level: Level, text: &str) {
        if !self.enabled {
            return;
        }
        self.finish();
        if level == Level::Error {
            self.counts.bump(level);
        }
        self.write(chrono::Local::now(), level, text);
        self.flush();
    }

    fn write(&mut self, ts: chrono::DateTime<chrono::Local>, level: Level, text: &str) {
        match self.writer.write(ts, level, text) {
            Ok(()) => self.failed = false,
            Err(e) if !self.failed => {
                self.failed = true;
                log::error!("writing a terminal log failed: {e}");
            }
            Err(_) => {}
        }
    }

    fn flush(&mut self) {
        let _ = self.writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_log(dir: &std::path::Path) -> String {
        std::fs::read_dir(dir).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect()
    }

    #[test]
    fn typed_command_echo_is_logged_once_and_not_counted() {
        let dir = std::env::temp_dir().join(format!("tm-termlog-{}", uuid::Uuid::new_v4()));
        let mut log = TermLog::new(dir.clone(), &LogSettings::default(), Arc::new(Classifier::default()));
        let cmd = "node -e \"console.log('fail: job timed out')\"";
        log.expect_echo(cmd);
        // PSReadLine-style redraws (one cut off), then the real output.
        log.feed(format!("> {}\r\n{cmd}\r\n> {cmd}\r\nfail: job timed out\r\n", &cmd[..30]).as_bytes());
        let text = read_log(&dir);
        let levels: Vec<&str> = text.lines().map(|l| l.split('\t').nth(1).unwrap()).collect();
        assert_eq!(levels, vec!["S", "E"], "{text}");
        assert!(text.contains(&format!("S\t$ {cmd}")));
        assert_eq!(log.counts.errors.load(Ordering::Relaxed), 1);
    }
}
