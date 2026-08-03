//! One thread per websocket connection. The socket runs with a short read
//! timeout so a single loop can both consume client messages and push
//! terminal output / state updates — tungstenite tolerates WouldBlock and
//! resumes partial frames on the next read.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tungstenite::error::Error as WsError;
use tungstenite::handshake::HandshakeError;
use tungstenite::protocol::Message;
use tungstenite::WebSocket;

use super::host::{Cmd, RemoteHost};
use super::registry;
use super::state::StateHub;

const POLL: Duration = Duration::from_millis(20);
const PING_EVERY: Duration = Duration::from_secs(20);

pub fn handle(
    stream: TcpStream,
    host: Arc<dyn RemoteHost>,
    hub: Arc<StateHub>,
    shutdown: Arc<AtomicBool>,
) {
    // Handshake on a blocking socket; the poll timeout is set afterwards.
    let mut ws = match tungstenite::accept(stream) {
        Ok(ws) => ws,
        Err(HandshakeError::Failure(_)) | Err(HandshakeError::Interrupted(_)) => return,
    };
    let _ = ws.get_ref().set_read_timeout(Some(POLL));
    let _ = ws.get_ref().set_nodelay(true);

    let mut attached: HashMap<u64, Receiver<Vec<u8>>> = HashMap::new();
    let mut seen_state = 0u64;
    let mut last_ping = Instant::now();

    // Initial state push, if one has been published.
    if let Some((version, json)) = hub.newer_than(0) {
        seen_state = version;
        if ws.send(Message::Text(json.into())).is_err() {
            return;
        }
    }

    loop {
        if shutdown.load(Ordering::Relaxed) {
            let _ = ws.close(None);
            return;
        }

        // 1. Client -> server.
        match ws.read() {
            Ok(Message::Text(text)) => {
                if !handle_message(&mut ws, &text, &host, &hub, &mut attached) {
                    return;
                }
            }
            Ok(Message::Binary(data)) => {
                if !handle_upload(&mut ws, &data, &hub) {
                    return;
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(WsError::Io(err))
                if err.kind() == ErrorKind::WouldBlock || err.kind() == ErrorKind::TimedOut => {}
            Err(_) => return,
        }

        // 2. Terminal output -> client.
        let mut dead: Vec<u64> = Vec::new();
        let mut frames: Vec<(u64, Vec<u8>)> = Vec::new();
        for (&id, rx) in &attached {
            let mut chunk: Vec<u8> = Vec::new();
            loop {
                match rx.try_recv() {
                    Ok(bytes) => {
                        chunk.extend_from_slice(&bytes);
                        // Bound one websocket frame; the rest goes next tick.
                        if chunk.len() >= 256 * 1024 {
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        dead.push(id);
                        break;
                    }
                }
            }
            if !chunk.is_empty() {
                frames.push((id, chunk));
            }
        }
        for (id, chunk) in frames {
            if ws.send(Message::Binary(term_frame(id, &chunk).into())).is_err() {
                return;
            }
        }
        for id in dead {
            attached.remove(&id);
            let _ = ws.send(Message::Text(
                format!("{{\"t\":\"exited\",\"term\":{id}}}").into(),
            ));
        }

        // 3. Chrome state -> client.
        if let Some((version, json)) = hub.newer_than(seen_state) {
            seen_state = version;
            if ws.send(Message::Text(json.into())).is_err() {
                return;
            }
        }

        // 4. Keepalive.
        if last_ping.elapsed() >= PING_EVERY {
            last_ping = Instant::now();
            if ws.send(Message::Ping(Vec::new().into())).is_err() {
                return;
            }
        }
    }
}

/// Max bytes of a file served to remote clients.
const FILE_CAP: usize = 512 * 1024;

/// Reads a file for the web viewer. Only paths inside a published session
/// root are served — the tree rows are the only legitimate source of paths.
fn read_file_reply(path: &str, hub: &Arc<StateHub>) -> serde_json::Value {
    let fail = |msg: &str| serde_json::json!({ "t": "file", "path": path, "error": msg });
    let Ok(canonical) = std::path::Path::new(path).canonicalize() else {
        return fail("file not found");
    };
    let roots = hub.roots();
    let allowed = roots.iter().any(|root| {
        std::path::Path::new(root)
            .canonicalize()
            .is_ok_and(|r| canonical.starts_with(&r))
    });
    if !allowed {
        return fail("outside session folders");
    }
    let bytes = match std::fs::read(&canonical) {
        Ok(b) => b,
        Err(err) => return fail(&err.to_string()),
    };
    let truncated = bytes.len() > FILE_CAP;
    let slice = &bytes[..bytes.len().min(FILE_CAP)];
    let text = match std::str::from_utf8(slice) {
        Ok(text) => text,
        // The cap may have split a multibyte char; keep the valid prefix.
        Err(e) if truncated && e.valid_up_to() + 4 >= slice.len() => {
            std::str::from_utf8(&slice[..e.valid_up_to()]).unwrap_or_default()
        }
        // Genuinely non-UTF-8; images etc. are out of scope.
        Err(_) => return fail("binary file"),
    };
    serde_json::json!({
        "t": "file",
        "path": path,
        "content": text,
        "truncated": truncated,
    })
}

/// Largest file a browser may drop onto the terminal.
const UPLOAD_CAP: usize = 25 * 1024 * 1024;

/// A dropped/attached file, sent as one binary frame:
/// `[u64 LE term id][u32 LE name length][name UTF-8][file bytes]`.
///
/// The browser is usually on a different machine, so a client-side path would
/// mean nothing to the agent — the bytes are written into the host's uploads
/// directory and *that* path is typed into the terminal, matching what a
/// desktop drag-and-drop does.
fn handle_upload(ws: &mut WebSocket<TcpStream>, data: &[u8], hub: &Arc<StateHub>) -> bool {
    let fail = |ws: &mut WebSocket<TcpStream>, msg: &str| {
        let json = serde_json::json!({ "t": "upload_error", "msg": msg });
        ws.send(Message::Text(json.to_string().into())).is_ok()
    };
    if data.len() < 12 {
        return true; // not an upload frame; ignore
    }
    let term = u64::from_le_bytes(data[0..8].try_into().unwrap());
    let name_len = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    if data.len() < 12 + name_len {
        return fail(ws, "malformed upload");
    }
    if !hub.owns_term(term) {
        return fail(ws, "unknown terminal");
    }
    let (name_bytes, body) = data[12..].split_at(name_len);
    if body.len() > UPLOAD_CAP {
        return fail(ws, "file too large (25 MB limit)");
    }

    let name = sanitize_filename(&String::from_utf8_lossy(name_bytes));
    let Some(dir) = crate::config::uploads_dir() else {
        return fail(ws, "no uploads directory");
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return fail(ws, &format!("cannot create uploads dir: {err}"));
    }
    let path = unique_path(&dir, &name);
    if let Err(err) = std::fs::write(&path, body) {
        return fail(ws, &format!("cannot write file: {err}"));
    }

    // Type the path in like a desktop drop: shell-quoted, trailing space, and
    // bracketed if the app asked for bracketed paste.
    let text = format!("{} ", crate::app::App::shell_escape(&path.display().to_string()));
    let bytes = super::registry::with_term_mode(term, |mode| {
        crate::term::keys::encode_paste(&text, mode)
    });
    if let Some(bytes) = bytes {
        super::registry::write(term, bytes);
    }

    let json = serde_json::json!({
        "t": "uploaded",
        "name": name,
        "path": path.display().to_string(),
    });
    ws.send(Message::Text(json.to_string().into())).is_ok()
}

/// Keeps a client-supplied name to a single harmless path component.
fn sanitize_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || c == '\0' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() {
        "upload".into()
    } else {
        cleaned.chars().take(120).collect()
    }
}

/// Never overwrite an earlier upload of the same name.
fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem}-{n}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(name)
}

/// Output frame: 8-byte little-endian term id, then raw PTY bytes.
fn term_frame(id: u64, bytes: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(8 + bytes.len());
    frame.extend_from_slice(&id.to_le_bytes());
    frame.extend_from_slice(bytes);
    frame
}

/// Returns false when the connection should be dropped.
fn handle_message(
    ws: &mut WebSocket<TcpStream>,
    text: &str,
    host: &Arc<dyn RemoteHost>,
    hub: &Arc<StateHub>,
    attached: &mut HashMap<u64, Receiver<Vec<u8>>>,
) -> bool {
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(text) else { return true };
    let kind = msg["t"].as_str().unwrap_or_default();
    let term = || msg["term"].as_u64().unwrap_or(0);
    let idx = |key: &str| msg[key].as_u64().unwrap_or(0) as usize;

    match kind {
        "input" => {
            let id = term();
            if hub.owns_term(id) {
                if let Some(data) = msg["data"].as_str() {
                    registry::write(id, data.as_bytes().to_vec());
                }
            }
        }
        "attach" => {
            let id = term();
            // Only this window's own terminals; ids are global, so a stray
            // id must not reach another window's shell.
            match hub.owns_term(id).then(|| registry::attach(id)).flatten() {
                Some((snapshot, rx, cols, rows)) => {
                    attached.insert(id, rx);
                    let resizable = host.allow_remote_resize();
                    let ack = format!(
                        "{{\"t\":\"attached\",\"term\":{id},\"cols\":{cols},\"rows\":{rows},\"resizable\":{resizable}}}"
                    );
                    if ws.send(Message::Text(ack.into())).is_err() {
                        return false;
                    }
                    if ws.send(Message::Binary(term_frame(id, &snapshot).into())).is_err() {
                        return false;
                    }
                }
                None => {
                    let err = format!("{{\"t\":\"exited\",\"term\":{id}}}");
                    if ws.send(Message::Text(err.into())).is_err() {
                        return false;
                    }
                }
            }
        }
        "detach" => {
            attached.remove(&term());
        }
        "resize" => {
            if host.allow_remote_resize() && hub.owns_term(term()) {
                host.command(Cmd::Resize {
                    term: term(),
                    cols: msg["cols"].as_u64().unwrap_or(80) as u16,
                    rows: msg["rows"].as_u64().unwrap_or(24) as u16,
                });
            }
        }
        "open_file" => {
            if let Some(path) = msg["path"].as_str() {
                let reply = read_file_reply(path, hub);
                if ws.send(Message::Text(reply.to_string().into())).is_err() {
                    return false;
                }
            }
        }
        "toggle_changes" => host.command(Cmd::ToggleChanges),
        "select_session" => host.command(Cmd::SelectSession(idx("idx"))),
        "select_term" => host.command(Cmd::SelectTerm { session: idx("session"), tab: idx("tab") }),
        "new_term" => host.command(Cmd::NewTerm { session: idx("session") }),
        "close_term" => host.command(Cmd::CloseTerm { session: idx("session"), tab: idx("tab") }),
        "preset" => host.command(Cmd::Preset(idx("idx"))),
        "row_toggle" => host.command(Cmd::RowToggle(msg["row"].as_i64().unwrap_or(-1) as i32)),
        _ => {}
    }
    true
}
