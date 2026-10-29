//! Writes one terminal's log as daily files: `2026-10-29.log`, continuing in `2026-10-29.1.log`,
//! `.2.log`, … past the size limit. Each line is `<local time with offset>\t<level>\t<text>`.

use super::level::Level;
use chrono::{DateTime, Local, NaiveDate};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f%:z";

pub struct LogWriter {
    dir: PathBuf,
    max_bytes: u64,
    retention_days: u32,
    open: Option<OpenFile>,
}

struct OpenFile {
    day: NaiveDate,
    part: u32,
    size: u64,
    out: BufWriter<File>,
}

/// File name for a day and continuation part (`part` 0 is the first file of the day).
pub fn file_name(day: NaiveDate, part: u32) -> String {
    if part == 0 { format!("{day}.log") } else { format!("{day}.{part}.log") }
}

/// Parses a log file name back into its day and part.
pub fn parse_file_name(name: &str) -> Option<(NaiveDate, u32)> {
    let stem = name.strip_suffix(".log")?;
    let (day, part) = match stem.split_once('.') {
        Some((d, p)) => (d, p.parse().ok()?),
        None => (stem, 0),
    };
    Some((NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?, part))
}

/// One formatted log line (without the newline).
pub fn format_line(ts: DateTime<Local>, level: Level, text: &str) -> String {
    // Tabs and newlines inside the text would break the format.
    let text: String = text.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect();
    format!("{}\t{}\t{}", ts.format(TIMESTAMP_FORMAT), level.code(), text)
}

/// Parses a line written by [`format_line`].
pub fn parse_line(line: &str) -> Option<(DateTime<chrono::FixedOffset>, Level, &str)> {
    let mut parts = line.splitn(3, '\t');
    let ts = DateTime::parse_from_str(parts.next()?, TIMESTAMP_FORMAT).ok()?;
    let level = Level::from_code(parts.next()?.chars().next()?);
    Some((ts, level, parts.next().unwrap_or("")))
}

impl LogWriter {
    pub fn new(dir: PathBuf, max_bytes: u64, retention_days: u32) -> Self {
        Self { dir, max_bytes: max_bytes.max(64 * 1024), retention_days, open: None }
    }

    pub fn write(&mut self, ts: DateTime<Local>, level: Level, text: &str) -> io::Result<()> {
        let line = format_line(ts, level, text);
        let day = ts.date_naive();
        let need = line.len() as u64 + 1;
        match &self.open {
            Some(f) if f.day == day && f.size + need <= self.max_bytes => {}
            // Same day, file full: continue in the next part.
            Some(f) if f.day == day => {
                let next = f.part + 1;
                self.roll(day, next, need)?;
            }
            _ => {
                let last = self.last_part(day);
                self.roll(day, last, need)?;
            }
        }
        let f = self.open.as_mut().expect("open after roll");
        f.out.write_all(line.as_bytes())?;
        f.out.write_all(b"\n")?;
        f.size += line.len() as u64 + 1;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        match self.open.as_mut() {
            Some(f) => f.out.flush(),
            None => Ok(()),
        }
    }

    /// Opens the first part of `day`, starting at `part`, with room for `need` more bytes (an
    /// empty file always qualifies). Also applies retention when the day changes.
    fn roll(&mut self, day: NaiveDate, mut part: u32, need: u64) -> io::Result<()> {
        if let Some(mut f) = self.open.take() {
            f.out.flush()?;
            if f.day != day {
                self.cleanup(day);
            }
        } else {
            self.cleanup(day);
        }
        fs::create_dir_all(&self.dir)?;
        loop {
            let path = self.dir.join(file_name(day, part));
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size == 0 || size + need <= self.max_bytes {
                let file = OpenOptions::new().create(true).append(true).open(&path)?;
                self.open = Some(OpenFile { day, part, size, out: BufWriter::new(file) });
                return Ok(());
            }
            part += 1;
        }
    }

    fn last_part(&self, day: NaiveDate) -> u32 {
        list_files(&self.dir).into_iter().filter(|(d, _, _)| *d == day).map(|(_, p, _)| p).max().unwrap_or(0)
    }

    /// Deletes files older than the retention period (0 keeps everything).
    pub fn cleanup(&self, today: NaiveDate) {
        if self.retention_days == 0 {
            return;
        }
        let oldest = today - chrono::Days::new(self.retention_days as u64);
        for (day, _, path) in list_files(&self.dir) {
            if day < oldest {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// Log files in a terminal's folder as (day, part, path), oldest first.
pub fn list_files(dir: &Path) -> Vec<(NaiveDate, u32, PathBuf)> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let (day, part) = parse_file_name(name.to_str()?)?;
            Some((day, part, e.path()))
        })
        .collect();
    files.sort_by_key(|(d, p, _)| (*d, *p));
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tm-log-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn lines_round_trip() {
        let ts = at(2026, 10, 29, 13);
        let line = format_line(ts, Level::Error, "boom\twith tab");
        let (parsed, level, text) = parse_line(&line).unwrap();
        assert_eq!(parsed, ts.fixed_offset());
        assert_eq!(level, Level::Error);
        assert_eq!(text, "boom with tab");
    }

    #[test]
    fn file_names_round_trip() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        assert_eq!(file_name(day, 0), "2026-10-29.log");
        assert_eq!(parse_file_name("2026-10-29.3.log"), Some((day, 3)));
        assert_eq!(parse_file_name("2026-10-29.log"), Some((day, 0)));
        assert_eq!(parse_file_name("notes.txt"), None);
    }

    #[test]
    fn rolls_daily_and_by_size() {
        let dir = temp_dir();
        let mut w = LogWriter::new(dir.clone(), 64 * 1024, 0);
        let long = "x".repeat(1000);
        for _ in 0..100 {
            w.write(at(2026, 10, 28, 23), Level::Info, &long).unwrap();
        }
        w.write(at(2026, 10, 29, 0), Level::Info, "next day").unwrap();
        w.flush().unwrap();
        let names: Vec<String> =
            list_files(&dir).iter().map(|(_, _, p)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["2026-10-28.log", "2026-10-28.1.log", "2026-10-29.log"]);
        assert!(fs::metadata(dir.join("2026-10-28.log")).unwrap().len() <= 64 * 1024);

        // A new writer (host restart) continues the last part instead of starting over.
        let mut w = LogWriter::new(dir.clone(), 64 * 1024, 0);
        w.write(at(2026, 10, 29, 1), Level::Info, "after restart").unwrap();
        w.flush().unwrap();
        let text = fs::read_to_string(dir.join("2026-10-29.log")).unwrap();
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn retention_removes_old_days() {
        let dir = temp_dir();
        let mut w = LogWriter::new(dir.clone(), 1024 * 1024, 7);
        w.write(at(2026, 10, 1, 12), Level::Info, "old").unwrap();
        w.write(at(2026, 10, 25, 12), Level::Info, "recent").unwrap();
        w.flush().unwrap();
        w.write(at(2026, 10, 29, 12), Level::Info, "today").unwrap();
        let days: Vec<String> = list_files(&dir).iter().map(|(d, _, _)| d.to_string()).collect();
        assert_eq!(days, vec!["2026-10-25", "2026-10-29"]);
    }
}
