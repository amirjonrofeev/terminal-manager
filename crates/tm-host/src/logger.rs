//! Minimal `log` backend writing to `host.log` in the data dir (a service has no console).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

struct FileLogger {
    file: Mutex<File>,
    echo: bool,
}

impl log::Log for FileLogger {
    fn enabled(&self, meta: &log::Metadata) -> bool {
        meta.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("{} {:<5} {}\n", tm_core::time::now_local_display(), record.level(), record.args());
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
        if self.echo {
            eprint!("{line}");
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// Starts logging to `<dir>/host.log` (rotated to `host.log.1` past 10 MB). `echo` also prints to stderr.
pub fn init(dir: &Path, echo: bool) {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("host.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join("host.log.1"));
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else { return };
    if log::set_boxed_logger(Box::new(FileLogger { file: Mutex::new(file), echo })).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
