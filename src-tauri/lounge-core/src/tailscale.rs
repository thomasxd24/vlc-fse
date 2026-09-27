//! Tailscale without its tray icon: the full screen experience has no taskbar or notification area, so
//! Lounge drives Tailscale's own CLI for status, connecting, disconnecting, exit nodes and signing in.
//! Direct port of the pure/injectable parts of `src/tailscale.js`.
//!
//! The `Tailscale` class itself — `status`/`up`/`down`/`setExitNode` (spawn the CLI with a timeout) and
//! especially `startLogin`/`cancelLogin` (spawns `tailscale up`, streams its stdout/stderr looking for a
//! sign-in URL, tracks one in-flight login) — is **not** ported here. None of it has a test, and it's
//! exactly the kind of subprocess-with-timeout / streaming-with-callback code that's better done against
//! the Tauri `app` crate's real async runtime than blocking-and-polled here, for the same reasons
//! `games::GameSession`, `vlc::VlcSession` and `system`'s `SystemHelper` are deferred.

use fancy_regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

pub static AUTH_URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https://login\.tailscale\.com/a/[\w-]+|https://[\w.-]+/a/[\w-]{6,}").unwrap());

/// Find a Tailscale sign-in URL in `tailscale up`'s combined stdout/stderr text so far.
pub fn find_auth_url(text: &str) -> Option<String> {
    AUTH_URL_RE.find(text).ok().flatten().map(|m| m.as_str().to_string())
}

/// The executable in a service ImagePath or command line: `"C:\x y\a.exe" -arg` or `C:\x\a.exe -arg`.
pub fn exe_of_command(cmd: &str) -> Option<String> {
    let s = cmd.trim_start();
    if s.is_empty() {
        return None;
    }
    let raw = if let Some(rest) = s.strip_prefix('"') {
        match rest.find('"') {
            Some(end) => &rest[..end],
            // No closing quote: falls back to a plain token (leading quote included), same as the JS
            // regex's `"([^"]+)"` alternative failing to match and `(\S+)` taking over.
            None => s.split_whitespace().next()?,
        }
    } else {
        s.split_whitespace().next()?
    };
    if raw.is_empty() {
        return None;
    }
    Some(raw.strip_prefix(r"\??\").unwrap_or(raw).to_string())
}

/// Windows-style path join, usable regardless of the host OS this is compiled for — mirroring the JS
/// version's explicit use of `path.win32` (Tailscale on a non-Windows host is looked up with plain Unix
/// paths instead; see [`locate_cli`]/[`find_cli`]).
fn win_join(parts: &[&str]) -> String {
    let mut out = String::new();
    for p in parts.iter().filter(|p| !p.is_empty()) {
        let p = p.trim_end_matches(['\\', '/']);
        if out.is_empty() {
            out.push_str(p);
        } else {
            out.push('\\');
            out.push_str(p.trim_start_matches(['\\', '/']));
        }
    }
    out
}

fn win_dirname(p: &str) -> String {
    match p.rfind(['\\', '/']) {
        Some(i) => p[..i].to_string(),
        None => ".".to_string(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct TailscaleEnv {
    pub program_w6432: Option<String>,
    pub program_files: Option<String>,
    pub program_files_x86: Option<String>,
    pub path: Option<String>,
}

impl TailscaleEnv {
    pub fn from_process_env() -> Self {
        TailscaleEnv {
            program_w6432: std::env::var("ProgramW6432").ok(),
            program_files: std::env::var("ProgramFiles").ok(),
            program_files_x86: std::env::var("ProgramFiles(x86)").ok(),
            path: std::env::var("PATH").or_else(|_| std::env::var("Path")).ok(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriedPath {
    pub path: String,
    pub source: &'static str,
    pub found: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocateResult {
    pub cli: Option<String>,
    pub tried: Vec<TriedPath>,
}

fn check(tried: &mut Vec<TriedPath>, p: Option<String>, source: &'static str, exists: &impl Fn(&str) -> bool) -> Option<String> {
    let p = p?;
    if p.is_empty() || tried.iter().any(|t| t.path.eq_ignore_ascii_case(&p)) {
        return None;
    }
    let found = exists(&p);
    tried.push(TriedPath { path: p.clone(), source, found });
    found.then_some(p)
}

/// Find Tailscale's CLI (`tailscale.exe`, installed next to the tray app and the service). Tries, in
/// order: the standard install folders, the folder of the Tailscale Windows service (its registered
/// `ImagePath`, readable without admin rights), folders given as hints (e.g. where the Start menu's
/// Tailscale app lives), and `PATH`. Returns what it tried too, so Settings can show where it looked.
///
/// `exists` and `query` (a registry lookup: `key, value -> Option<value>`) are injected so this needs no
/// real filesystem/registry access to test.
pub fn locate_cli(env: &TailscaleEnv, hints: &[String], is_windows: bool, exists: impl Fn(&str) -> bool, query: impl Fn(&str, &str) -> Option<String>) -> LocateResult {
    let mut tried: Vec<TriedPath> = Vec::new();

    if !is_windows {
        for p in ["/usr/bin/tailscale", "/usr/local/bin/tailscale", "/Applications/Tailscale.app/Contents/MacOS/Tailscale"] {
            if let Some(hit) = check(&mut tried, Some(p.to_string()), "default", &exists) {
                return LocateResult { cli: Some(hit), tried };
            }
        }
        return LocateResult { cli: None, tried };
    }

    for base in [&env.program_w6432, &env.program_files, &env.program_files_x86].into_iter().flatten() {
        let p = win_join(&[base, "Tailscale", "tailscale.exe"]);
        if let Some(hit) = check(&mut tried, Some(p), "default", &exists) {
            return LocateResult { cli: Some(hit), tried };
        }
    }
    let image = query("HKLM\\SYSTEM\\CurrentControlSet\\Services\\Tailscale", "ImagePath").and_then(|cmd| exe_of_command(&cmd));
    if let Some(image) = image {
        let p = win_join(&[&win_dirname(&image), "tailscale.exe"]);
        if let Some(hit) = check(&mut tried, Some(p), "service", &exists) {
            return LocateResult { cli: Some(hit), tried };
        }
    }
    for dir in hints.iter().filter(|d| !d.is_empty()) {
        let p = win_join(&[dir, "tailscale.exe"]);
        if let Some(hit) = check(&mut tried, Some(p), "startMenu", &exists) {
            return LocateResult { cli: Some(hit), tried };
        }
    }
    let path_env = env.path.clone().unwrap_or_default();
    for dir in path_env.split(';').filter(|d| !d.is_empty()) {
        let p = win_join(&[dir, "tailscale.exe"]);
        if !exists(&p) {
            continue;
        }
        let hit = check(&mut tried, Some(p), "path", &exists);
        return LocateResult { cli: hit, tried };
    }
    LocateResult { cli: None, tried }
}

/// Quick synchronous guess at startup (the standard folders); [`locate_cli`] does the full search.
pub fn find_cli(env: &TailscaleEnv, is_windows: bool, exists: impl Fn(&str) -> bool) -> Option<String> {
    let candidates: Vec<String> = if is_windows {
        [&env.program_w6432, &env.program_files, &env.program_files_x86].into_iter().flatten().map(|p| win_join(&[p, "Tailscale", "tailscale.exe"])).collect()
    } else {
        ["/usr/bin/tailscale", "/usr/local/bin/tailscale", "/Applications/Tailscale.app/Contents/MacOS/Tailscale"].into_iter().map(String::from).collect()
    };
    candidates.into_iter().find(|c| exists(c))
}

fn short_name(p: &Value) -> String {
    let dns = p.get("DNSName").and_then(Value::as_str).unwrap_or("");
    let host = p.get("HostName").and_then(Value::as_str).unwrap_or("");
    let from_dns = if dns.is_empty() { "" } else { dns.split('.').next().unwrap_or("") };
    if from_dns.is_empty() {
        host.to_string()
    } else {
        from_dns.to_string()
    }
}

fn first_ipv4_or_any(ips: &[&str]) -> Option<String> {
    ips.iter().find(|ip| ip.contains('.')).or_else(|| ips.first()).map(|s| s.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitNodeInfo {
    pub name: String,
    pub ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitNodeOption {
    pub name: String,
    pub ip: Option<String>,
    pub online: bool,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsStatus {
    pub state: &'static str,
    pub tailnet: Option<String>,
    pub user: Option<String>,
    pub host_name: Option<String>,
    pub ip: Option<String>,
    pub auth_url: Option<String>,
    pub peers_online: usize,
    pub peers_total: usize,
    pub exit_node: Option<ExitNodeInfo>,
    pub exit_nodes: Vec<ExitNodeOption>,
}

/// Shape `tailscale status --json` for the UI. `state` is one of "connected" | "stopped" |
/// "needsLogin" | "starting" | "unknown".
pub fn parse_status(json: &Value) -> TsStatus {
    let backend = json.get("BackendState").and_then(Value::as_str).unwrap_or("");
    let state = match backend {
        "Running" => "connected",
        "Stopped" => "stopped",
        "NeedsLogin" | "NeedsMachineAuth" => "needsLogin",
        "Starting" | "NoState" => "starting",
        _ => "unknown",
    };
    let empty = json!({});
    let peers: Vec<&Value> = json.get("Peer").and_then(Value::as_object).map(|m| m.values().collect()).unwrap_or_default();
    let exit_node_peer = peers.iter().find(|p| p.get("ExitNode").and_then(Value::as_bool).unwrap_or(false));
    let self_ = json.get("Self").unwrap_or(&empty);
    let self_ips: Vec<&str> = self_.get("TailscaleIPs").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();

    let user = self_.get("UserID").and_then(|uid| {
        let key = uid.as_i64().map(|n| n.to_string()).or_else(|| uid.as_str().map(String::from))?;
        json.get("User").and_then(Value::as_object)?.get(&key)?.get("LoginName")?.as_str().map(String::from)
    });

    let mut exit_nodes: Vec<ExitNodeOption> = peers
        .iter()
        .filter(|p| p.get("ExitNodeOption").and_then(Value::as_bool).unwrap_or(false))
        .map(|p| {
            let ips: Vec<&str> = p.get("TailscaleIPs").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            ExitNodeOption { name: short_name(p), ip: first_ipv4_or_any(&ips), online: p.get("Online").and_then(Value::as_bool).unwrap_or(false), active: p.get("ExitNode").and_then(Value::as_bool).unwrap_or(false) }
        })
        .collect();
    exit_nodes.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.name.cmp(&b.name)));

    let host_name = { let h = short_name(self_); if h.is_empty() { None } else { Some(h) } };

    TsStatus {
        state,
        tailnet: json.get("CurrentTailnet").and_then(|t| t.get("Name")).and_then(Value::as_str).map(String::from),
        user,
        host_name,
        ip: first_ipv4_or_any(&self_ips),
        auth_url: json.get("AuthURL").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from),
        peers_online: peers.iter().filter(|p| p.get("Online").and_then(Value::as_bool).unwrap_or(false)).count(),
        peers_total: peers.len(),
        exit_node: exit_node_peer.map(|p| {
            let ips: Vec<&str> = p.get("TailscaleIPs").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            ExitNodeInfo { name: short_name(p), ip: ips.first().map(|s| s.to_string()) }
        }),
        exit_nodes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn running_status() -> Value {
        json!({
            "BackendState": "Running",
            "AuthURL": "",
            "Self": {"HostName": "LEGION-GO", "DNSName": "legion-go.tail1234.ts.net.", "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1"], "UserID": 42, "Online": true},
            "User": {"42": {"LoginName": "thomas@example.com"}},
            "CurrentTailnet": {"Name": "thomas@example.com"},
            "Peer": {
                "nodekey:a": {"HostName": "nas", "DNSName": "nas.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.2"], "Online": true, "ExitNodeOption": true, "ExitNode": true},
                "nodekey:b": {"HostName": "Pixel 8", "DNSName": "pixel-8.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.3"], "Online": false},
                "nodekey:c": {"HostName": "vps", "DNSName": "vps.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.4"], "Online": false, "ExitNodeOption": true},
            },
        })
    }

    // Parity with the JS test "tailscale status: connected, with an exit node in use".
    #[test]
    fn status_connected_with_an_exit_node_in_use() {
        let s = parse_status(&running_status());
        assert_eq!(s.state, "connected");
        assert_eq!(s.host_name.as_deref(), Some("legion-go"));
        assert_eq!(s.ip.as_deref(), Some("100.101.102.103"));
        assert_eq!(s.user.as_deref(), Some("thomas@example.com"));
        assert_eq!(s.peers_online, 1);
        assert_eq!(s.peers_total, 3);
        assert_eq!(s.exit_node, Some(ExitNodeInfo { name: "nas".into(), ip: Some("100.64.0.2".into()) }));
        assert_eq!(s.exit_nodes.iter().map(|n| (n.name.as_str(), n.online, n.active)).collect::<Vec<_>>(), vec![("nas", true, true), ("vps", false, false)]);
    }

    // Parity with the JS test "tailscale status: stopped and signed out".
    #[test]
    fn status_stopped_and_signed_out() {
        assert_eq!(parse_status(&json!({"BackendState": "Stopped", "Self": {}})).state, "stopped");
        let out = parse_status(&json!({"BackendState": "NeedsLogin", "AuthURL": "https://login.tailscale.com/a/abc123", "Self": {}}));
        assert_eq!(out.state, "needsLogin");
        assert_eq!(out.auth_url.as_deref(), Some("https://login.tailscale.com/a/abc123"));
        assert_eq!(out.exit_node, None);
    }

    // Parity with the JS test "finds the sign-in URL in `tailscale up` output".
    #[test]
    fn finds_the_sign_in_url_in_tailscale_up_output() {
        let text = "\nTo authenticate, visit:\n\n\thttps://login.tailscale.com/a/1a2b3c4d5e6f\n\n";
        assert_eq!(find_auth_url(text).as_deref(), Some("https://login.tailscale.com/a/1a2b3c4d5e6f"));
    }

    // Parity with the JS test "finding tailscale.exe: standard folders, the service, the Start menu app, PATH".
    #[test]
    fn finding_tailscale_exe_standard_folders_service_start_menu_path() {
        let env = TailscaleEnv { program_w6432: Some(r"C:\Program Files".into()), program_files: Some(r"C:\Program Files".into()), program_files_x86: Some(r"C:\Program Files (x86)".into()), path: Some(r"C:\Windows;D:\Tools\TS".into()) };
        let at = |files: &'static [&'static str]| move |p: &str| files.iter().any(|f| f.eq_ignore_ascii_case(p));
        let no_service = |_: &str, _: &str| None;

        let r = locate_cli(&env, &[], true, at(&[r"C:\Program Files\Tailscale\tailscale.exe"]), no_service);
        assert_eq!(r.cli.as_deref(), Some(r"C:\Program Files\Tailscale\tailscale.exe"));

        // Installed elsewhere: the service's registered program path leads to it.
        let service = |key: &str, value: &str| if key.ends_with(r"Services\Tailscale") && value == "ImagePath" { Some(r#""E:\Apps\Tailscale\tailscaled.exe""#.to_string()) } else { None };
        let r = locate_cli(&env, &[], true, at(&[r"E:\Apps\Tailscale\tailscale.exe"]), service);
        assert_eq!(r.cli.as_deref(), Some(r"E:\Apps\Tailscale\tailscale.exe"));
        assert_eq!(r.tried.iter().map(|t| t.source).collect::<Vec<_>>(), vec!["default", "default", "service"]);

        // No service entry: the Start menu app's folder, then PATH.
        let r = locate_cli(&env, &["F:\\TS".to_string()], true, at(&[r"F:\TS\tailscale.exe"]), no_service);
        assert_eq!(r.cli.as_deref(), Some(r"F:\TS\tailscale.exe"));
        let r = locate_cli(&env, &[], true, at(&[r"D:\Tools\TS\tailscale.exe"]), no_service);
        assert_eq!(r.cli.as_deref(), Some(r"D:\Tools\TS\tailscale.exe"));
        assert_eq!(r.tried.last().unwrap().source, "path");

        // Not installed: nothing found, and the places tried are reported.
        let r = locate_cli(&env, &[], true, |_| false, no_service);
        assert_eq!(r.cli, None);
        assert_eq!(r.tried.iter().map(|t| t.path.as_str()).collect::<Vec<_>>(), vec![r"C:\Program Files\Tailscale\tailscale.exe", r"C:\Program Files (x86)\Tailscale\tailscale.exe"]);
    }

    #[test]
    fn exe_of_command_handles_quoted_and_bare_paths() {
        assert_eq!(exe_of_command(r#""C:\x y\a.exe" -arg"#).as_deref(), Some(r"C:\x y\a.exe"));
        assert_eq!(exe_of_command(r"C:\x\a.exe -arg").as_deref(), Some(r"C:\x\a.exe"));
        assert_eq!(exe_of_command(r"\??\C:\x\a.exe").as_deref(), Some(r"C:\x\a.exe"));
        assert_eq!(exe_of_command(""), None);
    }
}
