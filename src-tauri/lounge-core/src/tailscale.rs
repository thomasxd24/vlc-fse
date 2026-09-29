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
use std::time::Duration;

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

// ---------------------------------------------------------------------------
// The Tailscale driver: locating the CLI, status, up/down/exit-node, sign-in

/// [`TsStatus`] as the UI's `tailscaleStatus()` payload, merged under `"installed": true`.
pub fn status_json(s: &TsStatus) -> Value {
    json!({
        "installed": true,
        "state": s.state,
        "tailnet": s.tailnet,
        "user": s.user,
        "hostName": s.host_name,
        "ip": s.ip,
        "authUrl": s.auth_url,
        "peersOnline": s.peers_online,
        "peersTotal": s.peers_total,
        "exitNode": s.exit_node.as_ref().map(|e| json!({"name": e.name, "ip": e.ip})),
        "exitNodes": s.exit_nodes.iter().map(|e| json!({"name": e.name, "ip": e.ip, "online": e.online, "active": e.active})).collect::<Vec<_>>(),
    })
}

/// Run the CLI and capture its stdout, with a timeout. On failure the error is the first line of
/// stderr/stdout (or the IO error's message), like the JS `run()`'s `err.message` rewrite.
fn run(cli: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = std::process::Command::new(cli)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = child.stdout.take().map(|mut s| {
                    use std::io::Read;
                    let mut buf = String::new();
                    let _ = s.read_to_string(&mut buf);
                    buf
                });
                let err_out = child.stderr.take().map(|mut s| {
                    use std::io::Read;
                    let mut buf = String::new();
                    let _ = s.read_to_string(&mut buf);
                    buf
                });
                if status.success() {
                    return Ok(out.unwrap_or_default());
                }
                let message = [err_out.unwrap_or_default(), out.unwrap_or_default()].into_iter().find(|s| !s.trim().is_empty()).map(|s| s.lines().next().unwrap_or("").trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("tailscale exited with code {}", status.code().unwrap_or(-1)));
                return Err(message);
            }
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("tailscale timed out".into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn file_exists(p: &str) -> bool {
    std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn registry_value(key: &str, value: &str) -> Option<String> {
    let output = std::process::Command::new("reg").args(["query", key, "/v", value]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let re = Regex::new(&format!(r"{value}\s+REG_\w+\s+(.*)")).ok()?;
    let caps = re.captures(stdout.as_ref()).ok().flatten()?;
    Some(caps.get(1)?.as_str().trim().to_string())
}
#[cfg(not(target_os = "windows"))]
fn registry_value(_key: &str, _value: &str) -> Option<String> {
    None
}

/// The final outcome of a sign-in attempt: `Ok(Some(url))` when a URL was printed, `Ok(None)` when we
/// were already signed in, `Err(message)` on failure.
pub type LoginOutcome = Result<Option<String>, String>;

/// One sign-in attempt: `tailscale up` waiting for the user to complete sign-in in a browser.
pub struct LoginHandle {
    url: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// Everything `tailscale up` printed, for the failure message's last line (like the JS `buf`).
    output: std::sync::Arc<std::sync::Mutex<String>>,
    outcome: std::sync::Arc<std::sync::Mutex<Option<LoginOutcome>>>,
    child: std::sync::Arc<std::sync::Mutex<Option<std::process::Child>>>,
}

impl LoginHandle {
    /// The sign-in URL once `tailscale up` prints one (also what a second `startLogin` returns).
    pub fn url(&self) -> Option<String> {
        self.url.lock().unwrap().clone()
    }

    /// The final outcome (see [`LoginOutcome`]): `None` while the attempt is still running.
    pub fn outcome(&self) -> Option<LoginOutcome> {
        self.outcome.lock().unwrap().clone()
    }
}

/// The live state behind the app's tailscale commands: which CLI was found, where we looked, and any
/// in-flight sign-in. Mirrors the JS `Tailscale` class.
pub struct Tailscale {
    env: TailscaleEnv,
    inner: std::sync::Mutex<TailscaleState>,
}

struct TailscaleState {
    cli: Option<String>,
    tried: Vec<TriedPath>,
    login: Option<std::sync::Arc<LoginHandle>>,
}

impl Tailscale {
    /// Quick synchronous guess at startup (the standard folders); [`Tailscale::locate`] does the full search.
    pub fn new() -> Self {
        let env = TailscaleEnv::from_process_env();
        let cli = find_cli(&env, cfg!(target_os = "windows"), file_exists);
        Tailscale { env, inner: std::sync::Mutex::new(TailscaleState { cli, tried: Vec::new(), login: None }) }
    }

    /// A driver already pointed at a known CLI (used by tests, and anywhere a CLI was found by other means).
    pub fn with_cli(cli: Option<String>) -> Self {
        Tailscale { env: TailscaleEnv::default(), inner: std::sync::Mutex::new(TailscaleState { cli, tried: Vec::new(), login: None }) }
    }

    pub fn installed(&self) -> bool {
        self.inner.lock().unwrap().cli.is_some()
    }

    pub fn cli(&self) -> Option<String> {
        self.inner.lock().unwrap().cli.clone()
    }

    pub fn tried(&self) -> Vec<TriedPath> {
        self.inner.lock().unwrap().tried.clone()
    }

    /// Search everywhere Tailscale might be (see [`locate_cli`]); `hints` are extra folders to try.
    pub fn locate(&self, hints: &[String]) -> LocateResult {
        let is_windows = cfg!(target_os = "windows");
        let r = locate_cli(&self.env, hints, is_windows, file_exists, registry_value);
        let mut inner = self.inner.lock().unwrap();
        inner.cli = r.cli.clone();
        inner.tried = r.tried.clone();
        r
    }

    /// Shape `tailscale status --json` for the UI; an unreachable daemon becomes
    /// `{"installed": true, "state": "unknown", "error": ...}`.
    pub fn status(&self) -> Value {
        let Some(cli) = self.cli() else {
            return json!({"installed": false});
        };
        match run(&cli, &["status", "--json"], Duration::from_secs(15)) {
            Ok(out) => {
                let mut v = match serde_json::from_str::<Value>(&out) {
                    Ok(parsed) => status_json(&parse_status(&parsed)),
                    Err(_) => json!({"installed": true, "state": "unknown"}),
                };
                v["installed"] = json!(true);
                v
            }
            // "failed to connect to local tailscaled" when the service isn't running.
            Err(e) => json!({"installed": true, "state": "unknown", "error": e}),
        }
    }

    pub fn up(&self) -> Result<(), String> {
        let cli = self.cli().ok_or_else(|| "not installed".to_string())?;
        run(&cli, &["up"], Duration::from_secs(30)).map(|_| ())
    }

    pub fn down(&self) -> Result<(), String> {
        let cli = self.cli().ok_or_else(|| "not installed".to_string())?;
        run(&cli, &["down"], Duration::from_secs(15)).map(|_| ())
    }

    /// Route all traffic through a peer (its IP or name), or stop using an exit node (`None`).
    pub fn set_exit_node(&self, node: Option<&str>) -> Result<(), String> {
        let cli = self.cli().ok_or_else(|| "not installed".to_string())?;
        run(&cli, &["set", &format!("--exit-node={}", node.unwrap_or(""))], Duration::from_secs(15)).map(|_| ())
    }

    /// Start signing in: runs `tailscale up`, which prints a login URL and waits until the sign-in
    /// completes in a browser. Returns a handle to the running attempt (the existing one, if a login
    /// is already in flight); poll [`LoginHandle::url`] / [`LoginHandle::outcome`].
    pub fn start_login(&self) -> std::sync::Arc<LoginHandle> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(existing) = &inner.login {
            return std::sync::Arc::clone(existing);
        }
        let Some(cli) = inner.cli.clone() else {
            let handle = std::sync::Arc::new(LoginHandle {
                url: std::sync::Arc::new(std::sync::Mutex::new(None)),
                output: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
                outcome: std::sync::Arc::new(std::sync::Mutex::new(Some(Err("not installed".into())))),
                child: std::sync::Arc::new(std::sync::Mutex::new(None)),
            });
            return handle;
        };
        // Plain `up`: any settings flag would make Tailscale insist on restating every non-default setting.
        let child = std::process::Command::new(&cli)
            .arg("up")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let child = match child {
            Ok(c) => c,
            Err(e) => {
                let handle = std::sync::Arc::new(LoginHandle {
                    url: std::sync::Arc::new(std::sync::Mutex::new(None)),
                    output: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
                    outcome: std::sync::Arc::new(std::sync::Mutex::new(Some(Err(e.to_string())))),
                    child: std::sync::Arc::new(std::sync::Mutex::new(None)),
                });
                return handle;
            }
        };
        let handle = std::sync::Arc::new(LoginHandle {
            url: std::sync::Arc::new(std::sync::Mutex::new(None)),
            output: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            outcome: std::sync::Arc::new(std::sync::Mutex::new(None)),
            child: std::sync::Arc::new(std::sync::Mutex::new(Some(child))),
        });
        inner.login = Some(std::sync::Arc::clone(&handle));
        drop(inner);

        // Both output streams are scanned for the URL, like the JS `scan` on stdout+stderr.
        fn spawn_reader(pipe: Box<dyn std::io::Read + Send>, url_cell: std::sync::Arc<std::sync::Mutex<Option<String>>>, output_cell: std::sync::Arc<std::sync::Mutex<String>>) {
            std::thread::spawn(move || {
                let mut reader = std::io::BufReader::new(pipe);
                let mut line = String::new();
                loop {
                    line.clear();
                    match std::io::BufRead::read_line(&mut reader, &mut line) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    output_cell.lock().unwrap().push_str(&line);
                    let mut url = url_cell.lock().unwrap();
                    if url.is_none() {
                        if let Some(found) = find_auth_url(&line) {
                            *url = Some(found);
                        }
                    }
                }
            });
        }
        let mut child_guard = handle.child.lock().unwrap();
        let stdout = child_guard.as_mut().and_then(|c| c.stdout.take());
        let stderr = child_guard.as_mut().and_then(|c| c.stderr.take());
        if let Some(out) = stdout {
            spawn_reader(Box::new(out), std::sync::Arc::clone(&handle.url), std::sync::Arc::clone(&handle.output));
        }
        if let Some(err_pipe) = stderr {
            spawn_reader(Box::new(err_pipe), std::sync::Arc::clone(&handle.url), std::sync::Arc::clone(&handle.output));
        }
        drop(child_guard);
        let watch_child = std::sync::Arc::clone(&handle.child);
        let watch_url = std::sync::Arc::clone(&handle.url);
        let watch_outcome = std::sync::Arc::clone(&handle.outcome);
        let watch_output = std::sync::Arc::clone(&handle.output);
        std::thread::spawn(move || {
            // Watch in place (never taking the child), so `cancel_login` can kill the same process.
            let mut code: Option<i32> = None;
            loop {
                std::thread::sleep(Duration::from_millis(50));
                let observed = match watch_child.lock().unwrap().as_mut() {
                    None => break,
                    Some(c) => match c.try_wait() {
                        Ok(Some(status)) => status.code(),
                        Ok(None) => continue,
                        Err(_) => None,
                    },
                };
                code = observed;
                break;
            }
            *watch_child.lock().unwrap() = None;
            let url = watch_url.lock().unwrap().clone();
            let result = match (&url, code) {
                (Some(u), _) => Ok(Some(u.clone())),
                (None, Some(0)) => Ok(None),
                (None, code) => {
                    let last_line = watch_output.lock().unwrap().lines().rev().map(|l| l.trim()).find(|l| !l.is_empty()).map(String::from);
                    Err(last_line.unwrap_or_else(|| format!("tailscale exited with code {}", code.unwrap_or(-1))))
                }
            };
            let mut o = watch_outcome.lock().unwrap();
            if o.is_none() {
                *o = Some(result);
            }
        });
        handle
    }

    /// The running sign-in attempt, if any (its outcome is polled by the shell's event router).
    pub fn pending_login(&self) -> Option<std::sync::Arc<LoginHandle>> {
        self.inner.lock().unwrap().login.clone()
    }

    /// Drop the finished attempt so the next `start_login` starts fresh.
    pub fn clear_login_handle(&self, handle: &std::sync::Arc<LoginHandle>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(current) = &inner.login {
            if std::sync::Arc::ptr_eq(current, handle) {
                inner.login = None;
            }
        }
    }

    /// Stop an in-flight sign-in attempt. The attempt's watcher observes the kill and resolves.
    pub fn cancel_login(&self) {
        let child = self.inner.lock().unwrap().login.take();
        if let Some(handle) = child {
            if let Some(c) = handle.child.lock().unwrap().as_mut() {
                let _ = c.kill();
            }
        }
    }
}

impl Default for Tailscale {
    fn default() -> Self {
        Self::new()
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

    // ---- the Tailscale driver

    use std::time::Duration;
    use tempfile::tempdir;

    /// A fake CLI the driver can actually spawn: a shell script on Unix, a batch file on Windows
    /// (Rust runs `.cmd` through cmd.exe; a text file can't be executed as an `.exe` there). Each
    /// test provides both bodies, which express the same behaviour.
    #[cfg(unix)]
    fn fake_cli(dir: &std::path::Path, unix_body: &str, _windows_body: &str) -> String {
        let exe = dir.join("tailscale");
        std::fs::write(&exe, format!("#!/bin/sh\n{unix_body}\n")).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        exe.to_string_lossy().into_owned()
    }

    #[cfg(windows)]
    fn fake_cli(dir: &std::path::Path, _unix_body: &str, windows_body: &str) -> String {
        let exe = dir.join("tailscale.cmd");
        std::fs::write(&exe, format!("@echo off\r\n{windows_body}\r\n")).unwrap();
        exe.to_string_lossy().into_owned()
    }

    fn poll<F: Fn() -> Option<T>, T>(f: F) -> Option<T> {
        for _ in 0..200 {
            if let Some(v) = f() {
                return Some(v);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn status_without_a_cli_reports_not_installed() {
        let ts = Tailscale::with_cli(None);
        assert!(!ts.installed());
        assert_eq!(ts.status(), json!({"installed": false}));
    }

    #[test]
    fn status_runs_the_cli_and_shapes_the_result() {
        let dir = tempdir().unwrap();
        let cli = fake_cli(
            dir.path(),
            r#"if [ "$1" = "status" ]; then echo '{"BackendState":"Running","CurrentTailnet":{"Name":"example.com"},"Self":{"HostName":"box.example.com","TailscaleIPs":["100.64.0.1","fd7a::1"],"UserID":11},"User":{"11":{"LoginName":"me@example.com"}},"Peer":{"1":{"HostName":"peer.example.com","Online":true,"ExitNodeOption":true,"TailscaleIPs":["100.64.0.2"]}}}'; fi"#,
            r#"if "%~1"=="status" echo {"BackendState":"Running","CurrentTailnet":{"Name":"example.com"},"Self":{"HostName":"box.example.com","TailscaleIPs":["100.64.0.1","fd7a::1"],"UserID":11},"User":{"11":{"LoginName":"me@example.com"}},"Peer":{"1":{"HostName":"peer.example.com","Online":true,"ExitNodeOption":true,"TailscaleIPs":["100.64.0.2"]}}}"#,
        );
        let ts = Tailscale::with_cli(Some(cli));
        let st = ts.status();
        assert_eq!(st["installed"], json!(true));
        assert_eq!(st["state"], json!("connected"));
        assert_eq!(st["hostName"], json!("box.example.com"), "no DNSName: the HostName stands");
        assert_eq!(st["user"], json!("me@example.com"));
        assert_eq!(st["ip"], json!("100.64.0.1"));
        assert_eq!(st["peersOnline"], json!(1));
        assert_eq!(st["exitNodes"][0]["name"], json!("peer.example.com"));
    }

    #[test]
    fn an_unreachable_daemon_is_an_unknown_state_with_the_error() {
        let dir = tempdir().unwrap();
        let cli = fake_cli(dir.path(), "echo failed to connect to local tailscaled >&2; exit 1", "echo failed to connect to local tailscaled 1>&2\r\nexit /b 1");
        let ts = Tailscale::with_cli(Some(cli));
        let st = ts.status();
        assert_eq!(st["installed"], json!(true));
        assert_eq!(st["state"], json!("unknown"));
        assert!(st["error"].as_str().unwrap().contains("failed to connect"));
    }

    #[test]
    fn up_down_and_exit_node_pass_the_expected_arguments() {
        let dir = tempdir().unwrap();
        let log = dir.path().join("args.log");
        let log_display = log.to_string_lossy().into_owned();
        // Both fakes log one argument per line. (The batch loop's `%~1>>` is safe even when the
        // argument ends in a digit: cmd picks redirection targets before expanding `%~1`.)
        #[cfg(unix)]
        {
            let cli_path = dir.path().join("tailscale");
            std::fs::write(&cli_path, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{log_display}'\n")).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cli_path, std::fs::Permissions::from_mode(0o755)).unwrap();
            let ts = Tailscale::with_cli(Some(cli_path.to_string_lossy().into_owned()));
            ts.up().unwrap();
            ts.down().unwrap();
            ts.set_exit_node(Some("100.64.0.2")).unwrap();
            ts.set_exit_node(None).unwrap();
            let logged = std::fs::read_to_string(&log).unwrap();
            assert_eq!(logged, "up\ndown\nset\n--exit-node=100.64.0.2\nset\n--exit-node=\n");
        }
        #[cfg(windows)]
        {
            let cli_path = dir.path().join("tailscale.cmd");
            std::fs::write(
                &cli_path,
                format!("@echo off\r\n:next\r\nif \"%~1\"==\"\" goto :done\r\necho %~1>>\"{log_display}\"\r\nshift\r\ngoto :next\r\n:done\r\n"),
            )
            .unwrap();
            let ts = Tailscale::with_cli(Some(cli_path.to_string_lossy().into_owned()));
            ts.up().unwrap();
            ts.down().unwrap();
            ts.set_exit_node(Some("100.64.0.2")).unwrap();
            ts.set_exit_node(None).unwrap();
            let logged = std::fs::read_to_string(&log).unwrap();
            assert_eq!(logged, "up\r\ndown\r\nset\r\n--exit-node=100.64.0.2\r\nset\r\n--exit-node=\r\n");
        }
    }

    #[test]
    fn start_login_finds_the_url_and_cancel_stops_it() {
        let dir = tempdir().unwrap();
        // The trailing wait keeps the process alive until cancel kills it; on Windows ping's own
        // output goes to nul so the killed cmd.exe is the only holder of the output pipes.
        let cli = fake_cli(dir.path(), "echo 'https://login.tailscale.com/a/abc123'; sleep 30", "echo https://login.tailscale.com/a/abc123\r\nping -n 31 127.0.0.1 > nul");
        let ts = Tailscale::with_cli(Some(cli));

        let handle = ts.start_login();
        let url = poll(|| handle.url());
        assert_eq!(url.as_deref(), Some("https://login.tailscale.com/a/abc123"));
        assert!(handle.outcome().is_none(), "still waiting for sign-in");

        ts.cancel_login();
        let outcome = poll(|| handle.outcome());
        assert!(outcome.is_some(), "cancel resolves the attempt");
        // A second login after cancel starts fresh.
        let dir2 = tempdir().unwrap();
        let ts2 = Tailscale::with_cli(Some(fake_cli(dir2.path(), "exit 0", "exit /b 0")));
        let h2 = ts2.start_login();
        assert_eq!(poll(|| h2.outcome()), Some(Ok(None)), "exit 0 with no URL means already signed in");
    }

    #[test]
    fn an_in_flight_login_is_returned_to_a_second_caller() {
        let dir = tempdir().unwrap();
        let cli = fake_cli(dir.path(), "echo 'https://login.tailscale.com/a/xyz789'; sleep 30", "echo https://login.tailscale.com/a/xyz789\r\nping -n 31 127.0.0.1 > nul");
        let ts = Tailscale::with_cli(Some(cli));
        let first = ts.start_login();
        let second = ts.start_login();
        assert!(std::sync::Arc::ptr_eq(&first, &second), "the same attempt, not a second process");
        ts.cancel_login();
    }

    #[test]
    fn a_failed_login_carries_the_cli_output_line() {
        let dir = tempdir().unwrap();
        let cli = fake_cli(dir.path(), "echo 'not logged in' >&2; exit 3", "echo not logged in 1>&2\r\nexit /b 3");
        let ts = Tailscale::with_cli(Some(cli));
        let handle = ts.start_login();
        assert_eq!(poll(|| handle.outcome()), Some(Err("not logged in".into())));
    }
}
