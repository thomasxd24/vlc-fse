//! The Tauri shell for Lounge: the 1:1 port of `main.js`'s orchestration, wired to the ported
//! `lounge-core` crate.
//!
//! Everything `main.js` does lives here with the same behaviour and the same payloads, so
//! `renderer/**` runs completely unmodified: library scanning and the Steam scan, the view model,
//! VLC playback with progress/resume/watch-time, game launching with suspend/resume, TMDB and game
//! enrichment in background threads, servers/remote browsing/transfers, the Start-menu apps list,
//! Tailscale (status/connect/login with QR), the system helper (volume/brightness/sleep), the
//! self-updater, and every settings side effect. `window.lounge` is created by an initialization
//! script mapping each method to `window.__TAURI__.core.invoke(...)`.
//!
//! Known gaps:
//! - **WebHID device permissions** (the Legion Go controllers' battery/attach toasts): wry/WebView2
//!   has no device-permission handler, so `legion_hid` reads the device natively and the renderer
//!   consumes its `legion-report` events instead of WebHID.
//! - While a game runs the page is blanked and the window minimised (freeing GPU work).

mod legion_hid;

use lounge_core::library;
use lounge_core::remote::{self, RemoteClient, ServerSpec};
use lounge_core::store::JsonStore;
use lounge_core::transfers::{AddSpec, TransferQueue};
use lounge_core::updater::{self, Updater};
use lounge_core::viewmodel::{self, SteamGames, Stores};
use lounge_core::{apps, gameinfo, games, metadata, steam, system, tailscale, vlc};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

const REPO: &str = "thomasxd24/vlc-fse";
const UPDATE_FIRST_CHECK_MS: u64 = 30 * 1000;
const UPDATE_INTERVAL_MS: u64 = 6 * 3600 * 1000;
const SUSPEND_DELAY_MS: u64 = 4000;
const APP_STEP_ASIDE_MS: u64 = 1500;
/// Seconds of playback credited per VLC poll; bigger jumps are seeks, not watching.
const MAX_WATCH_STEP: f64 = 5.0;

fn default_settings() -> Map<String, Value> {
    Map::from_iter([
        ("libraries".into(), json!([])), // { path, type: 'movies' | 'tv' }
        ("vlcPath".into(), json!("")),
        ("vlcFullscreen".into(), json!(true)),
        ("vlcExtraArgs".into(), json!("")),
        ("autoplayNext".into(), json!(true)),
        ("audioLanguage".into(), json!("en")), // a language code, or 'original' for the file's default track
        ("subLanguage".into(), json!("en")),   // a language code, or 'off'
        ("tmdbKey".into(), json!("")),
        ("sgdbKey".into(), json!("")),
        ("steamEnabled".into(), json!(true)),
        ("steamPath".into(), json!("")),
        ("quietSteam".into(), json!(true)), // launch Steam games without Steam's own window coming up
        ("uiLanguage".into(), json!("auto")), // 'auto' | 'en' | 'fr'
        ("startFullscreen".into(), json!(true)),
        ("launchAtLogin".into(), json!(false)),
        ("uiScale".into(), json!(1)),
        ("haptics".into(), json!(true)),
        ("sounds".into(), json!(true)),
        ("animations".into(), json!("full")), // 'full' | 'reduced'
        ("freeWhilePlaying".into(), json!(true)),
        ("autoCheckUpdates".into(), json!(true)),
        ("skippedVersion".into(), json!("")),
    ])
}

// ---------------------------------------------------------------------------
// Paths & URLs

/// A local filesystem path as a `file://` URL, including the Windows drive-letter form.
fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if let Some(rest) = s.strip_prefix('/') {
        if !rest.starts_with('/') && rest.as_bytes().get(1) == Some(&b':') {
            // C:/... -> file:///C:/...
            return format!("file:///{rest}");
        }
        return format!("file:///{rest}");
    }
    format!("file:///{s}")
}

fn file_url_opt(p: Option<&str>) -> Value {
    match p {
        Some(p) if !p.is_empty() => json!(file_url(Path::new(p))),
        _ => Value::Null,
    }
}

/// An object literal as a `Map`, for JsonStore defaults.
fn jmap(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Windows paths lowercase in progress keys, same as the JS `progressKey`.
fn progress_stores(st: &AppState) -> Stores<'_> {
    Stores { progress: &st.progress, meta: &st.meta, games: &st.games_store, game_info: &st.game_info_store, prefs: &st.prefs, stats: &st.stats }
}

fn ui_lang(st: &AppState) -> &'static str {
    match st.settings.get("uiLanguage").and_then(|v| v.as_str().map(str::to_string)).as_deref() {
        Some("en") => "en",
        Some("fr") => "fr",
        _ => {
            // `app.getLocale()`: the OS UI language. Env vars cover Linux/macOS dev; on Windows the
            // lang env isn't reliably set, so this defaults to English there unless set.
            let locale = std::env::var("LC_ALL").or_else(|_| std::env::var("LANG")).or_else(|_| std::env::var("LOUNGE_LOCALE")).unwrap_or_default();
            if locale.to_lowercase().starts_with("fr") { "fr" } else { "en" }
        }
    }
}


// ---------------------------------------------------------------------------
// Application state

struct AppState {
    settings: JsonStore,
    progress: JsonStore,
    meta: JsonStore,
    games_store: JsonStore,
    game_info_store: JsonStore,
    prefs: JsonStore,
    stats: JsonStore,
    servers: JsonStore,
    apps_store: JsonStore,
    library: Mutex<library::LibraryScan>,
    steam_games: Mutex<SteamGames>,
    ui_state: Mutex<Option<Value>>,
    toasts: Mutex<Vec<Value>>,
    now_playing: Mutex<Option<Value>>,
    play: Mutex<Option<PlaySession>>,
    game: Mutex<Option<Value>>,
    game_session: Mutex<Option<games::GameSession>>,
    scanning: AtomicBool,
    apps_scanning: AtomicBool,
    meta_status: Mutex<(bool, Option<String>)>,
    game_info_status: Mutex<(bool, Option<String>)>,
    updater: Mutex<Option<Arc<Updater>>>,
    tailscale: tailscale::Tailscale,
    helper: system::SystemHelper,
    transfers: Arc<TransferQueue>,
    battery: Mutex<Option<Option<bool>>>,
    suspended: AtomicBool,
    /// Set when the game's suspend timer ran; cleared when the UI comes back.
    artwork_dir: PathBuf,
}

/// One VLC playback session and the bookkeeping its progress events need.
struct PlaySession {
    session: vlc::VlcSession,
    title: String,
    queue: Vec<viewmodel::PlayItem>,
    started_at: i64,
    /// Seconds actually spent watching per queue item (seeks don't count).
    watched: HashMap<String, f64>,
    last_pos: Option<(String, f64)>,
}

impl AppState {
    fn stores(&self) -> Stores<'_> {
        progress_stores(self)
    }

    fn user_file(&self, name: &str) -> PathBuf {
        self.artwork_dir.parent().unwrap_or(Path::new(".")).join(name)
    }

    /// A fresh `GameInfo` over the shared store: every instance sees the same in-memory state, so
    /// enrichment threads and interactive commands never block each other (the JS version interleaves
    /// through the event loop; here they share the store's lock instead).
    fn make_game_info(&self) -> gameinfo::GameInfo {
        gameinfo::GameInfo::new(
            self.game_info_store.clone(),
            self.artwork_dir.clone(),
            self.settings.get("sgdbKey").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            Some(ui_lang(self)),
        )
    }

    fn make_metadata(&self) -> metadata::Metadata {
        metadata::Metadata::new(
            self.meta.clone(),
            self.artwork_dir.clone(),
            self.settings.get("tmdbKey").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            Some(if ui_lang(self) == "fr" { "fr-FR".to_string() } else { "en-US".to_string() }),
        )
    }

    /// Raw games (Steam scan + manual), with overrides applied, as fed to both the UI and GameInfo.
    fn raw_games(&self, steam: &SteamGames) -> Vec<Value> {
        viewmodel::raw_games(steam, &self.stores())
    }

    fn flush_all(&self) {
        for store in [&self.settings, &self.progress, &self.meta, &self.games_store, &self.game_info_store, &self.prefs, &self.stats, &self.servers, &self.apps_store] {
            let _ = store.flush();
        }
    }

    fn push_state(&self, app: &AppHandle) {
        let _ = app.emit("state", build_state(self));
    }
}

// ---------------------------------------------------------------------------
// The state payload (main.js's `state()`)

fn public_server(x: &Value) -> Value {
    // A server as the page sees it: never the password.
    let mut o = x.as_object().cloned().unwrap_or_default();
    o.remove("secret");
    let mut v = Value::Object(o);
    v["hasSecret"] = json!(!x.get("secret").and_then(Value::as_str).unwrap_or("").is_empty());
    v
}

fn apps_view(st: &AppState) -> Vec<Value> {
    let icons = st.apps_store.get("icons").unwrap_or_else(|| json!({}));
    let hidden = st.apps_store.get("hidden").unwrap_or_else(|| json!({}));
    let recent = st.apps_store.get("recent").unwrap_or_else(|| json!({}));
    let ts_apps: Vec<Value> = st.apps_store.get("apps").and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().filter(|a: &Value| is_tailscale_app(a)).collect();
    let ts_ids: Vec<&str> = ts_apps.iter().filter_map(|a| a.get("id").and_then(Value::as_str)).collect();
    st.apps_store.get("apps").and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().map(|a| {
        let id = a.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let icon = icons.get(&id).and_then(Value::as_str).filter(|p| Path::new(p).is_file()).map(|p| file_url(Path::new(p)));
        json!({
            "id": id,
            "name": a.get("name"),
            "kind": a.get("kind"),
            "icon": icon,
            "hidden": hidden.get(&id).map(|h| !h.is_null()).unwrap_or(false),
            "lastLaunched": recent.get(&id).cloned().unwrap_or(json!(0)),
            "tailscale": ts_ids.contains(&id.as_str()),
        })
    }).collect()
}

fn is_tailscale_app(a: &Value) -> bool {
    let name = a.get("name").and_then(Value::as_str).unwrap_or("").to_lowercase();
    let exe = a.get("exe").and_then(Value::as_str).unwrap_or("").to_lowercase();
    name.contains("tailscale") || exe.contains("tailscale")
}

fn update_state_json(st: &updater::UpdateState) -> Value {
    let status = match st.status {
        updater::Status::Unsupported => "unsupported",
        updater::Status::Idle => "idle",
        updater::Status::Checking => "checking",
        updater::Status::UpToDate => "uptodate",
        updater::Status::Available => "available",
        updater::Status::Downloading => "downloading",
        updater::Status::Ready => "ready",
        updater::Status::Installing => "installing",
        updater::Status::Elevating => "elevating",
        updater::Status::Error => "error",
    };
    json!({
        "status": status,
        "current": st.current,
        "version": st.version,
        "notes": st.notes,
        "progress": st.progress,
        "size": st.size,
        "error": st.error,
        "checkedAt": st.checked_at,
    })
}

fn build_state(st: &AppState) -> Value {
    let lib = st.library.lock().unwrap();
    let steam = st.steam_games.lock().unwrap();
    let vm = viewmodel::build_view_model(&lib, &steam, &st.stores(), &file_url);

    // Overlay whatever's actually persisted over the defaults, exactly like settings.data merging.
    let mut settings_map = default_settings();
    for key in settings_map.clone().keys() {
        if let Some(v) = st.settings.get(key) {
            settings_map.insert(key.clone(), v);
        }
    }

    let servers = st.servers.get("servers").and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().map(public_server).collect::<Vec<_>>();
    let update = st.updater.lock().unwrap().as_ref().map(|u| update_state_json(&u.state()));
    let meta_status = st.meta_status.lock().unwrap();
    let game_info_status = st.game_info_status.lock().unwrap();
    json!({
        "settings": settings_map,
        "lang": ui_lang(st),
        "library": vm,
        "scanning": st.scanning.load(Ordering::SeqCst),
        "metaStatus": {"running": meta_status.0, "error": meta_status.1},
        "gameInfoStatus": {"running": game_info_status.0, "error": game_info_status.1},
        "nowPlaying": st.now_playing.lock().unwrap().clone(),
        "game": st.game.lock().unwrap().clone(),
        "uiState": st.ui_state.lock().unwrap().clone(),
        "toasts": st.toasts.lock().unwrap().drain(..).collect::<Vec<_>>(),
        "platform": platform(),
        "packaged": !cfg!(debug_assertions),
        "fsePackage": std::env::var("LOUNGE_FSE_PACKAGE").map(|v| v == "1").unwrap_or(false),
        "systemControls": st.helper.supported(),
        "update": update,
        "hasBattery": (*st.battery.lock().unwrap()).flatten(),
        "servers": servers,
        "apps": apps_view(st),
        "appsScanning": st.apps_scanning.load(Ordering::SeqCst),
        "appsScannedAt": st.apps_store.get("scannedAt").unwrap_or(json!(0)),
        "tailscaleInstalled": st.tailscale.installed(),
        "tailscaleCli": st.tailscale.cli().map(|c| json!(c)).unwrap_or(Value::Null),
        "tailscaleTried": st.tailscale.tried().iter().map(|t| json!({"path": t.path, "source": t.source, "found": t.found})).collect::<Vec<_>>(),
        "transfers": st.transfers.state().iter().map(transfer_state_json).collect::<Vec<_>>(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn transfer_state_json(j: &lounge_core::transfers::JobState) -> Value {
    let status = match j.status {
        lounge_core::transfers::JobStatus::Queued => "queued",
        lounge_core::transfers::JobStatus::Running => "running",
        lounge_core::transfers::JobStatus::Done => "done",
        lounge_core::transfers::JobStatus::Error => "error",
        lounge_core::transfers::JobStatus::Cancelled => "cancelled",
    };
    let kind = match j.kind {
        lounge_core::transfer_plan::Kind::Movie => "movie",
        lounge_core::transfer_plan::Kind::Tv => "tv",
    };
    json!({
        "id": j.id, "serverId": j.server_id, "title": j.title, "kind": kind, "status": status,
        "files": j.files, "fileIndex": j.file_index, "bytesDone": j.bytes_done, "bytesTotal": j.bytes_total,
        "rate": j.rate, "skipped": j.skipped, "error": j.error, "folders": j.folders,
    })
}

// ---------------------------------------------------------------------------
// Scanning & enrichment

fn scan_games_blocking(settings: &JsonStore) -> SteamGames {
    if !settings.get("steamEnabled").and_then(|v| v.as_bool()).unwrap_or(true) {
        return SteamGames::disabled();
    }
    let preferred = settings.get("steamPath").and_then(|v| v.as_str().map(str::to_string)).filter(|s| !s.is_empty()).map(PathBuf::from);
    match steam::scan_steam(preferred.as_deref(), &steam::SteamEnv::from_process_env()) {
        Some(scan) => SteamGames {
            steam_path: Some(scan.steam_path.clone()),
            steam_user: scan.user.as_ref().and_then(|u| u.name.clone()),
            entries: scan.games.iter().map(viewmodel::steam_game_json).collect(),
        },
        None => SteamGames::disabled(),
    }
}

fn do_rescan(st: &AppState, app: &AppHandle) {
    st.scanning.store(true, Ordering::SeqCst);
    st.push_state(app);
    let settings = st.settings.clone();
    // Blocking disk walks run off the main thread, like the JS version's async scan.
    let settings_for_steam = settings.clone();
    let media = tauri::async_runtime::spawn_blocking(move || {
        let libs_json = settings.get("libraries").and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let defs: Vec<library::LibraryDef> = libs_json.iter().filter_map(|l| {
            let p = l.get("path").and_then(Value::as_str)?;
            let kind = if l.get("type").and_then(Value::as_str) == Some("tv") { library::LibraryKind::Tv } else { library::LibraryKind::Movies };
            Some(library::LibraryDef { path: Path::new(p), kind })
        }).collect();
        library::scan_libraries(&defs)
    });
    let (media, steam_games) = tauri::async_runtime::block_on(async {
        let m = media.await.unwrap_or_else(|_| library::scan_libraries(&[]));
        let s = tauri::async_runtime::spawn_blocking(move || scan_games_blocking(&settings_for_steam)).await.unwrap_or_default();
        (m, s)
    });
    *st.library.lock().unwrap() = media;
    *st.steam_games.lock().unwrap() = steam_games;
    st.scanning.store(false, Ordering::SeqCst);
    st.push_state(app);
    enrich_metadata(app);
    enrich_games(app);
}

/// Refetch synopses/descriptions in the background (TMDB), pushing state as entries arrive.
fn enrich_metadata(app: &AppHandle) {
    let metadata = {
        let st = app.state::<AppState>();
        let m = st.make_metadata();
        if !m.enabled() {
            return;
        }
        let mut status = st.meta_status.lock().unwrap();
        if status.0 {
            return;
        }
        *status = (true, None);
        m
    };
    {
        let st = app.state::<AppState>();
        st.push_state(app);
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let lib = st.library.lock().unwrap().clone();
        let movies: Vec<metadata::MovieRef> = lib.movies.iter().map(|m| metadata::MovieRef { title: &m.title, year: m.year }).collect();
        let show_eps: Vec<Vec<metadata::EpisodeRef>> = lib.shows.iter().map(|s| {
            s.episodes.iter().map(|e| metadata::EpisodeRef { season: e.season, episode: e.episode.unwrap_or(0), has_thumb: e.thumb.is_some() }).collect()
        }).collect();
        let shows: Vec<metadata::ShowRef> = lib.shows.iter().zip(&show_eps).map(|(s, eps)| metadata::ShowRef { title: &s.title, year: s.year, episodes: eps }).collect();
        let lib_ref = metadata::LibraryRef { movies: &movies, shows: &shows };
        let on_update = || {
            st.push_state(&app);
        };
        let result = metadata.enrich(&lib_ref, on_update);
        let error = match result {
            Err(e) => Some(e.to_string()),
            Ok(_) => None,
        };
        *st.meta_status.lock().unwrap() = (false, error);
        st.push_state(&app);
    });
}

/// Refetch game descriptions/art in the background (Steam store + SteamGridDB).
fn enrich_games(app: &AppHandle) {
    {
        let st = app.state::<AppState>();
        let mut status = st.game_info_status.lock().unwrap();
        if status.0 {
            return;
        }
        *status = (true, None);
        st.push_state(app);
    }
    let mut game_info = {
        let st = app.state::<AppState>();
        st.make_game_info()
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let raw = {
            let steam = st.steam_games.lock().unwrap();
            st.raw_games(&steam)
        };
        let refs: Vec<gameinfo::GameRef> = raw.iter().map(|g| gameinfo::GameRef {
            id: g.get("id").and_then(Value::as_str).unwrap_or(""),
            source: g.get("source").and_then(Value::as_str).unwrap_or(""),
            appid: g.get("appid").and_then(Value::as_str),
            title: g.get("title").and_then(Value::as_str).unwrap_or(""),
            art: gameinfo::ExistingArt {
                poster: path_is_file(g.pointer("/art/poster")),
                hero: path_is_file(g.pointer("/art/hero")),
                logo: path_is_file(g.pointer("/art/logo")),
                header: path_is_file(g.pointer("/art/header")),
            },
            r#override: gameinfo::GameOverride {
                steam_app_id: g.pointer("/override/steamAppId").and_then(Value::as_str).map(String::from),
                sgdb_id: g.pointer("/override/sgdbId").and_then(Value::as_i64),
                no_steam_match: g.pointer("/override/noSteamMatch").and_then(Value::as_bool).unwrap_or(false),
            },
        }).collect();
        let on_update = || {
            st.push_state(&app);
        };
        let result = game_info.enrich(&refs, on_update, || false);
        let error = match result {
            Err(e) => Some(e.to_string()),
            Ok(_) => None,
        };
        *st.game_info_status.lock().unwrap() = (false, error);
        st.push_state(&app);
    });
}

fn path_is_file(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str).map(|p| Path::new(p).is_file()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Window bring-to-front / suspend / resume

fn bring_to_front(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        let fs = app.state::<AppState>().settings.get("startFullscreen").and_then(|v| v.as_bool()).unwrap_or(true);
        let _ = win.set_fullscreen(fs);
    }
}

/// While a game runs, Lounge gets out of the way: the page is blanked, the window minimised, and
/// every Lounge process drops to low CPU priority. It all comes back when the game exits or when
/// you switch back to Lounge.
#[cfg(target_os = "windows")]
fn suspend_ui(app: &AppHandle) {
    let st = app.state::<AppState>();
    if st.suspended.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.eval("document.open(); document.write('<!doctype html><body style=\"background:#07080c\"></body>');");
        let _ = win.minimize();
    }
    let pids: Vec<u32> = std::process::id().to_le_bytes().iter().map(|_| std::process::id()).collect();
    system::set_priority(&pids, true);
}

#[cfg(not(target_os = "windows"))]
fn suspend_ui(app: &AppHandle) {
    let st = app.state::<AppState>();
    if st.suspended.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.eval("document.open(); document.write('<!doctype html><body style=\"background:#07080c\"></body>');");
        let _ = win.minimize();
    }
}

fn resume_ui(app: &AppHandle) {
    let st = app.state::<AppState>();
    if st.suspended.swap(false, Ordering::SeqCst) {
        if let Some(win) = app.get_webview_window("main") {
            // Back to the real page; `?resume=1` skips the startup intro, like the JS version.
            let _ = win.eval("location.replace(location.pathname + '?resume=1');");
        }
    }
    bring_to_front(app);
}

fn toast(st: &AppState, app: &AppHandle, key: &str, vars: Value, kind: &str) {
    let t = json!({"key": key, "vars": vars, "kind": kind});
    let _ = app.emit("toast", t.clone());
    st.toasts.lock().unwrap().push(t);
}

// ---------------------------------------------------------------------------
// Media playback (VLC)

#[tauri::command]
fn play(app: AppHandle, st: State<AppState>, req: Value) -> Value {
    {
        let play = st.play.lock().unwrap();
        if let Some(p) = play.as_ref() {
            p.session.kill();
        }
    }
    let vlc_path = vlc::find_vlc(
        st.settings.get("vlcPath").and_then(|v| v.as_str().map(str::to_string)).filter(|s| !s.is_empty()).map(PathBuf::from).as_deref(),
        &vlc::VlcEnv::from_process_env(),
    );
    let Some(vlc_path) = vlc_path else { return json!({"ok": false, "errorKey": "err.vlcNotFound"}) };
    let job = viewmodel::build_queue(&st.library.lock().unwrap(), &st.stores(), &req, st.settings.get("autoplayNext").and_then(|v| v.as_bool()).unwrap_or(true));
    let Ok((title, queue)) = job else { return json!({"ok": false, "errorKey": "err.notFound"}) };

    // Per-item resume positions and language overrides; the Settings defaults ride in as global
    // options, which the per-item ones then override (the JS version's same ordering trick).
    let extra_args = split_args(&st.settings.get("vlcExtraArgs").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default());
    let mut items: Vec<vlc::QueueItem> = queue.iter().map(|i| vlc::QueueItem {
        path: &i.path,
        start_time: i.start_time,
        languages: i.languages.as_ref().map(|l| vlc::Languages {
            audio: l.get("audio").and_then(Value::as_str),
            subs: l.get("subs").and_then(Value::as_str),
        }),
    }).collect();
    let port = match vlc::free_port_pub() {
        Ok(p) => p,
        Err(e) => { toast(&st, &app, "err.vlcStart", json!({"message": e.to_string()}), "error"); return json!({"ok": false}); }
    };
    let password = vlc::random_password_pub();
    let audio_lang = st.settings.get("audioLanguage").and_then(|v| v.as_str().map(str::to_string));
    let sub_lang = st.settings.get("subLanguage").and_then(|v| v.as_str().map(str::to_string));
    let opts = vlc::BuildArgsOptions {
        port,
        password: &password,
        fullscreen: st.settings.get("vlcFullscreen").and_then(|v| v.as_bool()).unwrap_or(true),
        languages: vlc::Languages { audio: audio_lang.as_deref(), subs: sub_lang.as_deref() },
        extra_args: &extra_args.iter().map(String::as_str).collect::<Vec<_>>(),
    };
    let _ = &mut items;
    let started = vlc::VlcSession::spawn(&vlc_path, &items, &opts);
    let session = match started {
        Ok(s) => s,
        Err(e) => {
            toast(&st, &app, "err.vlcStart", json!({"message": e.to_string()}), "error");
            return json!({"ok": false});
        }
    };
    let current = queue.first().map(|i| i.path.clone()).unwrap_or_default();
    *st.play.lock().unwrap() = Some(PlaySession {
        session,
        title,
        watched: HashMap::new(),
        last_pos: None,
        started_at: now_millis(),
        queue,
    });
    let play = st.play.lock().unwrap();
    let ps = play.as_ref().unwrap();
    *st.now_playing.lock().unwrap() = Some(json!({
        "title": ps.title, "request": req, "current": current,
        "time": 0, "length": 0, "paused": false, "queueSize": ps.queue.len(), "index": 0,
    }));
    drop(play);
    let _ = app.emit("now-playing", st.now_playing.lock().unwrap().clone());
    power_save_blocker_start(app.state::<AppState>());
    json!({"ok": true})
}

/// Split a launch-options / extra-args string, honouring double quotes.
fn split_args(s: &str) -> Vec<String> {
    lounge_core::games::split_args(s)
}

#[tauri::command]
fn stop(app: AppHandle, st: State<AppState>) {
    if let Some(p) = st.play.lock().unwrap().as_ref() {
        p.session.kill();
    }
    let _ = app;
}

/// Playback controls on the "Playing in VLC" screen, mapped to VLC's HTTP commands. Track switching
/// goes through VLC's own hotkeys so the on-screen label updates.
#[tauri::command]
fn np_command(st: State<AppState>, name: String) {
    let commands: &[(&str, &[&str])] = &[
        ("pause", &["pl_pause"]),
        ("back", &["seek", "-10"]),
        ("forward", &["seek", "+30"]),
        ("next", &["pl_next"]),
        ("audio", &["key", "audio-track"]),
        ("subs", &["key", "subtitle-track"]),
    ];
    let Some((cmd, args)) = commands.iter().find(|(n, _)| *n == name) else { return };
    let play = st.play.lock().unwrap();
    let Some(ps) = play.as_ref() else { return };
    let status = ps.session.command(cmd, args.get(1).copied());
    if let Some(s) = status {
        let mut np = st.now_playing.lock().unwrap();
        if let Some(np) = np.as_mut() {
            if let Some(state) = s.get("state").and_then(Value::as_str) {
                np["paused"] = json!(state == "paused");
            }
            if let Some(time) = s.get("time") {
                np["time"] = time.clone();
            }
        }
    }
}

#[tauri::command]
fn set_watched(app: AppHandle, st: State<AppState>, req: Value) {
    let lib = st.library.lock().unwrap().clone();
    let paths = viewmodel::paths_for(&lib, &req);
    let watched = req.get("watched").and_then(Value::as_bool).unwrap_or(false);
    viewmodel::set_watched(&st.stores(), &paths, watched);
    st.push_state(&app);
}

#[tauri::command]
fn set_languages(app: AppHandle, st: State<AppState>, id: String, languages: Option<Value>) {
    viewmodel::set_languages(&st.stores(), &id, languages.as_ref());
    st.push_state(&app);
}

#[tauri::command]
fn set_pref(app: AppHandle, st: State<AppState>, id: String, key: String, value: bool) {
    viewmodel::set_pref(&st.stores(), &id, &key, value);
    st.push_state(&app);
}

#[tauri::command]
fn get_stats(st: State<AppState>) -> Value {
    let lib = st.library.lock().unwrap().clone();
    let steam = st.steam_games.lock().unwrap();
    viewmodel::stats_data(&lib, &steam, &st.stores(), &file_url)
}

#[tauri::command]
fn save_ui_state(st: State<AppState>, s: Value) {
    *st.ui_state.lock().unwrap() = Some(s);
}

// ---------------------------------------------------------------------------
// Games

#[tauri::command]
fn play_game(app: AppHandle, st: State<AppState>, id: String) -> Value {
    if st.game_session.lock().unwrap().is_some() {
        return json!({"ok": false, "errorKey": "err.alreadyPlaying"});
    }
    let steam = st.steam_games.lock().unwrap();
    let raw = st.raw_games(&steam).into_iter().find(|g| g.get("id").and_then(Value::as_str) == Some(id.as_str()));
    drop(steam);
    let Some(game) = raw else { return json!({"ok": false, "errorKey": "err.notFound"}) };
    let sg = games::SessionGame::from_json(&game);
    if sg.source == "manual" && !Path::new(&sg.exe).is_file() {
        return json!({"ok": false, "errorKey": "err.exeMissing"});
    }

    let quiet_steam = st.settings.get("quietSteam").and_then(|v| v.as_bool()).unwrap_or(true);
    let steam_path = st.steam_games.lock().unwrap().steam_path.clone();
    let deps = games::GameDeps {
        open_external: Box::new(move |url| { let _ = open_url_in_shell(url); }),
        open_path: Box::new(|p| {
            open_path_in_shell(Path::new(p)).err().map(|e| e.to_string())
        }),
        launch_steam: if quiet_steam {
            Some(Box::new(move |appid| steam::launch_quietly(appid, steam_path.as_deref()).unwrap_or(false)))
        } else {
            None
        },
        running_app_id: Box::new(steam::running_app_id),
        tuning: games::Tuning::default(),
    };
    let session = match games::GameSession::spawn(&sg, deps) {
        Ok(s) => s,
        Err(e) => {
            return json!({"ok": false, "errorKey": "err.gameStart", "vars": {"message": e.to_string()}});
        }
    };
    *st.game.lock().unwrap() = Some(json!({"id": id, "title": sg.title, "phase": "launching", "startedAt": now_millis()}));
    let _ = app.emit("game", st.game.lock().unwrap().clone());
    *st.game_session.lock().unwrap() = Some(session);
    json!({"ok": true})
}

#[tauri::command]
fn back_to_game(app: AppHandle, st: State<AppState>) {
    if st.game_session.lock().unwrap().is_some() {
        suspend_ui(&app);
    }
}

#[tauri::command]
fn end_game(app: AppHandle, st: State<AppState>) {
    let untracked = st.game.lock().unwrap().as_ref().and_then(|g| g.get("phase").and_then(Value::as_str)) == Some("untracked");
    if untracked {
        // Already counted as a launch; the exit event was swallowed. We couldn't watch the game
        // itself, so count the time until the user said they were done.
        let (game_id, started_at) = {
            let g = st.game.lock().unwrap();
            (
                g.as_ref().and_then(|g| g.get("id").and_then(Value::as_str)).unwrap_or("").to_string(),
                g.as_ref().and_then(|g| g.get("startedAt").and_then(Value::as_i64)).unwrap_or(0),
            )
        };
        record_play(&st, &game_id, now_millis() - started_at);
        *st.game_session.lock().unwrap() = None;
        *st.game.lock().unwrap() = None;
        let _ = app.emit("game", Value::Null);
        st.push_state(&app);
    } else if let Some(s) = st.game_session.lock().unwrap().as_ref() {
        s.stop_tracking();
    }
}

fn record_play(st: &AppState, id: &str, played_ms: i64) {
    st.games_store.update("stats", json!({}), |stats| {
        let Value::Object(m) = stats else { return };
        let entry = m.entry(id.to_string()).or_insert_with(|| json!({"playtime": 0, "lastPlayed": 0}));
        entry["lastPlayed"] = json!(now_millis());
        let prev = entry["playtime"].as_i64().unwrap_or(0);
        entry["playtime"] = json!(prev + (played_ms / 60000));
    });
    viewmodel::log_session(&st.stores(), "game", id, now_millis() - played_ms, played_ms as f64 / 60000.0);
}

/// The icon of a manual game's exe, saved as a PNG in the artwork folder (`app.getFileIcon` was the
/// Electron way; here a one-shot PowerShell `ExtractAssociatedIcon` call does the same job).
#[cfg(target_os = "windows")]
fn icon_for(artwork_dir: &Path, exe: &str) -> Option<String> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(exe.to_lowercase());
    let hash: String = b64.chars().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
    let _ = std::fs::create_dir_all(artwork_dir);
    let dest = artwork_dir.join(format!("icon-{hash}.png"));
    let script = format!(
        "Add-Type -AssemblyName System.Drawing; $i = [System.Drawing.Icon]::ExtractAssociatedIcon('{}'); if ($i) {{ $i.ToBitmap().Save('{}', [System.Drawing.Imaging.ImageFormat]::Png) }}",
        exe.replace('\'', "''"),
        dest.to_string_lossy().replace('\'', "''")
    );
    let ok = std::process::Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    (ok && dest.is_file()).then(|| dest.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "windows"))]
fn icon_for(_artwork_dir: &Path, _exe: &str) -> Option<String> {
    None
}

#[tauri::command]
fn add_game(app: AppHandle, st: State<AppState>) -> Value {
    use tauri_plugin_dialog::DialogExt;
    let picked = app.dialog().file().add_filter("Games", &["exe", "bat", "cmd", "lnk", "url"]).blocking_pick_file();
    let Some(picked) = picked else { return json!({"ok": false}) };
    let Some(exe) = picked.into_path().ok().map(|p| p.to_string_lossy().into_owned()) else { return json!({"ok": false}) };

    let manual = st.games_store.get("manual").and_then(|v| v.as_array().cloned()).unwrap_or_default();
    if manual.iter().any(|g| g.get("exe").and_then(Value::as_str).map(|e| e.to_lowercase() == exe.to_lowercase()).unwrap_or(false)) {
        return json!({"ok": false, "errorKey": "err.gameExists"});
    }
    let id = games::manual_id(&exe);
    let game = json!({
        "id": id,
        "source": "manual",
        "title": games::title_from_exe(&exe),
        "exe": exe,
        "args": "",
        "cwd": "",
        "addedAt": now_millis(),
        "art": {"icon": icon_for(&st.artwork_dir, &exe)},
    });
    st.games_store.update("manual", json!([]), |m| {
        if let Value::Array(a) = m {
            a.push(game.clone());
        }
    });
    st.push_state(&app);
    enrich_games(&app);
    json!({"ok": true, "id": id})
}

#[tauri::command]
fn edit_game(app: AppHandle, st: State<AppState>, id: String, patch: Value) {
    let mut o = st.games_store.get("overrides").and_then(|v| v.get(&id).cloned()).unwrap_or_else(|| json!({}));
    let manual_index = st.games_store.get("manual").and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().position(|g| g.get("id").and_then(Value::as_str) == Some(id.as_str()));
    let mut refetch = false;
    if let Some(title) = patch.get("title") {
        let t = title.as_str().unwrap_or("").trim().to_string();
        match manual_index {
            Some(i) => {
                st.games_store.update("manual", json!([]), |m| {
                    if let Value::Array(a) = m {
                        if !t.is_empty() {
                            a[i]["title"] = json!(t);
                        }
                    }
                });
            }
            None => {
                if t.is_empty() {
                    if let Value::Object(m) = &mut o {
                        m.remove("title");
                    }
                } else {
                    o["title"] = json!(t);
                }
            }
        }
        refetch = true;
    }
    if let Some(args) = patch.get("args") {
        if manual_index.is_some() {
            let v = args.as_str().unwrap_or("").to_string();
            st.games_store.update("manual", json!([]), |m| {
                if let Value::Array(a) = m {
                    if let Some(i) = a.iter().position(|g| g.get("id").and_then(Value::as_str) == Some(id.as_str())) {
                        a[i]["args"] = json!(v);
                    }
                }
            });
        }
    }
    if let Some(steam_app_id) = patch.get("steamAppId") {
        if steam_app_id.is_null() {
            if let Value::Object(m) = &mut o {
                m.insert("noSteamMatch".into(), json!(true));
                m.remove("steamAppId");
            }
        } else {
            let v = match steam_app_id {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            o["steamAppId"] = json!(v);
            if let Value::Object(m) = &mut o {
                m.remove("noSteamMatch");
            }
        }
        refetch = true;
    }
    if let Some(sgdb_id) = patch.get("sgdbId") {
        if sgdb_id.is_null() || sgdb_id.as_i64() == Some(0) {
            if let Value::Object(m) = &mut o {
                m.remove("sgdbId");
            }
        } else {
            o["sgdbId"] = sgdb_id.clone();
        }
        refetch = true;
    }
    if let Some(art) = patch.get("art").and_then(Value::as_object) {
        let mut art_map = o.get("art").and_then(Value::as_object).cloned().unwrap_or_default();
        for (k, v) in art {
            art_map.insert(k.clone(), v.clone());
        }
        o["art"] = Value::Object(art_map);
    }
    st.games_store.update("overrides", json!({}), |m| {
        if let Value::Object(m) = m {
            m.insert(id.clone(), o.clone());
        }
    });
    if refetch {
        st.make_game_info().forget(&id);
        enrich_games(&app);
    }
    st.push_state(&app);
}

#[tauri::command]
fn remove_game(app: AppHandle, st: State<AppState>, id: String) {
    st.games_store.update("manual", json!([]), |m| {
        if let Value::Array(a) = m {
            a.retain(|g| g.get("id").and_then(Value::as_str) != Some(id.as_str()));
        }
    });
    st.games_store.update("overrides", json!({}), |m| {
        if let Value::Object(m) = m {
            m.remove(&id);
        }
    });
    st.make_game_info().forget(&id);
    st.push_state(&app);
}

#[tauri::command]
fn search_steam(st: State<AppState>, term: String) -> Value {
    let mut game_info = st.make_game_info();
    match game_info.store_search(&term) {
        Ok(hits) => json!(hits.iter().map(|h| json!({"steamAppId": h.steam_app_id, "title": h.title, "thumb": h.thumb})).collect::<Vec<_>>()),
        Err(_) => json!([]),
    }
}

#[tauri::command]
fn search_sgdb(st: State<AppState>, term: String) -> Value {
    let game_info = st.make_game_info();
    match game_info.sgdb_search(&term) {
        Ok(hits) => json!(hits.iter().map(|h| json!({"sgdbId": h.sgdb_id, "title": h.title, "year": h.year})).collect::<Vec<_>>()),
        Err(e) => json!({"error": e.to_string()}),
    }
}

#[tauri::command]
fn sgdb_images(app: AppHandle, st: State<AppState>, kind: String, id: String) -> Value {
    let steam = st.steam_games.lock().unwrap();
    let raw = st.raw_games(&steam).into_iter().find(|g| g.get("id").and_then(Value::as_str) == Some(id.as_str()));
    drop(steam);
    let Some(g) = raw else { return json!([]) };
    let info = st.make_game_info().lookup(&id).unwrap_or_else(|| json!({}));
    let ref_sgdb = g.pointer("/override/sgdbId").and_then(Value::as_i64).or_else(|| info.get("sgdbId").and_then(Value::as_i64));
    let ref_steam = g.pointer("/override/steamAppId").and_then(Value::as_str).map(String::from).or_else(|| info.get("steamAppId").and_then(Value::as_str).map(String::from)).or_else(|| g.get("appid").and_then(Value::as_str).map(String::from));
    let game_info = st.make_game_info();
    let Ok(imgs) = game_info.sgdb_images(&kind, ref_sgdb, ref_steam.as_deref()) else { return json!([]) };
    // Thumbnails are remote; download them so the page (which only shows local files) can display them.
    let mut out = Vec::new();
    for i in imgs.into_iter().take(12) {
        if let Some(thumb) = game_info.download(&i.thumb) {
            out.push(json!({"url": i.url, "thumb": file_url(Path::new(&thumb))}));
        }
    }
    let _ = app;
    json!(out)
}

#[tauri::command]
fn set_game_art(app: AppHandle, st: State<AppState>, id: String, kind: String, url: String) -> Value {
    let game_info = st.make_game_info();
    let Some(p) = game_info.download(&url) else { return json!({"ok": false}) };
    let mut art = Map::new();
    art.insert(kind, json!(p));
    edit_game(app, st, id, json!({"art": Value::Object(art)}));
    json!({"ok": true})
}

#[tauri::command]
fn screenshot(st: State<AppState>, url: String) -> Value {
    // Only Steam's CDN domains, like the JS version's allowlist.
    let allowed = regex_lite_steam_cdn(&url);
    if !allowed {
        return Value::Null;
    }
    let game_info = st.make_game_info();
    match game_info.download(&url) {
        Some(p) => file_url_opt(p.to_str()),
        None => Value::Null,
    }
}

fn regex_lite_steam_cdn(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let Some((host, _)) = rest.split_once('/') else { return false };
    let host = host.to_lowercase();
    let ok_tld = host.ends_with(".com") || host.ends_with(".net");
    ok_tld && ["steamstatic", "steampowered", "akamaihd"].iter().any(|d| host.contains(d))
}

#[tauri::command]
fn show_game_folder(app: AppHandle, st: State<AppState>, id: String) {
    let steam = st.steam_games.lock().unwrap();
    let g = st.raw_games(&steam).into_iter().find(|g| g.get("id").and_then(Value::as_str) == Some(id.as_str()));
    drop(steam);
    if let Some(g) = g {
        let dir = g.get("installDir").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from).or_else(|| g.get("exe").and_then(Value::as_str).and_then(|e| Path::new(e).parent().map(|p| p.to_string_lossy().into_owned())));
        if let Some(dir) = dir {
            if Path::new(&dir).is_dir() {
                let _ = open_path_in_shell(Path::new(&dir));
            }
        }
    }
    let _ = app;
}

// ---------------------------------------------------------------------------
// Settings & dialogs

#[tauri::command]
fn save_settings(app: AppHandle, st: State<AppState>, patch: Map<String, Value>) -> Value {
    let before: Map<String, Value> = default_settings().keys().map(|k| (k.clone(), st.settings.get(k).unwrap_or(Value::Null))).collect();
    let prev_lang = ui_lang(&st);
    let allowed = default_settings();
    for (k, v) in patch {
        if allowed.contains_key(&k) {
            st.settings.set(k, v);
        }
    }
    let changed = |k: &str| before.get(k) != st.settings.get(k).as_ref();

    // Live fullscreen follows the checkbox, like the JS version's setFullScreen here.
    if let Some(win) = app.get_webview_window("main") {
        if changed("startFullscreen") {
            if let Some(fs) = st.settings.get("startFullscreen").and_then(|v| v.as_bool()) {
                let _ = win.set_fullscreen(fs);
            }
        }
    }
    let lang_changed = ui_lang(&st) != prev_lang;
    if lang_changed {
        // Synopses and game descriptions are per language: refetch them, but keep showing what we
        // have (artwork doesn't depend on language) until the new text arrives.
        for (store, key) in [(&st.meta, "entries"), (&st.game_info_store, "games")] {
            store.update(key, json!({}), |entries| {
                if let Value::Object(m) = entries {
                    for e in m.values_mut() {
                        e["stale"] = json!(true);
                    }
                }
            });
        }
    }
    if changed("sgdbKey") || lang_changed {
        enrich_games(&app);
    }
    if changed("libraries") || changed("steamEnabled") || changed("steamPath") {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let st = app2.state::<AppState>();
            do_rescan(&st, &app2);
        });
    } else if changed("tmdbKey") || lang_changed {
        enrich_metadata(&app);
    }
    st.push_state(&app);
    let mut settings_map = default_settings();
    for key in settings_map.clone().keys() {
        if let Some(v) = st.settings.get(key) {
            settings_map.insert(key.clone(), v);
        }
    }
    Value::Object(settings_map)
}

#[tauri::command]
fn pick_folder(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog().file().blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
fn pick_vlc(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog().file().add_filter("VLC", &["exe"]).blocking_pick_file().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
fn pick_key_file(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog().file().blocking_pick_file().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
fn detect_vlc(st: State<AppState>) -> Option<String> {
    vlc::find_vlc(
        st.settings.get("vlcPath").and_then(|v| v.as_str().map(str::to_string)).filter(|s| !s.is_empty()).map(PathBuf::from).as_deref(),
        &vlc::VlcEnv::from_process_env(),
    ).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
fn clear_metadata(app: AppHandle, st: State<AppState>) {
    st.meta.set("entries", json!({}));
    st.game_info_store.set("games", json!({}));
    enrich_metadata(&app);
    enrich_games(&app);
}

#[tauri::command]
fn show_in_folder(app: AppHandle, st: State<AppState>, p: String) {
    let lib = st.library.lock().unwrap();
    let known = lib.movies.iter().any(|m| m.path.to_string_lossy() == p) || lib.shows.iter().any(|s| s.episodes.iter().any(|e| e.path.to_string_lossy() == p));
    drop(lib);
    if known {
        let _ = reveal_in_shell(Path::new(&p));
    }
    let _ = app;
}

// ---------------------------------------------------------------------------
// System (quick menu & status bar)

#[tauri::command]
fn system_get(st: State<AppState>) -> Value {
    if !st.helper.supported() {
        return json!({"supported": false});
    }
    let call = |cmd: &str| st.helper.call(cmd, Value::Null).ok();
    json!({
        "supported": true,
        "volume": call("getVolume"),
        "muted": call("getMute"),
        "brightness": call("getBrightness"),
    })
}

#[tauri::command]
fn system_set(st: State<AppState>, key: String, value: Value) -> Option<Value> {
    let cmd = match key.as_str() {
        "volume" => "setVolume",
        "muted" => "setMute",
        "brightness" => "setBrightness",
        _ => return None,
    };
    st.helper.call(cmd, value).ok()
}

#[tauri::command]
fn wifi(_st: State<AppState>) -> Value {
    match system::wifi() {
        Some(w) => json!({"connected": w.connected, "ssid": w.ssid, "signal": w.signal}),
        None => Value::Null,
    }
}

#[tauri::command]
fn power(app: AppHandle, st: State<AppState>, action: String) -> Option<Value> {
    if action == "desktop" {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.minimize();
        }
        return None;
    }
    if !["sleep", "restart", "shutdown"].contains(&action.as_str()) {
        return None;
    }
    st.flush_all();
    let parsed = match action.as_str() {
        "sleep" => system::PowerAction::Sleep,
        "restart" => system::PowerAction::Restart,
        _ => system::PowerAction::Shutdown,
    };
    let helper_call = || st.helper.call("sleep", Value::Null).map(|_| ()).map_err(std::io::Error::other);
    system::power(parsed, helper_call).ok().map(|_| Value::Null)
}

#[tauri::command]
fn open_external(app: AppHandle, url: String) {
    // Restricted to https:// and ms-settings:, like the original.
    if url.starts_with("https://") || url.starts_with("ms-settings:") {
        let _ = open_url_in_shell(&url);
    }
    let _ = app;
}

// ---------------------------------------------------------------------------
// Servers, remote browsing & transfers

/// Passwords are encrypted with the OS's user key (DPAPI on Windows) when available; elsewhere they
/// fall back to plain base64, same as `safeStorage`'s basic mode.
fn seal_secret(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(sealed) = dpapi_protect(plain) {
            return format!("enc:{sealed}");
        }
    }
    use base64::Engine as _;
    format!("plain:{}", base64::engine::general_purpose::STANDARD.encode(plain))
}

fn open_secret(sealed: &str) -> String {
    if sealed.is_empty() {
        return String::new();
    }
    use base64::Engine as _;
    if let Some(rest) = sealed.strip_prefix("enc:") {
        #[cfg(target_os = "windows")]
        {
            if let Some(open) = dpapi_unprotect(rest) {
                return open;
            }
        }
        let _ = rest;
        return String::new();
    }
    if let Some(rest) = sealed.strip_prefix("plain:") {
        return base64::engine::general_purpose::STANDARD.decode(rest).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
    }
    String::new()
}

#[cfg(target_os = "windows")]
fn dpapi_protect(plain: &str) -> Option<String> {
    let script = format!(
        "$b=[System.Text.Encoding]::UTF8.GetBytes('{}'); $p=[System.Security.Cryptography.ProtectedData]::Protect($b, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser); [Convert]::ToBase64String($p)",
        plain.replace('\'', "''")
    );
    powershell_out(&script)
}

#[cfg(target_os = "windows")]
fn dpapi_unprotect(sealed_b64: &str) -> Option<String> {
    let script = format!(
        "$p=[Convert]::FromBase64String('{}'); $b=[System.Security.Cryptography.ProtectedData]::Unprotect($p, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser); [System.Text.Encoding]::UTF8.GetString($b)",
        sealed_b64.replace('\'', "''")
    );
    powershell_out(&script)
}

#[cfg(target_os = "windows")]
fn powershell_out(script: &str) -> Option<String> {
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn find_server(st: &AppState, id: &str) -> Option<Value> {
    st.servers.get("servers").and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().find(|s| s.get("id").and_then(Value::as_str) == Some(id))
}

fn connect_server(st: &AppState, app: &AppHandle, id: &str) -> Result<Arc<dyn RemoteClient>, remote::RemoteError> {
    let Some(srv) = find_server(st, id) else { return Err(remote::RemoteError { code: "noServer".into() }) };
    let spec = ServerSpec::from_json(&srv);
    let secret = open_secret(srv.get("secret").and_then(Value::as_str).unwrap_or(""));
    // Open a fresh connection to a saved server, remembering its SSH host key the first time.
    let servers = st.servers.clone();
    let app = app.clone();
    let on_host_key = move |key: &str| {
        servers.update("servers", json!([]), |list| {
            if let Value::Array(a) = list {
                if let Some(s) = a.iter_mut().find(|s| s.get("id").and_then(Value::as_str) == Some(id)) {
                    s["hostKey"] = json!(key);
                }
            }
        });
        if let Some(st) = app.try_state::<AppState>() {
            st.push_state(&app);
        }
    };
    remote::connect(&spec, &secret, Some(&on_host_key))
}

#[tauri::command]
fn save_server(app: AppHandle, st: State<AppState>, input: Value) {
    let mut server = input.as_object().cloned().unwrap_or_default();
    let secret = server.get("secret").and_then(Value::as_str).unwrap_or("");
    // An empty secret field means "keep whatever was saved" (the page never sees the password back).
    if server.get("hasSecret").and_then(Value::as_bool) == Some(false) || secret.is_empty() {
        if let Some(id) = server.get("id").and_then(Value::as_str) {
            if let Some(existing) = find_server(&st, id) {
                server.insert("secret".into(), existing.get("secret").cloned().unwrap_or(json!("")));
            }
        }
    } else {
        let sealed = seal_secret(secret);
        server.insert("secret".into(), json!(sealed));
    }
    server.remove("hasSecret");
    let id = server.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    if id.is_empty() {
        let new_id = format!("srv-{}", &lounge_core::games::manual_id(&format!("{:?}", server.get("host")))[5..]);
        server.insert("id".into(), json!(new_id));
    }
    st.servers.update("servers", json!([]), |list| {
        if let Value::Array(a) = list {
            let id = server.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            a.retain(|s| s.get("id").and_then(Value::as_str) != Some(id.as_str()));
            a.push(Value::Object(server.clone()));
        }
    });
    st.push_state(&app);
}

#[tauri::command]
fn remove_server(app: AppHandle, st: State<AppState>, id: String) {
    st.servers.update("servers", json!([]), |list| {
        if let Value::Array(a) = list {
            a.retain(|s| s.get("id").and_then(Value::as_str) != Some(id.as_str()));
        }
    });
    st.push_state(&app);
}

#[tauri::command]
fn forget_host_key(app: AppHandle, st: State<AppState>, id: String) {
    st.servers.update("servers", json!([]), |list| {
        if let Value::Array(a) = list {
            if let Some(s) = a.iter_mut().find(|s| s.get("id").and_then(Value::as_str) == Some(id.as_str())) {
                s["hostKey"] = json!("");
            }
        }
    });
    st.push_state(&app);
}

/// List one directory on a server (also used by "test connection", with the root).
fn remote_list_blocking(st: &AppState, app: &AppHandle, server_id: &str, path: Option<&str>) -> Value {
    let client = match connect_server(st, app, server_id) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.code}),
    };
    match client.list(path.unwrap_or("/")) {
        Ok(entries) => json!(entries.iter().map(|e| json!({"name": e.name, "isDir": e.is_dir, "size": e.size})).collect::<Vec<_>>()),
        Err(e) => json!({"error": e.code}),
    }
}

#[tauri::command]
fn test_server(app: AppHandle, st: State<AppState>, id: String) -> Value {
    remote_list_blocking(&st, &app, &id, None)
}

#[tauri::command]
fn remote_list(app: AppHandle, st: State<AppState>, server_id: String, path: Option<String>) -> Value {
    remote_list_blocking(&st, &app, &server_id, path.as_deref())
}

#[tauri::command]
fn remote_plan(app: AppHandle, st: State<AppState>, req: Value) -> Value {
    let outcome = plan_remote(&st, &app, &req);
    match outcome {
        Ok(plan) => json!({
            "ok": true,
            "kind": plan.kind_str,
            "root": plan.root,
            "roots": plan.roots,
            "files": plan.files,
            "videos": plan.videos,
            "totalSize": plan.total_size,
            "folders": plan.folders,
        }),
        Err(code) => json!({"ok": false, "error": code}),
    }
}

/// What a remote selection would become on disk: which library it belongs in and where each file
/// lands (`planRemote` in main.js — a folder is walked whole; a single video brings the subtitles
/// sitting next to it).
struct Planned {
    kind_str: &'static str,
    root: Value,
    roots: Value,
    files: usize,
    videos: usize,
    total_size: u64,
    folders: Vec<String>,
    plan: lounge_core::transfer_plan::Plan,
}

fn plan_remote(st: &AppState, app: &AppHandle, req: &Value) -> Result<Planned, String> {
    use lounge_core::transfer_plan::{self, PlanOptions, RemoteFile, Selection};
    use lounge_core::parse;
    let server_id = req.get("serverId").and_then(Value::as_str).unwrap_or("").to_string();
    let start = req.get("path").and_then(Value::as_str).unwrap_or("/").to_string();
    let is_dir = req.get("isDir").and_then(Value::as_bool).unwrap_or(false);
    let kind_req = req.get("kind").and_then(Value::as_str);
    let _ = kind_req;

    let client = connect_server(st, app, &server_id).map_err(|e| e.code)?;
    let name = start.rsplit('/').find(|s| !s.is_empty()).unwrap_or("").to_string();
    let files: Vec<RemoteFile> = if is_dir {
        client.walk(&start).map_err(|e| e.code)?.into_iter().map(|w| RemoteFile { remote: w.remote, rel: w.rel, size: w.size }).collect()
    } else {
        // A single video brings the subtitles sitting next to it.
        let dir = &start[..start.len().saturating_sub(name.len())];
        let dir = dir.trim_end_matches('/');
        let dir = if dir.is_empty() { "/" } else { dir };
        let siblings = client.list(dir).unwrap_or_default();
        let me = siblings.iter().find(|e| e.name == name).map(|e| e.size).unwrap_or(0);
        let mut files = vec![RemoteFile { remote: start.clone(), rel: name.clone(), size: me }];
        if parse::is_video_file(&name) {
            let base = parse::strip_extension(&name).to_lowercase();
            for e in &siblings {
                if !e.is_dir && e.name != name && e.name.to_lowercase().starts_with(&base) && is_sub_name(&e.name) {
                    files.push(RemoteFile { remote: format!("{}/{}", dir.trim_end_matches('/'), e.name), rel: e.name.clone(), size: e.size });
                }
            }
        }
        files
    };
    drop(client);

    let selection = Selection { name: &name, is_dir, parent: None };
    // Kind first, against throwaway roots, then the real plan against the user's matching library.
    let probe = transfer_plan::plan_transfer(&selection, &files, &PlanOptions { kind: None, movie_root: Some(Path::new("/")), tv_root: Some(Path::new("/")), existing_show_dirs: &[] });
    let kind_str = match probe.kind {
        transfer_plan::Kind::Movie => "movie",
        transfer_plan::Kind::Tv => "tv",
    };
    let libs = st.settings.get("libraries").and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let wanted = if kind_str == "tv" { "tv" } else { "movies" };
    let mut roots: Vec<String> = libs.iter().filter(|l| l.get("type").and_then(Value::as_str) == Some(wanted)).filter_map(|l| l.get("path").and_then(Value::as_str).map(String::from)).collect();
    if let Some(preferred) = req.get("library").and_then(Value::as_str).filter(|p| roots.iter().any(|r| r == p)) {
        roots.retain(|r| r != preferred);
        roots.insert(0, preferred.to_string());
    }
    let Some(root) = roots.first().cloned() else {
        return Ok(Planned { kind_str, root: Value::Null, roots: json!(roots), files: 0, videos: 0, total_size: 0, folders: vec![], plan: probe });
    };
    let existing: Vec<String> = std::fs::read_dir(&root).map(|rd| {
        rd.filter_map(|e| e.ok()).filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }).unwrap_or_default();
    let opts = PlanOptions {
        kind: Some(probe.kind),
        movie_root: if kind_str == "movie" { Some(Path::new(&root)) } else { None },
        tv_root: if kind_str == "tv" { Some(Path::new(&root)) } else { None },
        existing_show_dirs: &existing,
    };
    let plan = transfer_plan::plan_transfer(&selection, &files, &opts);
    Ok(Planned {
        kind_str,
        root: json!(root),
        roots: json!(roots),
        files: plan.items.len(),
        videos: plan.items.iter().filter(|i| parse::is_video_file(&i.rel)).count(),
        total_size: plan.total_size,
        folders: plan.folders.iter().map(|f| f.to_string_lossy().into_owned()).collect(),
        plan,
    })
}

fn is_sub_name(name: &str) -> bool {
    const SUBS: [&str; 9] = [".srt", ".ass", ".ssa", ".sub", ".idx", ".vtt", ".sup", ".sami", ".smi"];
    let lower = name.to_lowercase();
    SUBS.iter().any(|ext| lower.ends_with(ext))
}

#[tauri::command]
fn remote_download(app: AppHandle, st: State<AppState>, req: Value) -> Value {
    let planned = match plan_remote(&st, &app, &req) {
        Ok(p) => p,
        Err(code) => return json!({"ok": false, "error": code}),
    };
    if planned.plan.root.is_none() {
        return json!({"ok": false, "errorKey": if planned.kind_str == "tv" { "err.remote.noTvLibrary" } else { "err.remote.noMovieLibrary" }});
    }
    if planned.plan.items.is_empty() {
        return json!({"ok": false, "errorKey": "err.remote.nothing"});
    }
    let name = req.get("path").and_then(Value::as_str).unwrap_or("").rsplit('/').find(|s| !s.is_empty()).unwrap_or("").to_string();
    let title = if req.get("isDir").and_then(Value::as_bool).unwrap_or(false) { name } else { lounge_core::parse::strip_extension(&name).to_string() };
    let id = st.transfers.add(AddSpec { server_id: req.get("serverId").and_then(Value::as_str).unwrap_or("").to_string(), title, plan: planned.plan });
    st.push_state(&app);
    json!({"ok": true, "id": id})
}

#[tauri::command]
fn cancel_transfer(st: State<AppState>, id: String) {
    st.transfers.cancel(&id);
}

#[tauri::command]
fn clear_transfers(st: State<AppState>, id: Option<String>) {
    st.transfers.clear(id.as_deref());
}

#[tauri::command]
fn retry_transfer(app: AppHandle, st: State<AppState>, id: String) -> Option<String> {
    let new_id = st.transfers.retry(&id);
    st.push_state(&app);
    new_id
}

// ---------------------------------------------------------------------------
// Apps & Tailscale

#[tauri::command]
fn rescan_apps(app: AppHandle, st: State<AppState>) {
    if st.apps_scanning.swap(true, Ordering::SeqCst) {
        return;
    }
    st.push_state(&app);
    let app2 = app.clone();
    std::thread::spawn(move || {
        let st = app2.state::<AppState>();
        let result = apps::list_apps(&apps::AppEnv::from_process_env());
        match result {
            Ok(list) => {
                let list_json: Vec<Value> = list.iter().map(|a| json!({
                    "id": a.id, "name": a.name, "appId": a.app_id, "kind": a.kind, "exe": a.exe,
                    "packageDir": a.package_dir, "storeApp": a.store_app, "shortcut": a.shortcut,
                })).collect();
                st.apps_store.set("apps", json!(list_json));
                st.apps_store.set("scannedAt", json!(now_millis()));
                st.push_state(&app2);
                // Icons: the Store logo from the package, or the program's own icon.
                let apps_snapshot: Vec<apps::AppEntry> = list;
                let icons = st.apps_store.get("icons").unwrap_or_else(|| json!({}));
                let mut icons_map = icons.as_object().cloned().unwrap_or_default();
                for a in apps_snapshot {
                    if st.game_session.lock().unwrap().is_some() {
                        break;
                    }
                    let existing = icons_map.get(&a.id).and_then(Value::as_str).map(String::from);
                    if existing.map(|p| Path::new(&p).is_file()).unwrap_or(false) {
                        continue;
                    }
                    if let Some(file) = app_icon(&st.artwork_dir, &a) {
                        icons_map.insert(a.id.clone(), json!(file));
                        st.apps_store.set("icons", Value::Object(icons_map.clone()));
                        st.push_state(&app2);
                    }
                }
            }
            Err(e) => {
                let _ = e;
            }
        }
        st.apps_scanning.store(false, Ordering::SeqCst);
        st.push_state(&app2);
    });
}

/// Save an app's icon as a PNG in the artwork folder (Store logo, or the program's own icon).
fn app_icon(artwork_dir: &Path, a: &apps::AppEntry) -> Option<String> {
    let dir = artwork_dir.join("apps");
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(format!("{}.png", a.id));
    match a.kind {
        "store" => {
            let logo = apps::store_logo(a.package_dir.as_deref()?, a.store_app.as_deref())?;
            std::fs::copy(&logo, &dest).ok()?;
        }
        _ => {
            let exe = a.exe.as_deref().or(a.shortcut.as_deref())?;
            let icon = icon_for(artwork_dir, exe)?;
            std::fs::copy(&icon, &dest).ok()?;
        }
    }
    Some(dest.to_string_lossy().into_owned())
}

#[tauri::command]
fn launch_app(app: AppHandle, st: State<AppState>, id: String) -> Value {
    let a = st.apps_store.get("apps").and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().find(|a| a.get("id").and_then(Value::as_str) == Some(id.as_str()));
    let Some(a) = a else { return json!({"ok": false, "errorKey": "err.notFound"}) };
    if let Err(e) = apps::launch_app(a.get("appId").and_then(Value::as_str).unwrap_or("")) {
        return json!({"ok": false, "errorKey": "err.appLaunch", "vars": {"message": e}});
    }
    let app_id = id.clone();
    st.apps_store.update("recent", json!({}), |r| {
        if let Value::Object(m) = r {
            m.insert(app_id, json!(now_millis()));
        }
    });
    st.push_state(&app);
    // Step aside so the app comes up in front; the home button (or Alt+Tab) brings Lounge back.
    let app2 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(APP_STEP_ASIDE_MS));
        if let Some(win) = app2.get_webview_window("main") {
            let _ = win.minimize();
        }
    });
    json!({"ok": true})
}

#[tauri::command]
fn hide_app(app: AppHandle, st: State<AppState>, id: String, hidden: bool) {
    st.apps_store.update("hidden", json!({}), |m| {
        if let Value::Object(m) = m {
            if hidden {
                m.insert(id.clone(), json!(true));
            } else {
                m.remove(&id);
            }
        }
    });
    st.push_state(&app);
}

fn locate_tailscale(st: &AppState, app: &AppHandle) {
    let mut hints: Vec<String> = Vec::new();
    for a in st.apps_store.get("apps").and_then(|v| v.as_array().cloned()).unwrap_or_default() {
        if is_tailscale_app(&a) {
            if let Some(exe) = a.get("exe").and_then(Value::as_str) {
                if let Some(dir) = Path::new(exe).parent() {
                    hints.push(dir.to_string_lossy().into_owned());
                }
            }
        }
    }
    st.tailscale.locate(&hints);
    st.push_state(app);
}

#[tauri::command]
fn tailscale_locate(app: AppHandle, st: State<AppState>) {
    locate_tailscale(&st, &app);
}

#[tauri::command]
fn tailscale_status(app: AppHandle, st: State<AppState>) -> Value {
    let status = st.tailscale.status();
    let _ = app.emit("tailscale", status.clone());
    status
}

#[tauri::command]
fn tailscale_action(app: AppHandle, st: State<AppState>, action: String, node: Option<String>) -> Value {
    if !st.tailscale.installed() {
        return json!({"ok": false, "errorKey": "err.tsMissing"});
    }
    let result = match action.as_str() {
        "up" => st.tailscale.up().map(|_| ()),
        "down" => st.tailscale.down().map(|_| ()),
        "exitNode" => st.tailscale.set_exit_node(node.as_deref()).map(|_| ()),
        _ => Err("unknown action".into()),
    };
    let status = st.tailscale.status();
    let _ = app.emit("tailscale", status.clone());
    match result {
        Ok(()) => json!({"ok": true, "status": status}),
        Err(e) => json!({"ok": false, "errorKey": "err.tailscale", "vars": {"message": e}, "status": status}),
    }
}

#[tauri::command]
fn tailscale_login(_app: AppHandle, st: State<AppState>) -> Value {
    if !st.tailscale.installed() {
        return json!({"ok": false, "errorKey": "err.tsMissing"});
    }
    let handle = st.tailscale.start_login();
    // `tailscale up` prints the URL within moments; wait (bounded) so the page can show the QR.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut url = handle.url();
    while url.is_none() && handle.outcome().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        url = handle.url();
    }
    match (url, handle.outcome()) {
        (Some(u), _) => {
            let qr = qr_data_url(&u);
            json!({"ok": true, "url": u, "qr": qr})
        }
        (None, Some(Ok(None))) => json!({"ok": true, "url": Value::Null, "qr": Value::Null}),
        (None, Some(Ok(Some(u)))) => json!({"ok": true, "url": u, "qr": qr_data_url(&u)}),
        (None, Some(Err(e))) => json!({"ok": false, "errorKey": "err.tailscale", "vars": {"message": e}}),
        (None, None) => json!({"ok": true, "url": Value::Null, "qr": Value::Null}),
    }
}

/// The sign-in URL as a QR code data URL (SVG inside a data: URI — the page only ever puts it in an
/// `<img src>`, like the PNG data URL the JS `qrcode` module produced).
fn qr_data_url(url: &str) -> Value {
    use base64::Engine as _;
    let code = match qrcode::QrCode::with_error_correction_level(url.as_bytes(), qrcode::EcLevel::M) {
        Ok(c) => c,
        Err(_) => return Value::Null,
    };
    let svg = code.render::<qrcode::render::svg::Color>().quiet_zone(true).build();
    json!(format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(svg)))
}

#[tauri::command]
fn tailscale_cancel_login(st: State<AppState>) {
    st.tailscale.cancel_login();
}

#[tauri::command]
fn tailscale_open_app(app: AppHandle, st: State<AppState>) -> Value {
    let a = st.apps_store.get("apps").and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().find(is_tailscale_app);
    match a {
        Some(a) => launch_app(app, st, a.get("id").and_then(Value::as_str).unwrap_or("").to_string()),
        None => json!({"ok": false, "errorKey": "err.tsMissing"}),
    }
}

// ---------------------------------------------------------------------------
// Updates (always the user's call: we only check, then ask)

fn setup_updater(st: &AppState, app: &AppHandle) {
    let install_type = std::env::var("LOUNGE_UPDATE_TYPE").ok().and_then(|v| match v.as_str() {
        "nsis" => Some(updater::InstallType::Nsis),
        "zip" => Some(updater::InstallType::Zip),
        "fse" => Some(updater::InstallType::Fse),
        _ => None,
    }).or_else(|| {
        updater::detect_install_type(&updater::DetectInstallOpts {
            packaged: !cfg!(debug_assertions),
            windows_store: std::env::var("LOUNGE_FSE_PACKAGE").map(|v| v == "1").unwrap_or(false),
            exe_path: &std::env::current_exe().unwrap_or_default(),
            is_windows: cfg!(target_os = "windows"),
        })
    });
    let dir = st.user_file("updates");
    let app2 = app.clone();
    let updater = Updater::new(
        updater::UpdaterConfig { repo: REPO.into(), version: env!("CARGO_PKG_VERSION").into(), install_type, dir },
        move |state| {
            let _ = app2.emit("update", update_state_json(state));
        },
    );
    *st.updater.lock().unwrap() = Some(Arc::new(updater));
    if install_type.is_none() {
        return;
    }
    // Auto-check shortly after startup, then every 6 hours, unless one is downloading or we're in a game.
    let app2 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(UPDATE_FIRST_CHECK_MS));
        loop {
            {
                let st = app2.state::<AppState>();
                let auto = st.settings.get("autoCheckUpdates").and_then(|v| v.as_bool()).unwrap_or(true);
                let in_game = st.game_session.lock().unwrap().is_some();
                let downloading = st.updater.lock().unwrap().as_ref().map(|u| u.state().status == updater::Status::Downloading).unwrap_or(false);
                if auto && !in_game && !downloading {
                    if let Some(u) = st.updater.lock().unwrap().clone() {
                        let app3 = app2.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            u.check();
                            let _ = app3;
                        });
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(UPDATE_INTERVAL_MS));
        }
    });
}

#[tauri::command]
fn check_update(_app: AppHandle, st: State<AppState>) -> Value {
    let updater = st.updater.lock().unwrap().clone();
    let Some(u) = updater else { return Value::Null };
    let state = update_state_json(&u.state());
    // The network check runs in the background; the UI hears the result via the update event.
    tauri::async_runtime::spawn_blocking(move || {
        u.check();
    });
    state
}

#[tauri::command]
fn skip_update(app: AppHandle, st: State<AppState>, version: String) {
    st.settings.set("skippedVersion", json!(version));
    st.push_state(&app);
}

#[tauri::command]
fn install_update(app: AppHandle, st: State<AppState>) -> Value {
    let Some(u) = st.updater.lock().unwrap().clone() else { return json!({"ok": false}) };
    if st.game_session.lock().unwrap().is_some() {
        return json!({"ok": false, "errorKey": "err.updateWhilePlaying"});
    }
    st.flush_all();
    // Download (if needed) and compute the install command off the main thread.
    let result: Result<(String, Vec<String>, Option<PathBuf>), String> = tauri::async_runtime::block_on(async {
        tauri::async_runtime::spawn_blocking(move || -> Result<(String, Vec<String>, Option<PathBuf>), String> {
            if u.state().status != updater::Status::Ready {
                u.download()?;
            }
            let cmd = u.install_command(std::process::id(), &std::env::current_exe().unwrap_or_default())?;
            Ok((cmd.command, cmd.args, cmd.status))
        }).await.unwrap_or_else(|e| Err(e.to_string()))
    });
    match result {
        Err(e) => {
            // Surfaced through the updater's own error state.
            json!({"ok": false, "errorKey": "err.update", "vars": {"message": e}})
        }
        Ok((command, args, status)) => {
            if let Some(status_file) = status {
                return install_fse(&st, &command, &args, &status_file);
            }
            if let Some(p) = st.play.lock().unwrap().as_ref() {
                p.session.kill();
            }
            let spawn_ok = spawn_detached(&command, &args);
            if let Err(e) = spawn_ok {
                return json!({"ok": false, "errorKey": "err.update", "vars": {"message": e.to_string()}});
            }
            // Give the UI a moment to show "Installing…", then get out of the installer's way.
            let app2 = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1200));
                app2.exit(0);
            });
            json!({"ok": true})
        }
    }
}

/// FSE package: start the install script outside the package, then stay open until the installer has
/// its admin rights; the installer closes Lounge itself when it replaces the package.
fn install_fse(st: &AppState, command: &str, args: &[String], status_file: &Path) -> Value {
    let fail = |error: Option<String>, error_key: &str| -> Value {
        json!({"ok": false, "errorKey": error_key, "vars": error.map(|m| json!({"message": m})).unwrap_or(Value::Null)})
    };
    let out = std::process::Command::new(command)
        .args(args)
        .output();
    match out {
        Err(e) => return fail(Some(format!("couldn't start the update ({e})")), "err.update"),
        Ok(o) if !o.status.success() => {
            let err_out = String::from_utf8_lossy(&o.stderr);
            return fail(Some(format!("couldn't start the update ({})", err_out.trim().chars().take(200).collect::<String>())), "err.update");
        }
        Ok(_) => {}
    }
    let result = {
        let u = st.updater.lock().unwrap().clone();
        let Some(u) = u else { return fail(None, "err.update") };
        u.wait_for_fse(status_file, &updater::WaitForFseOpts::default())
    };
    match result.as_str() {
        "declined" => return fail(None, "upd.declined"),
        "noprompt" => return fail(None, "upd.noPrompt"),
        "elevated" => {}
        other => return fail(Some(other.trim_start_prefix_matches("failed: ").to_string()), "err.update"),
    }
    if let Some(u) = st.updater.lock().unwrap().as_ref() {
        u.set_status(updater::Status::Installing);
    }
    if let Some(p) = st.play.lock().unwrap().as_ref() {
        p.session.kill();
    }
    json!({"ok": true})
}

trait TrimStartPrefixMatches {
    fn trim_start_prefix_matches(&self, prefix: &str) -> &str;
}

impl TrimStartPrefixMatches for str {
    fn trim_start_prefix_matches(&self, prefix: &str) -> &str {
        self.strip_prefix(prefix).unwrap_or(self)
    }
}

#[cfg(target_os = "windows")]
fn spawn_detached(command: &str, args: &[String]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(command)
        .args(args)
        .creation_flags(0x00000008 | 0x08000000) // DETACHED_PROCESS | CREATE_NO_WINDOW
        .spawn()
        .map(|_| ())
}

#[cfg(not(target_os = "windows"))]
fn spawn_detached(command: &str, args: &[String]) -> std::io::Result<()> {
    std::process::Command::new(command).args(args).spawn().map(|_| ())
}

// ---------------------------------------------------------------------------
// Shell helpers (open/reveal), with the Electron defaults' scope

fn open_url_in_shell(url: &str) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ())
    }
}

fn open_path_in_shell(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe").arg(path).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
}

fn reveal_in_shell(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        // `explorer /select,` highlights the item in Explorer.
        std::process::Command::new("explorer.exe").arg(format!("/select,{}", path.to_string_lossy())).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").args(["-R", &path.to_string_lossy()]).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(parent) = path.parent() {
            std::process::Command::new("xdg-open").arg(parent).spawn().map(|_| ())
        } else {
            Err(std::io::Error::other("no parent"))
        }
    }
}

// ---------------------------------------------------------------------------
// The event router: drains VLC/game events and the background timers (main.js's event handlers)

fn power_save_blocker_start(st: State<'_, AppState>) {
    // Keep the display awake during playback (Electron's powerSaveBlocker). Windows: one call per
    // process is enough; the flag tracks nesting so stop only fires when playback is truly done.
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
        }
    }
    let _ = st;
}

fn power_save_blocker_stop() {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

fn handle_vlc_progress(st: &AppState, app: &AppHandle, progress: vlc::Progress) {
    viewmodel::record_progress(&st.stores(), Path::new(&progress.path), progress.time, progress.length);
    let mut play = st.play.lock().unwrap();
    let Some(ps) = play.as_mut() else { return };
    let index = ps.queue.iter().position(|q| q.path == progress.path).unwrap_or(0);
    if let Some(q) = ps.queue.get(index) {
        if let Some(item_id) = &q.item_id {
            if let Some((last_path, last_time)) = &ps.last_pos {
                if last_path == &progress.path && !progress.paused {
                    let step = progress.time - last_time;
                    if step > 0.0 {
                        let entry = ps.watched.entry(item_id.clone()).or_insert(0.0);
                        *entry += step.min(MAX_WATCH_STEP);
                    }
                }
            }
        }
    }
    ps.last_pos = Some((progress.path.clone(), progress.time));
    let title = ps.queue.get(index).and_then(|q| q.label.clone()).unwrap_or_else(|| ps.title.clone());
    let mut np = st.now_playing.lock().unwrap();
    if let Some(np) = np.as_mut() {
        np["title"] = json!(title);
        np["current"] = json!(progress.path);
        np["time"] = json!(progress.time);
        np["length"] = json!(progress.length);
        np["paused"] = json!(progress.paused);
        np["index"] = json!(index);
    }
    drop(np);
    drop(play);
    let _ = app.emit("now-playing", st.now_playing.lock().unwrap().clone());
}

fn finish_play(st: &AppState, app: &AppHandle, error_key: Option<(&str, Value)>) {
    // Credit the watch time gathered during the session, then clean up.
    let play = st.play.lock().unwrap().take();
    if let Some(ps) = play {
        let watch_start = ps.started_at;
        for (id, sec) in &ps.watched {
            viewmodel::log_session(&st.stores(), "watch", id, watch_start, sec / 60.0);
        }
    }
    power_save_blocker_stop();
    *st.now_playing.lock().unwrap() = None;
    let _ = app.emit("now-playing", Value::Null);
    if let Some((key, vars)) = error_key {
        toast(st, app, key, vars, "error");
    }
    st.push_state(app);
    bring_to_front(app);
}

fn poll_events(app: &AppHandle) {
    let st = app.state::<AppState>();

    // VLC progress / exit.
    let (progresses, exit) = {
        let play = st.play.lock().unwrap();
        match play.as_ref() {
            Some(ps) => {
                let mut progresses = Vec::new();
                let mut exit = None;
                while let Some(ev) = ps.session.try_event() {
                    match ev {
                        vlc::VlcEvent::Progress(p) => progresses.push(p),
                        vlc::VlcEvent::Exit { code, last } => {
                            exit = Some((code, last));
                            break;
                        }
                    }
                }
                (progresses, exit)
            }
            None => (Vec::new(), None),
        }
    };
    for p in progresses {
        handle_vlc_progress(&st, app, p);
    }
    if let Some((code, last)) = exit {
        let started_at = st.play.lock().unwrap().as_ref().map(|p| p.started_at).unwrap_or(0);
        // VLC that dies within seconds without ever reporting a position failed to start or open the file.
        let failed = code.unwrap_or(0) != 0 && last.is_none() && now_millis() - started_at < 15000;
        finish_play(&st, app, if failed { Some(("err.vlcExited", json!({"code": code}))) } else { None });
    }

    // Game sessions.
    let game_events = {
        let session = st.game_session.lock().unwrap();
        match session.as_ref() {
            Some(s) => {
                let mut out = Vec::new();
                while let Some(ev) = s.try_event() {
                    out.push(ev);
                }
                out
            }
            None => Vec::new(),
        }
    };
    for ev in game_events {
        match ev {
            games::GameEvent::Running => {
                {
                    let mut game = st.game.lock().unwrap();
                    if let Some(g) = game.as_mut() {
                        g["phase"] = json!("running");
                    }
                }
                let _ = app.emit("game", st.game.lock().unwrap().clone());
                let free = st.settings.get("freeWhilePlaying").and_then(|v| v.as_bool()).unwrap_or(true);
                if free {
                    let app2 = app.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(SUSPEND_DELAY_MS));
                        suspend_ui(&app2);
                    });
                }
            }
            games::GameEvent::Exit { reason, played_ms, error } => {
                let game_id = st.game.lock().unwrap().as_ref().and_then(|g| g.get("id").and_then(Value::as_str)).unwrap_or("").to_string();
                let steam_source = {
                    let steam = st.steam_games.lock().unwrap();
                    steam.entries.iter().any(|g| g.get("id").and_then(Value::as_str) == Some(game_id.as_str()))
                };
                if reason == games::ExitReason::Stub {
                    // The exe was a launcher that handed off to the real game: stay out of the way
                    // until the user comes back to Lounge and says they're done.
                    let mut game = st.game.lock().unwrap();
                    if let Some(g) = game.as_mut() {
                        g["phase"] = json!("untracked");
                    }
                    continue;
                }
                *st.game_session.lock().unwrap() = None;
                *st.game.lock().unwrap() = None;
                if played_ms > 0 || reason == games::ExitReason::Exited {
                    record_play(&st, &game_id, played_ms);
                }
                resume_ui(app);
                let _ = app.emit("game", Value::Null);
                if reason == games::ExitReason::Error {
                    toast(&st, app, "err.gameStart", json!({"message": error.unwrap_or_default()}), "error");
                } else if reason == games::ExitReason::Timeout {
                    let title = st.games_store.get("manual").and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().find(|g| g.get("id").and_then(Value::as_str) == Some(game_id.as_str())).and_then(|g| g.get("title").cloned()).unwrap_or(json!(""));
                    toast(&st, app, "err.gameTimeout", json!({"title": title}), "error");
                }
                // Steam updates its own playtime when a game closes; pick that up.
                if steam_source {
                    let app2 = app.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(3000));
                        let st = app2.state::<AppState>();
                        let settings = st.settings.clone();
                        let games = scan_games_blocking(&settings);
                        *st.steam_games.lock().unwrap() = games;
                        st.push_state(&app2);
                    });
                } else {
                    st.push_state(app);
                }
            }
        }
    }

    // The tailscale login watcher: when the attempt ends, refresh the status (and celebrate).
    if let Some(handle) = st.tailscale.pending_login() {
        if let Some(outcome) = handle.outcome() {
            st.tailscale.clear_login_handle(&handle);
            let status = st.tailscale.status();
            let _ = app.emit("tailscale", status.clone());
            if matches!(outcome, Ok(Some(_))) {
                let host = status.get("hostName").cloned().unwrap_or(json!(""));
                let suffix = if host.as_str().map(|h| !h.is_empty()).unwrap_or(false) { format!(" · {}", host.as_str().unwrap_or("")) } else { String::new() };
                toast(&st, app, "ts.signedIn", json!({"name": suffix}), "info");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Wiring

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .on_window_event(|window, event| {
            // Focus events: switching back to Lounge while a game runs brings the UI back.
            if let tauri::WindowEvent::Focused(true) = event {
                let app = window.app_handle();
                let st = app.state::<AppState>();
                if st.suspended.load(Ordering::SeqCst) {
                    resume_ui(app);
                }
            }
        })
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // A second launch focuses/resumes the existing window, like Electron's second-instance.
            let st = app.state::<AppState>();
            if st.suspended.load(Ordering::SeqCst) {
                resume_ui(app);
            } else {
                bring_to_front(app);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .invoke_handler(tauri::generate_handler![
            get_state, save_ui_state, rescan, play, stop, np_command, get_stats, set_languages, set_watched, set_pref,
            play_game, end_game, back_to_game, add_game, edit_game, remove_game, search_steam, search_sgdb,
            sgdb_images, set_game_art, screenshot, show_game_folder,
            save_settings, pick_folder, pick_vlc, pick_key_file, detect_vlc, clear_metadata, show_in_folder,
            system_get, system_set, wifi, power, open_external,
            save_server, remove_server, forget_host_key, test_server, remote_list, remote_plan, remote_download,
            cancel_transfer, clear_transfers, retry_transfer,
            rescan_apps, launch_app, hide_app,
            tailscale_locate, tailscale_status, tailscale_action, tailscale_login, tailscale_cancel_login, tailscale_open_app,
            check_update, install_update, skip_update,
            quit, minimize, toggle_fullscreen,
        ])
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("no app data dir");
            std::fs::create_dir_all(&dir)?;
            std::fs::create_dir_all(dir.join("artwork"))?;
            let settings = JsonStore::new(dir.join("settings.json"), default_settings());
            let stores = (
                JsonStore::new(dir.join("progress.json"), jmap(json!({"items": {}}))),
                JsonStore::new(dir.join("metadata.json"), jmap(json!({"entries": {}}))),
                JsonStore::new(dir.join("games.json"), jmap(json!({"manual": [], "overrides": {}, "stats": {}}))),
                JsonStore::new(dir.join("gameinfo.json"), jmap(json!({"games": {}}))),
                JsonStore::new(dir.join("prefs.json"), jmap(json!({"favorites": {}, "hidden": {}, "languages": {}}))),
                JsonStore::new(dir.join("stats.json"), jmap(json!({"sessions": []}))),
                JsonStore::new(dir.join("servers.json"), jmap(json!({"servers": []}))),
                JsonStore::new(dir.join("apps.json"), jmap(json!({"apps": [], "icons": {}, "hidden": {}, "recent": {}, "scannedAt": 0}))),
            );

            // The initial library comes from a scan, like the JS version's did-finish-load rescan.
            let libs_json = settings.get("libraries").and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let defs: Vec<library::LibraryDef> = libs_json.iter().filter_map(|l| {
                let p = l.get("path").and_then(Value::as_str)?;
                let kind = if l.get("type").and_then(Value::as_str) == Some("tv") { library::LibraryKind::Tv } else { library::LibraryKind::Movies };
                Some(library::LibraryDef { path: Path::new(p), kind })
            }).collect();
            let initial_library = library::scan_libraries(&defs);
            let steam_games = scan_games_blocking(&settings);

            // The transfer queue: opens connections on demand and streams state changes out.
            let app_handle = app.handle().clone();
            let connect = move |server_id: &str| -> Result<Arc<dyn RemoteClient>, String> {
                let st = app_handle.state::<AppState>();
                match connect_server_inner(&st, &app_handle, server_id) {
                    Ok(c) => Ok(c),
                    Err(e) => Err(e.code),
                }
            };
            let app_handle2 = app.handle().clone();
            let transfers = Arc::new(TransferQueue::new(connect, move |_event| {
                if let Some(st) = app_handle2.try_state::<AppState>() { { let _ = &st; }
                    let _ = app_handle2.emit("transfers", st.transfers.state().iter().map(transfer_state_json).collect::<Vec<_>>());
                }
            }));

            legion_hid::start(app.handle().clone());

            app.manage(AppState {
                settings,
                progress: stores.0,
                meta: stores.1,
                games_store: stores.2,
                game_info_store: stores.3,
                prefs: stores.4,
                stats: stores.5,
                servers: stores.6,
                apps_store: stores.7,
                library: Mutex::new(initial_library),
                steam_games: Mutex::new(steam_games),
                ui_state: Mutex::new(None),
                toasts: Mutex::new(Vec::new()),
                now_playing: Mutex::new(None),
                play: Mutex::new(None),
                game: Mutex::new(None),
                game_session: Mutex::new(None),
                scanning: AtomicBool::new(false),
                apps_scanning: AtomicBool::new(false),
                meta_status: Mutex::new((false, None)),
                game_info_status: Mutex::new((false, None)),
                updater: Mutex::new(None),
                tailscale: tailscale::Tailscale::new(),
                helper: system::SystemHelper::new(),
                transfers,
                battery: Mutex::new(None),
                suspended: AtomicBool::new(false),
                artwork_dir: dir.join("artwork"),
            });

            // Battery presence for the quick menu (asked once, like the JS version).
            {
                let app2 = app.handle().clone();
                std::thread::spawn(move || {
                    let present = system::has_battery();
                    let st = app2.state::<AppState>();
                    *st.battery.lock().unwrap() = Some(present);
                    st.push_state(&app2);
                });
            }

            setup_updater(&app.state::<AppState>(), app.handle());

            // The event router (VLC/game/login events at ~10 Hz).
            {
                let app2 = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(100));
                    poll_events(&app2);
                });
            }

            let fullscreen = app.state::<AppState>().settings.get("startFullscreen").and_then(|v| v.as_bool()).unwrap_or(true);
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Lounge")
                .inner_size(1600.0, 900.0)
                .min_inner_size(960.0, 540.0)
                .fullscreen(fullscreen)
                .background_color(tauri::webview::Color(0x07, 0x08, 0x0c, 0xff))
                .initialization_script(INIT_SCRIPT)
                .on_navigation(move |url| {
                    // Never navigate away from the app; links go to the browser instead.
                    let same_origin = matches!(url.scheme(), "tauri" | "http" | "https" | "file")
                        && matches!(url.host_str(), None | Some("localhost") | Some("tauri.localhost"));
                    if !same_origin {
                        if url.scheme() == "https" || url.scheme() == "http" {
                            let _ = open_url_in_shell(url.as_str());
                        }
                        return false;
                    }
                    true
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Lounge Tauri shell");
}

fn connect_server_inner(st: &AppState, app: &AppHandle, id: &str) -> Result<Arc<dyn RemoteClient>, remote::RemoteError> {
    connect_server(st, app, id)
}

/// Recreates `preload.js`'s `contextBridge`-exposed `window.lounge` object, so `renderer/**` runs
/// completely unmodified against either shell.
const INIT_SCRIPT: &str = r#"
(function () {
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
  window.lounge = {
    kind: 'tauri',
    getState: () => invoke('get_state'),
    saveUiState: (s) => invoke('save_ui_state', { s }),
    rescan: () => invoke('rescan'),
    play: (req) => invoke('play', { req }),
    stop: () => invoke('stop'),
    npCommand: (name) => invoke('np_command', { name }),
    setLanguages: (a) => invoke('set_languages', { id: a.id, languages: a.languages }),
    setWatched: (req) => invoke('set_watched', { req }),
    setPref: (a) => invoke('set_pref', { id: a.id, key: a.key, value: a.value }),
    showInFolder: (p) => invoke('show_in_folder', { p }),
    getStats: () => invoke('get_stats'),
    playGame: (id) => invoke('play_game', { id }),
    endGame: () => invoke('end_game'),
    backToGame: () => invoke('back_to_game'),
    addGame: () => invoke('add_game'),
    editGame: (a) => invoke('edit_game', { id: a.id, patch: a.patch }),
    removeGame: (id) => invoke('remove_game', { id }),
    searchSteam: (term) => invoke('search_steam', { term }),
    searchSgdb: (term) => invoke('search_sgdb', { term }),
    sgdbImages: (a) => invoke('sgdb_images', { kind: a.kind, id: a.id }),
    setGameArt: (a) => invoke('set_game_art', { id: a.id, kind: a.kind, url: a.url }),
    screenshot: (url) => invoke('screenshot', { url }),
    showGameFolder: (id) => invoke('show_game_folder', { id }),
    saveSettings: (patch) => invoke('save_settings', { patch }),
    pickFolder: () => invoke('pick_folder'),
    pickVlc: () => invoke('pick_vlc'),
    detectVlc: () => invoke('detect_vlc'),
    clearMetadata: () => invoke('clear_metadata'),
    saveServer: (input) => invoke('save_server', { input: input || {} }),
    removeServer: (id) => invoke('remove_server', { id }),
    forgetHostKey: (id) => invoke('forget_host_key', { id }),
    testServer: (id) => invoke('test_server', { id }),
    pickKeyFile: () => invoke('pick_key_file'),
    remoteList: (a) => invoke('remote_list', { serverId: a.serverId, path: a.path }),
    remotePlan: (req) => invoke('remote_plan', { req }),
    remoteDownload: (req) => invoke('remote_download', { req }),
    cancelTransfer: (id) => invoke('cancel_transfer', { id }),
    clearTransfers: (id) => invoke('clear_transfers', { id }),
    retryTransfer: (id) => invoke('retry_transfer', { id }),
    rescanApps: () => invoke('rescan_apps'),
    launchApp: (id) => invoke('launch_app', { id }),
    hideApp: (a) => invoke('hide_app', { id: a.id, hidden: a.hidden }),
    tailscaleStatus: () => invoke('tailscale_status'),
    tailscaleLocate: () => invoke('tailscale_locate'),
    tailscaleAction: (req) => invoke('tailscale_action', { action: req.action, node: req.node }),
    tailscaleLogin: () => invoke('tailscale_login'),
    tailscaleCancelLogin: () => invoke('tailscale_cancel_login'),
    tailscaleOpenApp: () => invoke('tailscale_open_app'),
    systemGet: () => invoke('system_get'),
    systemSet: (a) => invoke('system_set', { key: a.key, value: a.value }),
    wifi: () => invoke('wifi'),
    power: (action) => invoke('power', { action }),
    openExternal: (url) => invoke('open_external', { url }),
    checkUpdate: () => invoke('check_update'),
    installUpdate: () => invoke('install_update'),
    skipUpdate: (version) => invoke('skip_update', { version }),
    toggleFullscreen: () => invoke('toggle_fullscreen'),
    minimize: () => invoke('minimize'),
    quit: () => invoke('quit'),
    onState: on('state'), onNowPlaying: on('now-playing'), onGame: on('game'), onToast: on('toast'),
    onUpdate: on('update'), onTransfers: on('transfers'), onTailscale: on('tailscale'),
    onLegionReport: on('legion-report'), onLegionState: on('legion-state'),
  };
  function on(event) {
    return (cb) => {
      let unlisten = () => {};
      window.__TAURI__.event.listen(event, (e) => cb(e.payload)).then((u) => { unlisten = u; });
      return () => unlisten();
    };
  }
})();
"#;

#[tauri::command]
fn quit(app: AppHandle, st: State<AppState>) {
    // before-quit: kill playback, cancel any login, flush everything.
    if let Some(p) = st.play.lock().unwrap().as_ref() {
        p.session.kill();
    }
    st.tailscale.cancel_login();
    st.helper.stop();
    st.flush_all();
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

#[tauri::command]
fn rescan(app: AppHandle, st: State<AppState>) -> Value {
    do_rescan(&st, &app);
    build_state(&st)
}

#[tauri::command]
fn get_state(st: State<AppState>) -> Value {
    build_state(&st)
}
