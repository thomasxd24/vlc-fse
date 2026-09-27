//! The Tauri shell for Lounge: a proof-of-concept slice of `main.js`'s orchestration, wired up for real
//! against the ported `lounge-core` crate, not a full 1:1 port.
//!
//! **Scope, honestly:** `main.js` is ~1,500 lines integrating every module in this migration plus a long
//! list of Electron APIs (`dialog`, `shell`, `safeStorage`, `powerSaveBlocker`, WebHID device permission
//! for the Legion Go, singleton-instance locking, login items…) that have no Rust equivalent chosen yet,
//! and several pieces (`GameSession`, `VlcSession`, `SystemHelper`, `Tailscale`'s process methods,
//! `remote`'s actual client, `src/apps.js`'s Start-menu scanning) that this migration deliberately left
//! unbuilt pending exactly this crate's existence. Porting all of that blind, unverified, in one pass
//! would be a much bigger bet than everything else in this migration, which was ported module by module
//! against real tests.
//!
//! So this crate proves the *architecture* end to end instead: a real window, loading the actual
//! `renderer/**` UI unmodified, driven by real `lounge-core` calls (library scanning, settings
//! persistence) through Tauri commands — no Node, no Electron, no mocked data. `window.lounge` (what
//! `preload.js`'s `contextBridge` exposes today) is recreated here as a small init script mapping each
//! method to `window.__TAURI__.core.invoke(...)`, so `renderer/**` needed no changes at all. Commands
//! not yet backed by a real implementation return a clearly-marked "not yet implemented" error rather
//! than silently pretending to succeed.

use lounge_core::library;
use lounge_core::store::JsonStore;
use lounge_core::vlc;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

struct AppState {
    settings: JsonStore,
    library: Mutex<library::LibraryScan>,
}

fn default_settings() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("libraries".into(), json!([]));
    m.insert("vlcPath".into(), json!(""));
    m.insert("vlcFullscreen".into(), json!(true));
    m.insert("vlcExtraArgs".into(), json!(""));
    m.insert("autoplayNext".into(), json!(true));
    m.insert("audioLanguage".into(), json!("en"));
    m.insert("subLanguage".into(), json!("en"));
    m.insert("tmdbKey".into(), json!(""));
    m.insert("sgdbKey".into(), json!(""));
    m.insert("steamEnabled".into(), json!(true));
    m.insert("steamPath".into(), json!(""));
    m.insert("quietSteam".into(), json!(true));
    m.insert("uiLanguage".into(), json!("auto"));
    m.insert("startFullscreen".into(), json!(false));
    m.insert("launchAtLogin".into(), json!(false));
    m.insert("uiScale".into(), json!(1));
    m.insert("haptics".into(), json!(true));
    m.insert("sounds".into(), json!(true));
    m.insert("animations".into(), json!("full"));
    m.insert("freeWhilePlaying".into(), json!(true));
    m.insert("autoCheckUpdates".into(), json!(true));
    m.insert("skippedVersion".into(), json!(""));
    m
}

/// A local filesystem path as a `file://` URL the webview can load. Good enough for the Unix-style
/// paths this runs against on this (Linux) development machine; a real Windows build needs this to also
/// handle backslashes and the drive-letter prefix (`file:///C:/...`) — worth revisiting once this is
/// actually run under WebView2.
fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = s.strip_prefix('/') {
        format!("file:///{stripped}")
    } else {
        format!("file:///{s}")
    }
}

fn empty_progress() -> Value {
    json!({"time": 0, "length": 0, "watched": false, "updatedAt": 0, "resumable": false})
}

fn movie_json(m: &library::Movie) -> Value {
    json!({
        "id": m.id,
        "type": "movie",
        "title": m.title,
        "year": m.year,
        "overview": "",
        "tagline": "",
        "rating": Value::Null,
        "runtime": Value::Null,
        "genres": Vec::<String>::new(),
        "poster": m.poster.as_deref().map(file_url),
        "backdrop": m.backdrop.as_deref().map(file_url),
        "path": m.path.to_string_lossy(),
        "addedAt": m.added_at,
        "progress": empty_progress(),
        "languages": Value::Null,
        "favorite": false,
        "hidden": false,
    })
}

fn episode_json(e: &library::Episode) -> Value {
    json!({
        "id": e.id,
        "season": e.season,
        "episode": e.episode.unwrap_or(0),
        "episodeEnd": e.episode_end,
        "title": e.title,
        "overview": "",
        "airDate": Value::Null,
        "runtime": Value::Null,
        "thumb": e.thumb.as_deref().map(file_url),
        "path": e.path.to_string_lossy(),
        "addedAt": e.added_at,
        "progress": empty_progress(),
    })
}

fn show_json(s: &library::Show) -> Value {
    let episodes: Vec<Value> = s.episodes.iter().map(episode_json).collect();
    let mut seasons: Vec<i32> = s.episodes.iter().map(|e| e.season).collect();
    seasons.sort_unstable();
    seasons.dedup();
    json!({
        "id": s.id,
        "type": "show",
        "title": s.title,
        "year": s.year,
        "overview": "",
        "rating": Value::Null,
        "genres": Vec::<String>::new(),
        "status": Value::Null,
        "poster": s.poster.as_deref().map(file_url),
        "backdrop": s.backdrop.as_deref().map(file_url),
        "addedAt": s.added_at,
        "episodes": episodes,
        "seasons": seasons,
        "watchedCount": 0,
        "nextUp": Value::Null,
        "lastActivity": 0,
        "languages": Value::Null,
        "favorite": false,
        "hidden": false,
    })
}

fn build_state(state: &AppState) -> Value {
    let lib = state.library.lock().unwrap();
    let movies: Vec<Value> = lib.movies.iter().map(movie_json).collect();
    let shows: Vec<Value> = lib.shows.iter().map(show_json).collect();
    let settings_json = state.settings.get("libraries").map(|_| ()).map(|_| Value::Object(default_settings())).unwrap_or_else(|| Value::Object(default_settings()));
    // Overlay whatever's actually persisted over the defaults.
    let mut settings_map = match settings_json {
        Value::Object(m) => m,
        _ => default_settings(),
    };
    for key in settings_map.clone().keys() {
        if let Some(v) = state.settings.get(key) {
            settings_map.insert(key.clone(), v);
        }
    }

    json!({
        "settings": settings_map,
        "lang": "en",
        "library": {
            "movies": movies,
            "shows": shows,
            "games": Vec::<Value>::new(),
            "continueWatching": Vec::<Value>::new(),
            "scannedAt": lib.scanned_at,
            "steamFound": false,
            "steamUser": Value::Null,
        },
        "scanning": false,
        "metaStatus": {"running": false, "error": Value::Null},
        "gameInfoStatus": {"running": false, "error": Value::Null},
        "nowPlaying": Value::Null,
        "game": Value::Null,
        "uiState": Value::Null,
        "toasts": Vec::<Value>::new(),
        "platform": std::env::consts::OS,
        "packaged": false,
        "fsePackage": false,
        "systemControls": false,
        "update": Value::Null,
        "hasBattery": Value::Null,
        "servers": Vec::<Value>::new(),
        "apps": Vec::<Value>::new(),
        "appsScanning": false,
        "appsScannedAt": 0,
        "tailscaleInstalled": false,
        "tailscaleCli": Value::Null,
        "tailscaleTried": Vec::<Value>::new(),
        "transfers": Vec::<Value>::new(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

fn scan_from_settings(settings: &JsonStore) -> library::LibraryScan {
    let libs_json = settings.get("libraries").and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut owned: Vec<(PathBuf, library::LibraryKind)> = Vec::new();
    for l in &libs_json {
        let Some(p) = l.get("path").and_then(Value::as_str) else { continue };
        let kind = if l.get("type").and_then(Value::as_str) == Some("tv") { library::LibraryKind::Tv } else { library::LibraryKind::Movies };
        owned.push((PathBuf::from(p), kind));
    }
    let defs: Vec<library::LibraryDef> = owned.iter().map(|(p, k)| library::LibraryDef { path: p.as_path(), kind: *k }).collect();
    library::scan_libraries(&defs)
}

#[tauri::command]
fn get_state(state: State<AppState>) -> Value {
    build_state(&state)
}

#[tauri::command]
fn rescan(app: AppHandle, state: State<AppState>) -> Value {
    let scanned = scan_from_settings(&state.settings);
    *state.library.lock().unwrap() = scanned;
    let payload = build_state(&state);
    let _ = app.emit("state", payload.clone());
    payload
}

#[tauri::command]
fn save_settings(app: AppHandle, state: State<AppState>, patch: Map<String, Value>) -> Value {
    let allowed = default_settings();
    for (k, v) in patch {
        if allowed.contains_key(&k) {
            state.settings.set(k, v);
        }
    }
    let payload = build_state(&state);
    let _ = app.emit("state", payload.clone());
    payload
}

#[tauri::command]
fn not_yet_implemented(name: &str) -> Result<Value, String> {
    Err(format!("'{name}' isn't wired up in the Tauri shell yet — see lounge_app_lib's module doc"))
}

/// Wired for real (not a stub): `renderer/app.js`'s boot sequence does `await api.detectVlc()`
/// unconditionally, with no `.catch` — a rejection here would silently abort the rest of `boot()`
/// (including ending the startup intro), which is exactly what happened before this was backed by
/// `vlc::find_vlc` instead of the generic "not yet implemented" stub.
#[tauri::command]
fn detect_vlc() -> Option<String> {
    vlc::find_vlc(None, &vlc::VlcEnv::from_process_env()).map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn minimize(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
}

#[tauri::command]
fn toggle_fullscreen(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if let Ok(is_fs) = w.is_fullscreen() {
            let _ = w.set_fullscreen(!is_fs);
        }
    }
}

/// Recreates `preload.js`'s `contextBridge`-exposed `window.lounge` object, so `renderer/**` runs
/// completely unmodified against either shell. Everything not yet backed by a real command still exists
/// as a callable method — it just rejects with a clear "not implemented" error instead of doing nothing
/// or throwing a raw "unknown command".
const INIT_SCRIPT: &str = r#"
(function () {
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
  const call = (cmd) => (arg) => invoke(cmd, { arg });
  const stub = (name) => () => invoke('not_yet_implemented', { name });
  const on = (event) => (cb) => {
    let unlisten = () => {};
    window.__TAURI__.event.listen(event, (e) => cb(e.payload)).then((u) => { unlisten = u; });
    return () => unlisten();
  };
  window.lounge = {
    getState: () => invoke('get_state'),
    saveUiState: stub('saveUiState'),
    rescan: () => invoke('rescan'),
    play: stub('play'), stop: stub('stop'), npCommand: stub('npCommand'),
    setLanguages: stub('setLanguages'), setWatched: stub('setWatched'),
    setPref: stub('setPref'), showInFolder: stub('showInFolder'), getStats: stub('getStats'),
    playGame: stub('playGame'), endGame: stub('endGame'), backToGame: stub('backToGame'),
    addGame: stub('addGame'), editGame: stub('editGame'), removeGame: stub('removeGame'),
    searchSteam: stub('searchSteam'), searchSgdb: stub('searchSgdb'), sgdbImages: stub('sgdbImages'),
    setGameArt: stub('setGameArt'), screenshot: stub('screenshot'), showGameFolder: stub('showGameFolder'),
    saveSettings: (patch) => invoke('save_settings', { patch }),
    pickFolder: stub('pickFolder'), pickVlc: stub('pickVlc'), detectVlc: () => invoke('detect_vlc'),
    clearMetadata: stub('clearMetadata'),
    saveServer: stub('saveServer'), removeServer: stub('removeServer'), forgetHostKey: stub('forgetHostKey'),
    testServer: stub('testServer'), pickKeyFile: stub('pickKeyFile'), remoteList: stub('remoteList'),
    remotePlan: stub('remotePlan'), remoteDownload: stub('remoteDownload'), cancelTransfer: stub('cancelTransfer'),
    clearTransfers: stub('clearTransfers'), retryTransfer: stub('retryTransfer'),
    rescanApps: stub('rescanApps'), launchApp: stub('launchApp'), hideApp: stub('hideApp'),
    tailscaleStatus: stub('tailscaleStatus'), tailscaleLocate: stub('tailscaleLocate'),
    tailscaleAction: stub('tailscaleAction'), tailscaleLogin: stub('tailscaleLogin'),
    tailscaleCancelLogin: stub('tailscaleCancelLogin'), tailscaleOpenApp: stub('tailscaleOpenApp'),
    systemGet: stub('systemGet'), systemSet: stub('systemSet'), wifi: stub('wifi'), power: stub('power'),
    openExternal: stub('openExternal'), checkUpdate: stub('checkUpdate'), installUpdate: stub('installUpdate'),
    skipUpdate: stub('skipUpdate'),
    toggleFullscreen: () => invoke('toggle_fullscreen'),
    minimize: () => invoke('minimize'),
    quit: () => invoke('quit'),
    onState: on('state'), onNowPlaying: on('now-playing'), onGame: on('game'), onToast: on('toast'),
    onUpdate: on('update'), onTransfers: on('transfers'), onTailscale: on('tailscale'),
  };
})();
"#;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("no app data dir");
            std::fs::create_dir_all(&dir)?;
            let settings = JsonStore::new(dir.join("settings.json"), default_settings());
            let initial_library = scan_from_settings(&settings);
            app.manage(AppState { settings, library: Mutex::new(initial_library) });

            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Lounge")
                .inner_size(1600.0, 900.0)
                .min_inner_size(960.0, 540.0)
                .background_color(tauri::webview::Color(0x07, 0x08, 0x0c, 0xff))
                .initialization_script(INIT_SCRIPT)
                .visible(true)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_state, rescan, save_settings, not_yet_implemented, detect_vlc, quit, minimize, toggle_fullscreen])
        .run(tauri::generate_context!())
        .expect("error while running the Lounge Tauri shell");
}
