//! Lounge's backend: application state, the stores, sessions (VLC, games), background work
//! (scans, enrichment, updater, transfers) and every command the UI can issue. A port of the Tauri
//! shell's `src-tauri/app/src/lib.rs` (branch `tauri-migration`) with Tauri taken out: events go out
//! through [`Host::emit`], window operations through [`Host::window`], and commands are plain blocking
//! methods on [`Backend`]. Nothing in here depends on Slint either.
//!
//! Commands block (they may touch disk, the network or spawn processes), so the UI calls them off its
//! event-loop thread. Payloads are the same `serde_json::Value` shapes the web renderer used, except
//! that every artwork/icon field is a plain absolute filesystem path (or null) instead of an
//! asset-protocol URL.
//!
//! Layout: `state` (the shared state, defaults, payload builders), `scan` (library/Steam scans and
//! enrichment), `media` (VLC playback), `games`, `settings` (settings + dialogs), `servers` (remote
//! browsing and transfers), `apps`, `update`, `system` (quick menu), `window` (suspend/resume and window
//! commands), `events` (the 10 Hz event router), `platform` (shell helpers, autostart),
//! `single_instance`.

#![allow(dead_code)]

mod apps;
mod events;
mod games;
mod media;
mod platform;
mod scan;
mod servers;
mod settings;
pub mod single_instance;
mod state;
mod system;
mod update;
mod window;

use lounge_core::store::JsonStore;
use lounge_core::transfers::TransferQueue;
use serde_json::{json, Value};
use state::{default_settings, jmap, Inner};
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

/// What the backend needs from the shell hosting it. Called from any thread.
pub trait Host: Send + Sync + 'static {
    /// One of the renderer's event channels: `state`, `now-playing`, `game`, `toast`, `update`,
    /// `transfers`, `legion-report`, `legion-state` — same names and payloads as the Tauri shell.
    fn emit(&self, event: &str, payload: Value);
    fn window(&self, op: WindowOp);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOp {
    Minimize,
    /// Restore if minimised, show and focus.
    BringToFront,
    SetFullscreen(bool),
    ToggleFullscreen,
    /// A game is running: unload the interface (drop pages, stop animations) and minimise.
    SuspendUi,
    /// The game ended or Lounge was focused again: rebuild the interface and bring it to front.
    ResumeUi,
    /// Leave the event loop; the backend has already flushed its stores.
    Quit,
}

pub(crate) mod paths {
    use std::path::PathBuf;

    const APP_DIR_NAME: &str = "com.foyer.launcher";

    /// Where the JSON stores, artwork and updater files live: the same place the Tauri build kept them
    /// (`%APPDATA%\com.foyer.launcher` on Windows), so users' data carries over. `LOUNGE_DATA_DIR`
    /// overrides it (tests, demo mode).
    pub fn data_dir() -> PathBuf {
        match std::env::var_os("LOUNGE_DATA_DIR").filter(|v| !v.is_empty()) {
            Some(d) => PathBuf::from(d),
            None => dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR_NAME),
        }
    }

    /// Regenerable caches (thumbnails): `dirs::cache_dir()/com.foyer.launcher`, or `<data dir>/cache`
    /// under `LOUNGE_DATA_DIR`.
    pub fn cache_dir() -> PathBuf {
        match std::env::var_os("LOUNGE_DATA_DIR").filter(|v| !v.is_empty()) {
            Some(d) => PathBuf::from(d).join("cache"),
            None => dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR_NAME),
        }
    }
}

/// The backend handle: cheap to clone, `Send + Sync`. All commands are methods on it (see the
/// submodules); they block, so call them from worker threads.
#[derive(Clone)]
pub struct Backend(Arc<Inner>);

impl Deref for Backend {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl Backend {
    /// Everything the Tauri `.setup()` did: data dirs, stores, the initial library + Steam scan, the
    /// transfer queue, the Legion HID reader, the battery probe, the updater and the 100 ms event
    /// poller. Blocks for the initial scan.
    pub fn start(host: Arc<dyn Host>) -> Result<Backend, String> {
        let dir = paths::data_dir();
        let cache_dir = paths::cache_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
        let artwork_dir = dir.join("artwork");
        std::fs::create_dir_all(&artwork_dir).map_err(|e| format!("can't create {}: {e}", artwork_dir.display()))?;
        let _ = std::fs::create_dir_all(cache_dir.join("thumbs"));

        let settings = JsonStore::new(dir.join("settings.json"), default_settings());
        let progress = JsonStore::new(dir.join("progress.json"), jmap(json!({"items": {}})));
        let meta = JsonStore::new(dir.join("metadata.json"), jmap(json!({"entries": {}})));
        let games_store = JsonStore::new(dir.join("games.json"), jmap(json!({"manual": [], "overrides": {}, "stats": {}})));
        let game_info_store = JsonStore::new(dir.join("gameinfo.json"), jmap(json!({"games": {}})));
        let prefs = JsonStore::new(dir.join("prefs.json"), jmap(json!({"favorites": {}, "hidden": {}, "languages": {}})));
        let stats = JsonStore::new(dir.join("stats.json"), jmap(json!({"sessions": []})));
        let servers = JsonStore::new(dir.join("servers.json"), jmap(json!({"servers": []})));
        let apps_store = JsonStore::new(dir.join("apps.json"), jmap(json!({"apps": [], "icons": {}, "hidden": {}, "recent": {}, "scannedAt": 0})));

        // The initial library comes from a scan, like the JS version's did-finish-load rescan.
        let initial_library = scan::scan_media_blocking(&settings);
        let steam_games = scan::scan_games_blocking(&settings);

        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            // The transfer queue: opens connections on demand and streams state changes out.
            let connect_weak = weak.clone();
            let event_weak = weak.clone();
            let transfers = Arc::new(TransferQueue::new(
                move |server_id: &str| {
                    let inner = connect_weak.upgrade().ok_or_else(|| "gone".to_string())?;
                    Backend(inner).connect_server(server_id).map_err(|e| e.code)
                },
                move |_event| {
                    if let Some(inner) = event_weak.upgrade() {
                        let b = Backend(inner);
                        b.host.emit("transfers", b.transfers_json());
                    }
                },
            ));
            Inner {
                host: host.clone(),
                data_dir: dir.clone(),
                cache_dir: cache_dir.clone(),
                artwork_dir: artwork_dir.clone(),
                settings,
                progress,
                meta,
                games_store,
                game_info_store,
                prefs,
                stats,
                servers,
                apps_store,
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
                helper: lounge_core::system::SystemHelper::new(),
                transfers,
                battery: Mutex::new(None),
                suspended: AtomicBool::new(false),
            }
        });
        let backend = Backend(inner);

        crate::legion_hid::start(host);

        // Battery presence for the quick menu (asked once, like the JS version).
        {
            let weak = Arc::downgrade(&backend.0);
            std::thread::spawn(move || {
                let present = lounge_core::system::has_battery();
                if let Some(inner) = weak.upgrade() {
                    *inner.battery.lock().unwrap() = Some(present);
                    inner.push_state();
                }
            });
        }

        backend.setup_updater();

        // The event router (VLC/game/login events at ~10 Hz). It holds only a weak handle, so it ends
        // when the last `Backend` clone is dropped.
        {
            let weak = Arc::downgrade(&backend.0);
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(100));
                let Some(inner) = weak.upgrade() else { return };
                Backend(inner).poll_events();
            });
        }

        Ok(backend)
    }

    /// The directory holding the JSON stores, artwork and updater downloads.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.0.data_dir
    }

    /// Where `crate::thumbs::thumbnail` should cache resized artwork (`<cache dir>/thumbs`).
    pub fn thumbs_dir(&self) -> PathBuf {
        self.0.cache_dir.join("thumbs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct Recorder {
        events: StdMutex<Vec<(String, Value)>>,
        ops: StdMutex<Vec<WindowOp>>,
    }

    impl Host for Recorder {
        fn emit(&self, event: &str, payload: Value) {
            self.events.lock().unwrap().push((event.to_string(), payload));
        }
        fn window(&self, op: WindowOp) {
            self.ops.lock().unwrap().push(op);
        }
    }

    /// `LOUNGE_DATA_DIR` is process-wide, so tests that start a backend take turns.
    static ENV: StdMutex<()> = StdMutex::new(());

    fn start_in(dir: &std::path::Path) -> (Backend, Arc<Recorder>) {
        std::env::set_var("LOUNGE_DATA_DIR", dir);
        let rec = Arc::new(Recorder::default());
        let backend = Backend::start(rec.clone()).expect("backend starts");
        (backend, rec)
    }

    #[test]
    fn get_state_has_the_keys_the_renderer_expects() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (backend, _rec) = start_in(tmp.path());
        let state = backend.get_state();
        let obj = state.as_object().expect("state is an object");
        for key in [
            "settings", "lang", "library", "scanning", "metaStatus", "gameInfoStatus", "nowPlaying", "game", "uiState", "toasts",
            "platform", "packaged", "fsePackage", "systemControls", "update", "hasBattery", "servers", "apps", "appsScanning",
            "appsScannedAt", "transfers", "version",
        ] {
            assert!(obj.contains_key(key), "state is missing `{key}`");
        }
        assert_eq!(state["settings"]["vlcFullscreen"], json!(true));
        assert_eq!(state["scanning"], json!(false));
        assert_eq!(state["version"], json!(env!("CARGO_PKG_VERSION")));
        assert!(backend.thumbs_dir().starts_with(tmp.path()));
        assert!(tmp.path().join("artwork").is_dir());
    }

    #[test]
    fn save_settings_round_trips_and_persists() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (backend, rec) = start_in(tmp.path());
        let patch = json!({"uiScale": 1.25, "vlcExtraArgs": "--foo", "startFullscreen": false, "notASetting": 1});
        let saved = backend.save_settings(patch.as_object().unwrap().clone());
        assert_eq!(saved["uiScale"], json!(1.25));
        assert_eq!(saved["vlcExtraArgs"], json!("--foo"));
        assert!(saved.get("notASetting").is_none(), "unknown keys are dropped");
        assert_eq!(backend.get_state()["settings"]["uiScale"], json!(1.25));
        assert!(rec.ops.lock().unwrap().contains(&WindowOp::SetFullscreen(false)), "fullscreen follows the setting");
        assert!(rec.events.lock().unwrap().iter().any(|(e, _)| e == "state"), "a state event went out");

        backend.quit();
        assert!(rec.ops.lock().unwrap().contains(&WindowOp::Quit));
        let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(tmp.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!(on_disk["uiScale"], json!(1.25));
    }

    #[test]
    fn data_dirs_honour_the_override() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LOUNGE_DATA_DIR", "/tmp/lounge-x");
        assert_eq!(paths::data_dir(), PathBuf::from("/tmp/lounge-x"));
        assert_eq!(paths::cache_dir(), PathBuf::from("/tmp/lounge-x/cache"));
        std::env::remove_var("LOUNGE_DATA_DIR");
        assert!(paths::data_dir().ends_with("com.foyer.launcher"));
        assert!(paths::cache_dir().ends_with("com.foyer.launcher"));
    }
}
