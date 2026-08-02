//! Shells out to the Tailscale CLI to publish the loopback server at
//! https://<machine>.<tailnet>.ts.net. All calls block on subprocesses, so
//! run them on a worker thread, never the UI event loop.

use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub enum TsStatus {
    NotInstalled,
    /// Installed but logged out / stopped.
    NotRunning,
    Serving { url: String },
    Error(String),
}

impl TsStatus {
    pub fn label(&self) -> String {
        match self {
            TsStatus::NotInstalled => "Tailscale is not installed".into(),
            TsStatus::NotRunning => {
                "Tailscale is not running — open the Tailscale app and log in".into()
            }
            TsStatus::Serving { url } => format!("Serving at {url}"),
            TsStatus::Error(msg) => format!("Tailscale error: {msg}"),
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

/// The machine's HTTPS URL on the tailnet, if Tailscale is up.
fn dns_name(cli: &PathBuf) -> Result<String, TsStatus> {
    let output = Command::new(cli)
        .args(["status", "--json"])
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
    match json["Self"]["DNSName"].as_str() {
        Some(name) if !name.is_empty() => Ok(name.trim_end_matches('.').to_string()),
        _ => Err(TsStatus::Error("no tailnet DNS name (MagicDNS off?)".into())),
    }
}

pub fn enable(port: u16) -> TsStatus {
    let Some(cli) = find_cli() else { return TsStatus::NotInstalled };
    let name = match dns_name(&cli) {
        Ok(name) => name,
        Err(status) => return status,
    };
    let output = Command::new(&cli)
        .args(["serve", "--bg", "--https=443", &format!("http://127.0.0.1:{port}")])
        .output();
    match output {
        Ok(out) if out.status.success() => TsStatus::Serving { url: format!("https://{name}") },
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            TsStatus::Error(err.lines().next().unwrap_or("serve failed").to_string())
        }
        Err(e) => TsStatus::Error(e.to_string()),
    }
}

pub fn disable() {
    if let Some(cli) = find_cli() {
        let _ = Command::new(&cli).args(["serve", "--https=443", "off"]).output();
    }
}
