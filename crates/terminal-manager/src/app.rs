//! App state and the actions the UI can trigger. The host owns projects and processes; the app
//! mirrors its workspace, renders terminal screens, and keeps per-user UI state.

use crate::client::{ClientEvent, HostClient};
use crate::term::Screen;
use crate::ui;
use crate::ui::dialogs::{ConfirmAction, Dialog};
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

/// Per-user view state, saved in `ui.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct UiState {
    pub active_project: Option<Uuid>,
    pub active_tab: HashMap<Uuid, Uuid>,
    pub window: Option<WindowState>,
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
    /// Terminals we asked the host to reset unseen counts for, until it confirms.
    pending_seen: HashSet<Uuid>,
    ui_path: PathBuf,
    dirty: bool,
    last_save: Instant,
    pub banner: Option<String>,
    pub dialog: Option<Dialog>,
    pub focus_terminal: bool,
    actions: Vec<Action>,
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
            client: HostClient::start(cc.egui_ctx.clone()),
            attached: HashSet::new(),
            pending_seen: HashSet::new(),
            ui_path,
            dirty: false,
            last_save: Instant::now(),
            banner: ui_state.warning,
            dialog: None,
            focus_terminal: true,
            actions: Vec::new(),
        }
    }

    // ---------- queries ----------

    pub fn is_connected(&self) -> bool {
        matches!(self.connection, Connection::Connected(_))
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
                if let Some(spec) = self.ws.project(pid).and_then(|p| p.terminal(tid)) {
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
                if let Some(spec) = self.ws.project(pid).and_then(|p| p.terminal(tid)) {
                    self.dialog = Some(Dialog::confirm(
                        format!("Remove terminal \"{}\"?", spec.name),
                        "The process is stopped and the terminal is removed from this project.".into(),
                        "Remove terminal",
                        ConfirmAction::RemoveTerminal(pid, tid),
                    ));
                }
            }
            Action::Start(tid) => {
                self.send(ClientMsg::Start(tid));
                self.focus_terminal = true;
            }
            Action::Stop(tid) => self.send(ClientMsg::Stop(tid)),
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
        }
    }

    /// Applies a finished dialog.
    pub fn dialog_confirmed(&mut self, dialog: Dialog) {
        match dialog {
            Dialog::Project { draft, is_new } => {
                let id = draft.id;
                self.send(ClientMsg::UpsertProject(draft));
                if is_new {
                    self.select_project(id);
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
                ConfirmAction::RemoveProject(id) => self.send(ClientMsg::RemoveProject(id)),
                ConfirmAction::RemoveTerminal(project, terminal) => {
                    self.send(ClientMsg::RemoveTerminal { project, terminal })
                }
            },
        }
    }

    // ---------- host messages ----------

    fn pump_host(&mut self) {
        while let Some(event) = self.client.try_recv() {
            match event {
                ClientEvent::Connected(info) => {
                    self.connection = Connection::Connected(info);
                    // Fresh connection: every terminal needs a new replay.
                    self.attached.clear();
                }
                ClientEvent::Disconnected(reason) => {
                    self.connection = Connection::Disconnected(reason);
                    self.attached.clear();
                }
                ClientEvent::Denied(reason) => {
                    self.connection = Connection::Denied(reason);
                    self.attached.clear();
                }
                ClientEvent::Msg(msg) => self.handle_host(msg),
            }
        }
    }

    fn handle_host(&mut self, msg: HostMsg) {
        match msg {
            HostMsg::Welcome(info) => self.connection = Connection::Connected(info),
            HostMsg::Workspace(ws) => {
                self.ws = ws;
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
        }
    }

    /// Attaches to every terminal in the workspace and drops screens for removed ones.
    fn sync_terminals(&mut self) {
        let ids: HashSet<Uuid> = self.ws.projects.iter().flat_map(|p| p.terminals.iter().map(|t| t.id)).collect();
        self.screens.retain(|id, _| ids.contains(id));
        self.states.retain(|id, _| ids.contains(id));
        self.attached.retain(|id| ids.contains(id));
        for id in ids {
            if self.attached.insert(id) {
                let sender = self.client.sender();
                self.screens.entry(id).or_insert_with(|| Screen::new(id, sender));
                self.send(ClientMsg::Attach(id));
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
        if let Some(pid) = pid {
            if hit(cs, Key::T) {
                self.push(Action::NewTerminal(pid));
            }
            if let Some(tid) = tid {
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
        for action in std::mem::take(&mut self.actions) {
            self.apply(action);
        }
    }

    fn on_exit(&mut self) {
        // Terminals keep running in the host; only the view state is saved here.
        self.save_now();
    }
}
