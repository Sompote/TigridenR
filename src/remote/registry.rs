//! Thread-safe registry of live terminals, keyed by the global term id.
//!
//! The UI-side window registry is thread_local; server threads can't touch it.
//! This map holds only `Send` handles cloned out of each `TermSession` at
//! spawn time, so any thread can attach to or write into a terminal.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{LazyLock, Mutex};

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;

use crate::term::EventProxy;

pub struct Endpoint {
    pub term: std::sync::Arc<FairMutex<Term<EventProxy>>>,
    pub input_tx: Sender<Vec<u8>>,
    pub taps: std::sync::Arc<Mutex<Vec<Sender<Vec<u8>>>>>,
    pub size: std::sync::Arc<Mutex<WindowSize>>,
}

static ENDPOINTS: LazyLock<Mutex<HashMap<u64, Endpoint>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn insert(id: u64, endpoint: Endpoint) {
    ENDPOINTS.lock().unwrap().insert(id, endpoint);
}

pub fn remove(id: u64) {
    ENDPOINTS.lock().unwrap().remove(&id);
}

/// Writes input bytes into a terminal's PTY. Returns false if the terminal is
/// gone (client should drop its attachment).
pub fn write(id: u64, bytes: Vec<u8>) -> bool {
    let map = ENDPOINTS.lock().unwrap();
    match map.get(&id) {
        Some(ep) => ep.input_tx.send(bytes).is_ok(),
        None => false,
    }
}

/// Atomically snapshots the current grid as ANSI and subscribes to the raw
/// output stream. Lock order (term, then taps) matches the PTY reader thread,
/// so the snapshot and the stream are gap-free and duplicate-free.
pub fn attach(id: u64) -> Option<(Vec<u8>, Receiver<Vec<u8>>, u16, u16)> {
    let map = ENDPOINTS.lock().unwrap();
    let ep = map.get(&id)?;
    let term = ep.term.lock();
    let snapshot = super::snapshot::grid_to_ansi(&term);
    let (tx, rx) = channel();
    ep.taps.lock().unwrap().push(tx);
    let size = *ep.size.lock().unwrap();
    Some((snapshot, rx, size.num_cols, size.num_lines))
}
