//! Host state: the saved workspace, the live terminals, and connected clients.

use crate::terminal::{HostTerminal, RunEvent};
use crate::termlog::TermLog;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tm_core::logs::{self, Classifier, IndexEntry, LogIndex, LogSettings};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, Weak};
use tm_core::model::Workspace;
use tm_core::persist;
use tm_core::protocol::{ClientMsg, HostInfo, HostMsg};
use uuid::Uuid;

pub struct Client {
    pub id: u64,
    pub tx: SyncSender<HostMsg>,
    /// The search this client is waiting for; older searches stop when it changes.
    pub search: AtomicU64,
}

#[derive(Default)]
struct State {
    ws: Workspace,
    terms: HashMap<Uuid, HostTerminal>,
    clients: HashMap<u64, Arc<Client>>,
}

pub struct Host {
    pub info: HostInfo,
    store: PathBuf,
    logs_root: PathBuf,
    log_settings: LogSettings,
    classifier: Arc<Classifier>,
    state: Mutex<State>,
    /// Terminals each client fell behind on; its writer thread re-attaches them. Kept outside
    /// `state` because it is written from PTY threads that hold a terminal lock.
    lagging: Mutex<HashMap<u64, HashSet<Uuid>>>,
    next_client: AtomicU64,
    me: Weak<Host>,
}

impl Host {
    pub fn new(info: HostInfo, data_dir: PathBuf, ws: Workspace, log_settings: LogSettings) -> Arc<Self> {
        let (error_pattern, warning_pattern) = log_settings.patterns();
        let (classifier, problems) = Classifier::new(error_pattern, warning_pattern);
        for p in problems {
            log::warn!("settings.json: {p}");
        }
        let host = Arc::new_cyclic(|me| Host {
            info,
            store: data_dir.join("workspace.json"),
            logs_root: logs::logs_root(&data_dir),
            log_settings,
            classifier: Arc::new(classifier),
            state: Mutex::new(State { ws, ..Default::default() }),
            lagging: Mutex::default(),
            next_client: AtomicU64::new(1),
            me: me.clone(),
        });
        {
            let mut st = host.state.lock().unwrap();
            let ids: Vec<Uuid> = st.ws.projects.iter().flat_map(|p| p.terminals.iter().map(|t| t.id)).collect();
            for id in ids {
                st.terms.insert(id, host.new_terminal(id));
            }
            host.write_log_index(&st);
            let open: Vec<Uuid> = st.ws.projects.iter().filter(|p| p.open).map(|p| p.id).collect();
            for pid in open {
                host.auto_start(&mut st, pid);
            }
        }
        let me = Arc::downgrade(&host);
        let _ = std::thread::Builder::new().name("counts".into()).spawn(move || {
            // Error/warning counts change on PTY threads; publish them from here so those
            // threads never take the state lock.
            while let Some(host) = me.upgrade() {
                host.publish_counts();
                drop(host);
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        host
    }

    fn new_terminal(&self, id: Uuid) -> HostTerminal {
        let dir = logs::terminal_dir(&self.logs_root, id);
        HostTerminal::new(id, TermLog::new(dir, &self.log_settings, self.classifier.clone()))
    }

    fn publish_counts(&self) {
        let mut st = self.state.lock().unwrap();
        let mut changed = Vec::new();
        for (id, t) in st.terms.iter_mut() {
            let counts = t.log.lock().unwrap().counts.clone();
            if counts.changed.swap(false, Ordering::AcqRel) {
                t.state.unseen_errors = counts.errors.load(Ordering::Relaxed);
                t.state.unseen_warnings = counts.warnings.load(Ordering::Relaxed);
                changed.push(*id);
            }
        }
        for id in changed {
            Self::broadcast_state(&st, id);
        }
    }

    /// Keeps `logs/index.json` naming every terminal folder, removed terminals included.
    fn write_log_index(&self, st: &State) {
        let path = self.logs_root.join("index.json");
        let mut index = persist::load::<LogIndex>(&path).value;
        let before = index.clone();
        for p in &st.ws.projects {
            for t in &p.terminals {
                index.terminals.insert(
                    t.id,
                    IndexEntry { project: p.name.clone(), terminal: t.name.clone(), project_id: Some(p.id) },
                );
            }
        }
        if index != before
            && let Err(e) = persist::save(&path, &index)
        {
            log::warn!("writing {}: {e}", path.display());
        }
    }

    // ---------- clients ----------

    pub fn add_client(&self, tx: SyncSender<HostMsg>) -> Arc<Client> {
        let id = self.next_client.fetch_add(1, Ordering::Relaxed);
        let client = Arc::new(Client { id, tx, search: AtomicU64::new(0) });
        let mut st = self.state.lock().unwrap();
        let _ = client.tx.try_send(HostMsg::Welcome(self.info.clone()));
        let _ = client.tx.try_send(HostMsg::Workspace(st.ws.clone()));
        for t in st.terms.values() {
            let _ = client.tx.try_send(HostMsg::State(t.state.clone()));
        }
        st.clients.insert(id, client.clone());
        log::info!("client {id} connected");
        client
    }

    pub fn remove_client(&self, id: u64) {
        let mut st = self.state.lock().unwrap();
        st.clients.remove(&id);
        self.lagging.lock().unwrap().remove(&id);
        for t in st.terms.values() {
            t.hub.lock().unwrap().detach(id);
        }
        log::info!("client {id} disconnected");
    }

    /// Re-attaches terminals a client fell behind on, giving it a fresh replay.
    pub fn resync(&self, client: &Client) {
        let pending: Vec<Uuid> =
            self.lagging.lock().unwrap().get_mut(&client.id).map(|s| s.drain().collect()).unwrap_or_default();
        if pending.is_empty() {
            return;
        }
        let st = self.state.lock().unwrap();
        for tid in pending {
            if let Some(t) = st.terms.get(&tid) {
                t.hub.lock().unwrap().attach(tid, client.id, client.tx.clone());
            }
        }
    }

    fn mark_lagging(&self, terminal: Uuid, clients: Vec<u64>) {
        let mut lagging = self.lagging.lock().unwrap();
        for id in clients {
            lagging.entry(id).or_default().insert(terminal);
        }
    }

    fn broadcast(st: &State, msg: HostMsg) {
        for c in st.clients.values() {
            let _ = c.tx.try_send(msg.clone());
        }
    }

    fn broadcast_state(st: &State, tid: Uuid) {
        if let Some(t) = st.terms.get(&tid) {
            Self::broadcast(st, HostMsg::State(t.state.clone()));
        }
    }

    fn workspace_changed(&self, st: &State) {
        self.write_log_index(st);
        if let Err(e) = persist::save(&self.store, &st.ws) {
            log::error!("saving {}: {e}", self.store.display());
            Self::broadcast(st, HostMsg::Error(format!("The host could not save its workspace: {e}")));
        }
        Self::broadcast(st, HostMsg::Workspace(st.ws.clone()));
    }

    // ---------- terminals ----------

    fn start(&self, st: &mut State, tid: Uuid) {
        let Some((project, spec)) = st.ws.terminal(tid) else { return };
        let (project, spec) = (project.clone(), spec.clone());
        let me = self.me.clone();
        let me2 = self.me.clone();
        if !st.terms.contains_key(&tid) {
            let t = self.new_terminal(tid);
            st.terms.insert(tid, t);
        }
        let term = st.terms.get_mut(&tid).expect("inserted above");
        term.start(
            &project,
            &spec,
            move |generation, event| {
                if let Some(host) = me.upgrade() {
                    host.on_run_event(tid, generation, event);
                }
            },
            move |clients| {
                if let Some(host) = me2.upgrade() {
                    host.mark_lagging(tid, clients);
                }
            },
        );
        log::info!("started {} ({}) -> {:?}", spec.name, tid, term.state.status);
        Self::broadcast_state(st, tid);
    }

    fn stop(st: &mut State, tid: Uuid) {
        if let Some(t) = st.terms.get_mut(&tid) {
            t.stop();
        }
        Self::broadcast_state(st, tid);
    }

    fn auto_start(&self, st: &mut State, pid: Uuid) {
        let ids: Vec<Uuid> = st
            .ws
            .project(pid)
            .map(|p| p.terminals.iter().filter(|t| t.auto_start && !t.dev).map(|t| t.id).collect())
            .unwrap_or_default();
        for tid in ids {
            if !st.terms.get(&tid).is_some_and(|t| t.state.status.is_running()) {
                self.start(st, tid);
            }
        }
    }

    fn project_terminals(st: &State, pid: Uuid) -> Vec<Uuid> {
        st.ws.project(pid).map(|p| p.terminals.iter().map(|t| t.id).collect()).unwrap_or_default()
    }

    /// The service terminals of a project (Start all / Stop all leave dev terminals alone).
    fn service_terminals(st: &State, pid: Uuid) -> Vec<Uuid> {
        st.ws.project(pid).map(|p| p.terminals.iter().filter(|t| !t.dev).map(|t| t.id).collect()).unwrap_or_default()
    }

    /// Dev terminals run as the signed-in user; a service (another account, no desktop) refuses them.
    fn refuses_dev(&self, client: &Client, dev: bool) -> bool {
        let refuse = dev && self.info.mode == tm_core::paths::HostMode::Service;
        if refuse {
            let _ = client.tx.try_send(HostMsg::Error(
                "Dev terminals run as you in your own session, not in the Windows service.".into(),
            ));
        }
        refuse
    }

    fn on_run_event(&self, tid: Uuid, generation: u64, event: RunEvent) {
        let mut st = self.state.lock().unwrap();
        if let RunEvent::Exited(code) = &event {
            log::info!("terminal {tid} exited with {code:?}");
        }
        if st.terms.get_mut(&tid).is_some_and(|t| t.handle(generation, event)) {
            Self::broadcast_state(&st, tid);
        }
    }

    /// Stops every terminal (service shutdown).
    pub fn shutdown(&self) {
        let mut st = self.state.lock().unwrap();
        for t in st.terms.values_mut() {
            t.stop();
        }
        let _ = persist::save(&self.store, &st.ws);
    }

    // ---------- requests ----------

    /// Runs log searches and reads off the request thread, outside the state lock.
    pub fn handle_logs(&self, client: &Arc<Client>, msg: ClientMsg) -> Option<ClientMsg> {
        match msg {
            ClientMsg::Search { req, query } => {
                client.search.store(req, Ordering::SeqCst);
                let root = self.logs_root.clone();
                let client = client.clone();
                let _ = std::thread::Builder::new().name("search".into()).spawn(move || {
                    let started = std::time::Instant::now();
                    let cancelled = || client.search.load(Ordering::SeqCst) != req;
                    let reply = match logs::search::search(&root, &query, &cancelled) {
                        Ok(outcome) => {
                            HostMsg::SearchDone { req, outcome, elapsed_ms: started.elapsed().as_millis() as u64 }
                        }
                        Err(_) if cancelled() => return,
                        Err(error) => HostMsg::SearchFailed { req, error },
                    };
                    // Results can be large; wait for room rather than drop them.
                    let _ = client.tx.send(reply);
                });
                None
            }
            ClientMsg::CancelSearch => {
                client.search.store(0, Ordering::SeqCst);
                None
            }
            ClientMsg::ReadLog { req, terminal, around_ms, before, after, levels } => {
                let dir = logs::terminal_dir(&self.logs_root, terminal);
                let client = client.clone();
                let _ = std::thread::Builder::new().name("read-log".into()).spawn(move || {
                    let window =
                        logs::search::read_window(&dir, around_ms, before.min(5000) as usize, after.min(5000) as usize, &levels);
                    let _ = client.tx.send(HostMsg::LogPage { req, terminal, window });
                });
                None
            }
            other => Some(other),
        }
    }

    pub fn handle(&self, client: &Client, msg: ClientMsg) {
        let mut st = self.state.lock().unwrap();
        let st = &mut *st;
        match msg {
            ClientMsg::Hello { .. } => {}
            ClientMsg::UpsertProject(project) => {
                if self.refuses_dev(client, project.terminals.iter().any(|t| t.dev)) {
                    return;
                }
                let pid = project.id;
                match st.ws.project_mut(pid) {
                    Some(existing) => {
                        existing.name = project.name;
                        existing.root = project.root;
                        existing.default_shell = project.default_shell;
                        existing.env = project.env;
                    }
                    None => {
                        let open = project.open;
                        for t in &project.terminals {
                            let term = self.new_terminal(t.id);
                            st.terms.insert(t.id, term);
                            Self::broadcast_state(st, t.id);
                        }
                        st.ws.projects.push(project);
                        if open {
                            self.auto_start(st, pid);
                        }
                    }
                }
                self.workspace_changed(st);
            }
            ClientMsg::RemoveProject(pid) => {
                for tid in Self::project_terminals(st, pid) {
                    st.terms.remove(&tid);
                }
                st.ws.projects.retain(|p| p.id != pid);
                self.workspace_changed(st);
            }
            ClientMsg::SetProjectOpen { project, open } => {
                if let Some(p) = st.ws.project_mut(project) {
                    p.open = open;
                }
                if open {
                    self.auto_start(st, project);
                } else {
                    for tid in Self::project_terminals(st, project) {
                        Self::stop(st, tid);
                    }
                }
                self.workspace_changed(st);
            }
            ClientMsg::UpsertTerminal { project, spec, index } => {
                if self.refuses_dev(client, spec.dev) {
                    return;
                }
                let Some(p) = st.ws.project_mut(project) else {
                    let _ = client.tx.try_send(HostMsg::Error("That project no longer exists.".into()));
                    return;
                };
                let tid = spec.id;
                match p.terminals.iter_mut().find(|t| t.id == tid) {
                    Some(slot) => *slot = spec,
                    None => {
                        let i = index.unwrap_or(p.terminals.len()).min(p.terminals.len());
                        p.terminals.insert(i, spec);
                    }
                }
                if !st.terms.contains_key(&tid) {
                    let term = self.new_terminal(tid);
                    st.terms.insert(tid, term);
                    Self::broadcast_state(st, tid);
                }
                self.workspace_changed(st);
            }
            ClientMsg::RemoveTerminal { project, terminal } => {
                st.terms.remove(&terminal);
                if let Some(p) = st.ws.project_mut(project) {
                    p.terminals.retain(|t| t.id != terminal);
                }
                self.workspace_changed(st);
            }
            ClientMsg::Start(tid) => self.start(st, tid),
            ClientMsg::Stop(tid) => Self::stop(st, tid),
            ClientMsg::StartAll(pid) => {
                for tid in Self::service_terminals(st, pid) {
                    if !st.terms.get(&tid).is_some_and(|t| t.state.status.is_running()) {
                        self.start(st, tid);
                    }
                }
            }
            ClientMsg::StopAll(pid) => {
                for tid in Self::service_terminals(st, pid) {
                    Self::stop(st, tid);
                }
            }
            ClientMsg::Attach(tid) => {
                if let Some(t) = st.terms.get(&tid) {
                    t.hub.lock().unwrap().attach(tid, client.id, client.tx.clone());
                }
            }
            ClientMsg::Detach(tid) => {
                if let Some(t) = st.terms.get(&tid) {
                    t.hub.lock().unwrap().detach(client.id);
                }
            }
            ClientMsg::Input { terminal, bytes } => {
                if let Some(t) = st.terms.get(&terminal) {
                    t.input(bytes);
                }
            }
            ClientMsg::Resize { terminal, cols, lines } => {
                if let Some(t) = st.terms.get_mut(&terminal) {
                    t.resize(cols, lines);
                }
            }
            ClientMsg::Search { .. } | ClientMsg::CancelSearch | ClientMsg::ReadLog { .. } => {}
            ClientMsg::MarkSeen(terminal) => {
                if let Some(t) = st.terms.get(&terminal) {
                    t.log.lock().unwrap().counts.reset();
                }
            }
        }
    }
}
