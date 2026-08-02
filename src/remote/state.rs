//! Versioned snapshot of the sidebar/chrome state, published by the UI thread
//! (or the headless manager) and consumed by every websocket connection.
//! This is the only shared mutable state crossing the UI/server boundary.

use std::sync::{LazyLock, Mutex};

use serde::Serialize;

#[derive(Serialize, Clone, Default)]
pub struct UiState {
    pub active_session: usize,
    pub presets: Vec<String>,
    pub sessions: Vec<UiSession>,
    pub tree: Vec<UiTreeRow>,
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
}

pub struct StateHub {
    inner: Mutex<(u64, String)>,
}

pub static HUB: LazyLock<StateHub> = LazyLock::new(|| StateHub {
    inner: Mutex::new((0, String::new())),
});

impl StateHub {
    pub fn publish(&self, state: &UiState) {
        let mut value = match serde_json::to_value(state) {
            Ok(v) => v,
            Err(_) => return,
        };
        value["t"] = "state".into();
        let mut inner = self.inner.lock().unwrap();
        inner.0 += 1;
        inner.1 = value.to_string();
    }

    /// Non-blocking check used by the websocket poll loop.
    pub fn newer_than(&self, seen: u64) -> Option<(u64, String)> {
        let inner = self.inner.lock().unwrap();
        if inner.0 > seen && !inner.1.is_empty() {
            Some((inner.0, inner.1.clone()))
        } else {
            None
        }
    }
}
