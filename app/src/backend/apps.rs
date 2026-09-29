//! The Start-menu apps list.

use super::platform::icon_for;
use super::state::now_millis;
use super::{Backend, WindowOp};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

const APP_STEP_ASIDE_MS: u64 = 1500;

/// Save an app's icon as a PNG in the artwork folder (Store logo, or the program's own icon).
fn app_icon(artwork_dir: &Path, a: &lounge_core::apps::AppEntry) -> Option<String> {
    let dir = artwork_dir.join("apps");
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(format!("{}.png", a.id));
    match a.kind {
        "store" => {
            let logo = lounge_core::apps::store_logo(a.package_dir.as_deref()?, a.store_app.as_deref())?;
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

impl Backend {
    pub fn rescan_apps(&self) {
        if self.apps_scanning.swap(true, Ordering::SeqCst) {
            return;
        }
        self.push_state();
        let this = self.clone();
        std::thread::spawn(move || {
            let result = lounge_core::apps::list_apps(&lounge_core::apps::AppEnv::from_process_env());
            if let Ok(list) = result {
                let list_json: Vec<Value> = list.iter().map(|a| json!({
                    "id": a.id, "name": a.name, "appId": a.app_id, "kind": a.kind, "exe": a.exe,
                    "packageDir": a.package_dir, "storeApp": a.store_app, "shortcut": a.shortcut,
                })).collect();
                this.apps_store.set("apps", json!(list_json));
                this.apps_store.set("scannedAt", json!(now_millis()));
                this.push_state();
                // Icons: the Store logo from the package, or the program's own icon.
                let icons = this.apps_store.get("icons").unwrap_or_else(|| json!({}));
                let mut icons_map = icons.as_object().cloned().unwrap_or_default();
                for a in list {
                    if this.game_session.lock().unwrap().is_some() {
                        break;
                    }
                    let existing = icons_map.get(&a.id).and_then(Value::as_str).map(String::from);
                    if existing.map(|p| Path::new(&p).is_file()).unwrap_or(false) {
                        continue;
                    }
                    if let Some(file) = app_icon(&this.artwork_dir, &a) {
                        icons_map.insert(a.id.clone(), json!(file));
                        this.apps_store.set("icons", Value::Object(icons_map.clone()));
                        this.push_state();
                    }
                }
            }
            this.apps_scanning.store(false, Ordering::SeqCst);
            this.push_state();
        });
    }

    pub fn launch_app(&self, id: &str) -> Value {
        let a = self.apps_store.get("apps").and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().find(|a| a.get("id").and_then(Value::as_str) == Some(id));
        let Some(a) = a else { return json!({"ok": false, "errorKey": "err.notFound"}) };
        if let Err(e) = lounge_core::apps::launch_app(a.get("appId").and_then(Value::as_str).unwrap_or("")) {
            return json!({"ok": false, "errorKey": "err.appLaunch", "vars": {"message": e}});
        }
        self.apps_store.update("recent", json!({}), |r| {
            if let Value::Object(m) = r {
                m.insert(id.to_string(), json!(now_millis()));
            }
        });
        self.push_state();
        // Step aside so the app comes up in front; the home button (or Alt+Tab) brings Lounge back.
        let host = self.host.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(APP_STEP_ASIDE_MS));
            host.window(WindowOp::Minimize);
        });
        json!({"ok": true})
    }

    pub fn hide_app(&self, id: &str, hidden: bool) {
        self.apps_store.update("hidden", json!({}), |m| {
            if let Value::Object(m) = m {
                if hidden {
                    m.insert(id.to_string(), json!(true));
                } else {
                    m.remove(id);
                }
            }
        });
        self.push_state();
    }
}
