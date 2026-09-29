//! Media playback (VLC), watch state and stats.

use super::platform::{power_save_blocker_start, power_save_blocker_stop};
use super::state::{now_millis, PlaySession};
use super::Backend;
use lounge_core::viewmodel;
use lounge_core::vlc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Seconds of playback credited per VLC poll; bigger jumps are seeks, not watching.
const MAX_WATCH_STEP: f64 = 5.0;

/// Split a launch-options / extra-args string, honouring double quotes.
pub(super) fn split_args(s: &str) -> Vec<String> {
    lounge_core::games::split_args(s)
}

impl Backend {
    pub fn play(&self, req: Value) -> Value {
        {
            let play = self.play.lock().unwrap();
            if let Some(p) = play.as_ref() {
                p.session.kill();
            }
        }
        let vlc_path = vlc::find_vlc(
            self.setting_str("vlcPath").filter(|s| !s.is_empty()).map(PathBuf::from).as_deref(),
            &vlc::VlcEnv::from_process_env(),
        );
        let Some(vlc_path) = vlc_path else { return json!({"ok": false, "errorKey": "err.vlcNotFound"}) };
        let job = viewmodel::build_queue(&self.library.lock().unwrap(), &self.stores(), &req, self.setting_bool("autoplayNext", true));
        let Ok((title, queue)) = job else { return json!({"ok": false, "errorKey": "err.notFound"}) };

        // Per-item resume positions and language overrides; the Settings defaults ride in as global
        // options, which the per-item ones then override (the JS version's same ordering trick).
        let extra_args = split_args(&self.setting_str("vlcExtraArgs").unwrap_or_default());
        let items: Vec<vlc::QueueItem> = queue.iter().map(|i| vlc::QueueItem {
            path: &i.path,
            start_time: i.start_time,
            languages: i.languages.as_ref().map(|l| vlc::Languages {
                audio: l.get("audio").and_then(Value::as_str),
                subs: l.get("subs").and_then(Value::as_str),
            }),
        }).collect();
        let port = match vlc::free_port_pub() {
            Ok(p) => p,
            Err(e) => {
                self.toast("err.vlcStart", json!({"message": e.to_string()}), "error");
                return json!({"ok": false});
            }
        };
        let password = vlc::random_password_pub();
        let audio_lang = self.setting_str("audioLanguage");
        let sub_lang = self.setting_str("subLanguage");
        let opts = vlc::BuildArgsOptions {
            port,
            password: &password,
            fullscreen: self.setting_bool("vlcFullscreen", true),
            languages: vlc::Languages { audio: audio_lang.as_deref(), subs: sub_lang.as_deref() },
            extra_args: &extra_args.iter().map(String::as_str).collect::<Vec<_>>(),
        };
        let started = vlc::VlcSession::spawn(&vlc_path, &items, &opts);
        let session = match started {
            Ok(s) => s,
            Err(e) => {
                self.toast("err.vlcStart", json!({"message": e.to_string()}), "error");
                return json!({"ok": false});
            }
        };
        let current = queue.first().map(|i| i.path.clone()).unwrap_or_default();
        *self.play.lock().unwrap() = Some(PlaySession {
            session,
            title,
            watched: HashMap::new(),
            last_pos: None,
            started_at: now_millis(),
            queue,
        });
        let play = self.play.lock().unwrap();
        let ps = play.as_ref().unwrap();
        *self.now_playing.lock().unwrap() = Some(json!({
            "title": ps.title, "request": req, "current": current,
            "time": 0, "length": 0, "paused": false, "queueSize": ps.queue.len(), "index": 0,
        }));
        drop(play);
        self.host.emit("now-playing", self.now_playing.lock().unwrap().clone().unwrap_or(Value::Null));
        power_save_blocker_start();
        json!({"ok": true})
    }

    pub fn stop(&self) {
        if let Some(p) = self.play.lock().unwrap().as_ref() {
            p.session.kill();
        }
    }

    /// Playback controls on the "Playing in VLC" screen, mapped to VLC's HTTP commands. Track switching
    /// goes through VLC's own hotkeys so the on-screen label updates.
    pub fn np_command(&self, name: &str) {
        let commands: &[(&str, &[&str])] = &[
            ("pause", &["pl_pause"]),
            ("back", &["seek", "-10"]),
            ("forward", &["seek", "+30"]),
            ("next", &["pl_next"]),
            ("audio", &["key", "audio-track"]),
            ("subs", &["key", "subtitle-track"]),
        ];
        let Some((cmd, args)) = commands.iter().find(|(n, _)| *n == name) else { return };
        let play = self.play.lock().unwrap();
        let Some(ps) = play.as_ref() else { return };
        let status = ps.session.command(cmd, args.get(1).copied());
        if let Some(s) = status {
            let mut np = self.now_playing.lock().unwrap();
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

    pub fn set_watched(&self, req: Value) {
        let lib = self.library.lock().unwrap().clone();
        let paths = viewmodel::paths_for(&lib, &req);
        let watched = req.get("watched").and_then(Value::as_bool).unwrap_or(false);
        viewmodel::set_watched(&self.stores(), &paths, watched);
        self.push_state();
    }

    pub fn set_languages(&self, id: &str, languages: Option<Value>) {
        viewmodel::set_languages(&self.stores(), id, languages.as_ref());
        self.push_state();
    }

    pub fn set_pref(&self, id: &str, key: &str, value: bool) {
        viewmodel::set_pref(&self.stores(), id, key, value);
        self.push_state();
    }

    pub fn get_stats(&self) -> Value {
        let lib = self.library.lock().unwrap().clone();
        let steam = self.steam_games.lock().unwrap();
        viewmodel::stats_data(&lib, &steam, &self.stores(), &super::state::file_url)
    }

    pub(super) fn handle_vlc_progress(&self, progress: vlc::Progress) {
        viewmodel::record_progress(&self.stores(), Path::new(&progress.path), progress.time, progress.length);
        let mut play = self.play.lock().unwrap();
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
        let mut np = self.now_playing.lock().unwrap();
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
        self.host.emit("now-playing", self.now_playing.lock().unwrap().clone().unwrap_or(Value::Null));
    }

    pub(super) fn finish_play(&self, error_key: Option<(&str, Value)>) {
        // Credit the watch time gathered during the session, then clean up.
        let play = self.play.lock().unwrap().take();
        if let Some(ps) = play {
            let watch_start = ps.started_at;
            for (id, sec) in &ps.watched {
                viewmodel::log_session(&self.stores(), "watch", id, watch_start, sec / 60.0);
            }
        }
        power_save_blocker_stop();
        *self.now_playing.lock().unwrap() = None;
        self.host.emit("now-playing", Value::Null);
        if let Some((key, vars)) = error_key {
            self.toast(key, vars, "error");
        }
        self.push_state();
        self.bring_to_front();
    }
}
