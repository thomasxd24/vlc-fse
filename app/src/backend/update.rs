//! Updates (always the user's call: we only check, then ask).

use super::platform::spawn_detached;
use super::state::{update_state_json, Inner};
use super::{Backend, WindowOp};
use lounge_core::updater::{self, Updater};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

const REPO: &str = "thomasxd24/vlc-fse";
const UPDATE_FIRST_CHECK_MS: u64 = 30 * 1000;
const UPDATE_INTERVAL_MS: u64 = 6 * 3600 * 1000;

impl Backend {
    pub(super) fn setup_updater(&self) {
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
        let dir = self.user_file("updates");
        let host = self.host.clone();
        let updater = Updater::new(
            updater::UpdaterConfig { repo: REPO.into(), version: env!("CARGO_PKG_VERSION").into(), install_type, dir },
            move |state| {
                host.emit("update", update_state_json(state));
            },
        );
        *self.updater.lock().unwrap() = Some(Arc::new(updater));
        if install_type.is_none() {
            return;
        }
        // Auto-check shortly after startup, then every 6 hours, unless one is downloading or we're in a
        // game. The thread only holds a weak handle, so it ends when the backend is dropped.
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(UPDATE_FIRST_CHECK_MS));
            loop {
                let Some(inner) = weak.upgrade() else { return };
                {
                    let auto = inner.setting_bool("autoCheckUpdates", true);
                    let in_game = inner.game_session.lock().unwrap().is_some();
                    let downloading = inner.updater.lock().unwrap().as_ref().map(|u| u.state().status == updater::Status::Downloading).unwrap_or(false);
                    if auto && !in_game && !downloading {
                        if let Some(u) = inner.updater.lock().unwrap().clone() {
                            std::thread::spawn(move || {
                                u.check();
                            });
                        }
                    }
                }
                drop(inner);
                std::thread::sleep(Duration::from_millis(UPDATE_INTERVAL_MS));
            }
        });
    }

    pub fn check_update(&self) -> Value {
        let updater = self.updater.lock().unwrap().clone();
        let Some(u) = updater else { return Value::Null };
        let state = update_state_json(&u.state());
        // The network check runs in the background; the UI hears the result via the update event.
        std::thread::spawn(move || {
            u.check();
        });
        state
    }

    pub fn skip_update(&self, version: &str) {
        self.settings.set("skippedVersion", json!(version));
        self.push_state();
    }

    pub fn install_update(&self) -> Value {
        let Some(u) = self.updater.lock().unwrap().clone() else { return json!({"ok": false}) };
        if self.game_session.lock().unwrap().is_some() {
            return json!({"ok": false, "errorKey": "err.updateWhilePlaying"});
        }
        self.flush_all();
        // Download (if needed) and compute the install command (blocking; we're off the UI thread).
        let result: Result<(String, Vec<String>, Option<PathBuf>), String> = (|| {
            if u.state().status != updater::Status::Ready {
                u.download()?;
            }
            let cmd = u.install_command(std::process::id(), &std::env::current_exe().unwrap_or_default())?;
            Ok((cmd.command, cmd.args, cmd.status))
        })();
        match result {
            Err(e) => {
                // Surfaced through the updater's own error state.
                json!({"ok": false, "errorKey": "err.update", "vars": {"message": e}})
            }
            Ok((command, args, status)) => {
                if let Some(status_file) = status {
                    return self.install_fse(&command, &args, &status_file);
                }
                if let Some(p) = self.play.lock().unwrap().as_ref() {
                    p.session.kill();
                }
                if let Err(e) = spawn_detached(&command, &args) {
                    return json!({"ok": false, "errorKey": "err.update", "vars": {"message": e.to_string()}});
                }
                // Give the UI a moment to show "Installing…", then get out of the installer's way.
                let host = self.host.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1200));
                    host.window(WindowOp::Quit);
                });
                json!({"ok": true})
            }
        }
    }

    /// FSE package: start the install script outside the package, then stay open until the installer has
    /// its admin rights; the installer closes Lounge itself when it replaces the package.
    fn install_fse(&self, command: &str, args: &[String], status_file: &Path) -> Value {
        let fail = |error: Option<String>, error_key: &str| -> Value {
            json!({"ok": false, "errorKey": error_key, "vars": error.map(|m| json!({"message": m})).unwrap_or(Value::Null)})
        };
        let out = lounge_core::hidden_command(command).args(args).output();
        match out {
            Err(e) => return fail(Some(format!("couldn't start the update ({e})")), "err.update"),
            Ok(o) if !o.status.success() => {
                let err_out = String::from_utf8_lossy(&o.stderr);
                return fail(Some(format!("couldn't start the update ({})", err_out.trim().chars().take(200).collect::<String>())), "err.update");
            }
            Ok(_) => {}
        }
        let result = {
            let u = self.updater.lock().unwrap().clone();
            let Some(u) = u else { return fail(None, "err.update") };
            u.wait_for_fse(status_file, &updater::WaitForFseOpts::default())
        };
        match result.as_str() {
            "declined" => return fail(None, "upd.declined"),
            "noprompt" => return fail(None, "upd.noPrompt"),
            "elevated" => {}
            other => return fail(Some(other.strip_prefix("failed: ").unwrap_or(other).to_string()), "err.update"),
        }
        if let Some(u) = self.updater.lock().unwrap().as_ref() {
            u.set_status(updater::Status::Installing);
        }
        if let Some(p) = self.play.lock().unwrap().as_ref() {
            p.session.kill();
        }
        json!({"ok": true})
    }
}
