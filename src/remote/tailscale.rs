//! Shells out to the Tailscale CLI to publish the loopback server at
//! https://<machine>.<tailnet>.ts.net. All calls block on subprocesses, so
//! run them on a worker thread, never the UI event loop.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

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

fn find_cli() -> Option<PathBuf> {
    // PATH first, then the macOS app-bundle CLI (not linked into PATH by
    // default when installed from the App Store).
    let from_path = Command::new("tailscale").arg("--version").output();
    if from_path.map(|o| o.status.success()).unwrap_or(false) {
        return Some(PathBuf::from("tailscale"));
    }
    let bundled = PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale");
    if bundled.is_file() {
        return Some(bundled);
    }
    None
}

/// The machine's tailnet DNS name plus whether the tailnet has HTTPS
/// certificates enabled (`CertDomains` non-empty).
fn tailnet_info(cli: &PathBuf) -> Result<(String, bool), TsStatus> {
    let output = Command::new(cli)
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| TsStatus::Error(e.to_string()))?;
    if !output.status.success() {
        return Err(TsStatus::NotRunning);
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| TsStatus::Error(format!("status parse: {e}")))?;
    if json["BackendState"].as_str() != Some("Running") {
        return Err(TsStatus::NotRunning);
    }
    let https = json["CertDomains"].as_array().is_some_and(|d| !d.is_empty());
    match json["Self"]["DNSName"].as_str() {
        Some(name) if !name.is_empty() => Ok((name.trim_end_matches('.').to_string(), https)),
        _ => Err(TsStatus::Error("no tailnet DNS name (MagicDNS off?)".into())),
    }
}

pub fn enable(port: u16) -> TsStatus {
    let Some(cli) = find_cli() else { return TsStatus::NotInstalled };
    let (name, https) = match tailnet_info(&cli) {
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
    let Some(cli) = find_cli() else { return };
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
