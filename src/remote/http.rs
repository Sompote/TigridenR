//! Minimal hand-rolled HTTP for the handful of embedded static assets, plus
//! routing of /ws upgrades into the websocket handler. tungstenite performs
//! the upgrade handshake itself, so the request is only peeked (not consumed)
//! to decide the route.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use super::host::RemoteHost;
use super::state::StateHub;

const INDEX_HTML: &[u8] = include_bytes!("../../assets/web/index.html");
const APP_JS: &[u8] = include_bytes!("../../assets/web/app.js");
const STYLE_CSS: &[u8] = include_bytes!("../../assets/web/style.css");
const XTERM_JS: &[u8] = include_bytes!("../../assets/web/vendor/xterm.js");
const XTERM_CSS: &[u8] = include_bytes!("../../assets/web/vendor/xterm.css");

pub fn handle(
    stream: TcpStream,
    host: Arc<dyn RemoteHost>,
    hub: Arc<StateHub>,
    shutdown: Arc<AtomicBool>,
) {
    let Some(path) = peek_path(&stream) else { return };
    if path == "/ws" {
        super::ws::handle(stream, host, hub, shutdown);
    } else {
        serve_static(stream, &path);
    }
}

/// Reads the request line without consuming it, so tungstenite can run the
/// full handshake on an untouched stream.
fn peek_path(stream: &TcpStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 2048];
    // Retry briefly: the first peek can race the client's send.
    for _ in 0..50 {
        let n = stream.peek(&mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf[..n]);
        if let Some(line) = text.lines().next() {
            if line.ends_with("HTTP/1.1") || line.ends_with("HTTP/1.0") {
                let mut parts = line.split_whitespace();
                let _method = parts.next()?;
                return parts.next().map(|p| p.to_string());
            }
        }
        if n == buf.len() {
            return None; // request line longer than the buffer; give up
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn serve_static(mut stream: TcpStream, path: &str) {
    // Consume the request (GET only, no body) up to the blank line.
    let mut buf = [0u8; 2048];
    let mut seen: Vec<u8> = Vec::new();
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                seen.extend_from_slice(&buf[..n]);
                if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if seen.len() > 64 * 1024 {
                    return;
                }
            }
        }
    }

    let (body, content_type): (&[u8], &str) = match path {
        "/" | "/index.html" => (INDEX_HTML, "text/html; charset=utf-8"),
        "/app.js" => (APP_JS, "application/javascript"),
        "/style.css" => (STYLE_CSS, "text/css"),
        "/vendor/xterm.js" => (XTERM_JS, "application/javascript"),
        "/vendor/xterm.css" => (XTERM_CSS, "text/css"),
        _ => {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found",
            );
            return;
        }
    };

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}
