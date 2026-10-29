//! App state and the actions the UI can trigger. The host owns projects and processes; the app
//! mirrors its workspace, renders terminal screens, and keeps per-user UI state.

use crate::client::{ClientEvent, HostClient, Target};
use crate::term::Screen;
use crate::ui;
use crate::ui::dialogs::{ConfirmAction, Dialog};
use crate::ui::logs::{GlobalSearch, Row, TermLogs};
use tm_core::logs::search::Entry;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tm_core::model::{Project, TerminalSpec, Workspace};
use tm_core::persist;
use tm_core::protocol::{ClientMsg, HostInfo, HostMsg, Status, TerminalState};
use uuid::Uuid;

const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub maximized: bool,
}

/// Which side of the project view the dev panel tiles on.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DevSide {
    #[default]
    Left,
    Right,
}

/// Per-user view state, saved in `ui.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct UiState {
    pub active_project: Option<Uuid>,
    pub active_tab: HashMap<Uuid, Uuid>,
    pub window: Option<WindowState>,
    /// Dev panel on/off; `None` = on, except when connected to the Windows service.
    pub dev_enabled: Option<bool>,
    pub dev_side: DevSide,
    /// Selected dev terminal per project.
    pub dev_tab: HashMap<Uuid, Uuid>,
    pub sidebar_collapsed: bool,
}

/// What a new dev terminal runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevPreset {
    ClaudeCode,
    Shell,
    Custom,
}

/// Which host connection a message came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Conn {
    Main,
    Dev,
}

pub enum Connection {
    Connecting,
    Connected(HostInfo),
    Disconnected(String),
    Denied(String),
}

#[derive(Clone, Debug)]
pub enum Action {
    SelectProject(Uuid),
    NewProject,
    EditProject(Uuid),
    OpenProject(Uuid),
    CloseProject(Uuid),
    RemoveProject(Uuid),
    StartAll(Uuid),
    StopAll(Uuid),
    SelectTab(Uuid, Uuid),
    NewTerminal(Uuid),
    EditTerminal(Uuid, Uuid),
    DuplicateTerminal(Uuid, Uuid),
    RemoveTerminal(Uuid, Uuid),
    Start(Uuid),
    Stop(Uuid),
    NextTab(i32),
    DismissBanner,
    /// Switch a terminal between its live screen and its saved logs.
    ShowLogs(Uuid, bool),
    LoadTermLogs(Uuid),
    TermLogsSearch,
    OpenSearch,
    CloseSearch,
    GlobalSearch,
    /// Show a log line in its terminal's Logs view, with the lines around it.
    OpenLogLine(Uuid, Entry),
    /// Open (or focus) the find bar over a terminal's live screen.
    OpenFind(Uuid),
    NewDevTerminal(Uuid, DevPreset),
    SelectDevTab(Uuid, Uuid),
    /// Open the project folder in VS Code (`code <folder>`).
    OpenInEditor(Uuid),
    SetDevEnabled(bool),
    SetDevSide(DevSide),
    ToggleSidebar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Rollup {
    Idle,
    Ok,
    Warn,
    Bad,
}

pub struct App {
    pub ws: Workspace,
    pub screens: HashMap<Uuid, Screen>,
    pub states: HashMap<Uuid, TerminalState>,
    pub connection: Connection,
    pub ui: UiState,
    client: HostClient,
    attached: HashSet<Uuid>,
    /// Dev terminals live in the per-user host; this mirrors its workspace (dev terminals only).
    pub dev_ws: Workspace,
    pub dev_connection: Connection,
    dev_client: Option<HostClient>,
    attached_dev: HashSet<Uuid>,
    /// Give the dev terminal keyboard focus next frame.
    pub focus_dev: bool,
    ctx: egui::Context,
    /// Terminals we asked the host to reset unseen counts for, until it confirms.
    pending_seen: HashSet<Uuid>,
    ui_path: PathBuf,
    dirty: bool,
    last_save: Instant,
    pub banner: Option<String>,
    pub dialog: Option<Dialog>,
    pub focus_terminal: bool,
    actions: Vec<Action>,
    /// Terminals currently showing their Logs view instead of the live screen.
    pub log_mode: HashSet<Uuid>,
    pub term_logs: Option<TermLogs>,
    pub search: GlobalSearch,
    pub search_open: bool,
    next_req: u64,
    /// Find bar (Ctrl+F) over a terminal's live screen.
    pub find: Option<FindBar>,
    /// Detected project type per terminal, with the command/folder it was detected from.
    kinds: HashMap<Uuid, (String, crate::ui::kind::Kind)>,
}

pub struct FindBar {
    pub terminal: Uuid,
    pub query: String,
    pub case_sensitive: bool,
    pub focus: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, ui_path: PathBuf, ui_state: persist::Loaded<UiState>) -> Self {
        ui::setup(&cc.egui_ctx);
        Self {
            ws: Workspace::default(),
            screens: HashMap::new(),
            states: HashMap::new(),
            connection: Connection::Connecting,
            ui: ui_state.value,
            client: HostClient::start(cc.egui_ctx.clone(), Target::Main),
            attached: HashSet::new(),
            dev_ws: Workspace::default(),
            dev_connection: Connection::Connecting,
            dev_client: None,
            attached_dev: HashSet::new(),
            focus_dev: false,
            ctx: cc.egui_ctx.clone(),
            pending_seen: HashSet::new(),
            ui_path,
            dirty: false,
            last_save: Instant::now(),
            banner: ui_state.warning,
            dialog: None,
            focus_terminal: true,
            actions: Vec::new(),
            log_mode: HashSet::new(),
            term_logs: None,
            search: GlobalSearch::default(),
            search_open: false,
            next_req: 1,
            find: None,
            kinds: HashMap::new(),
        }
    }

    // ---------- queries ----------

    pub fn is_connected(&self) -> bool {
        matches!(self.connection, Connection::Connected(_))
    }

    /// Whether the dev panel is on. Off by default when connected to the Windows service, so
    /// servers don't start a per-user host unless someone turns it on.
    pub fn dev_enabled(&self) -> bool {
        match (self.ui.dev_enabled, &self.connection) {
            (Some(on), _) => on,
            (None, Connection::Connected(info)) => info.mode != tm_core::paths::HostMode::Service,
            _ => false,
        }
    }

    pub fn dev_connected(&self) -> bool {
        matches!(self.dev_connection, Connection::Connected(_))
    }

    /// A project's dev terminals (from the per-user host).
    pub fn dev_terminals(&self, pid: Uuid) -> Vec<TerminalSpec> {
        self.dev_ws.project(pid).map(|p| p.terminals.clone()).unwrap_or_default()
    }

    pub fn active_dev_tab(&self, pid: Uuid) -> Option<Uuid> {
        let terms = self.dev_ws.project(pid).map(|p| &p.terminals[..]).unwrap_or(&[]);
        self.ui
            .dev_tab
            .get(&pid)
            .copied()
            .filter(|id| terms.iter().any(|t| t.id == *id))
            .or_else(|| terms.first().map(|t| t.id))
    }

    fn is_dev(&self, tid: Uuid) -> bool {
        self.dev_ws.terminal(tid).is_some()
    }

    /// The host connection that owns a terminal.
    fn send_for(&self, tid: Uuid, msg: ClientMsg) {
        match &self.dev_client {
            Some(dev) if self.is_dev(tid) => dev.send(msg),
            _ => self.client.send(msg),
        }
    }

    fn send_dev(&self, msg: ClientMsg) {
        if let Some(dev) = &self.dev_client {
            dev.send(msg);
        }
    }

    /// Makes sure the per-user host has the project (folder, env) for its dev terminals.
    fn ensure_dev_project(&self, pid: Uuid) {
        if let Some(p) = self.ws.project(pid) {
            let meta = Project { terminals: Vec::new(), open: true, ..p.clone() };
            self.send_dev(ClientMsg::UpsertProject(meta));
        }
    }

    /// Starts or stops the dev connection to follow the setting.
    fn sync_dev_client(&mut self) {
        let on = self.dev_enabled();
        if on && self.dev_client.is_none() {
            self.dev_client = Some(HostClient::start(self.ctx.clone(), Target::User));
            self.dev_connection = Connection::Connecting;
        } else if !on && self.dev_client.is_some() {
            self.dev_client = None;
            self.dev_ws = Workspace::default();
            self.attached_dev.clear();
            self.sync_terminals();
        }
    }

    pub fn active_project(&self) -> Option<&Project> {
        self.ui.active_project.and_then(|id| self.ws.project(id)).filter(|p| p.open)
    }

    /// The selected tab of a project, falling back to its first terminal.
    pub fn active_tab(&self, project: &Project) -> Option<Uuid> {
        self.ui
            .active_tab
            .get(&project.id)
            .copied()
            .filter(|id| project.terminal(*id).is_some())
            .or_else(|| project.terminals.first().map(|t| t.id))
    }

    pub fn state(&self, terminal: Uuid) -> TerminalState {
        self.states.get(&terminal).cloned().unwrap_or_else(|| TerminalState::new(terminal))
    }

    pub fn status(&self, terminal: Uuid) -> Status {
        self.states.get(&terminal).map(|s| s.status.clone()).unwrap_or(Status::NotStarted)
    }

    pub fn running_count(&self, project: &Project) -> usize {
        project.terminals.iter().filter(|t| self.status(t.id).is_running()).count()
    }

    /// Terminals that were never started don't count, so an idle helper tab doesn't turn the dot amber.
    pub fn rollup(&self, project: &Project) -> Rollup {
        let statuses: Vec<Status> =
            project.terminals.iter().map(|t| self.status(t.id)).filter(|s| *s != Status::NotStarted).collect();
        let running = statuses.iter().filter(|s| s.is_running()).count();
        let errors = statuses.iter().any(|s| s.is_error());
        match (running, statuses.len()) {
            (_, 0) => Rollup::Idle,
            (r, n) if r == n => Rollup::Ok,
            (0, _) if errors => Rollup::Bad,
            (0, _) => Rollup::Idle,
            _ => Rollup::Warn,
        }
    }

    /// The project type shown on a terminal's tab (detection is cached until its command or folder changes).
    pub fn kind_of(&mut self, pid: Uuid, tid: Uuid) -> crate::ui::kind::Kind {
        use crate::ui::kind::Kind;
        let found = self.ws.project(pid).and_then(|p| p.terminal(tid).map(|t| (p, t)));
        let Some((project, spec)) = found.or_else(|| self.dev_ws.terminal(tid)) else {
            return Kind::Shell;
        };
        let key = Kind::cache_key(project, spec);
        if let Some((k, kind)) = self.kinds.get(&tid)
            && *k == key
        {
            return *kind;
        }
        let kind = Kind::detect(project, spec);
        self.kinds.insert(tid, (key, kind));
        kind
    }

    /// Unseen errors and warnings across a project's terminals.
    pub fn unseen(&self, project: &Project) -> (u32, u32) {
        project.terminals.iter().fold((0, 0), |(e, w), t| {
            let s = self.states.get(&t.id);
            (e + s.map_or(0, |s| s.unseen_errors), w + s.map_or(0, |s| s.unseen_warnings))
        })
    }

    /// The user is looking at this terminal: clear its badge.
    pub fn mark_seen(&mut self, terminal: Uuid) {
        let unseen = self.states.get(&terminal).is_some_and(|s| s.unseen_errors > 0 || s.unseen_warnings > 0);
        if unseen && self.pending_seen.insert(terminal) {
            self.send(ClientMsg::MarkSeen(terminal));
        }
    }

    // ---------- actions ----------

    pub fn push(&mut self, action: Action) {
        self.actions.push(action);
    }

    fn send(&self, msg: ClientMsg) {
        self.client.send(msg);
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn select_project(&mut self, id: Uuid) {
        if self.ui.active_project != Some(id) {
            self.ui.active_project = Some(id);
            self.mark_dirty();
        }
        self.focus_terminal = true;
    }

    fn select_tab(&mut self, pid: Uuid, tid: Uuid) {
        if self.ui.active_tab.get(&pid) != Some(&tid) {
            self.ui.active_tab.insert(pid, tid);
            self.mark_dirty();
        }
        self.focus_terminal = true;
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::SelectProject(id) => self.select_project(id),
            Action::NewProject => {
                if let Some(root) = rfd::FileDialog::new().set_title("Choose the project folder").pick_folder() {
                    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let draft =
                        Project { name, root, terminals: vec![TerminalSpec::default()], ..Default::default() };
                    self.dialog = Some(Dialog::project(draft, true));
                }
            }
            Action::EditProject(id) => {
                if let Some(p) = self.ws.project(id) {
                    self.dialog = Some(Dialog::project(p.clone(), false));
                }
            }
            Action::OpenProject(id) => {
                self.send(ClientMsg::SetProjectOpen { project: id, open: true });
                self.select_project(id);
            }
            Action::CloseProject(id) => self.send(ClientMsg::SetProjectOpen { project: id, open: false }),
            Action::RemoveProject(id) => {
                if let Some(p) = self.ws.project(id) {
                    self.dialog = Some(Dialog::confirm(
                        format!("Remove {}?", p.name),
                        "Its terminals are stopped and removed from Terminal Manager. Files on disk are not touched."
                            .into(),
                        "Remove project",
                        ConfirmAction::RemoveProject(id),
                    ));
                }
            }
            Action::StartAll(id) => self.send(ClientMsg::StartAll(id)),
            Action::StopAll(id) => self.send(ClientMsg::StopAll(id)),
            Action::SelectTab(pid, tid) => self.select_tab(pid, tid),
            Action::NewTerminal(pid) => {
                if let Some(p) = self.ws.project(pid) {
                    let name = format!("terminal {}", p.terminals.len() + 1);
                    self.dialog = Some(Dialog::terminal(pid, TerminalSpec { name, ..Default::default() }, true));
                }
            }
            Action::EditTerminal(pid, tid) => {
                let spec = self.ws.project(pid).and_then(|p| p.terminal(tid)).or_else(|| self.dev_ws.terminal(tid).map(|(_, t)| t));
                if let Some(spec) = spec {
                    self.dialog = Some(Dialog::terminal(pid, spec.clone(), false));
                }
            }
            Action::DuplicateTerminal(pid, tid) => {
                if let Some(p) = self.ws.project(pid)
                    && let Some(i) = p.terminals.iter().position(|t| t.id == tid)
                {
                    let mut copy = p.terminals[i].clone();
                    copy.id = Uuid::new_v4();
                    copy.name = format!("{} copy", copy.name);
                    let id = copy.id;
                    self.send(ClientMsg::UpsertTerminal { project: pid, spec: copy, index: Some(i + 1) });
                    self.select_tab(pid, id);
                }
            }
            Action::RemoveTerminal(pid, tid) => {
                let spec = self.ws.project(pid).and_then(|p| p.terminal(tid)).or_else(|| self.dev_ws.terminal(tid).map(|(_, t)| t));
                if let Some(spec) = spec {
                    self.dialog = Some(Dialog::confirm(
                        format!("Remove terminal \"{}\"?", spec.name),
                        "The process is stopped and the terminal is removed from this project.".into(),
                        "Remove terminal",
                        ConfirmAction::RemoveTerminal(pid, tid),
                    ));
                }
            }
            Action::Start(tid) => {
                if self.is_dev(tid) {
                    self.focus_dev = true;
                } else {
                    self.focus_terminal = true;
                }
                self.send_for(tid, ClientMsg::Start(tid));
            }
            Action::Stop(tid) => self.send_for(tid, ClientMsg::Stop(tid)),
            Action::NextTab(delta) => {
                if let Some(p) = self.active_project()
                    && !p.terminals.is_empty()
                    && let Some(cur) = self.active_tab(p)
                {
                    let n = p.terminals.len() as i32;
                    let i = p.terminals.iter().position(|t| t.id == cur).unwrap_or(0) as i32;
                    let next = p.terminals[((i + delta).rem_euclid(n)) as usize].id;
                    let pid = p.id;
                    self.select_tab(pid, next);
                }
            }
            Action::DismissBanner => self.banner = None,
            Action::ShowLogs(tid, on) => {
                if on {
                    self.log_mode.insert(tid);
                } else {
                    self.log_mode.remove(&tid);
                    self.focus_terminal = true;
                }
            }
            Action::LoadTermLogs(tid) => {
                self.term_logs = Some(TermLogs::new(tid));
                self.run_term_search();
            }
            Action::TermLogsSearch => self.run_term_search(),
            Action::OpenSearch => self.search_open = true,
            Action::CloseSearch => {
                if self.search.results.busy {
                    self.send(ClientMsg::CancelSearch);
                    self.search.results.busy = false;
                }
                self.search_open = false;
                self.focus_terminal = true;
            }
            Action::GlobalSearch => self.run_global_search(),
            Action::OpenLogLine(tid, entry) => self.open_log_line(tid, entry),
            Action::NewDevTerminal(pid, preset) => {
                let (name, command) = match preset {
                    DevPreset::ClaudeCode => ("Claude Code", "claude"),
                    DevPreset::Shell => ("shell", ""),
                    DevPreset::Custom => ("dev terminal", ""),
                };
                let spec = TerminalSpec {
                    name: name.into(),
                    command: command.into(),
                    auto_start: false,
                    dev: true,
                    log: false,
                    ..Default::default()
                };
                if preset == DevPreset::Custom {
                    self.dialog = Some(Dialog::terminal(pid, spec, true));
                } else {
                    let id = spec.id;
                    self.ensure_dev_project(pid);
                    self.send_dev(ClientMsg::UpsertTerminal { project: pid, spec, index: None });
                    self.send_dev(ClientMsg::Start(id));
                    self.ui.dev_tab.insert(pid, id);
                    self.focus_dev = true;
                    self.mark_dirty();
                }
            }
            Action::SelectDevTab(pid, tid) => {
                self.ui.dev_tab.insert(pid, tid);
                self.focus_dev = true;
                self.mark_dirty();
            }
            Action::OpenInEditor(pid) => {
                if let Some(p) = self.ws.project(pid)
                    && let Err(e) = crate::client::open_in_editor(&p.root)
                {
                    self.banner = Some(e);
                }
            }
            Action::SetDevEnabled(on) => {
                self.ui.dev_enabled = Some(on);
                self.mark_dirty();
                self.sync_dev_client();
            }
            Action::SetDevSide(side) => {
                self.ui.dev_side = side;
                self.mark_dirty();
            }
            Action::ToggleSidebar => {
                self.ui.sidebar_collapsed = !self.ui.sidebar_collapsed;
                self.mark_dirty();
            }
            Action::OpenFind(tid) => match self.find.as_mut().filter(|f| f.terminal == tid) {
                Some(f) => f.focus = true,
                None => self.find = Some(FindBar { terminal: tid, query: String::new(), case_sensitive: false, focus: true }),
            },
        }
    }

    fn take_req(&mut self) -> u64 {
        self.next_req += 1;
        self.next_req
    }

    fn run_term_search(&mut self) {
        let req = self.take_req();
        let Some(tl) = self.term_logs.as_mut() else { return };
        tl.context_of = None;
        let r = &mut tl.results;
        (r.req, r.busy, r.error, r.highlight) = (req, true, None, None);
        r.status = "Searching…".into();
        let query = tl.filters.query(Some(vec![tl.terminal]), 5000, 0);
        self.send(ClientMsg::Search { req, query });
    }

    fn run_global_search(&mut self) {
        let req = self.take_req();
        let terminals = self
            .search
            .project
            .and_then(|pid| self.ws.project(pid))
            .map(|p| p.terminals.iter().map(|t| t.id).collect());
        let query = self.search.filters.query(terminals, 500, 2);
        let r = &mut self.search.results;
        (r.req, r.busy, r.error, r.highlight) = (req, true, None, None);
        r.status = "Searching…".into();
        self.send(ClientMsg::Search { req, query });
    }

    fn open_log_line(&mut self, tid: Uuid, entry: Entry) {
        let Some(pid) = self.ws.terminal(tid).map(|(p, _)| p.id) else {
            self.banner = Some("That terminal has been removed. Its log files are still in the host's logs folder.".into());
            return;
        };
        self.search_open = false;
        if !self.ws.project(pid).is_some_and(|p| p.open) {
            self.send(ClientMsg::SetProjectOpen { project: pid, open: true });
        }
        self.select_project(pid);
        self.select_tab(pid, tid);
        self.log_mode.insert(tid);
        let req = self.take_req();
        let mut tl = match self.term_logs.take() {
            Some(t) if t.terminal == tid => t,
            _ => TermLogs::new(tid),
        };
        let r = &mut tl.results;
        (r.req, r.busy, r.error) = (req, true, None);
        r.highlight = Some((entry.file.clone(), entry.line_no));
        r.scroll_to_highlight = true;
        r.status = "Loading…".into();
        self.send(ClientMsg::ReadLog {
            req,
            terminal: tid,
            around_ms: Some(entry.ts_ms),
            before: 300,
            after: 300,
            levels: vec![],
        });
        tl.context_of = Some(entry);
        self.term_logs = Some(tl);
    }

    /// Applies a finished dialog.
    pub fn dialog_confirmed(&mut self, dialog: Dialog) {
        match dialog {
            Dialog::Project { draft, is_new } => {
                let id = draft.id;
                if self.dev_ws.project(id).is_some() {
                    let meta = Project { terminals: Vec::new(), open: true, ..draft.clone() };
                    self.send_dev(ClientMsg::UpsertProject(meta));
                }
                self.send(ClientMsg::UpsertProject(draft));
                if is_new {
                    self.select_project(id);
                }
            }
            Dialog::Terminal { project, draft, is_new } if draft.dev => {
                let id = draft.id;
                self.ensure_dev_project(project);
                self.send_dev(ClientMsg::UpsertTerminal { project, spec: draft, index: None });
                if is_new {
                    self.send_dev(ClientMsg::Start(id));
                    self.ui.dev_tab.insert(project, id);
                    self.focus_dev = true;
                    self.mark_dirty();
                }
            }
            Dialog::Terminal { project, draft, is_new } => {
                let id = draft.id;
                self.send(ClientMsg::UpsertTerminal { project, spec: draft, index: None });
                if is_new {
                    self.send(ClientMsg::Start(id));
                    self.select_tab(project, id);
                }
            }
            Dialog::Confirm { action, .. } => match action {
                ConfirmAction::RemoveProject(id) => {
                    if self.dev_ws.project(id).is_some() {
                        self.send_dev(ClientMsg::RemoveProject(id));
                    }
                    self.send(ClientMsg::RemoveProject(id));
                }
                ConfirmAction::RemoveTerminal(project, terminal) => {
                    self.send_for(terminal, ClientMsg::RemoveTerminal { project, terminal })
                }
            },
        }
    }

    // ---------- host messages ----------

    fn pump_host(&mut self) {
        while let Some(event) = self.client.try_recv() {
            self.on_event(Conn::Main, event);
        }
        while let Some(event) = self.dev_client.as_ref().and_then(|c| c.try_recv()) {
            self.on_event(Conn::Dev, event);
        }
    }

    fn on_event(&mut self, conn: Conn, event: ClientEvent) {
        let state = match event {
            ClientEvent::Connected(info) => Connection::Connected(info),
            ClientEvent::Disconnected(reason) => Connection::Disconnected(reason),
            ClientEvent::Denied(reason) => Connection::Denied(reason),
            ClientEvent::Msg(msg) => return self.handle_host(conn, msg),
        };
        // Any change of connection means every terminal needs a fresh replay.
        match conn {
            Conn::Main => {
                self.connection = state;
                self.attached.clear();
            }
            Conn::Dev => {
                self.dev_connection = state;
                self.attached_dev.clear();
            }
        }
    }

    fn handle_host(&mut self, conn: Conn, msg: HostMsg) {
        match msg {
            HostMsg::Welcome(info) => match conn {
                Conn::Main => self.connection = Connection::Connected(info),
                Conn::Dev => self.dev_connection = Connection::Connected(info),
            },
            // Each view only shows its own kind of terminal, even when both connections reach
            // the same per-user host.
            HostMsg::Workspace(mut ws) => {
                let keep_dev = conn == Conn::Dev;
                for p in ws.projects.iter_mut() {
                    p.terminals.retain(|t| t.dev == keep_dev);
                }
                match conn {
                    Conn::Main => self.ws = ws,
                    Conn::Dev => self.dev_ws = ws,
                }
                self.sync_terminals();
            }
            HostMsg::State(state) => {
                if state.unseen_errors == 0 && state.unseen_warnings == 0 {
                    self.pending_seen.remove(&state.terminal);
                }
                self.states.insert(state.terminal, state);
            }
            HostMsg::Replay { terminal, bytes } => {
                if let Some(screen) = self.screens.get_mut(&terminal) {
                    screen.reset();
                    screen.feed(&bytes);
                    screen.announce_size();
                }
            }
            HostMsg::Output { terminal, bytes } => {
                if let Some(screen) = self.screens.get_mut(&terminal) {
                    screen.feed(&bytes);
                }
            }
            HostMsg::Error(e) => self.banner = Some(e),
            HostMsg::SearchDone { req, outcome, elapsed_ms } => {
                let n = outcome.hits.len();
                let status = format!(
                    "{n} line{}{} · {} lines scanned in {elapsed_ms} ms",
                    if n == 1 { "" } else { "s" },
                    if outcome.truncated { " (the newest; narrow the search to see older ones)" } else { "" },
                    outcome.lines_scanned
                );
                if let Some(tl) = self.term_logs.as_mut().filter(|t| t.results.req == req) {
                    // Oldest first, like a terminal; start at the bottom.
                    tl.results.rows = outcome.hits.into_iter().rev().map(Row::from_hit).collect();
                    (tl.results.busy, tl.results.status, tl.results.scroll_to_end) = (false, status, true);
                } else if self.search.results.req == req {
                    let r = &mut self.search.results;
                    r.rows = outcome.hits.into_iter().map(Row::from_hit).collect();
                    (r.busy, r.status) = (false, status);
                }
            }
            HostMsg::SearchFailed { req, error } => {
                let views = [self.term_logs.as_mut().map(|t| &mut t.results), Some(&mut self.search.results)];
                for r in views.into_iter().flatten() {
                    if r.req == req {
                        (r.busy, r.error, r.status) = (false, Some(error.clone()), String::new());
                    }
                }
            }
            HostMsg::LogPage { req, terminal, window } => {
                if let Some(tl) = self.term_logs.as_mut().filter(|t| t.results.req == req) {
                    let n = window.entries.len();
                    tl.results.rows = window.entries.into_iter().map(|e| Row::plain(terminal, e)).collect();
                    tl.results.busy = false;
                    tl.results.status = format!(
                        "{n} lines{}{}",
                        if window.more_before { " · earlier lines above" } else { "" },
                        if window.more_after { " · later lines below" } else { "" }
                    );
                }
            }
        }
    }

    /// Attaches to every terminal of both workspaces and drops screens for removed ones.
    fn sync_terminals(&mut self) {
        let ids = |ws: &Workspace| -> HashSet<Uuid> { ws.projects.iter().flat_map(|p| p.terminals.iter().map(|t| t.id)).collect() };
        let (main_ids, dev_ids) = (ids(&self.ws), ids(&self.dev_ws));
        self.screens.retain(|id, _| main_ids.contains(id) || dev_ids.contains(id));
        self.states.retain(|id, _| main_ids.contains(id) || dev_ids.contains(id));
        self.attached.retain(|id| main_ids.contains(id));
        self.attached_dev.retain(|id| dev_ids.contains(id));
        for id in main_ids {
            if self.attached.insert(id) {
                let sender = self.client.sender();
                self.screens.entry(id).or_insert_with(|| Screen::new(id, sender));
                self.send(ClientMsg::Attach(id));
            }
        }
        if let Some(dev) = &self.dev_client {
            for id in dev_ids {
                if self.attached_dev.insert(id) {
                    self.screens.entry(id).or_insert_with(|| Screen::new(id, dev.sender()));
                    dev.send(ClientMsg::Attach(id));
                }
            }
        }
        if self.active_project().is_none() {
            let first = self.ws.projects.iter().find(|p| p.open).map(|p| p.id);
            if self.ui.active_project != first {
                self.ui.active_project = first;
                self.mark_dirty();
            }
        }
    }

    // ---------- per-frame housekeeping ----------

    fn track_window(&mut self, ctx: &egui::Context) {
        let (inner, outer, maximized) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.outer_rect, v.maximized.unwrap_or(false))
        });
        let Some(inner) = inner else { return };
        let mut state = self.ui.window.unwrap_or(WindowState {
            width: inner.width(),
            height: inner.height(),
            x: None,
            y: None,
            maximized,
        });
        state.maximized = maximized;
        if !maximized {
            state.width = inner.width().round();
            state.height = inner.height().round();
            if let Some(outer) = outer {
                state.x = Some(outer.min.x.round());
                state.y = Some(outer.min.y.round());
            }
        }
        if self.ui.window != Some(state) {
            self.ui.window = Some(state);
            self.mark_dirty();
        }
    }

    fn autosave(&mut self, ctx: &egui::Context) {
        if !self.dirty {
            return;
        }
        let since = self.last_save.elapsed();
        if since < SAVE_DEBOUNCE {
            ctx.request_repaint_after(SAVE_DEBOUNCE - since);
            return;
        }
        self.save_now();
    }

    fn save_now(&mut self) {
        self.dirty = false;
        self.last_save = Instant::now();
        if let Err(e) = persist::save(&self.ui_path, &self.ui) {
            self.banner = Some(format!("Could not save {}: {e}", self.ui_path.display()));
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if self.dialog.is_some() {
            return;
        }
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        let hit = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let pid = self.active_project().map(|p| p.id);
        let tid = self.active_project().and_then(|p| self.active_tab(p));
        if hit(cs, Key::Tab) {
            self.push(Action::NextTab(-1));
        }
        if hit(Modifiers::CTRL, Key::Tab) {
            self.push(Action::NextTab(1));
        }
        if hit(cs, Key::N) {
            self.push(Action::NewProject);
        }
        if hit(cs, Key::F) {
            self.push(Action::OpenSearch);
        }
        if hit(Modifiers::CTRL, Key::B) {
            self.push(Action::ToggleSidebar);
        }
        if let Some(pid) = pid {
            if hit(cs, Key::T) {
                self.push(Action::NewTerminal(pid));
            }
            if let Some(tid) = tid {
                if !self.search_open && !self.log_mode.contains(&tid) && hit(Modifiers::CTRL, Key::F) {
                    self.push(Action::OpenFind(tid));
                }
                if hit(cs, Key::W) {
                    self.push(Action::RemoveTerminal(pid, tid));
                }
                if hit(cs, Key::R) {
                    self.push(Action::Start(tid));
                }
            }
        }
    }
}

/// Short uptime text for a run that started at `started_at_ms` (Unix ms).
pub fn uptime(started_at_ms: Option<i64>) -> Option<String> {
    let secs = ((tm_core::time::now_ms() - started_at_ms?).max(0) / 1000) as u64;
    Some(match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, (secs / 60) % 60),
    })
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_dev_client();
        self.pump_host();
        self.track_window(ctx);
        if self.states.values().any(|s| s.status.is_running()) {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        self.autosave(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);
        ui::sidebar::show(self, ui);
        ui::main_area::show(self, ui);
        ui::dialogs::show(self, &ctx);
        self.focus_terminal = false;
        self.focus_dev = false;
        for action in std::mem::take(&mut self.actions) {
            self.apply(action);
        }
    }

    fn on_exit(&mut self) {
        // Terminals keep running in the host; only the view state is saved here.
        self.save_now();
    }
}
