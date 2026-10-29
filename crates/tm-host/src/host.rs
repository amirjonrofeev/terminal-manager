//! Host state: the saved workspace, the live terminals, and connected clients.

use crate::terminal::{HostTerminal, RunEvent};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, Weak};
use tm_core::model::Workspace;
use tm_core::persist;
use tm_core::protocol::{ClientMsg, HostInfo, HostMsg};
use uuid::Uuid;

pub struct Client {
    pub id: u64,
    pub tx: SyncSender<HostMsg>,
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
    state: Mutex<State>,
    /// Terminals each client fell behind on; its writer thread re-attaches them. Kept outside
    /// `state` because it is written from PTY threads that hold a terminal lock.
    lagging: Mutex<HashMap<u64, HashSet<Uuid>>>,
    next_client: AtomicU64,
    me: Weak<Host>,
}

impl Host {
    pub fn new(info: HostInfo, data_dir: PathBuf, ws: Workspace) -> Arc<Self> {
        let host = Arc::new_cyclic(|me| Host {
            info,
            store: data_dir.join("workspace.json"),
            state: Mutex::new(State { ws, ..Default::default() }),
            lagging: Mutex::default(),
            next_client: AtomicU64::new(1),
            me: me.clone(),
        });
        {
            let mut st = host.state.lock().unwrap();
            let ids: Vec<Uuid> = st.ws.projects.iter().flat_map(|p| p.terminals.iter().map(|t| t.id)).collect();
            for id in ids {
                st.terms.insert(id, HostTerminal::new(id));
            }
            let open: Vec<Uuid> = st.ws.projects.iter().filter(|p| p.open).map(|p| p.id).collect();
            for pid in open {
                host.auto_start(&mut st, pid);
            }
        }
        host
    }

    // ---------- clients ----------

    pub fn add_client(&self, tx: SyncSender<HostMsg>) -> Arc<Client> {
        let id = self.next_client.fetch_add(1, Ordering::Relaxed);
        let client = Arc::new(Client { id, tx });
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
        let term = st.terms.entry(tid).or_insert_with(|| HostTerminal::new(tid));
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
            .map(|p| p.terminals.iter().filter(|t| t.auto_start).map(|t| t.id).collect())
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

    pub fn handle(&self, client: &Client, msg: ClientMsg) {
        let mut st = self.state.lock().unwrap();
        let st = &mut *st;
        match msg {
            ClientMsg::Hello { .. } => {}
            ClientMsg::UpsertProject(project) => {
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
                            st.terms.insert(t.id, HostTerminal::new(t.id));
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
                    st.terms.insert(tid, HostTerminal::new(tid));
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
                for tid in Self::project_terminals(st, pid) {
                    if !st.terms.get(&tid).is_some_and(|t| t.state.status.is_running()) {
                        self.start(st, tid);
                    }
                }
            }
            ClientMsg::StopAll(pid) => {
                for tid in Self::project_terminals(st, pid) {
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
        }
    }
}
