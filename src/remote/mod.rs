//! Remote web access: a loopback-only HTTP/WebSocket server that mirrors the
//! live terminals into a browser (desktop or phone). Exposure beyond this
//! machine is handled exclusively by Tailscale (`tailscale serve`), so the
//! listener never binds a routable address.

pub mod headless;
pub mod host;
pub mod http;
pub mod registry;
pub mod snapshot;
pub mod state;
pub mod tailscale;
pub mod ws;

use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use host::RemoteHost;
use state::StateHub;

/// Servers by owner id (a window's app id, or 0 for headless). Each window
/// serves its own port with its own sessions.
static SERVERS: LazyLock<Mutex<HashMap<u64, ServerHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn is_active(owner: u64) -> bool {
    SERVERS.lock().unwrap().contains_key(&owner)
}

/// True when some *other* owner already holds this port — checked before
/// binding so the Settings dialog can report the clash.
pub fn port_taken_by_other(owner: u64, port: u16) -> bool {
    SERVERS.lock().unwrap().iter().any(|(id, s)| *id != owner && s.port == port)
}

/// Starts a server for `owner`, replacing any server it already had.
/// Returns the hub the owner must publish its state into.
pub fn activate(
    owner: u64,
    port: u16,
    host: Arc<dyn RemoteHost>,
) -> Result<Arc<StateHub>, String> {
    if port_taken_by_other(owner, port) {
        return Err(format!("port {port} is already used by another window"));
    }
    // Drop the old server first so re-binding the same port succeeds.
    deactivate(owner);
    let handle = start(port, host)?;
    let hub = handle.hub.clone();
    SERVERS.lock().unwrap().insert(owner, handle);
    Ok(hub)
}

pub fn deactivate(owner: u64) {
    let handle = SERVERS.lock().unwrap().remove(&owner);
    if let Some(handle) = handle {
        handle.stop();
    }
}

pub struct ServerHandle {
    shutdown: Arc<AtomicBool>,
    port: u16,
    hub: Arc<StateHub>,
}

impl ServerHandle {
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Unblock the accept loop; connection threads notice the flag on
        // their next poll tick.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

pub fn start(port: u16, host: Arc<dyn RemoteHost>) -> Result<ServerHandle, String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("bind 127.0.0.1:{port}: {e}"))?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let hub: Arc<StateHub> = Arc::new(StateHub::default());

    let accept_shutdown = shutdown.clone();
    let accept_hub = hub.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if accept_shutdown.load(Ordering::Relaxed) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let host = host.clone();
            let shutdown = accept_shutdown.clone();
            let hub = accept_hub.clone();
            std::thread::spawn(move || http::handle(stream, host, hub, shutdown));
        }
    });

    Ok(ServerHandle { shutdown, port, hub })
}
