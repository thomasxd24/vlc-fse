//! Locating VLC, building its command line, and driving a playback session. Direct port of
//! `src/vlc.js`, including `VlcSession` (spawns VLC with its HTTP interface, polls
//! `status.json`/`playlist.json` and maps VLC's idea of the current item back to the queue).
//!
//! Where the JS version is an `EventEmitter`, this hands back a `std::sync::mpsc::Receiver` of
//! [`VlcEvent`]s, drained by the caller (the Tauri `app` crate turns them into `now-playing` pushes
//! and progress records). Everything else follows the JS structure one-to-one; the pure helpers
//! (`resolve_current`, playlist flattening, the previous-item-finished rule) are split out so they're
//! testable without a real VLC — the one thing that can't be exercised here (`test/vlc-real.test.js`
//! needs an installed VLC) is the live spawn/poll loop itself.

#[cfg(target_os = "windows")]
use fancy_regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// VLC matches track languages against ISO 639 codes; listing both forms catches files tagged either way.
static LANG_CODES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([("en", "eng,en"), ("fr", "fre,fra,fr"), ("de", "ger,deu,de"), ("es", "spa,es"), ("it", "ita,it"), ("ja", "jpn,ja")])
});

#[derive(Debug, Clone, Default)]
pub struct Languages<'a> {
    pub audio: Option<&'a str>,
    pub subs: Option<&'a str>,
}

/// VLC options for the preferred audio and subtitle languages. `audio` is a language code or
/// `"original"` (the file's default track); `subs` is a language code, or `"off"` to start without
/// subtitles.
pub fn language_args(languages: &Languages, prefix: &str) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(codes) = languages.audio.and_then(|a| LANG_CODES.get(a)) {
        args.push(format!("{prefix}audio-language={codes},any"));
    } else if languages.audio == Some("original") {
        // Lets one show opt out of a global preference.
        args.push(format!("{prefix}audio-language=any"));
    }
    if languages.subs == Some("off") {
        args.push(format!("{prefix}sub-language=none"));
    } else if let Some(codes) = languages.subs.and_then(|s| LANG_CODES.get(s)) {
        args.push(format!("{prefix}sub-language={codes}"));
    }
    args
}

#[derive(Debug, Clone, Default)]
pub struct QueueItem<'a> {
    pub path: &'a str,
    pub start_time: Option<f64>,
    pub languages: Option<Languages<'a>>,
}

#[derive(Debug, Clone)]
pub struct BuildArgsOptions<'a> {
    pub port: u16,
    pub password: &'a str,
    pub fullscreen: bool,
    pub languages: Languages<'a>,
    pub extra_args: &'a [&'a str],
}

impl<'a> Default for BuildArgsOptions<'a> {
    fn default() -> Self {
        BuildArgsOptions { port: 0, password: "", fullscreen: true, languages: Languages::default(), extra_args: &[] }
    }
}

/// Build VLC's command line. Each queue item may carry a start offset, applied as a per-item
/// `:start-time` option, and its own languages (e.g. a show whose tracks were picked by hand), applied
/// as per-item language options.
pub fn build_args(queue: &[QueueItem], opts: &BuildArgsOptions) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--extraintf=http".into(),
        "--http-host=127.0.0.1".into(),
        format!("--http-port={}", opts.port),
        format!("--http-password={}", opts.password),
        "--play-and-exit".into(),
        "--no-random".into(),
        "--no-loop".into(),
        "--no-repeat".into(),
        "--qt-continue=0".into(),          // this launcher owns resume; don't let VLC ask as well
        "--no-qt-privacy-ask".into(),      // VLC's first-run network-policy dialog would otherwise block fullscreen playback
        "--no-one-instance-when-started-from-file".into(), // on/off options take --no-, never =0 (VLC refuses to start)
        "--no-one-instance".into(),
    ];
    if opts.fullscreen {
        args.push("--fullscreen".into());
    }
    args.extend(language_args(&opts.languages, "--")); // before extra_args, so the user's own options still win
    args.extend(opts.extra_args.iter().map(|s| s.to_string()));
    for item in queue {
        args.push(item.path.to_string());
        if let Some(t) = item.start_time {
            if t > 5.0 {
                args.push(format!(":start-time={}", t.floor() as i64));
            }
        }
        if let Some(langs) = &item.languages {
            args.extend(language_args(langs, ":"));
        }
    }
    args
}

fn exists_file(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

fn which(cmd: &str, path_env: &str, is_windows: bool) -> Option<PathBuf> {
    let exts: &[&str] = if is_windows { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    for dir in std::env::split_paths(path_env) {
        for ext in exts {
            let p = dir.join(format!("{cmd}{ext}"));
            if exists_file(&p) {
                return Some(p);
            }
        }
    }
    None
}

/// Roots to check for a VideoLAN\VLC install under, on Windows (`%ProgramFiles%`,
/// `%ProgramFiles(x86)%`, `%ProgramW6432%`, `%LOCALAPPDATA%\Programs`) — kept explicit rather than read
/// from `std::env` inside `find_vlc` itself, same reasoning as `steam::SteamEnv`.
#[derive(Debug, Clone, Default)]
pub struct VlcEnv {
    pub path: String,
    pub program_files: Option<PathBuf>,
    pub program_files_x86: Option<PathBuf>,
    pub program_w6432: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub is_windows: bool,
    pub is_macos: bool,
}

impl VlcEnv {
    pub fn from_process_env() -> Self {
        VlcEnv {
            path: std::env::var("PATH").unwrap_or_default(),
            program_files: std::env::var_os("ProgramFiles").map(PathBuf::from),
            program_files_x86: std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
            program_w6432: std::env::var_os("ProgramW6432").map(PathBuf::from),
            local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            is_windows: cfg!(target_os = "windows"),
            is_macos: cfg!(target_os = "macos"),
        }
    }
}

/// Locate the VLC executable. Returns an absolute path or `None`.
pub fn find_vlc(preferred: Option<&Path>, env: &VlcEnv) -> Option<PathBuf> {
    if let Some(p) = preferred {
        if exists_file(p) {
            return Some(p.to_path_buf());
        }
    }

    if env.is_windows {
        #[cfg(target_os = "windows")]
        for key in ["HKLM\\SOFTWARE\\VideoLAN\\VLC", "HKLM\\SOFTWARE\\WOW6432Node\\VideoLAN\\VLC", "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\vlc.exe", "HKCU\\SOFTWARE\\VideoLAN\\VLC"] {
            if let Some(v) = registry_default_value(key) {
                if exists_file(&v) {
                    return Some(v);
                }
            }
        }
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(p) = &env.program_files {
            roots.push(p.clone());
        }
        if let Some(p) = &env.program_files_x86 {
            roots.push(p.clone());
        }
        if let Some(p) = &env.program_w6432 {
            roots.push(p.clone());
        }
        if let Some(p) = &env.local_app_data {
            roots.push(p.join("Programs"));
        }
        for r in roots {
            let p = r.join("VideoLAN").join("VLC").join("vlc.exe");
            if exists_file(&p) {
                return Some(p);
            }
        }
    } else if env.is_macos {
        let p = PathBuf::from("/Applications/VLC.app/Contents/MacOS/VLC");
        if exists_file(&p) {
            return Some(p);
        }
    }
    which("vlc", &env.path, env.is_windows)
}

#[cfg(target_os = "windows")]
fn registry_default_value(key: &str) -> Option<PathBuf> {
    let output = crate::hidden_command("reg").args(["query", key, "/ve"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let re = Regex::new(r"REG_SZ\s+(.+)").unwrap();
    let caps = re.captures(stdout.as_ref()).ok()??;
    Some(PathBuf::from(caps.get(1)?.as_str().trim()))
}
#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
fn registry_default_value(_key: &str) -> Option<PathBuf> {
    None
}

// ---------------------------------------------------------------------------
// VlcSession

const POLL_MS: u64 = 1500;

/// One playback-status report, as `progress` events carried in the JS version.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub path: String,
    pub time: f64,
    pub length: f64,
    pub paused: bool,
}

/// Events from a running session: roughly one [`VlcEvent::Progress`] per [`POLL_MS`] while playing,
/// and one [`VlcEvent::Exit`] when VLC closes.
#[derive(Debug, Clone)]
pub enum VlcEvent {
    Progress(Progress),
    Exit { code: Option<i32>, last: Option<Progress> },
}

struct SessionInner {
    port: u16,
    password: String,
    queue: Vec<String>,
    last: Option<Progress>,
    /// VLC's last reported state ("playing"/"paused"/"stopped"), as `this.state` in the JS version.
    state: Option<String>,
}

/// A single VLC playback session. `spawn` starts VLC and a poller thread; `events` yields
/// [`VlcEvent`]s, `command` drives the "Playing in VLC" screen's controls, `kill` stops playback.
pub struct VlcSession {
    inner: std::sync::Arc<std::sync::Mutex<SessionInner>>,
    child: std::sync::Arc<std::sync::Mutex<std::process::Child>>,
    events: std::sync::mpsc::Receiver<VlcEvent>,
}

/// VLC's basic-auth password: 24 hex characters like the JS version's `randomBytes(12)`. Entropy only
/// needs to defeat other local processes guessing; a hash of clock + pid + a process-wide counter is
/// plenty for a loopback-only interface.
/// The HTTP password for a session, generated by the shell before `spawn` (see [`random_password`]).
pub fn random_password_pub() -> String {
    random_password()
}

/// A free loopback port for VLC's HTTP interface, generated before `spawn`.
pub fn free_port_pub() -> std::io::Result<u16> {
    free_port()
}

fn random_password() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos().to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hex::encode(hasher.finalize())[..24].to_string()
}

fn free_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn request(port: u16, password: &str, file: &str, params: &[(&str, String)]) -> Option<Value> {
    use base64::Engine as _;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!(":{password}"));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_millis(1200)))
        .build()
        .into();
    let mut req = agent.get(format!("http://127.0.0.1:{port}/requests/{file}")).header("Authorization", format!("Basic {auth}"));
    for (k, v) in params {
        req = req.query(k, v);
    }
    match req.call() {
        Ok(mut resp) => resp.body_mut().read_json::<serde_json::Value>().ok(),
        Err(_) => None,
    }
}

/// File name of a queue path, with and without its extension — VLC only reports the basename.
fn base_names(p: &str) -> (String, String) {
    let name = p.rsplit(['/', '\\']).next().unwrap_or(p).to_string();
    let stem = match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name.clone(),
    };
    (name, stem)
}

/// Map VLC's idea of the current item back to a queue index. Matching is by reported file name, with a
/// single-item queue as the fallback (`resolveCurrent` in the JS version).
pub fn resolve_current(queue: &[String], meta_filename: Option<&str>) -> Option<usize> {
    if let Some(name) = meta_filename.filter(|n| !n.is_empty()) {
        if let Some(i) = queue.iter().position(|q| {
            let (base, stem) = base_names(q);
            base == name || stem == name
        }) {
            return Some(i);
        }
    }
    if queue.len() == 1 {
        return Some(0);
    }
    None
}

/// Flatten VLC's `playlist.json` tree to the leaf ids in queue order (`collect` in the JS version).
pub fn playlist_leaf_ids(node: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if let Some(children) = n.get("children").and_then(|v| v.as_array()) {
            // Depth-first in order: push reversed so the first child is visited first.
            for c in children.iter().rev() {
                stack.push(c);
            }
        } else if n.get("type").and_then(Value::as_str) == Some("leaf") {
            if let Some(id) = n.get("id") {
                out.push(match id {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
            }
        }
    }
    out
}

impl VlcSession {
    /// Start VLC for `queue` and begin polling. Returns the session handle; events arrive on
    /// [`VlcSession::events`]. Fails only when VLC itself couldn't be spawned.
    pub fn spawn(vlc_path: &Path, queue: &[QueueItem], opts: &BuildArgsOptions) -> std::io::Result<VlcSession> {
        // The caller picks the port and password (via `free_port_pub`/`random_password_pub`) so the
        // shell can hold them before the session exists.
        let args = build_args(queue, opts);
        let child = std::process::Command::new(vlc_path).args(&args).spawn()?;

        let paths: Vec<String> = queue.iter().map(|q| q.path.to_string()).collect();
        let inner = std::sync::Arc::new(std::sync::Mutex::new(SessionInner { port: opts.port, password: opts.password.to_string(), queue: paths, last: None, state: None }));
        let (tx, rx) = std::sync::mpsc::channel();
        let poll_inner = std::sync::Arc::clone(&inner);
        let child_cell = std::sync::Arc::new(std::sync::Mutex::new(child));
        let poll_child = std::sync::Arc::clone(&child_cell);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
                let exited = poll_child.lock().unwrap().try_wait();
                match exited {
                    Ok(Some(status)) => {
                        let last = poll_inner.lock().unwrap().last.clone();
                        let _ = tx.send(VlcEvent::Exit { code: status.code(), last });
                        return;
                    }
                    Err(_) => {
                        let _ = tx.send(VlcEvent::Exit { code: None, last: poll_inner.lock().unwrap().last.clone() });
                        return;
                    }
                    Ok(None) => {}
                }
                for p in poll(&poll_inner) {
                    let _ = tx.send(VlcEvent::Progress(p));
                }
            }
        });
        Ok(VlcSession { inner, child: child_cell, events: rx })
    }

    /// Receive progress/exit events (never blocks the caller; `try_recv` semantics).
    pub fn try_event(&self) -> Option<VlcEvent> {
        self.events.try_recv().ok()
    }

    /// Stop the session: kills VLC, which ends the poller thread with an `Exit` event.
    pub fn kill(&self) {
        let _ = self.child.lock().unwrap().kill();
        let _ = self.child.lock().unwrap().wait();
    }

    /// Send a playback command (`pl_pause`, `seek`, `pl_next`, `key`, …) and return VLC's updated
    /// status. Track switching goes through VLC's own hotkeys (`key` + `audio-track` /
    /// `subtitle-track`): the "Stream N" numbers in the status don't reliably match the ids
    /// `audio_track` expects, and cycling also shows VLC's on-screen label for the new track.
    pub fn command(&self, command: &str, val: Option<&str>) -> Option<Value> {
        let params: Vec<(&str, String)> = match val {
            Some(v) => vec![("command", command.into()), ("val", v.into())],
            None => vec![("command", command.into())],
        };
        let (port, password) = {
            let inner = self.inner.lock().unwrap();
            (inner.port, inner.password.clone())
        };
        let status = request(port, &password, "status.json", &params);
        if let Some(s) = &status {
            if let Some(state) = s.get("state").and_then(Value::as_str) {
                self.inner.lock().unwrap().state = Some(state.to_string());
            }
        }
        status
    }
}

/// One poll tick, shared by the poller thread. Returns the progress events to emit (none, one, or —
/// when VLC moved on to the next item — two: the previous item played to its end, then the new position).
fn poll(inner: &std::sync::Mutex<SessionInner>) -> Vec<Progress> {
    let (port, password, queue_len, queue) = {
        let i = inner.lock().unwrap();
        (i.port, i.password.clone(), i.queue.len(), i.queue.clone())
    };
    let Some(status) = request(port, &password, "status.json", &[]) else { return Vec::new() };
    if status.get("state").and_then(Value::as_str) == Some("stopped") {
        return Vec::new();
    }
    inner.lock().unwrap().state = status.get("state").and_then(Value::as_str).map(String::from);

    let name = status.pointer("/information/category/meta/filename").and_then(Value::as_str);
    let mut item = resolve_current(&queue, name);
    if item.is_none() {
        // Fall back to the playlist: items appear in queue order.
        let currentplid = status.get("currentplid").and_then(Value::as_i64).unwrap_or(-1);
        if currentplid >= 0 {
            if let Some(pl) = request(port, &password, "playlist.json", &[]) {
                let leaves = playlist_leaf_ids(&pl);
                let key = currentplid.to_string();
                if let Some(idx) = leaves.iter().position(|l| *l == key) {
                    if idx < queue_len {
                        item = Some(idx);
                    }
                }
            }
        }
    }
    let Some(idx) = item else { return Vec::new() };
    let length = num_field(&status, "length");
    let time = num_field(&status, "time");
    if length == 0.0 {
        return Vec::new();
    }
    let Some(path) = queue.get(idx).cloned() else { return Vec::new() };

    let mut out = Vec::new();
    let mut inner = inner.lock().unwrap();
    if let Some(last) = inner.last.clone() {
        let last_idx = queue.iter().position(|q| q == &last.path).unwrap_or(0);
        if last.path != path && idx > last_idx {
            // VLC moved on to a later item, so the previous one played to the end (or was skipped on purpose).
            out.push(Progress { path: last.path.clone(), time: last.length, length: last.length, paused: false });
        }
    }
    let paused = status.get("state").and_then(Value::as_str) == Some("paused");
    let progress = Progress { path, time, length, paused };
    inner.last = Some(progress.clone());
    out.push(progress);
    out
}

fn num_field(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn args_to_str(args: &[String]) -> Vec<&str> {
        args.iter().map(|s| s.as_str()).collect()
    }

    // Parity with the JS test "builds VLC arguments with per-item resume" (test/library.test.js).
    #[test]
    fn builds_vlc_arguments_with_per_item_resume() {
        let queue = [QueueItem { path: "C:\\a.mkv", start_time: Some(125.7), languages: None }, QueueItem { path: "C:\\b.mkv", start_time: None, languages: None }];
        let opts = BuildArgsOptions { port: 1234, password: "pw", ..Default::default() };
        let args = build_args(&queue, &opts);
        let s = args_to_str(&args);
        assert!(s.contains(&"--fullscreen"));
        assert!(s.contains(&"--http-port=1234"));
        let i = s.iter().position(|a| *a == "C:\\a.mkv").unwrap();
        assert_eq!(s[i + 1], ":start-time=125");
        assert_eq!(s[i + 2], "C:\\b.mkv");
        assert_eq!(*s.last().unwrap(), "C:\\b.mkv");
    }

    // Parity with the JS test "builds VLC language options, with per-item overrides and the user's own
    // options last".
    #[test]
    fn builds_vlc_language_options_with_overrides_and_user_options_last() {
        let queue = [
            QueueItem { path: "a.mkv", start_time: None, languages: Some(Languages { audio: Some("original"), subs: Some("off") }) },
            QueueItem { path: "b.mkv", start_time: None, languages: None },
        ];
        let opts = BuildArgsOptions {
            port: 1,
            password: "pw",
            languages: Languages { audio: Some("en"), subs: Some("en") },
            extra_args: &["--sub-language=fre"],
            ..Default::default()
        };
        let args = build_args(&queue, &opts);
        let s = args_to_str(&args);
        let audio_pos = s.iter().position(|a| *a == "--audio-language=eng,en,any").unwrap();
        let sub_pos = s.iter().position(|a| *a == "--sub-language=fre").unwrap();
        assert!(audio_pos < sub_pos);
        assert!(s.contains(&"--sub-language=eng,en"));
        let a = s.iter().position(|a| *a == "a.mkv").unwrap();
        assert_eq!(&s[a..a + 4], ["a.mkv", ":audio-language=any", ":sub-language=none", "b.mkv"]);

        // No preference: VLC's own defaults.
        let plain_queue = [QueueItem { path: "a.mkv", start_time: None, languages: None }];
        let plain_opts = BuildArgsOptions { port: 1, password: "pw", languages: Languages { audio: Some("original"), subs: Some("") }, ..Default::default() };
        let plain = build_args(&plain_queue, &plain_opts);
        assert!(!plain.iter().any(|x| x.contains("sub-language")));
    }

    #[test]
    fn find_vlc_prefers_an_existing_preferred_path() {
        let dir = tempdir().unwrap();
        let vlc = dir.path().join("vlc.exe");
        fs::write(&vlc, "").unwrap();
        let env = VlcEnv { path: String::new(), is_windows: false, is_macos: false, ..Default::default() };
        assert_eq!(find_vlc(Some(&vlc), &env), Some(vlc));
    }

    #[test]
    fn find_vlc_returns_none_when_nothing_matches() {
        let env = VlcEnv { path: String::new(), is_windows: false, is_macos: false, ..Default::default() };
        assert_eq!(find_vlc(Some(Path::new("/nope/vlc")), &env), None);
    }

    #[test]
    fn which_finds_an_executable_on_the_path() {
        let dir = tempdir().unwrap();
        let vlc = dir.path().join("vlc");
        fs::write(&vlc, "").unwrap();
        let path_env = dir.path().to_string_lossy().into_owned();
        assert_eq!(which("vlc", &path_env, false), Some(vlc));
        assert_eq!(which("nope", &path_env, false), None);
    }

    fn queue(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn resolve_current_matches_by_reported_file_name() {
        let q = queue(&["/media/Film (2020)/Film.mkv", "/media/Show/s01e02.mkv"]);
        assert_eq!(resolve_current(&q, Some("Film.mkv")), Some(0));
        assert_eq!(resolve_current(&q, Some("s01e02.mkv")), Some(1));
        assert_eq!(resolve_current(&q, Some("Film")), Some(0), "VLC sometimes reports the name without its extension");
        assert_eq!(resolve_current(&q, Some("other.mkv")), None);
        assert_eq!(resolve_current(&q, None), None);
        assert_eq!(resolve_current(&q, Some("")), None);
    }

    #[test]
    fn a_single_item_queue_is_its_own_fallback() {
        let q = queue(&["/media/Film.mkv"]);
        assert_eq!(resolve_current(&q, Some("whatever.avi")), Some(0));
        assert_eq!(resolve_current(&q, None), Some(0));
    }

    #[test]
    fn resolve_current_understands_windows_separators() {
        let q = queue(&[r"C:\Movies\Film.mkv"]);
        assert_eq!(resolve_current(&q, Some("Film.mkv")), Some(0));
        assert_eq!(base_names(&q[0]).0, "Film.mkv");
        assert_eq!(base_names(&q[0]).1, "Film");
    }

    #[test]
    fn playlist_leaf_ids_walk_the_tree_in_order() {
        let pl = json!({
            "children": [
                { "type": "leaf", "id": 10 },
                { "type": "node", "children": [
                    { "type": "leaf", "id": 11 },
                    { "type": "leaf", "id": "12" },
                ]},
                { "type": "node", "name": "empty" },
                { "type": "leaf", "id": 13 },
            ]
        });
        assert_eq!(playlist_leaf_ids(&pl), vec!["10", "11", "12", "13"]);
        assert!(playlist_leaf_ids(&json!({ "type": "leaf", "id": 1 })).len() == 1);
        assert!(playlist_leaf_ids(&json!({})).is_empty());
    }


    #[test]
    fn passwords_are_24_hex_chars_and_unique() {
        let a = random_password();
        let b = random_password();
        assert_eq!(a.len(), 24);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn free_port_binds_a_usable_loopback_port() {
        let port = free_port().unwrap();
        assert!(port > 0);
        // A second bind of the same port must now fail (it was released, but that's not what this
        // asserts): what matters is the listener is gone so VLC can take it.
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        drop(listener);
    }
}
