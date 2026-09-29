//! Application state, the stores' defaults, and the JSON payload builders (`state`, update, transfers).

use super::{Backend, Host};
use lounge_core::library;
use lounge_core::store::JsonStore;
use lounge_core::transfers::TransferQueue;
use lounge_core::updater::{self, Updater};
use lounge_core::viewmodel::{self, SteamGames, Stores};
use lounge_core::{gameinfo, games, metadata, system, vlc};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub(super) fn default_settings() -> Map<String, Value> {
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
// Paths & small helpers

/// A local artwork/icon file as the string the payloads carry. The Tauri shell turned this into an
/// asset-protocol URL for its webview; the Slint UI loads images straight from disk, so it is the
/// plain absolute path.
pub(super) fn file_url(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub(super) fn file_url_opt(p: Option<&str>) -> Value {
    match p {
        Some(p) if !p.is_empty() => json!(file_url(Path::new(p))),
        _ => Value::Null,
    }
}

/// An object literal as a `Map`, for JsonStore defaults.
pub(super) fn jmap(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

pub(super) fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub(super) fn path_is_file(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str).map(|p| Path::new(p).is_file()).unwrap_or(false)
}

pub(super) fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Application state

pub struct Inner {
    pub(super) host: Arc<dyn Host>,
    pub(super) data_dir: PathBuf,
    pub(super) cache_dir: PathBuf,
    pub(super) artwork_dir: PathBuf,
    pub(super) settings: JsonStore,
    pub(super) progress: JsonStore,
    pub(super) meta: JsonStore,
    pub(super) games_store: JsonStore,
    pub(super) game_info_store: JsonStore,
    pub(super) prefs: JsonStore,
    pub(super) stats: JsonStore,
    pub(super) servers: JsonStore,
    pub(super) apps_store: JsonStore,
    pub(super) library: Mutex<library::LibraryScan>,
    pub(super) steam_games: Mutex<SteamGames>,
    pub(super) ui_state: Mutex<Option<Value>>,
    pub(super) toasts: Mutex<Vec<Value>>,
    pub(super) now_playing: Mutex<Option<Value>>,
    pub(super) play: Mutex<Option<PlaySession>>,
    pub(super) game: Mutex<Option<Value>>,
    pub(super) game_session: Mutex<Option<games::GameSession>>,
    pub(super) scanning: AtomicBool,
    pub(super) apps_scanning: AtomicBool,
    pub(super) meta_status: Mutex<(bool, Option<String>)>,
    pub(super) game_info_status: Mutex<(bool, Option<String>)>,
    pub(super) updater: Mutex<Option<Arc<Updater>>>,
    pub(super) helper: system::SystemHelper,
    pub(super) transfers: Arc<TransferQueue>,
    pub(super) battery: Mutex<Option<Option<bool>>>,
    /// Set when the game's suspend timer ran (or the user chose "back to game"); cleared when the UI
    /// comes back.
    pub(super) suspended: AtomicBool,
}

/// One VLC playback session and the bookkeeping its progress events need.
pub(super) struct PlaySession {
    pub(super) session: vlc::VlcSession,
    pub(super) title: String,
    pub(super) queue: Vec<viewmodel::PlayItem>,
    pub(super) started_at: i64,
    /// Seconds actually spent watching per queue item (seeks don't count).
    pub(super) watched: HashMap<String, f64>,
    pub(super) last_pos: Option<(String, f64)>,
}

impl Inner {
    pub(super) fn stores(&self) -> Stores<'_> {
        Stores { progress: &self.progress, meta: &self.meta, games: &self.games_store, game_info: &self.game_info_store, prefs: &self.prefs, stats: &self.stats }
    }

    /// A file next to the stores (e.g. the updater's download folder).
    pub(super) fn user_file(&self, name: &str) -> PathBuf {
        self.data_dir.join(name)
    }

    pub(super) fn ui_lang(&self) -> &'static str {
        match self.settings.get("uiLanguage").and_then(|v| v.as_str().map(str::to_string)).as_deref() {
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

    /// A fresh `GameInfo` over the shared store: every instance sees the same in-memory state, so
    /// enrichment threads and interactive commands never block each other (the JS version interleaves
    /// through the event loop; here they share the store's lock instead).
    pub(super) fn make_game_info(&self) -> gameinfo::GameInfo {
        gameinfo::GameInfo::new(
            self.game_info_store.clone(),
            self.artwork_dir.clone(),
            self.settings.get("sgdbKey").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            Some(self.ui_lang()),
        )
    }

    pub(super) fn make_metadata(&self) -> metadata::Metadata {
        metadata::Metadata::new(
            self.meta.clone(),
            self.artwork_dir.clone(),
            self.settings.get("tmdbKey").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            Some(if self.ui_lang() == "fr" { "fr-FR".to_string() } else { "en-US".to_string() }),
        )
    }

    /// Raw games (Steam scan + manual), with overrides applied, as fed to both the UI and GameInfo.
    pub(super) fn raw_games(&self, steam: &SteamGames) -> Vec<Value> {
        viewmodel::raw_games(steam, &self.stores())
    }

    pub(super) fn flush_all(&self) {
        for store in [&self.settings, &self.progress, &self.meta, &self.games_store, &self.game_info_store, &self.prefs, &self.stats, &self.servers, &self.apps_store] {
            let _ = store.flush();
        }
    }

    pub(super) fn push_state(&self) {
        self.host.emit("state", build_state(self));
    }

    pub(super) fn toast(&self, key: &str, vars: Value, kind: &str) {
        let t = json!({"key": key, "vars": vars, "kind": kind});
        self.host.emit("toast", t.clone());
        self.toasts.lock().unwrap().push(t);
    }

    pub(super) fn setting_bool(&self, key: &str, default: bool) -> bool {
        self.settings.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    pub(super) fn setting_str(&self, key: &str) -> Option<String> {
        self.settings.get(key).and_then(|v| v.as_str().map(str::to_string))
    }

    /// The persisted settings overlaid on the defaults, exactly like settings.data merging.
    pub(super) fn settings_map(&self) -> Map<String, Value> {
        let mut m = default_settings();
        for key in m.clone().keys() {
            if let Some(v) = self.settings.get(key) {
                m.insert(key.clone(), v);
            }
        }
        m
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

fn apps_view(st: &Inner) -> Vec<Value> {
    let icons = st.apps_store.get("icons").unwrap_or_else(|| json!({}));
    let hidden = st.apps_store.get("hidden").unwrap_or_else(|| json!({}));
    let recent = st.apps_store.get("recent").unwrap_or_else(|| json!({}));
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
        })
    }).collect()
}

pub(super) fn update_state_json(st: &updater::UpdateState) -> Value {
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

pub(super) fn transfer_state_json(j: &lounge_core::transfers::JobState) -> Value {
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

pub(super) fn build_state(st: &Inner) -> Value {
    let lib = st.library.lock().unwrap();
    let steam = st.steam_games.lock().unwrap();
    let vm = viewmodel::build_view_model(&lib, &steam, &st.stores(), &file_url);

    let servers = st.servers.get("servers").and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().map(public_server).collect::<Vec<_>>();
    let update = st.updater.lock().unwrap().as_ref().map(|u| update_state_json(&u.state()));
    let meta_status = st.meta_status.lock().unwrap();
    let game_info_status = st.game_info_status.lock().unwrap();
    json!({
        "settings": st.settings_map(),
        "lang": st.ui_lang(),
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
        "transfers": st.transfers.state().iter().map(transfer_state_json).collect::<Vec<_>>(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

impl Backend {
    /// The current state payload (also what the `state` event carries).
    pub fn get_state(&self) -> Value {
        build_state(self)
    }

    pub fn save_ui_state(&self, s: Value) {
        *self.ui_state.lock().unwrap() = Some(s);
    }
}
