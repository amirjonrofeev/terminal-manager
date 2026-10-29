//! App state and the actions the UI can trigger.

use crate::model::{Project, TerminalSpec, WindowState, Workspace};
use crate::persist;
use crate::session::{EventKey, Session, Status};
use crate::term::TermEvent;
use crate::ui;
use crate::ui::dialogs::{ConfirmAction, Dialog};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use uuid::Uuid;

const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);

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
    Start(Uuid, Uuid),
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
    pub sessions: HashMap<Uuid, Session>,
    events_tx: Sender<(EventKey, TermEvent)>,
    events_rx: Receiver<(EventKey, TermEvent)>,
    ctx: egui::Context,
    store_path: PathBuf,
    dirty: bool,
    last_save: Instant,
    pub save_error: Option<String>,
    pub banner: Option<String>,
    pub dialog: Option<Dialog>,
    pub focus_terminal: bool,
    allow_quit: bool,
    actions: Vec<Action>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, store_path: PathBuf, loaded: persist::Loaded) -> Self {
        ui::setup(&cc.egui_ctx);
        let (events_tx, events_rx) = mpsc::channel();
        let mut app = Self {
            ws: loaded.workspace,
            sessions: HashMap::new(),
            events_tx,
            events_rx,
            ctx: cc.egui_ctx.clone(),
            store_path,
            dirty: false,
            last_save: Instant::now(),
            save_error: None,
            banner: loaded.warning,
            dialog: None,
            focus_terminal: true,
            allow_quit: false,
            actions: Vec::new(),
        };
        if app.ws.active_project.and_then(|id| app.ws.project(id)).is_none_or(|p| !p.open) {
            app.ws.active_project = app.ws.projects.iter().find(|p| p.open).map(|p| p.id);
        }
        let open: Vec<Uuid> = app.ws.projects.iter().filter(|p| p.open).map(|p| p.id).collect();
        for id in open {
            app.auto_start(id);
        }
        app
    }

    // ---------- queries ----------

    pub fn active_project(&self) -> Option<&Project> {
        self.ws.active_project.and_then(|id| self.ws.project(id))
    }

    /// The selected tab of a project, falling back to its first terminal.
    pub fn active_tab(project: &Project) -> Option<Uuid> {
        project
            .active_tab
            .filter(|id| project.terminal(*id).is_some())
            .or_else(|| project.terminals.first().map(|t| t.id))
    }

    pub fn status(&self, terminal: Uuid) -> Status {
        self.sessions.get(&terminal).map(|s| s.status.clone()).unwrap_or(Status::NotStarted)
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

    pub fn total_running(&self) -> usize {
        self.sessions.values().filter(|s| s.status.is_running()).count()
    }

    // ---------- actions ----------

    pub fn push(&mut self, action: Action) {
        self.actions.push(action);
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn start(&mut self, project_id: Uuid, terminal_id: Uuid) {
        let Some(project) = self.ws.project(project_id) else { return };
        let Some(spec) = project.terminal(terminal_id) else { return };
        self.sessions.entry(terminal_id).or_default().start(project, spec, &self.events_tx, &self.ctx);
    }

    fn stop(&mut self, terminal_id: Uuid) {
        if let Some(s) = self.sessions.get_mut(&terminal_id) {
            s.stop();
        }
    }

    fn auto_start(&mut self, project_id: Uuid) {
        let Some(project) = self.ws.project(project_id) else { return };
        let ids: Vec<Uuid> = project
            .terminals
            .iter()
            .filter(|t| t.auto_start && !self.status(t.id).is_running())
            .map(|t| t.id)
            .collect();
        for id in ids {
            self.start(project_id, id);
        }
    }

    fn terminal_ids(&self, project_id: Uuid) -> Vec<Uuid> {
        self.ws.project(project_id).map(|p| p.terminals.iter().map(|t| t.id).collect()).unwrap_or_default()
    }

    fn select_project(&mut self, id: Uuid) {
        if self.ws.active_project != Some(id) {
            self.ws.active_project = Some(id);
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
                if let Some(p) = self.ws.project_mut(id) {
                    p.open = true;
                }
                self.auto_start(id);
                self.select_project(id);
                self.mark_dirty();
            }
            Action::CloseProject(id) => {
                for t in self.terminal_ids(id) {
                    self.stop(t);
                }
                if let Some(p) = self.ws.project_mut(id) {
                    p.open = false;
                }
                if self.ws.active_project == Some(id) {
                    self.ws.active_project = self.ws.projects.iter().find(|p| p.open).map(|p| p.id);
                }
                self.mark_dirty();
            }
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
            Action::StartAll(id) => {
                for t in self.terminal_ids(id) {
                    if !self.status(t).is_running() {
                        self.start(id, t);
                    }
                }
            }
            Action::StopAll(id) => {
                for t in self.terminal_ids(id) {
                    self.stop(t);
                }
            }
            Action::SelectTab(pid, tid) => {
                if let Some(p) = self.ws.project_mut(pid) {
                    p.active_tab = Some(tid);
                }
                self.focus_terminal = true;
                self.mark_dirty();
            }
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
                if let Some(p) = self.ws.project_mut(pid)
                    && let Some(i) = p.terminals.iter().position(|t| t.id == tid)
                {
                    let mut copy = p.terminals[i].clone();
                    copy.id = Uuid::new_v4();
                    copy.name = format!("{} copy", copy.name);
                    p.active_tab = Some(copy.id);
                    p.terminals.insert(i + 1, copy);
                    self.mark_dirty();
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
            Action::Start(pid, tid) => {
                self.start(pid, tid);
                self.focus_terminal = true;
            }
            Action::Stop(tid) => self.stop(tid),
            Action::NextTab(delta) => {
                if let Some(p) = self.active_project()
                    && !p.terminals.is_empty()
                    && let Some(cur) = Self::active_tab(p)
                {
                    let n = p.terminals.len() as i32;
                    let i = p.terminals.iter().position(|t| t.id == cur).unwrap_or(0) as i32;
                    let next = p.terminals[((i + delta).rem_euclid(n)) as usize].id;
                    let pid = p.id;
                    self.apply(Action::SelectTab(pid, next));
                }
            }
            Action::DismissBanner => self.banner = None,
        }
    }

    /// Applies a finished dialog.
    pub fn dialog_confirmed(&mut self, dialog: Dialog) {
        match dialog {
            Dialog::Project { draft, is_new } => {
                if is_new {
                    let id = draft.id;
                    self.ws.projects.push(draft);
                    self.auto_start(id);
                    self.select_project(id);
                } else if let Some(p) = self.ws.project_mut(draft.id) {
                    p.name = draft.name;
                    p.root = draft.root;
                    p.default_shell = draft.default_shell;
                    p.env = draft.env;
                }
            }
            Dialog::Terminal { project, draft, is_new } => {
                let Some(p) = self.ws.project_mut(project) else { return };
                let id = draft.id;
                if is_new {
                    p.terminals.push(draft);
                    p.active_tab = Some(id);
                    self.start(project, id);
                    self.focus_terminal = true;
                } else if let Some(slot) = p.terminals.iter_mut().find(|t| t.id == id) {
                    *slot = draft;
                }
            }
            Dialog::Confirm { action, .. } => match action {
                ConfirmAction::Quit => {
                    self.allow_quit = true;
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ConfirmAction::RemoveProject(id) => {
                    for t in self.terminal_ids(id) {
                        self.sessions.remove(&t);
                    }
                    self.ws.projects.retain(|p| p.id != id);
                    if self.ws.active_project == Some(id) {
                        self.ws.active_project = self.ws.projects.iter().find(|p| p.open).map(|p| p.id);
                    }
                }
                ConfirmAction::RemoveTerminal(pid, tid) => {
                    self.sessions.remove(&tid);
                    if let Some(p) = self.ws.project_mut(pid) {
                        p.terminals.retain(|t| t.id != tid);
                        if p.active_tab == Some(tid) {
                            p.active_tab = p.terminals.first().map(|t| t.id);
                        }
                    }
                }
            },
        }
        self.mark_dirty();
    }

    // ---------- per-frame housekeeping ----------

    fn pump_events(&mut self) {
        while let Ok(((tid, generation), event)) = self.events_rx.try_recv() {
            if let Some(s) = self.sessions.get_mut(&tid) {
                s.handle(generation, event);
            }
        }
    }

    fn track_window(&mut self, ctx: &egui::Context) {
        let (inner, outer, maximized) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.outer_rect, v.maximized.unwrap_or(false))
        });
        let Some(inner) = inner else { return };
        let mut state = self.ws.window.unwrap_or(WindowState {
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
        if self.ws.window != Some(state) {
            self.ws.window = Some(state);
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
        self.save_error = persist::save(&self.store_path, &self.ws).err().map(|e| e.to_string());
    }

    pub fn store_path(&self) -> &PathBuf {
        &self.store_path
    }

    pub fn last_save(&self) -> Instant {
        self.last_save
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if self.dialog.is_some() {
            return;
        }
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        let hit = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let pid = self.ws.active_project;
        let tid = self.active_project().and_then(Self::active_tab);
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
                    self.push(Action::Start(pid, tid));
                }
            }
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_events();
        self.track_window(ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_quit {
            let running = self.total_running();
            if running > 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if self.dialog.is_none() {
                    self.dialog = Some(Dialog::confirm(
                        "Quit Terminal Manager?".into(),
                        format!(
                            "{running} terminal{} still running and will be stopped. Your workspace is saved, and auto-start terminals run again next time you open the app.",
                            if running == 1 { " is" } else { "s are" }
                        ),
                        "Stop and quit",
                        ConfirmAction::Quit,
                    ));
                }
            }
        }

        if self.total_running() > 0 {
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
        self.save_now();
        for s in self.sessions.values_mut() {
            s.stop();
        }
    }
}
