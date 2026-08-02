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

use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use host::RemoteHost;

/// The process-wide active server (at most one).
static ACTIVE: LazyLock<Mutex<Option<ServerHandle>>> = LazyLock::new(|| Mutex::new(None));

pub fn is_active() -> bool {
    ACTIVE.lock().unwrap().is_some()
}

/// Starts the server and records it as the process-wide instance.
pub fn activate(port: u16, host: Arc<dyn RemoteHost>) -> Result<(), String> {
    let mut active = ACTIVE.lock().unwrap();
    if active.is_some() {
        return Ok(());
    }
    *active = Some(start(port, host)?);
    Ok(())
}

pub fn deactivate() {
    if let Some(handle) = ACTIVE.lock().unwrap().take() {
        handle.stop();
    }
}

pub struct ServerHandle {
    shutdown: Arc<AtomicBool>,
    port: u16,
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

    let accept_shutdown = shutdown.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if accept_shutdown.load(Ordering::Relaxed) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let host = host.clone();
            let shutdown = accept_shutdown.clone();
            std::thread::spawn(move || http::handle(stream, host, shutdown));
        }
    });

    Ok(ServerHandle { shutdown, port })
}
