//! Searching and paging through terminal logs on disk.
//!
//! Searches scan only the daily files that can contain the requested time range. There is no
//! index: a linear scan of a few hundred MB takes on the order of a second.

use super::level::Level;
use super::writer::{list_files, parse_line};
use chrono::{Local, NaiveDate, TimeZone};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SearchQuery {
    /// Text to find; empty matches every line (useful with a level filter).
    pub text: String,
    pub regex: bool,
    pub case_sensitive: bool,
    /// Unix ms, inclusive. `None` = no lower bound.
    pub from_ms: Option<i64>,
    /// Unix ms, exclusive. `None` = no upper bound.
    pub to_ms: Option<i64>,
    /// Levels to include; empty = all.
    pub levels: Vec<Level>,
    /// Terminals to search; `None` = every terminal that has logs.
    pub terminals: Option<Vec<Uuid>>,
    /// Most hits to return (the newest ones).
    pub limit: usize,
    /// Lines of context before and after each hit.
    pub context: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            regex: false,
            case_sensitive: false,
            from_ms: None,
            to_ms: None,
            levels: Vec::new(),
            terminals: None,
            limit: 500,
            context: 2,
        }
    }
}

/// One log line, with where it lives (so a viewer can jump to it).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub ts_ms: i64,
    /// The timestamp as written (local time with offset).
    pub ts: String,
    pub level: Level,
    pub text: String,
    pub file: String,
    pub line_no: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Hit {
    pub terminal: Uuid,
    pub entry: Entry,
    pub before: Vec<Entry>,
    pub after: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct SearchOutcome {
    /// Newest first.
    pub hits: Vec<Hit>,
    /// More lines matched than `limit`.
    pub truncated: bool,
    pub lines_scanned: u64,
    pub files_scanned: u32,
}

/// A page of one terminal's log around a point in time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Window {
    /// Oldest first.
    pub entries: Vec<Entry>,
    pub more_before: bool,
    pub more_after: bool,
}

enum Matcher {
    All,
    Plain { needle: String, case_sensitive: bool },
    Regex(Regex),
}

impl Matcher {
    fn new(q: &SearchQuery) -> Result<Self, String> {
        if q.text.is_empty() {
            return Ok(Matcher::All);
        }
        if q.regex {
            return RegexBuilder::new(&q.text)
                .case_insensitive(!q.case_sensitive)
                .build()
                .map(Matcher::Regex)
                .map_err(|e| format!("That regular expression is not valid: {e}"));
        }
        let needle = if q.case_sensitive { q.text.clone() } else { q.text.to_lowercase() };
        Ok(Matcher::Plain { needle, case_sensitive: q.case_sensitive })
    }

    fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::All => true,
            Matcher::Plain { needle, case_sensitive: true } => text.contains(needle.as_str()),
            Matcher::Plain { needle, case_sensitive: false } => text.to_lowercase().contains(needle.as_str()),
            Matcher::Regex(r) => r.is_match(text),
        }
    }
}

fn to_entry(raw: &str, file: &str, line_no: u32) -> Option<Entry> {
    let (ts, level, text) = parse_line(raw)?;
    Some(Entry {
        ts_ms: ts.timestamp_millis(),
        ts: raw.split('\t').next().unwrap_or_default().to_string(),
        level,
        text: text.to_string(),
        file: file.to_string(),
        line_no,
    })
}

/// The level code is the field after the first tab; read it without parsing the timestamp.
fn quick_level(raw: &str) -> Option<Level> {
    let rest = &raw[raw.find('\t')? + 1..];
    Some(Level::from_code(rest.chars().next()?))
}

fn quick_text(raw: &str) -> &str {
    raw.splitn(3, '\t').nth(2).unwrap_or("")
}

fn local_day(ms: i64) -> NaiveDate {
    Local.timestamp_millis_opt(ms).single().map(|t| t.date_naive()).unwrap_or_default()
}

/// Files of a terminal whose day can hold lines in `[from, to)`, oldest first. A day of slack
/// on each side covers lines written in another time zone offset.
fn files_in_range(dir: &Path, from_ms: Option<i64>, to_ms: Option<i64>) -> Vec<(String, PathBuf)> {
    let first = from_ms.map(|ms| local_day(ms).pred_opt().unwrap_or(NaiveDate::MIN));
    let last = to_ms.map(|ms| local_day(ms).succ_opt().unwrap_or(NaiveDate::MAX));
    list_files(dir)
        .into_iter()
        .filter(|(day, _, _)| first.is_none_or(|f| *day >= f) && last.is_none_or(|l| *day <= l))
        .map(|(_, _, path)| (path.file_name().unwrap_or_default().to_string_lossy().into_owned(), path))
        .collect()
}

/// Terminal folders under the logs root (named by terminal id).
pub fn terminals_with_logs(root: &Path) -> Vec<Uuid> {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().and_then(|n| Uuid::parse_str(n).ok()))
        .collect()
}

/// Searches the logs under `root`. `cancelled` is polled regularly; when it returns true the
/// search stops and returns `Err("cancelled")`.
pub fn search(root: &Path, q: &SearchQuery, cancelled: &dyn Fn() -> bool) -> Result<SearchOutcome, String> {
    let matcher = Matcher::new(q)?;
    let terminals = q.terminals.clone().unwrap_or_else(|| terminals_with_logs(root));
    let limit = q.limit.max(1);
    let mut out = SearchOutcome::default();
    let mut all: Vec<Hit> = Vec::new();

    for terminal in terminals {
        let dir = super::terminal_dir(root, terminal);
        // Newest `limit` hits of this terminal; a hit waits in `open` until its after-context is in.
        let mut kept: VecDeque<Hit> = VecDeque::new();
        let mut open: Vec<Hit> = Vec::new();
        // Context lines carry across files, so a hit at midnight shows the previous day's tail.
        let mut recent: VecDeque<Entry> = VecDeque::new();
        for (name, path) in files_in_range(&dir, q.from_ms, q.to_ms) {
            out.files_scanned += 1;
            let Ok(file) = File::open(&path) else { continue };
            for (i, raw) in BufReader::new(file).lines().enumerate() {
                let Ok(raw) = raw else { break };
                out.lines_scanned += 1;
                if out.lines_scanned % 20_000 == 0 && cancelled() {
                    return Err("cancelled".into());
                }
                let line_no = i as u32 + 1;
                let needs_entry = !open.is_empty() || q.context > 0 || {
                    let level_ok = q.levels.is_empty() || quick_level(&raw).is_some_and(|l| q.levels.contains(&l));
                    level_ok && matcher.is_match(quick_text(&raw))
                };
                if !needs_entry {
                    continue;
                }
                let Some(entry) = to_entry(&raw, &name, line_no) else { continue };

                for hit in open.iter_mut() {
                    hit.after.push(entry.clone());
                }
                while open.first().is_some_and(|h| h.after.len() >= q.context) {
                    kept.push_back(open.remove(0));
                }

                let in_range = q.from_ms.is_none_or(|f| entry.ts_ms >= f) && q.to_ms.is_none_or(|t| entry.ts_ms < t);
                let level_ok = q.levels.is_empty() || q.levels.contains(&entry.level);
                if in_range && level_ok && matcher.is_match(&entry.text) {
                    let hit = Hit {
                        terminal,
                        entry: entry.clone(),
                        before: recent.iter().cloned().collect(),
                        after: Vec::new(),
                    };
                    if q.context == 0 { kept.push_back(hit) } else { open.push(hit) }
                }
                while kept.len() > limit {
                    kept.pop_front();
                    out.truncated = true;
                }
                if q.context > 0 {
                    recent.push_back(entry);
                    if recent.len() > q.context {
                        recent.pop_front();
                    }
                }
            }
        }
        kept.extend(open);
        while kept.len() > limit {
            kept.pop_front();
            out.truncated = true;
        }
        all.extend(kept);
    }

    all.sort_by(|a, b| b.entry.ts_ms.cmp(&a.entry.ts_ms).then(b.entry.line_no.cmp(&a.entry.line_no)));
    if all.len() > limit {
        all.truncate(limit);
        out.truncated = true;
    }
    out.hits = all;
    Ok(out)
}

/// Reads up to `before` lines before `around_ms` and `after` lines from it on (or, with no
/// time, the newest `before` lines). `levels` filters which lines count; empty = all.
pub fn read_window(dir: &Path, around_ms: Option<i64>, before: usize, after: usize, levels: &[Level]) -> Window {
    let files: Vec<(String, PathBuf)> = list_files(dir)
        .into_iter()
        .map(|(_, _, p)| (p.file_name().unwrap_or_default().to_string_lossy().into_owned(), p))
        .collect();
    // Start one day before the target so "before" context can come from the previous day.
    let start = match around_ms {
        Some(ms) => {
            let first_day = local_day(ms).pred_opt().unwrap_or(NaiveDate::MIN);
            list_files(dir).iter().position(|(d, _, _)| *d >= first_day).unwrap_or(files.len())
        }
        None => 0,
    };
    let mut win = Window { more_before: start > 0, ..Default::default() };
    let mut ring: VecDeque<Entry> = VecDeque::new();
    let mut later: Vec<Entry> = Vec::new();
    'files: for (name, path) in &files[start..] {
        let Ok(file) = File::open(path) else { continue };
        for (i, raw) in BufReader::new(file).lines().enumerate() {
            let Ok(raw) = raw else { break };
            if !levels.is_empty() && !quick_level(&raw).is_some_and(|l| levels.contains(&l)) {
                continue;
            }
            let Some(entry) = to_entry(&raw, name, i as u32 + 1) else { continue };
            if around_ms.is_some_and(|t| entry.ts_ms >= t) {
                if later.len() >= after {
                    win.more_after = true;
                    break 'files;
                }
                later.push(entry);
            } else {
                ring.push_back(entry);
                if ring.len() > before {
                    ring.pop_front();
                    win.more_before = true;
                }
            }
        }
    }
    win.entries = ring.into_iter().chain(later).collect();
    win
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::writer::LogWriter;
    use chrono::{DateTime, Local, TimeZone};

    fn at(d: u32, h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap()
    }

    /// Two terminals with a few days of logs.
    fn fixture() -> (PathBuf, Uuid, Uuid) {
        let root = std::env::temp_dir().join(format!("tm-search-{}", Uuid::new_v4()));
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut wa = LogWriter::new(crate::logs::terminal_dir(&root, a), 1 << 20, 0);
        wa.write(at(27, 9, 0), Level::Info, "sync started").unwrap();
        wa.write(at(27, 9, 1), Level::Error, "fail: sync timed out for client 4471").unwrap();
        wa.write(at(27, 9, 2), Level::Info, "retrying").unwrap();
        wa.write(at(28, 10, 0), Level::Warning, "warn: slow query on invoices").unwrap();
        wa.write(at(29, 11, 0), Level::Error, "fail: Invoice 4471 rejected").unwrap();
        wa.write(at(29, 11, 1), Level::Info, "done").unwrap();
        wa.flush().unwrap();
        let mut wb = LogWriter::new(crate::logs::terminal_dir(&root, b), 1 << 20, 0);
        wb.write(at(28, 12, 0), Level::Info, "GET /invoices/4471 200").unwrap();
        wb.flush().unwrap();
        (root, a, b)
    }

    fn texts(o: &SearchOutcome) -> Vec<&str> {
        o.hits.iter().map(|h| h.entry.text.as_str()).collect()
    }

    #[test]
    fn finds_text_across_terminals_newest_first() {
        let (root, _, _) = fixture();
        let q = SearchQuery { text: "4471".into(), ..Default::default() };
        let o = search(&root, &q, &|| false).unwrap();
        assert_eq!(texts(&o), vec!["fail: Invoice 4471 rejected", "GET /invoices/4471 200", "fail: sync timed out for client 4471"]);
        assert!(!o.truncated);
    }

    #[test]
    fn date_range_level_and_scope() {
        let (root, a, _) = fixture();
        let day28 = SearchQuery {
            from_ms: Some(at(28, 0, 0).timestamp_millis()),
            to_ms: Some(at(29, 0, 0).timestamp_millis()),
            ..Default::default()
        };
        assert_eq!(texts(&search(&root, &day28, &|| false).unwrap()), vec!["GET /invoices/4471 200", "warn: slow query on invoices"]);

        let errors_in_a = SearchQuery { levels: vec![Level::Error], terminals: Some(vec![a]), ..Default::default() };
        assert_eq!(
            texts(&search(&root, &errors_in_a, &|| false).unwrap()),
            vec!["fail: Invoice 4471 rejected", "fail: sync timed out for client 4471"]
        );
    }

    #[test]
    fn case_regex_context_and_limit() {
        let (root, a, _) = fixture();
        let q = SearchQuery { text: "INVOICE".into(), case_sensitive: true, ..Default::default() };
        assert!(search(&root, &q, &|| false).unwrap().hits.is_empty());

        let q = SearchQuery { text: r"client \d+".into(), regex: true, terminals: Some(vec![a]), ..Default::default() };
        let o = search(&root, &q, &|| false).unwrap();
        let hit = &o.hits[0];
        assert_eq!(hit.before.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), vec!["sync started"]);
        // After-context continues into the next day's file.
        assert_eq!(
            hit.after.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            vec!["retrying", "warn: slow query on invoices"]
        );
        assert_eq!((hit.entry.file.as_str(), hit.entry.line_no), ("2026-10-27.log", 2));

        let q = SearchQuery { limit: 2, ..Default::default() };
        let o = search(&root, &q, &|| false).unwrap();
        assert_eq!(o.hits.len(), 2);
        assert!(o.truncated);

        let bad = SearchQuery { text: "(".into(), regex: true, ..Default::default() };
        assert!(search(&root, &bad, &|| false).is_err());
    }

    #[test]
    fn windows_around_a_time() {
        let (root, a, _) = fixture();
        let dir = crate::logs::terminal_dir(&root, a);
        let w = read_window(&dir, Some(at(28, 10, 0).timestamp_millis()), 2, 2, &[]);
        let t: Vec<&str> = w.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(t, vec!["fail: sync timed out for client 4471", "retrying", "warn: slow query on invoices", "fail: Invoice 4471 rejected"]);
        assert!(w.more_before && w.more_after);

        let latest = read_window(&dir, None, 2, 0, &[]);
        assert_eq!(latest.entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), vec!["fail: Invoice 4471 rejected", "done"]);

        let errors = read_window(&dir, None, 10, 0, &[Level::Error]);
        assert_eq!(errors.entries.len(), 2);
    }
}
