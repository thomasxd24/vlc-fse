//! The event router: drains VLC/game events and the background timers (main.js's event handlers).

use super::state::now_millis;
use super::scan::scan_games_blocking;
use super::Backend;
use lounge_core::{games, vlc};
use serde_json::{json, Value};
use std::time::Duration;

/// After a game starts running, the interface gets out of its way this long later.
pub(super) const SUSPEND_DELAY_MS: u64 = 4000;

impl Backend {
    pub(super) fn poll_events(&self) {
        // VLC progress / exit.
        let (progresses, exit) = {
            let play = self.play.lock().unwrap();
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
            self.handle_vlc_progress(p);
        }
        if let Some((code, last)) = exit {
            let started_at = self.play.lock().unwrap().as_ref().map(|p| p.started_at).unwrap_or(0);
            // VLC that dies within seconds without ever reporting a position failed to start or open the file.
            let failed = code.unwrap_or(0) != 0 && last.is_none() && now_millis() - started_at < 15000;
            self.finish_play(if failed { Some(("err.vlcExited", json!({"code": code}))) } else { None });
        }

        // Game sessions.
        let game_events = {
            let session = self.game_session.lock().unwrap();
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
                        let mut game = self.game.lock().unwrap();
                        if let Some(g) = game.as_mut() {
                            g["phase"] = json!("running");
                        }
                    }
                    self.host.emit("game", self.game.lock().unwrap().clone().unwrap_or(Value::Null));
                    if self.setting_bool("freeWhilePlaying", true) {
                        let this = self.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(SUSPEND_DELAY_MS));
                            // The Electron version cancelled this timer on resume; here it only fires
                            // if the game is still up.
                            if this.game_session.lock().unwrap().is_some() {
                                this.suspend_ui();
                            }
                        });
                    }
                }
                games::GameEvent::Exit { reason, played_ms, error } => {
                    let game_id = self.game.lock().unwrap().as_ref().and_then(|g| g.get("id").and_then(Value::as_str)).unwrap_or("").to_string();
                    let steam_source = {
                        let steam = self.steam_games.lock().unwrap();
                        steam.entries.iter().any(|g| g.get("id").and_then(Value::as_str) == Some(game_id.as_str()))
                    };
                    if reason == games::ExitReason::Stub {
                        // The exe was a launcher that handed off to the real game: stay out of the way
                        // until the user comes back to Lounge and says they're done.
                        let mut game = self.game.lock().unwrap();
                        if let Some(g) = game.as_mut() {
                            g["phase"] = json!("untracked");
                        }
                        continue;
                    }
                    *self.game_session.lock().unwrap() = None;
                    *self.game.lock().unwrap() = None;
                    if played_ms > 0 || reason == games::ExitReason::Exited {
                        self.record_play(&game_id, played_ms);
                    }
                    self.resume_ui();
                    self.host.emit("game", Value::Null);
                    if reason == games::ExitReason::Error {
                        self.toast("err.gameStart", json!({"message": error.unwrap_or_default()}), "error");
                    } else if reason == games::ExitReason::Timeout {
                        let title = self.games_store.get("manual").and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().find(|g| g.get("id").and_then(Value::as_str) == Some(game_id.as_str())).and_then(|g| g.get("title").cloned()).unwrap_or(json!(""));
                        self.toast("err.gameTimeout", json!({"title": title}), "error");
                    }
                    // Steam updates its own playtime when a game closes; pick that up.
                    if steam_source {
                        let this = self.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(3000));
                            let games = scan_games_blocking(&this.settings);
                            *this.steam_games.lock().unwrap() = games;
                            this.push_state();
                        });
                    } else {
                        self.push_state();
                    }
                }
            }
        }
    }
}
