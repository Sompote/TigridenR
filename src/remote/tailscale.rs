//! Shells out to the Tailscale CLI to publish the loopback server at
//! https://<machine>.<tailnet>.ts.net. All calls block on subprocesses, so
//! run them on a worker thread, never the UI event loop.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub enum TsStatus {
    NotInstalled,
    /// Installed but logged out / stopped.
    NotRunning,
    Serving {
        url: String,
        /// False when serving plain HTTP over the tailnet because the tailnet
        /// has no HTTPS certificates enabled.
        https: bool,
    },
    Error(String),
}

impl TsStatus {
    pub fn label(&self) -> String {
        match self {
            TsStatus::NotInstalled => "Tailscale is not installed".into(),
            TsStatus::NotRunning => {
                "Tailscale is not running — open the Tailscale app and log in".into()
            }
            TsStatus::Serving { url, https: true } => format!("Serving at {url}"),
            TsStatus::Serving { url, https: false } => format!(
                "Serving at {url} (encrypted by Tailscale, but not HTTPS — \
                 enable HTTPS Certificates at login.tailscale.com/admin/dns \
                 for an https:// address)"
            ),
            TsStatus::Error(msg) => format!("Tailscale error: {msg}"),
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            TsStatus::Serving { url, .. } => Some(url),
            _ => None,
        }
    }
}

/// Where a working `tailscale` CLI may live. PATH first (covers terminal
/// launches and custom installs), then the usual absolute locations, and the
/// app bundle last — see `status_json` for why that one is a poor first
/// choice.
const CLI_CANDIDATES: [&str; 4] = [
    "tailscale",
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

/// The candidate that last answered successfully, so `serve` and `serve off`
/// reuse it instead of probing again.
static WORKING_CLI: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Runs `status --json` on each candidate and returns the first whose output
/// actually parses.
///
/// Exit status is not a usable test here: launched from Finder (no GUI
/// session environment), the app-bundle binary prints "The Tailscale GUI
/// failed to start…" to *stdout* and still exits 0. Requiring parseable JSON
/// is what separates a working CLI from that one.
fn status_json() -> Result<(PathBuf, serde_json::Value), TsStatus> {
    let mut ran_something = false;
    for candidate in CLI_CANDIDATES {
        let Ok(output) =
            Command::new(candidate).args(["status", "--json"]).stdin(Stdio::null()).output()
        else {
            continue; // not installed at this path
        };
        ran_something = true;
        let text = String::from_utf8_lossy(&output.stdout);
        // Tolerate any chatter printed before the JSON body.
        let Some(start) = text.find('{') else { continue };
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text[start..]) {
            let path = PathBuf::from(candidate);
            *WORKING_CLI.lock().unwrap() = Some(path.clone());
            return Ok((path, json));
        }
    }
    Err(if ran_something {
        TsStatus::Error(
            "found Tailscale but its CLI would not answer — install the CLI \
             (Tailscale ▸ Install CLI, or `brew install tailscale`)"
                .into(),
        )
    } else {
        TsStatus::NotInstalled
    })
}

/// The CLI to run for non-status commands; falls back to probing.
fn cli() -> Option<PathBuf> {
    if let Some(path) = WORKING_CLI.lock().unwrap().clone() {
        return Some(path);
    }
    status_json().ok().map(|(path, _)| path)
}

/// The machine's tailnet DNS name plus whether the tailnet has HTTPS
/// certificates enabled (`CertDomains` non-empty).
fn tailnet_info() -> Result<(PathBuf, String, bool), TsStatus> {
    let (cli, json) = status_json()?;
    if json["BackendState"].as_str() != Some("Running") {
        return Err(TsStatus::NotRunning);
    }
    let https = json["CertDomains"].as_array().is_some_and(|d| !d.is_empty());
    match json["Self"]["DNSName"].as_str() {
        Some(name) if !name.is_empty() => {
            Ok((cli, name.trim_end_matches('.').to_string(), https))
        }
        _ => Err(TsStatus::Error("no tailnet DNS name (MagicDNS off?)".into())),
    }
}

pub fn enable(port: u16) -> TsStatus {
    let (cli, name, https) = match tailnet_info() {
        Ok(info) => info,
        Err(status) => return status,
    };

    // `serve --https` blocks indefinitely when the tailnet has no HTTPS
    // certificates enabled, so only ask for it when certs are available;
    // otherwise publish plain HTTP, which WireGuard still encrypts.
    let (flag, scheme) =
        if https { ("--https=443", "https") } else { ("--http=80", "http") };

    // stdin is closed so any unexpected interactive prompt fails fast rather
    // than hanging the dialog forever.
    let output = Command::new(&cli)
        .args(["serve", "--bg", flag, &format!("http://127.0.0.1:{port}")])
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => {
            SERVING.store(true, Ordering::Release);
            TsStatus::Serving { url: format!("{scheme}://{name}"), https }
        }
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stderr);
            let msg = text
                .lines()
                .find(|l| !l.trim().is_empty() && !l.starts_with("Warning:"))
                .unwrap_or("serve failed");
            TsStatus::Error(msg.to_string())
        }
        Err(e) => TsStatus::Error(e.to_string()),
    }
}

pub fn disable() {
    SERVING.store(false, Ordering::Release);
    let Some(cli) = cli() else { return };
    // Tear down whichever listener was set up.
    for flag in ["--https=443", "--http=80"] {
        let _ = Command::new(&cli)
            .args(["serve", flag, "off"])
            .stdin(Stdio::null())
            .output();
    }
}

/// True once `enable` has published a URL, until `disable` tears it down.
static SERVING: AtomicBool = AtomicBool::new(false);

/// Cleanup for process exit: only shells out when we actually published a
/// serve config, so quitting a normal local session costs nothing.
pub fn disable_if_serving() {
    if SERVING.load(Ordering::Acquire) {
        disable();
    }
}
