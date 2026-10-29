//! Wall-clock helpers.

/// Unix time in milliseconds.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Local time for banners and messages, e.g. `2026-10-29 13:58:25`.
pub fn now_local_display() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
