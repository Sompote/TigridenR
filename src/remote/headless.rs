//! `--headless`: serves remote clients with no window and no Slint event
//! loop. A minimal session manager reuses TermSession / TreeState / git
//! tracking directly; remote clients drive the PTY size.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

use crate::config::Config;
use crate::git;
use crate::term::{TermHooks, TermSession};
use crate::tree::TreeState;

use super::host::{Cmd, RemoteHost};
use super::state::{UiSession, UiState, UiTreeRow, HUB};

/// Nominal cell size for PTY pixel dimensions; headless has no renderer.
const CELL_PX: (u16, u16) = (8, 17);

enum Event {
    /// Filesystem changed under a session root.
    Fs(PathBuf),
    /// A shell exited or state otherwise needs republishing.
    Publish,
}

enum RowTarget {
    Session(usize),
    Dir(usize, PathBuf),
    Inert,
}

struct HSession {
    root: PathBuf,
    name: String,
    terms: Vec<TermSession>,
    active_term: usize,
    tree: TreeState,
    tracking: Option<git::Tracking>,
    changes: Vec<git::Change>,
    _watcher: Option<notify::RecommendedWatcher>,
}

pub struct Manager {
    config: Config,
    sessions: Vec<HSession>,
    active: usize,
    row_map: Vec<RowTarget>,
    events: Sender<Event>,
    /// Shared with the PTY threads so OSC color answerbacks use the
    /// configured theme (fixed for the process in headless mode).
    theme_index: Arc<std::sync::atomic::AtomicU8>,
}

impl Manager {
    fn spawn_term(&self, root: &Path) -> Option<TermSession> {
        let id = crate::app::NEXT_TERM_ID.fetch_add(1, Ordering::Relaxed);
        let events = self.events.clone();
        let hooks = TermHooks {
            // Remote taps carry the output; nothing to repaint locally.
            repaint: Arc::new(|| {}),
            exited: Arc::new(move || {
                let _ = events.send(Event::Publish);
            }),
        };
        match TermSession::spawn(
            id,
            root,
            80,
            24,
            CELL_PX,
            self.config.scrollback,
            self.theme_index.clone(),
            hooks,
        ) {
            Ok(term) => Some(term),
            Err(err) => {
                eprintln!("tigridenr: {err}");
                None
            }
        }
    }

    fn add_session(&mut self, root: PathBuf) {
        let root = root.canonicalize().unwrap_or(root);
        if self.sessions.iter().any(|s| s.root == root) {
            return;
        }
        let Some(term) = self.spawn_term(&root) else { return };
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());

        let events = self.events.clone();
        let watch_root = root.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                if !event.paths.is_empty() {
                    let _ = events.send(Event::Fs(watch_root.clone()));
                }
            }
        })
        .ok()
        .and_then(|mut w| w.watch(&root, RecursiveMode::Recursive).ok().map(|_| w));

        let tracking = git::detect(&root);
        if let Some(git::Tracking::Shadow(dir)) = &tracking {
            git::snapshot_baseline(&root, dir);
        }
        let changes = tracking.as_ref().map(|t| git::status(&root, t)).unwrap_or_default();

        self.sessions.push(HSession {
            tree: TreeState::new(root.clone()),
            root,
            name,
            terms: vec![term],
            active_term: 0,
            tracking,
            changes,
            _watcher: watcher,
        });
    }

    fn active_term_write(&self, bytes: Vec<u8>) {
        if let Some(session) = self.sessions.get(self.active) {
            if let Some(term) = session.terms.get(session.active_term) {
                term.write(bytes);
            }
        }
    }

    fn publish(&mut self) {
        self.row_map.clear();
        let mut tree: Vec<UiTreeRow> = Vec::new();
        for i in 0..self.sessions.len() {
            let active = i == self.active;
            let name = self.sessions[i].name.clone();
            tree.push(UiTreeRow {
                kind: 0,
                indent: 0,
                name,
                expanded: true,
                session: i as i32,
                row_id: self.row_map.len() as i32,
                path: None,
            });
            self.row_map.push(RowTarget::Session(i));
            let _ = active;

            if self.sessions[i].tracking.is_some() {
                tree.push(UiTreeRow {
                    kind: 3,
                    indent: 1,
                    name: format!("Changes ({})", self.sessions[i].changes.len()),
                    expanded: true,
                    session: i as i32,
                    row_id: self.row_map.len() as i32,
                    path: None,
                });
                self.row_map.push(RowTarget::Inert);
                for change in &self.sessions[i].changes {
                    tree.push(UiTreeRow {
                        kind: 4,
                        indent: 2,
                        name: format!("{}  {}", change.status, change.rel),
                        expanded: false,
                        session: i as i32,
                        row_id: self.row_map.len() as i32,
                        path: None,
                    });
                    self.row_map.push(RowTarget::Inert);
                }
            }
            for flat in self.sessions[i].tree.flatten() {
                tree.push(UiTreeRow {
                    kind: flat.kind,
                    indent: flat.indent,
                    name: flat.name.clone(),
                    expanded: flat.expanded,
                    session: i as i32,
                    row_id: self.row_map.len() as i32,
                    path: (flat.kind == 2).then(|| flat.path.display().to_string()),
                });
                self.row_map.push(if flat.kind == 1 {
                    RowTarget::Dir(i, flat.path)
                } else {
                    RowTarget::Inert
                });
            }
        }

        let state = UiState {
            active_session: self.active,
            presets: self.config.presets.iter().map(|p| p.label.clone()).collect(),
            sessions: self
                .sessions
                .iter()
                .map(|s| UiSession {
                    name: s.name.clone(),
                    root: s.root.display().to_string(),
                    terms: s.terms.iter().map(|t| t.id).collect(),
                    active_term: s.active_term,
                    exited: s.terms.iter().map(|t| t.exited.load(Ordering::Acquire)).collect(),
                })
                .collect(),
            tree,
        };
        HUB.publish(&state);
    }

    fn refresh_fs(&mut self, root: &Path) {
        let Some(idx) = self.sessions.iter().position(|s| s.root == root) else { return };
        // Cheap full invalidation: drop all cached listings by rebuilding.
        self.sessions[idx].tree = {
            let mut fresh = TreeState::new(root.to_path_buf());
            // Preserve expansion state by re-expanding known-open dirs.
            for row in self.sessions[idx].tree.flatten() {
                if row.kind == 1 && row.expanded {
                    fresh.toggle(&row.path);
                }
            }
            fresh
        };
        if let Some(tracking) = self.sessions[idx].tracking.clone() {
            self.sessions[idx].changes = git::status(root, &tracking);
        }
    }
}

pub struct HeadlessHost {
    manager: Arc<Mutex<Manager>>,
}

impl RemoteHost for HeadlessHost {
    fn command(&self, cmd: Cmd) {
        let mut m = self.manager.lock().unwrap();
        match cmd {
            Cmd::Resize { term, cols, rows } => {
                for session in &mut m.sessions {
                    if let Some(t) = session.terms.iter_mut().find(|t| t.id == term) {
                        t.resize(cols, rows, CELL_PX);
                        return; // no state change to publish
                    }
                }
                return;
            }
            Cmd::SelectSession(idx) => {
                if idx < m.sessions.len() {
                    m.active = idx;
                }
            }
            Cmd::SelectTerm { session, tab } => {
                if let Some(s) = m.sessions.get_mut(session) {
                    if tab < s.terms.len() {
                        m.active = session;
                        m.sessions[session].active_term = tab;
                    }
                }
            }
            Cmd::NewTerm { session } => {
                if session < m.sessions.len() {
                    let root = m.sessions[session].root.clone();
                    if let Some(term) = m.spawn_term(&root) {
                        let s = &mut m.sessions[session];
                        s.terms.push(term);
                        s.active_term = s.terms.len() - 1;
                        m.active = session;
                    }
                }
            }
            Cmd::CloseTerm { session, tab } => {
                if let Some(s) = m.sessions.get_mut(session) {
                    if tab < s.terms.len() && s.terms.len() > 1 {
                        let mut term = s.terms.remove(tab);
                        term.shutdown();
                        s.active_term = s.active_term.min(s.terms.len() - 1);
                    }
                }
            }
            Cmd::Preset(idx) => {
                if let Some(preset) = m.config.presets.get(idx).cloned() {
                    let mut bytes = preset.command.into_bytes();
                    if preset.send_enter {
                        bytes.push(b'\r');
                    }
                    m.active_term_write(bytes);
                }
                return;
            }
            // Change tracking is always on in headless mode.
            Cmd::ToggleChanges => return,
            Cmd::RowToggle(row) => {
                if row < 0 {
                    return;
                }
                match m.row_map.get(row as usize) {
                    Some(RowTarget::Session(idx)) => m.active = *idx,
                    Some(RowTarget::Dir(idx, path)) => {
                        let (idx, path) = (*idx, path.clone());
                        m.sessions[idx].tree.toggle(&path);
                    }
                    _ => return,
                }
            }
        }
        m.publish();
    }

    fn allow_remote_resize(&self) -> bool {
        true
    }
}

/// Entry point for `tigridenr --headless`. Blocks forever.
pub fn run(config: Config, port: u16) -> ! {
    let (events_tx, events_rx) = channel::<Event>();

    let mut manager = Manager {
        theme_index: Arc::new(std::sync::atomic::AtomicU8::new(crate::theme::index_of(
            &config.theme,
        ))),
        config: config.clone(),
        sessions: Vec::new(),
        active: 0,
        row_map: Vec::new(),
        events: events_tx.clone(),
    };
    // Restore the same folders the GUI would.
    let state = crate::config::load_state();
    for folder in &state.folders {
        if folder.is_dir() {
            manager.add_session(folder.clone());
        }
    }
    manager.active = state.active.min(manager.sessions.len().saturating_sub(1));
    manager.publish();
    let manager = Arc::new(Mutex::new(manager));

    let host = Arc::new(HeadlessHost { manager: manager.clone() });
    match super::activate(port, host) {
        Ok(()) => eprintln!("tigridenr: remote server on http://127.0.0.1:{port}"),
        Err(err) => {
            eprintln!("tigridenr: {err}");
            std::process::exit(1);
        }
    }

    std::thread::spawn(move || {
        let status = super::tailscale::enable(port);
        eprintln!("tigridenr: {}", status.label());
    });

    // Event loop: debounce fs events, republish on demand.
    event_loop(&manager, events_rx)
}

fn event_loop(manager: &Arc<Mutex<Manager>>, events: Receiver<Event>) -> ! {
    loop {
        match events.recv() {
            Ok(Event::Fs(root)) => {
                // Debounce: coalesce further events for 250 ms.
                let mut roots = vec![root];
                let deadline = std::time::Instant::now() + Duration::from_millis(250);
                loop {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match events.recv_timeout(deadline - now) {
                        Ok(Event::Fs(r)) => {
                            if !roots.contains(&r) {
                                roots.push(r);
                            }
                        }
                        Ok(Event::Publish) => {}
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                let mut m = manager.lock().unwrap();
                for root in roots {
                    m.refresh_fs(&root);
                }
                m.publish();
            }
            Ok(Event::Publish) => manager.lock().unwrap().publish(),
            Err(_) => {
                // All senders gone; nothing left to do but serve terminals.
                std::thread::park();
            }
        }
    }
}
