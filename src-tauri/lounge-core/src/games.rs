//! Manually-added game helpers (id generation, a readable default title from an .exe path,
//! launch-options splitting) and the running-game tracker. Direct port of `src/games.js`, including
//! `GameSession`: Steam games are tracked through Steam's `RunningAppID` registry value (so it works
//! however the game is actually launched), manual games through their own process.
//!
//! Where the JS version is an `EventEmitter`, `spawn` hands back a `std::sync::mpsc::Receiver` of
//! [`GameEvent`]s. Everything the JS version reached out to (opening a `steam://` link, opening a
//! shortcut by shell, Steam's registry) arrives as injected closures in [`GameDeps`], which is also
//! what makes the state machine testable here without Steam or Windows.

use sha1::{Digest, Sha1};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub fn manual_id(exe: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(exe.to_lowercase());
    hasher.update(now_millis().to_string());
    let digest = hasher.finalize();
    format!("game-{}", &hex::encode(digest)[..12])
}

fn is_generic_build_folder(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "bin" | "bin32" | "bin64" | "binaries" | "win64" | "win32" | "windows" | "x64" | "x86" | "game" | "release" | "shipping" | "retail"
    )
}

fn is_drive_letter(name: &str) -> bool {
    let mut chars = name.chars();
    matches!((chars.next(), chars.next(), chars.next()), (Some(c), Some(':'), None) if c.is_ascii_alphabetic())
}

fn is_generic_container_folder(name: &str) -> bool {
    matches!(name.to_lowercase().as_str(), "game" | "games" | "program files" | "program files (x86)" | "jeux")
}

/// Collapse runs of `.`, `_` and whitespace into a single space, then trim the ends.
fn tidy_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = false;
    for c in s.chars() {
        if c == '.' || c == '_' || c.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            last_was_space = false;
        }
    }
    out.trim().to_string()
}

/// Turn `"C:\Games\Hades II\Hades2.exe"` into a readable default title (`"Hades II"`).
pub fn title_from_exe(exe: &str) -> String {
    let exe_path = Path::new(exe);
    let base = exe_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

    // Skip build-output folders ("Game\Binaries\Win64\Game.exe") to reach the game's own folder.
    let mut dir = exe_path.parent().map(Path::to_path_buf);
    while let Some(d) = &dir {
        let Some(name) = d.file_name() else { break };
        if !is_generic_build_folder(&name.to_string_lossy()) {
            break;
        }
        match d.parent() {
            Some(p) if p != d => dir = Some(p.to_path_buf()),
            _ => break,
        }
    }

    let mut name = dir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.is_empty() || is_drive_letter(&name) || is_generic_container_folder(&name) {
        name = base;
    }
    tidy_spaces(&name)
}

/// Split a launch-options string, honouring double quotes.
pub fn split_args(s: &str) -> Vec<String> {
    fn strip_quotes(s: &str) -> String {
        let s = s.strip_prefix('"').unwrap_or(s);
        s.strip_suffix('"').unwrap_or(s).to_string()
    }

    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        if chars[i] == '"' {
            // A quoted token requires a literal closing quote somewhere ahead, same as the JS
            // regex `"[^"]*"`; if there isn't one, this falls through to the plain-token case below,
            // exactly like the JS alternation would.
            if let Some(close_offset) = chars[i + 1..].iter().position(|&c| c == '"') {
                let close = i + 1 + close_offset;
                let token: String = chars[i..=close].iter().collect();
                out.push(strip_quotes(&token));
                i = close + 1;
                continue;
            }
        }
        let start = i;
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        out.push(strip_quotes(&chars[start..i].iter().collect::<String>()));
    }
    out
}

// ---------------------------------------------------------------------------
// GameSession

const STEAM_START_TIMEOUT_MS: i64 = 3 * 60 * 1000; // first launches can sit on "installing prerequisites"
const STEAM_POLL_MS: u64 = 3000;
const LAUNCHER_STUB_MS: i64 = 15000;

/// Why a game session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// The process (or Steam) says the game is no longer running.
    Exited,
    /// The exe was a launcher that handed off to the real game and quit at once: we can't see when
    /// that one ends, so the session stays open until the user comes back and says they're done.
    Stub,
    /// Steam never reported our app id as running.
    Timeout,
    /// The user said they're done (e.g. we couldn't track the game).
    User,
    /// The exe couldn't be started at all.
    Error,
}

/// Events from one play session, as the JS version's `running`/`exit` emits.
#[derive(Debug, Clone)]
pub enum GameEvent {
    Running,
    Exit { reason: ExitReason, played_ms: i64, error: Option<String> },
}

/// The fields of a raw game the session needs (a Steam or manual entry from the view model).
#[derive(Debug, Clone, Default)]
pub struct SessionGame {
    pub id: String,
    pub source: String,
    pub appid: String,
    pub title: String,
    pub exe: String,
    pub args: String,
    pub cwd: String,
}

impl SessionGame {
    pub fn from_json(g: &serde_json::Value) -> SessionGame {
        let s = |k: &str| g.get(k).and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        SessionGame { id: s("id"), source: s("source"), appid: s("appid"), title: s("title"), exe: s("exe"), args: s("args"), cwd: s("cwd") }
    }
}

/// The process-touching effects a session can perform, as injectable closures.
pub type OpenUrl = Box<dyn Fn(&str) + Send + Sync>;
pub type OpenPath = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;
pub type LaunchSteam = Box<dyn Fn(&str) -> bool + Send + Sync>;
pub type RunningAppId = Box<dyn Fn() -> Option<i64> + Send + Sync>;

/// Everything the session does outside this process, injected like the JS constructor's callbacks.
pub struct GameDeps {

    /// Open a URL externally (`shell.openExternal`): used for the `steam://rungameid/<id>` fallback.
    pub open_external: OpenUrl,
    /// Open a path by shell (`shell.openPath`), returning its error message if it failed: used for
    /// `.lnk`/`.url` shortcuts and scripts, where there's no process of ours to watch.
    pub open_path: OpenPath,
    /// Start a Steam game its own way (e.g. keeping Steam's window hidden); `None` or `false` falls
    /// back to the `steam://` link.
    pub launch_steam: Option<LaunchSteam>,
    /// Steam's `RunningAppID` (a registry read, Windows-only): `None` when it can't be observed.
    pub running_app_id: RunningAppId,
    /// Poll/timeout tuning — production values by default, shortened in tests.
    pub tuning: Tuning,
}

#[derive(Debug, Clone, Copy)]
pub struct Tuning {
    pub poll_ms: u64,
    pub steam_start_timeout_ms: i64,
    pub launcher_stub_ms: i64,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { poll_ms: STEAM_POLL_MS, steam_start_timeout_ms: STEAM_START_TIMEOUT_MS, launcher_stub_ms: LAUNCHER_STUB_MS }
    }
}

impl Default for GameDeps {
    fn default() -> Self {
        GameDeps {
            open_external: Box::new(|_| {}),
            open_path: Box::new(|_| None),
            launch_steam: None,
            running_app_id: Box::new(crate::steam::running_app_id),
            tuning: Tuning::default(),
        }
    }
}

struct Shared {
    started_at: i64,
    running_since: i64,
    done: bool,
}

/// One play session, as a handle plus an event receiver (see [`GameSession::spawn`]).
pub struct GameSession {
    shared: std::sync::Arc<std::sync::Mutex<Shared>>,
    tx: std::sync::Mutex<std::sync::mpsc::Sender<GameEvent>>,
    events: std::sync::mpsc::Receiver<GameEvent>,
}

impl GameSession {
    /// Start the game and begin tracking it. `Err` only when a manual game's exe couldn't be spawned
    /// (the JS version reports this via the session's `error` event; here it's the spawn result).
    pub fn spawn(game: &SessionGame, deps: GameDeps) -> std::io::Result<GameSession> {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Shared { started_at: now_millis() as i64, running_since: 0, done: false }));
        let (tx, rx) = std::sync::mpsc::channel();

        if game.source == "steam" {
            let quiet = match &deps.launch_steam {
                Some(launch) => launch(&game.appid),
                None => false,
            };
            if !quiet {
                (deps.open_external)(&format!("steam://rungameid/{}", game.appid));
            }
            let want = game.appid.parse::<i64>().unwrap_or(-1);
            let poll_shared = std::sync::Arc::clone(&shared);
            let tx_poll = tx.clone();
            let tuning = deps.tuning;
            std::thread::spawn(move || {
                loop {
                    if poll_shared.lock().unwrap().done {
                        return;
                    }
                    let running = (deps.running_app_id)();
                    if running.is_none() {
                        // Can't observe Steam (not Windows): assume it started, and let the user say when they're done.
                        mark_running(&poll_shared, &tx_poll);
                    } else if running == Some(want) {
                        mark_running(&poll_shared, &tx_poll);
                    } else if poll_shared.lock().unwrap().running_since > 0 {
                        finish(&poll_shared, &tx_poll, ExitReason::Exited, None, tuning.launcher_stub_ms);
                        return;
                    } else if now_millis() as i64 - poll_shared.lock().unwrap().started_at > tuning.steam_start_timeout_ms {
                        finish(&poll_shared, &tx_poll, ExitReason::Timeout, None, tuning.launcher_stub_ms);
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(tuning.poll_ms));
                }
            });
            Ok(GameSession { shared, tx: std::sync::Mutex::new(tx), events: rx })
        } else if !game.exe.to_lowercase().ends_with(".exe") {
            // Shortcuts (.lnk/.url) and scripts are opened by the shell; there's no process of ours to watch.
            let err = (deps.open_path)(&game.exe);
            match err {
                Some(message) => {
                    finish(&shared, &tx, ExitReason::Error, Some(message), deps.tuning.launcher_stub_ms);
                    Ok(GameSession { shared, tx: std::sync::Mutex::new(tx), events: rx })
                }
                None => {
                    mark_running(&shared, &tx);
                    finish(&shared, &tx, ExitReason::Stub, None, deps.tuning.launcher_stub_ms);
                    Ok(GameSession { shared, tx: std::sync::Mutex::new(tx), events: rx })
                }
            }
        } else {
            let cwd = if game.cwd.is_empty() { dirname_of(&game.exe) } else { game.cwd.clone() };
            let mut command = std::process::Command::new(&game.exe);
            command.args(crate::games::split_args(&game.args)).current_dir(cwd).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            let child = command.spawn()?;
            let watch_shared = std::sync::Arc::clone(&shared);
            let tx_watch = tx.clone();
            let tuning = deps.tuning;
            std::thread::spawn(move || {
                let mut child = child;
                let _ = child.wait();
                // Some games are stubs that start the real game and quit at once; don't count that as the session.
                let reason = if now_millis() as i64 - watch_shared.lock().unwrap().started_at < tuning.launcher_stub_ms {
                    ExitReason::Stub
                } else {
                    ExitReason::Exited
                };
                finish(&watch_shared, &tx_watch, reason, None, tuning.launcher_stub_ms);
            });
            mark_running(&shared, &tx);
            Ok(GameSession { shared, tx: std::sync::Mutex::new(tx), events: rx })
        }
    }

    /// Receive the next event without blocking (`try_recv` semantics).
    pub fn try_event(&self) -> Option<GameEvent> {
        self.events.try_recv().ok()
    }

    /// The user says they're done (e.g. we couldn't track the game).
    pub fn stop_tracking(&self) {
        let tx = self.tx.lock().unwrap().clone();
        finish(&self.shared, &tx, ExitReason::User, None, LAUNCHER_STUB_MS);
    }
}

fn mark_running(shared: &std::sync::Arc<std::sync::Mutex<Shared>>, tx: &std::sync::mpsc::Sender<GameEvent>) {
    let mut s = shared.lock().unwrap();
    if s.done || s.running_since > 0 {
        return;
    }
    s.running_since = now_millis() as i64;
    drop(s);
    let _ = tx.send(GameEvent::Running);
}

fn finish(shared: &std::sync::Arc<std::sync::Mutex<Shared>>, tx: &std::sync::mpsc::Sender<GameEvent>, reason: ExitReason, error: Option<String>, _stub_ms: i64) {
    let mut s = shared.lock().unwrap();
    if s.done {
        return;
    }
    s.done = true;
    let played_ms = if s.running_since > 0 && reason != ExitReason::Stub { now_millis() as i64 - s.running_since } else { 0 };
    drop(s);
    let _ = tx.send(GameEvent::Exit { reason, played_ms, error });
}

fn dirname_of(p: &str) -> String {
    match p.rfind(['\\', '/']) {
        Some(i) => p[..i].to_string(),
        None => ".".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Parity with the JS test "manual game titles and launch options" (test/games.test.js). That test
    // builds its paths with `path.sep`, i.e. platform-native separators; these use `/`, this crate's
    // separator when built for this (Linux) target. The logic itself is identical either way — Rust's
    // `std::path::Path` parses `\`-separated paths the same way Node's `path` module does when actually
    // compiled for Windows, but that specific parsing can only be exercised on a Windows build.
    #[test]
    fn manual_game_titles() {
        assert_eq!(title_from_exe("C:/Games/Hades II/Hades2.exe"), "Hades II");
        assert_eq!(title_from_exe("C:/Games/Celeste/bin/x64/Celeste.exe"), "Celeste");
        assert_eq!(title_from_exe("C:/Games/Tetris.exe"), "Tetris");
        assert_eq!(title_from_exe("D:/Emu/Dolphin/Binaries/Dolphin.exe"), "Dolphin");
    }

    #[test]
    fn launch_options() {
        assert_eq!(split_args(r#"-windowed --profile "My Profile""#), vec!["-windowed", "--profile", "My Profile"]);
        assert_eq!(split_args(""), Vec::<String>::new());
    }

    #[test]
    fn manual_id_has_the_expected_shape() {
        let id = manual_id("C:/Games/Foo/Foo.exe");
        assert!(id.starts_with("game-"));
        assert_eq!(id.len(), "game-".len() + 12);
        assert!(id["game-".len()..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ---- GameSession

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::tempdir;

    fn fast_tuning() -> Tuning {
        Tuning { poll_ms: 10, steam_start_timeout_ms: 200, launcher_stub_ms: 15_000 }
    }

    /// Collects events for up to `ms` milliseconds.
    fn drain(session: &GameSession, ms: u64) -> Vec<GameEvent> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        let mut out = Vec::new();
        while std::time::Instant::now() < deadline {
            if let Some(e) = session.try_event() {
                out.push(e);
                if matches!(out.last(), Some(GameEvent::Exit { .. })) {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        out
    }

    fn steam_game(appid: &str) -> SessionGame {
        SessionGame { id: format!("steam-{appid}"), source: "steam".into(), appid: appid.into(), title: "TF2".into(), ..Default::default() }
    }

    #[test]
    fn a_steam_game_marks_running_then_exits_when_another_app_takes_over() {
        // Poll 1: nothing running yet; poll 2: our game; poll 3: a different app id.
        let calls = Arc::new(AtomicUsize::new(0));
        let seq = Arc::new(std::sync::Mutex::new(vec![None, Some(440i64), Some(0)]));
        let mut deps = GameDeps {
            running_app_id: Box::new(move || {
                let i = calls.fetch_add(1, Ordering::SeqCst);
                seq.lock().unwrap().get(i).copied().flatten()
            }),
            tuning: fast_tuning(),
            ..Default::default()
        };
        let opened = Arc::new(AtomicUsize::new(0));
        let opened2 = Arc::clone(&opened);
        deps.open_external = Box::new(move |_| {
            opened2.fetch_add(1, Ordering::SeqCst);
        });

        let session = GameSession::spawn(&steam_game("440"), deps).unwrap();
        let events = drain(&session, 2000);
        assert!(matches!(events.first(), Some(GameEvent::Running)), "{events:?}");
        assert!(matches!(events.last(), Some(GameEvent::Exit { reason: ExitReason::Exited, played_ms, .. }) if *played_ms >= 0), "{events:?}");
        assert_eq!(opened.load(Ordering::SeqCst), 1, "the steam:// link was opened");
    }

    #[test]
    fn quiet_launch_skips_the_steam_link() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seq = Arc::new(std::sync::Mutex::new(vec![Some(440i64)]));
        let mut deps = GameDeps {
            running_app_id: Box::new(move || {
                let i = calls.fetch_add(1, Ordering::SeqCst);
                seq.lock().unwrap().get(i).copied().flatten()
            }),
            tuning: fast_tuning(),
            launch_steam: Some(Box::new(|_| true)),
            ..Default::default()
        };
        let opened = Arc::new(AtomicUsize::new(0));
        let opened2 = Arc::clone(&opened);
        deps.open_external = Box::new(move |_| {
            opened2.fetch_add(1, Ordering::SeqCst);
        });

        let session = GameSession::spawn(&steam_game("440"), deps).unwrap();
        let events = drain(&session, 2000);
        assert_eq!(opened.load(Ordering::SeqCst), 0, "no steam:// link when the quiet launch took over");
        assert!(matches!(events.first(), Some(GameEvent::Running)));
    }

    #[test]
    fn when_steam_cant_be_observed_the_session_stays_open_for_the_user() {
        let mut deps = GameDeps { running_app_id: Box::new(|| None), tuning: fast_tuning(), ..Default::default() };
        deps.launch_steam = Some(Box::new(|_| true));
        let session = GameSession::spawn(&steam_game("440"), deps).unwrap();
        let events = drain(&session, 150);
        assert!(matches!(events.first(), Some(GameEvent::Running)), "{events:?}");
        assert!(session.try_event().is_none(), "no exit of its own");

        session.stop_tracking();
        let events = drain(&session, 2000);
        assert!(matches!(events.last(), Some(GameEvent::Exit { reason: ExitReason::User, .. })), "{events:?}");
    }

    #[test]
    fn steam_never_coming_up_times_out() {
        let mut deps = GameDeps { running_app_id: Box::new(|| Some(0)), tuning: Tuning { poll_ms: 10, steam_start_timeout_ms: 60, launcher_stub_ms: 15_000 }, ..Default::default() };
        deps.launch_steam = Some(Box::new(|_| true));
        let session = GameSession::spawn(&steam_game("440"), deps).unwrap();
        let events = drain(&session, 2000);
        assert!(matches!(events.last(), Some(GameEvent::Exit { reason: ExitReason::Timeout, played_ms: 0, .. })), "{events:?}");
    }

    #[test]
    fn a_manual_game_is_watched_until_its_process_exits() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        std::fs::write(&exe, "#!/bin/sh\nsleep 0.5\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let game = SessionGame { id: "manual-x".into(), source: "manual".into(), exe: exe.to_string_lossy().into(), ..Default::default() };
        // A short stub window so a game that outlives it counts as a real session.
        let deps = GameDeps { tuning: Tuning { launcher_stub_ms: 100, ..fast_tuning() }, ..Default::default() };

        let session = GameSession::spawn(&game, deps).unwrap();
        let events = drain(&session, 4000);
        assert!(matches!(events.first(), Some(GameEvent::Running)), "{events:?}");
        assert!(matches!(events.last(), Some(GameEvent::Exit { reason: ExitReason::Exited, played_ms, .. }) if *played_ms > 0), "{events:?}");
    }

    #[test]
    fn a_manual_launcher_that_quits_at_once_counts_as_a_stub() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("launcher.exe");
        std::fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let game = SessionGame { id: "manual-x".into(), source: "manual".into(), exe: exe.to_string_lossy().into(), ..Default::default() };
        let session = GameSession::spawn(&game, GameDeps { tuning: fast_tuning(), ..Default::default() }).unwrap();
        let events = drain(&session, 4000);
        assert!(matches!(events.last(), Some(GameEvent::Exit { reason: ExitReason::Stub, played_ms: 0, .. })), "{events:?}");
    }

    #[test]
    fn a_shortcut_opened_by_the_shell_is_a_stub_with_no_process() {
        let dir = tempdir().unwrap();
        let lnk = dir.path().join("game.url");
        std::fs::write(&lnk, "[InternetShortcut]\n").unwrap();
        let game = SessionGame { id: "manual-x".into(), source: "manual".into(), exe: lnk.to_string_lossy().into(), ..Default::default() };
        let mut deps = GameDeps { tuning: fast_tuning(), ..Default::default() };
        let opened = Arc::new(AtomicUsize::new(0));
        let opened2 = Arc::clone(&opened);
        deps.open_path = Box::new(move |_| {
            opened2.fetch_add(1, Ordering::SeqCst);
            None
        });

        let session = GameSession::spawn(&game, deps).unwrap();
        let events = drain(&session, 2000);
        assert_eq!(opened.load(Ordering::SeqCst), 1);
        assert!(matches!(events.first(), Some(GameEvent::Running)));
        assert!(matches!(events.last(), Some(GameEvent::Exit { reason: ExitReason::Stub, played_ms: 0, .. })), "{events:?}");
    }

    #[test]
    fn a_shortcut_the_shell_fails_to_open_is_an_error() {
        let game = SessionGame { id: "manual-x".into(), source: "manual".into(), exe: "Z:\\gone\\game.lnk".into(), ..Default::default() };
        let deps = GameDeps {
            open_path: Box::new(|_| Some("File not found".into())),
            tuning: fast_tuning(),
            ..Default::default()
        };
        let session = GameSession::spawn(&game, deps).unwrap();
        let events = drain(&session, 2000);
        assert!(matches!(events.last(), Some(GameEvent::Exit { reason: ExitReason::Error, error: Some(m), .. }) if m == "File not found"), "{events:?}");
    }

    #[test]
    fn a_missing_manual_exe_fails_to_spawn() {
        let game = SessionGame { id: "manual-x".into(), source: "manual".into(), exe: "/nowhere/game.exe".into(), ..Default::default() };
        assert!(GameSession::spawn(&game, GameDeps { tuning: fast_tuning(), ..Default::default() }).is_err());
    }
}
