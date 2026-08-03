//! Versioned snapshot of the sidebar/chrome state, published by the UI thread
//! (or the headless manager) and consumed by that server's websocket
//! connections. One hub per server, so several windows can each serve their
//! own port with their own sessions.

use std::collections::HashSet;
use std::sync::Mutex;

use serde::Serialize;

#[derive(Serialize, Clone, Default)]
pub struct UiState {
    pub active_session: usize,
    pub presets: Vec<String>,
    pub sessions: Vec<UiSession>,
    pub tree: Vec<UiTreeRow>,
    /// Window title shown by the web client (e.g. the team name).
    pub title: String,
    /// Colors + font the web client should render with, mirroring the
    /// desktop theme.
    pub theme: UiTheme,
}

/// The subset of the active theme the web client needs. Hex strings so they
/// drop straight into CSS custom properties.
#[derive(Serialize, Clone, Default)]
pub struct UiTheme {
    pub bg: String,
    pub panel: String,
    pub panel_hover: String,
    pub selection: String,
    pub border: String,
    pub text: String,
    pub text_dim: String,
    pub accent: String,
    pub font_family: String,
    pub font_size: f32,
    pub ui_font_size: f32,
    /// ANSI 0-15 for xterm.js, in palette order.
    pub ansi: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct UiSession {
    pub name: String,
    pub root: String,
    pub terms: Vec<u64>,
    pub active_term: usize,
    pub exited: Vec<bool>,
}

/// Mirrors the Slint TreeRow model: kind 0 = session header, 1 = dir,
/// 2 = file, 3 = changes header, 4 = change row.
#[derive(Serialize, Clone)]
pub struct UiTreeRow {
    pub kind: i32,
    pub indent: i32,
    pub name: String,
    pub expanded: bool,
    pub session: i32,
    pub row_id: i32,
    /// Absolute path for file rows (kind 2) so remote clients can request the
    /// content; None for everything else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Default)]
struct HubInner {
    version: u64,
    json: String,
    /// Session roots from the last publish; file reads are confined to these.
    roots: Vec<String>,
    /// Terminal ids from the last publish; a client may only attach to these,
    /// so one window's page cannot reach another window's shells.
    terms: HashSet<u64>,
}

#[derive(Default)]
pub struct StateHub {
    inner: Mutex<HubInner>,
}

impl StateHub {
    pub fn publish(&self, state: &UiState) {
        let mut value = match serde_json::to_value(state) {
            Ok(v) => v,
            Err(_) => return,
        };
        value["t"] = "state".into();
        let mut inner = self.inner.lock().unwrap();
        inner.version += 1;
        inner.json = value.to_string();
        inner.roots = state.sessions.iter().map(|s| s.root.clone()).collect();
        inner.terms = state.sessions.iter().flat_map(|s| s.terms.iter().copied()).collect();
    }

    /// Non-blocking check used by the websocket poll loop.
    pub fn newer_than(&self, seen: u64) -> Option<(u64, String)> {
        let inner = self.inner.lock().unwrap();
        if inner.version > seen && !inner.json.is_empty() {
            Some((inner.version, inner.json.clone()))
        } else {
            None
        }
    }

    pub fn roots(&self) -> Vec<String> {
        self.inner.lock().unwrap().roots.clone()
    }

    pub fn owns_term(&self, id: u64) -> bool {
        self.inner.lock().unwrap().terms.contains(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(root: &str, terms: Vec<u64>) -> UiState {
        UiState {
            sessions: vec![UiSession {
                name: "s".into(),
                root: root.into(),
                terms,
                active_term: 0,
                exited: vec![false],
            }],
            ..Default::default()
        }
    }

    /// Two windows in one process each get their own hub, so neither can
    /// reach the other's terminals or read files outside its own folders.
    #[test]
    fn hubs_are_isolated_per_window() {
        let a = StateHub::default();
        let b = StateHub::default();
        a.publish(&state_with("/tmp/a", vec![1, 2]));
        b.publish(&state_with("/tmp/b", vec![3]));

        assert!(a.owns_term(1) && a.owns_term(2));
        assert!(!a.owns_term(3), "window A must not own window B's terminal");
        assert!(b.owns_term(3));
        assert!(!b.owns_term(1), "window B must not own window A's terminal");

        assert_eq!(a.roots(), vec!["/tmp/a".to_string()]);
        assert_eq!(b.roots(), vec!["/tmp/b".to_string()]);
    }

    /// Closing a terminal drops it from the owner set, so a stale client can
    /// no longer attach to the id.
    #[test]
    fn ownership_follows_the_latest_publish() {
        let hub = StateHub::default();
        hub.publish(&state_with("/tmp/a", vec![1, 2]));
        assert!(hub.owns_term(2));
        hub.publish(&state_with("/tmp/a", vec![1]));
        assert!(!hub.owns_term(2), "closed terminal must lose ownership");
    }

    #[test]
    fn versions_advance_so_clients_see_updates() {
        let hub = StateHub::default();
        assert!(hub.newer_than(0).is_none(), "nothing published yet");
        hub.publish(&state_with("/tmp/a", vec![1]));
        let (v1, json) = hub.newer_than(0).expect("first state");
        assert!(json.contains("\"t\":\"state\""));
        assert!(hub.newer_than(v1).is_none(), "no newer state yet");
        hub.publish(&state_with("/tmp/a", vec![1, 2]));
        let (v2, _) = hub.newer_than(v1).expect("second state");
        assert!(v2 > v1);
    }
}
