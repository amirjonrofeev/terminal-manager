//! Log views: a terminal's Logs tab and the search across all terminals. Both read the
//! host's log files (timestamped, one per day) through Search / ReadLog requests.

use super::{ACCENT, BAD, BG, DIM, FG, LINE, SIDE, WARN};
use crate::app::{Action, App};
use chrono::{Duration as ChronoDuration, Local, NaiveDate, TimeZone};
use egui::text::LayoutJob;
use egui::{Align, Color32, FontId, Frame, Layout, Margin, RichText, ScrollArea, Sense, TextFormat, Ui};
use regex::{Regex, RegexBuilder};
use tm_core::logs::Level;
use tm_core::logs::search::{Entry, Hit, SearchQuery};
use uuid::Uuid;

const ROW_HEIGHT: f32 = 19.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Range {
    Today,
    Yesterday,
    Last7,
    Last30,
    All,
    Custom,
}

impl Range {
    const ALL: [Range; 6] = [Range::Today, Range::Yesterday, Range::Last7, Range::Last30, Range::All, Range::Custom];

    fn label(self) -> &'static str {
        match self {
            Range::Today => "Today",
            Range::Yesterday => "Yesterday",
            Range::Last7 => "Last 7 days",
            Range::Last30 => "Last 30 days",
            Range::All => "All time",
            Range::Custom => "Dates…",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Levels {
    All,
    WarningsUp,
    Errors,
}

#[derive(Clone, Debug)]
pub struct Filters {
    pub text: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub range: Range,
    pub from: jiff::civil::Date,
    pub to: jiff::civil::Date,
    pub levels: Levels,
}

fn today_jiff() -> jiff::civil::Date {
    let d = Local::now().date_naive();
    to_jiff(d)
}

fn to_jiff(d: NaiveDate) -> jiff::civil::Date {
    use chrono::Datelike;
    jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8).unwrap_or_default()
}

fn to_chrono(d: jiff::civil::Date) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32).unwrap_or_default()
}

fn local_midnight_ms(d: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default())
        .earliest()
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

impl Filters {
    pub fn new(range: Range) -> Self {
        let today = today_jiff();
        Self {
            text: String::new(),
            regex: false,
            case_sensitive: false,
            range,
            from: today,
            to: today,
            levels: Levels::All,
        }
    }

    /// The time range as Unix ms `[from, to)`.
    pub fn bounds(&self) -> (Option<i64>, Option<i64>) {
        let today = Local::now().date_naive();
        let day = |d: NaiveDate| Some(local_midnight_ms(d));
        let now = Local::now().timestamp_millis();
        match self.range {
            Range::Today => (day(today), None),
            Range::Yesterday => (day(today - ChronoDuration::days(1)), day(today)),
            Range::Last7 => (Some(now - 7 * 86_400_000), None),
            Range::Last30 => (Some(now - 30 * 86_400_000), None),
            Range::All => (None, None),
            Range::Custom => {
                let (a, b) = (to_chrono(self.from), to_chrono(self.to));
                let (a, b) = if a <= b { (a, b) } else { (b, a) };
                (day(a), day(b + ChronoDuration::days(1)))
            }
        }
    }

    pub fn query(&self, terminals: Option<Vec<Uuid>>, limit: usize, context: usize) -> SearchQuery {
        let (from_ms, to_ms) = self.bounds();
        SearchQuery {
            text: self.text.trim().to_string(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            from_ms,
            to_ms,
            levels: match self.levels {
                Levels::All => vec![],
                Levels::WarningsUp => vec![Level::Warning, Level::Error],
                Levels::Errors => vec![Level::Error],
            },
            terminals,
            limit,
            context,
        }
    }

    /// Regex for highlighting the matches in results.
    fn highlighter(&self) -> Option<Regex> {
        let text = self.text.trim();
        if text.is_empty() {
            return None;
        }
        let pattern = if self.regex { text.to_string() } else { regex::escape(text) };
        RegexBuilder::new(&pattern).case_insensitive(!self.case_sensitive).build().ok()
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub terminal: Uuid,
    pub entry: Entry,
    pub before: Vec<Entry>,
    pub after: Vec<Entry>,
}

impl Row {
    pub fn from_hit(h: Hit) -> Self {
        Row { terminal: h.terminal, entry: h.entry, before: h.before, after: h.after }
    }

    pub fn plain(terminal: Uuid, entry: Entry) -> Self {
        Row { terminal, entry, before: vec![], after: vec![] }
    }
}

/// State of a list of log lines plus the request filling it.
#[derive(Default)]
pub struct Results {
    pub rows: Vec<Row>,
    pub req: u64,
    pub busy: bool,
    pub status: String,
    pub error: Option<String>,
    /// (file, line) of the line to highlight and scroll to.
    pub highlight: Option<(String, u32)>,
    pub scroll_to_highlight: bool,
    pub scroll_to_end: bool,
}

/// A terminal's Logs view.
pub struct TermLogs {
    pub terminal: Uuid,
    pub filters: Filters,
    pub results: Results,
    /// Showing the lines around one hit rather than the filtered list.
    pub context_of: Option<Entry>,
}

impl TermLogs {
    pub fn new(terminal: Uuid) -> Self {
        Self { terminal, filters: Filters::new(Range::Last7), results: Results::default(), context_of: None }
    }
}

/// Search across terminals.
pub struct GlobalSearch {
    pub filters: Filters,
    /// `None` = all projects.
    pub project: Option<Uuid>,
    pub results: Results,
}

impl Default for GlobalSearch {
    fn default() -> Self {
        Self { filters: Filters::new(Range::Last7), project: None, results: Results::default() }
    }
}

/// What a filter bar interaction asks for.
#[derive(PartialEq, Eq)]
enum BarEvent {
    None,
    Search,
}

fn filter_bar(ui: &mut Ui, id: &str, f: &mut Filters, busy: bool) -> BarEvent {
    let mut event = BarEvent::None;
    ui.horizontal_wrapped(|ui| {
        let edit = ui.add(
            egui::TextEdit::singleline(&mut f.text)
                .id_salt((id, "text"))
                .hint_text("Search text (empty = every line)")
                .font(egui::TextStyle::Monospace)
                .desired_width(280.0),
        );
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            event = BarEvent::Search;
        }
        if ui.selectable_label(f.case_sensitive, "Aa").on_hover_text("Match case").clicked() {
            f.case_sensitive = !f.case_sensitive;
            event = BarEvent::Search;
        }
        if ui.selectable_label(f.regex, ".*").on_hover_text("Regular expression").clicked() {
            f.regex = !f.regex;
            event = BarEvent::Search;
        }
        ui.separator();
        let before = f.range;
        egui::ComboBox::from_id_salt((id, "range")).selected_text(f.range.label()).width(110.0).show_ui(ui, |ui| {
            for r in Range::ALL {
                ui.selectable_value(&mut f.range, r, r.label());
            }
        });
        if f.range != before {
            event = BarEvent::Search;
        }
        if f.range == Range::Custom {
            let (from, to) = (f.from, f.to);
            ui.add(egui_extras::DatePickerButton::new(&mut f.from).id_salt(&format!("{id}-from")).calendar_week(false));
            ui.label(RichText::new("to").color(DIM));
            ui.add(egui_extras::DatePickerButton::new(&mut f.to).id_salt(&format!("{id}-to")).calendar_week(false));
            if (from, to) != (f.from, f.to) {
                event = BarEvent::Search;
            }
        }
        ui.separator();
        for (lv, label) in [(Levels::All, "All"), (Levels::WarningsUp, "Warnings + errors"), (Levels::Errors, "Errors")] {
            if ui.selectable_label(f.levels == lv, label).clicked() && f.levels != lv {
                f.levels = lv;
                event = BarEvent::Search;
            }
        }
        ui.separator();
        if ui.add_enabled(!busy, egui::Button::new("Search")).clicked() {
            event = BarEvent::Search;
        }
        if busy {
            ui.spinner();
        }
    });
    event
}

fn level_style(level: Level) -> (&'static str, Color32) {
    match level {
        Level::Error => ("E", BAD),
        Level::Warning => ("W", WARN),
        Level::System => ("•", ACCENT),
        Level::Info => (" ", DIM),
    }
}

/// `2026-10-29T14:47:38.449+05:00` -> `10-29 14:47:38.449`
fn short_time(ts: &str) -> String {
    if ts.len() >= 23 { format!("{} {}", &ts[5..10], &ts[11..23]) } else { ts.to_string() }
}

fn highlighted(text: &str, re: Option<&Regex>, color: Color32) -> LayoutJob {
    let font = FontId::monospace(13.0);
    let plain = TextFormat { font_id: font.clone(), color, ..Default::default() };
    let mark = TextFormat { font_id: font, color: FG, background: ACCENT.gamma_multiply(0.35), ..Default::default() };
    let mut job = LayoutJob::default();
    let mut last = 0;
    if let Some(re) = re {
        for m in re.find_iter(text).filter(|m| !m.is_empty()) {
            job.append(&text[last..m.start()], 0.0, plain.clone());
            job.append(m.as_str(), 0.0, mark.clone());
            last = m.end();
        }
    }
    job.append(&text[last..], 0.0, plain);
    job
}

/// What happened in a result list.
enum ListEvent {
    None,
    Open(usize),
}

fn result_list(
    ui: &mut Ui,
    id: &str,
    results: &mut Results,
    highlighter: Option<&Regex>,
    terminal_name: Option<&dyn Fn(Uuid) -> String>,
) -> ListEvent {
    let mut event = ListEvent::None;
    let n = results.rows.len();
    // Rows are exactly ROW_HEIGHT apart, so scroll offsets can be computed from row indices.
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut area = ScrollArea::vertical().id_salt((id, "list")).auto_shrink([false, false]);
    if results.scroll_to_highlight
        && let Some((file, line)) = &results.highlight
        && let Some(i) = results.rows.iter().position(|r| &r.entry.file == file && r.entry.line_no == *line)
    {
        let visible = ui.available_height();
        area = area.vertical_scroll_offset((i as f32 * ROW_HEIGHT - visible / 3.0).max(0.0));
        results.scroll_to_highlight = false;
    } else if results.scroll_to_end {
        area = area.vertical_scroll_offset(n as f32 * ROW_HEIGHT);
        results.scroll_to_end = false;
    }
    area.show_rows(ui, ROW_HEIGHT, n, |ui, range| {
        for i in range {
            let row = &results.rows[i];
            let is_highlight =
                results.highlight.as_ref().is_some_and(|(f, l)| *f == row.entry.file && *l == row.entry.line_no);
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            let painter = ui.painter_at(rect);
            if is_highlight {
                painter.rect_filled(rect, 0.0, ACCENT.gamma_multiply(0.18));
            } else if resp.hovered() {
                painter.rect_filled(rect, 0.0, SIDE);
            }
            let (code, color) = level_style(row.entry.level);
            let mut x = rect.min.x + 6.0;
            let mid = rect.center().y;
            painter.text(egui::pos2(x, mid), egui::Align2::LEFT_CENTER, short_time(&row.entry.ts), FontId::monospace(12.0), DIM);
            x += 150.0;
            painter.text(egui::pos2(x, mid), egui::Align2::LEFT_CENTER, code, FontId::monospace(12.0), color);
            x += 16.0;
            if let Some(name) = terminal_name {
                let galley = painter.layout_no_wrap(name(row.terminal), FontId::proportional(12.0), ACCENT);
                painter.galley(egui::pos2(x, mid - galley.size().y / 2.0), galley, ACCENT);
                x += 140.0;
            }
            let text_color = match row.entry.level {
                Level::Error => BAD,
                Level::Warning => WARN,
                _ => FG,
            };
            let galley = ui.fonts_mut(|f| f.layout_job(highlighted(&row.entry.text, highlighter, text_color)));
            painter.galley(egui::pos2(x, mid - galley.size().y / 2.0), galley, text_color);

            let resp = resp.on_hover_ui(|ui| {
                ui.label(RichText::new(&row.entry.ts).monospace().color(DIM));
                if !row.before.is_empty() || !row.after.is_empty() {
                    for e in &row.before {
                        ui.label(RichText::new(&e.text).monospace().color(DIM));
                    }
                    ui.label(RichText::new(&row.entry.text).monospace().color(FG));
                    for e in &row.after {
                        ui.label(RichText::new(&e.text).monospace().color(DIM));
                    }
                } else {
                    ui.label(RichText::new(&row.entry.text).monospace().color(FG));
                }
                ui.label(RichText::new("Click to see it in context").size(11.0).color(DIM));
            });
            let label = format!("{} {}", row.entry.ts, row.entry.text);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
            if resp.clicked() {
                event = ListEvent::Open(i);
            }
        }
    });
    event
}

fn status_line(ui: &mut Ui, results: &Results) {
    if let Some(e) = &results.error {
        ui.label(RichText::new(e).color(BAD));
    } else if !results.status.is_empty() {
        ui.label(RichText::new(&results.status).size(12.0).color(DIM));
    }
}

/// A terminal's Logs view (replaces the live terminal while selected).
pub fn terminal_logs(app: &mut App, ui: &mut Ui, tid: Uuid) {
    if app.term_logs.as_ref().is_none_or(|t| t.terminal != tid) {
        app.push(Action::LoadTermLogs(tid));
        return;
    }
    let Some(tl) = app.term_logs.as_mut() else { return };
    let mut actions = Vec::new();
    Frame::new().fill(BG).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
        if let Some(entry) = tl.context_of.clone() {
            ui.horizontal(|ui| {
                if ui.button("← Back to results").clicked() {
                    actions.push(Action::TermLogsSearch);
                }
                ui.label(RichText::new(format!("Lines around {}", entry.ts)).color(DIM));
            });
        } else if filter_bar(ui, "term-logs", &mut tl.filters, tl.results.busy) == BarEvent::Search {
            actions.push(Action::TermLogsSearch);
        }
        status_line(ui, &tl.results);
    });
    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), egui::Stroke::new(1.0, LINE));
    let highlighter = if tl.context_of.is_some() { None } else { tl.filters.highlighter() };
    if tl.results.rows.is_empty() && !tl.results.busy && tl.results.error.is_none() {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(RichText::new("No log lines match. Try a longer date range or fewer filters.").color(DIM));
        });
    } else if let ListEvent::Open(i) = result_list(ui, "term-logs", &mut tl.results, highlighter.as_ref(), None) {
        if tl.context_of.is_none() {
            let row = &tl.results.rows[i];
            actions.push(Action::OpenLogLine(row.terminal, row.entry.clone()));
        }
    }
    for a in actions {
        app.push(a);
    }
}

/// The search across terminals (shown in place of the terminal area).
pub fn global_search(app: &mut App, ui: &mut Ui) {
    let projects: Vec<(Uuid, String)> = app.ws.projects.iter().map(|p| (p.id, p.name.clone())).collect();
    let names: std::collections::HashMap<Uuid, String> = app
        .ws
        .projects
        .iter()
        .flat_map(|p| p.terminals.iter().map(move |t| (t.id, format!("{} › {}", p.name, t.name))))
        .collect();
    let mut actions = Vec::new();
    let s = &mut app.search;

    Frame::new().fill(SIDE).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Search logs").size(17.0).strong().color(FG));
            ui.label(RichText::new("in").color(DIM));
            let current = s.project.and_then(|id| projects.iter().find(|(p, _)| *p == id)).map(|(_, n)| n.clone());
            let before = s.project;
            egui::ComboBox::from_id_salt("search-scope")
                .selected_text(current.unwrap_or_else(|| "all projects".into()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.project, None, "all projects");
                    for (id, name) in &projects {
                        ui.selectable_value(&mut s.project, Some(*id), name);
                    }
                });
            if s.project != before {
                actions.push(Action::GlobalSearch);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Close").on_hover_text("Back to the terminals (Esc)").clicked() {
                    actions.push(Action::CloseSearch);
                }
            });
        });
        ui.add_space(4.0);
        if filter_bar(ui, "global", &mut s.filters, s.results.busy) == BarEvent::Search {
            actions.push(Action::GlobalSearch);
        }
        status_line(ui, &s.results);
    });

    let highlighter = s.filters.highlighter();
    let name_of = |id: Uuid| names.get(&id).cloned().unwrap_or_else(|| "(removed terminal)".into());
    if s.results.rows.is_empty() && !s.results.busy {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            let msg = if s.results.status.is_empty() {
                "Search every terminal's saved output: an error message, an order number, a client id…"
            } else {
                "Nothing found. Try a longer date range, fewer filters, or different words."
            };
            ui.label(RichText::new(msg).color(DIM));
        });
    } else if let ListEvent::Open(i) = result_list(ui, "global", &mut s.results, highlighter.as_ref(), Some(&name_of)) {
        let row = &s.results.rows[i];
        actions.push(Action::OpenLogLine(row.terminal, row.entry.clone()));
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        actions.push(Action::CloseSearch);
    }
    for a in actions {
        app.push(a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_whole_local_days() {
        let mut f = Filters::new(Range::Custom);
        f.from = jiff::civil::Date::new(2026, 10, 29).unwrap();
        f.to = jiff::civil::Date::new(2026, 10, 27).unwrap(); // reversed on purpose
        let (a, b) = f.bounds();
        let day = |d| local_midnight_ms(NaiveDate::from_ymd_opt(2026, 10, d).unwrap());
        assert_eq!((a, b), (Some(day(27)), Some(day(30))));

        let y = Filters::new(Range::Yesterday).bounds();
        assert_eq!(y.1.unwrap() - y.0.unwrap(), 86_400_000);
    }

    #[test]
    fn query_maps_levels_and_trims_text() {
        let mut f = Filters::new(Range::All);
        f.text = "  4471 ".into();
        f.levels = Levels::WarningsUp;
        let q = f.query(None, 10, 2);
        assert_eq!(q.text, "4471");
        assert_eq!(q.levels, vec![Level::Warning, Level::Error]);
        assert_eq!((q.from_ms, q.to_ms), (None, None));
    }

    #[test]
    fn short_time_format() {
        assert_eq!(short_time("2026-10-29T14:47:38.449+05:00"), "10-29 14:47:38.449");
    }
}
