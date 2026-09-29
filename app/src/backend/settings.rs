//! Settings, file dialogs and the small library-maintenance commands.

use super::platform::{reveal_in_shell, set_autostart};
use super::state::default_settings;
use super::{Backend, WindowOp};
use lounge_core::vlc;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

impl Backend {
    pub fn save_settings(&self, patch: Map<String, Value>) -> Value {
        let before: Map<String, Value> = default_settings().keys().map(|k| (k.clone(), self.settings.get(k).unwrap_or(Value::Null))).collect();
        let prev_lang = self.ui_lang();
        let allowed = default_settings();
        let login_patch = patch.get("launchAtLogin").and_then(Value::as_bool);
        for (k, v) in patch {
            if allowed.contains_key(&k) {
                self.settings.set(k, v);
            }
        }
        let changed = |k: &str| before.get(k) != self.settings.get(k).as_ref();

        // Live fullscreen follows the checkbox, like the JS version's setFullScreen here.
        if changed("startFullscreen") {
            if let Some(fs) = self.settings.get("startFullscreen").and_then(|v| v.as_bool()) {
                self.host.window(WindowOp::SetFullscreen(fs));
            }
        }
        // Launch at login (Electron: `app.setLoginItemSettings`, packaged builds only). The Tauri
        // port registered its autostart plugin but never applied the setting; this does.
        if let Some(enabled) = login_patch {
            if !cfg!(debug_assertions) {
                set_autostart(enabled);
            }
        }
        let lang_changed = self.ui_lang() != prev_lang;
        if lang_changed {
            // Synopses and game descriptions are per language: refetch them, but keep showing what we
            // have (artwork doesn't depend on language) until the new text arrives.
            for (store, key) in [(&self.meta, "entries"), (&self.game_info_store, "games")] {
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
            self.enrich_games();
        }
        if changed("libraries") || changed("steamEnabled") || changed("steamPath") {
            let this = self.clone();
            std::thread::spawn(move || this.do_rescan());
        } else if changed("tmdbKey") || lang_changed {
            self.enrich_metadata();
        }
        self.push_state();
        Value::Object(self.settings_map())
    }

    pub fn pick_folder(&self) -> Option<String> {
        rfd::FileDialog::new().pick_folder().map(|p| p.to_string_lossy().into_owned())
    }

    pub fn pick_vlc(&self) -> Option<String> {
        rfd::FileDialog::new().add_filter("VLC", &["exe"]).pick_file().map(|p| p.to_string_lossy().into_owned())
    }

    pub fn pick_key_file(&self) -> Option<String> {
        rfd::FileDialog::new().pick_file().map(|p| p.to_string_lossy().into_owned())
    }

    pub fn detect_vlc(&self) -> Option<String> {
        vlc::find_vlc(
            self.setting_str("vlcPath").filter(|s| !s.is_empty()).map(PathBuf::from).as_deref(),
            &vlc::VlcEnv::from_process_env(),
        ).map(|p| p.to_string_lossy().into_owned())
    }

    pub fn clear_metadata(&self) {
        self.meta.set("entries", json!({}));
        self.game_info_store.set("games", json!({}));
        self.enrich_metadata();
        self.enrich_games();
    }

    pub fn show_in_folder(&self, p: &str) {
        let lib = self.library.lock().unwrap();
        let known = lib.movies.iter().any(|m| m.path.to_string_lossy() == p) || lib.shows.iter().any(|s| s.episodes.iter().any(|e| e.path.to_string_lossy() == p));
        drop(lib);
        if known {
            let _ = reveal_in_shell(Path::new(p));
        }
    }
}
