//! Games: launching and tracking sessions, the manual list, overrides, artwork search.

use super::platform::{icon_for, open_path_in_shell, open_url_in_shell};
use super::state::{file_url, file_url_opt, now_millis};
use super::Backend;
use lounge_core::viewmodel;
use lounge_core::{games, steam};
use serde_json::{json, Map, Value};
use std::path::Path;

impl Backend {
    pub fn play_game(&self, id: &str) -> Value {
        if self.game_session.lock().unwrap().is_some() {
            return json!({"ok": false, "errorKey": "err.alreadyPlaying"});
        }
        let steam = self.steam_games.lock().unwrap();
        let raw = self.raw_games(&steam).into_iter().find(|g| g.get("id").and_then(Value::as_str) == Some(id));
        drop(steam);
        let Some(game) = raw else { return json!({"ok": false, "errorKey": "err.notFound"}) };
        let sg = games::SessionGame::from_json(&game);
        if sg.source == "manual" && !Path::new(&sg.exe).is_file() {
            return json!({"ok": false, "errorKey": "err.exeMissing"});
        }

        let quiet_steam = self.setting_bool("quietSteam", true);
        let steam_path = self.steam_games.lock().unwrap().steam_path.clone();
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
        *self.game.lock().unwrap() = Some(json!({"id": id, "title": sg.title, "phase": "launching", "startedAt": now_millis()}));
        self.host.emit("game", self.game.lock().unwrap().clone().unwrap_or(Value::Null));
        *self.game_session.lock().unwrap() = Some(session);
        json!({"ok": true})
    }

    pub fn back_to_game(&self) {
        if self.game_session.lock().unwrap().is_some() {
            self.suspend_ui();
        }
    }

    pub fn end_game(&self) {
        let untracked = self.game.lock().unwrap().as_ref().and_then(|g| g.get("phase").and_then(Value::as_str)) == Some("untracked");
        if untracked {
            // Already counted as a launch; the exit event was swallowed. We couldn't watch the game
            // itself, so count the time until the user said they were done.
            let (game_id, started_at) = {
                let g = self.game.lock().unwrap();
                (
                    g.as_ref().and_then(|g| g.get("id").and_then(Value::as_str)).unwrap_or("").to_string(),
                    g.as_ref().and_then(|g| g.get("startedAt").and_then(Value::as_i64)).unwrap_or(0),
                )
            };
            self.record_play(&game_id, now_millis() - started_at);
            *self.game_session.lock().unwrap() = None;
            *self.game.lock().unwrap() = None;
            self.host.emit("game", Value::Null);
            self.push_state();
        } else if let Some(s) = self.game_session.lock().unwrap().as_ref() {
            s.stop_tracking();
        }
    }

    pub(super) fn record_play(&self, id: &str, played_ms: i64) {
        self.games_store.update("stats", json!({}), |stats| {
            let Value::Object(m) = stats else { return };
            let entry = m.entry(id.to_string()).or_insert_with(|| json!({"playtime": 0, "lastPlayed": 0}));
            entry["lastPlayed"] = json!(now_millis());
            let prev = entry["playtime"].as_i64().unwrap_or(0);
            entry["playtime"] = json!(prev + (played_ms / 60000));
        });
        viewmodel::log_session(&self.stores(), "game", id, now_millis() - played_ms, played_ms as f64 / 60000.0);
    }

    pub fn add_game(&self) -> Value {
        let picked = rfd::FileDialog::new().add_filter("Games", &["exe", "bat", "cmd", "lnk", "url"]).pick_file();
        let Some(exe) = picked.map(|p| p.to_string_lossy().into_owned()) else { return json!({"ok": false}) };

        let manual = self.games_store.get("manual").and_then(|v| v.as_array().cloned()).unwrap_or_default();
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
            "art": {"icon": icon_for(&self.artwork_dir, &exe)},
        });
        self.games_store.update("manual", json!([]), |m| {
            if let Value::Array(a) = m {
                a.push(game.clone());
            }
        });
        self.push_state();
        self.enrich_games();
        json!({"ok": true, "id": id})
    }

    pub fn edit_game(&self, id: &str, patch: Value) {
        let mut o = self.games_store.get("overrides").and_then(|v| v.get(id).cloned()).unwrap_or_else(|| json!({}));
        let manual_index = self.games_store.get("manual").and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().position(|g| g.get("id").and_then(Value::as_str) == Some(id));
        let mut refetch = false;
        if let Some(title) = patch.get("title") {
            let t = title.as_str().unwrap_or("").trim().to_string();
            match manual_index {
                Some(i) => {
                    self.games_store.update("manual", json!([]), |m| {
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
                self.games_store.update("manual", json!([]), |m| {
                    if let Value::Array(a) = m {
                        if let Some(i) = a.iter().position(|g| g.get("id").and_then(Value::as_str) == Some(id)) {
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
        self.games_store.update("overrides", json!({}), |m| {
            if let Value::Object(m) = m {
                m.insert(id.to_string(), o.clone());
            }
        });
        if refetch {
            self.make_game_info().forget(id);
            self.enrich_games();
        }
        self.push_state();
    }

    pub fn remove_game(&self, id: &str) {
        self.games_store.update("manual", json!([]), |m| {
            if let Value::Array(a) = m {
                a.retain(|g| g.get("id").and_then(Value::as_str) != Some(id));
            }
        });
        self.games_store.update("overrides", json!({}), |m| {
            if let Value::Object(m) = m {
                m.remove(id);
            }
        });
        self.make_game_info().forget(id);
        self.push_state();
    }

    pub fn search_steam(&self, term: &str) -> Value {
        let mut game_info = self.make_game_info();
        match game_info.store_search(term) {
            Ok(hits) => json!(hits.iter().map(|h| json!({"steamAppId": h.steam_app_id, "title": h.title, "thumb": h.thumb})).collect::<Vec<_>>()),
            Err(_) => json!([]),
        }
    }

    pub fn search_sgdb(&self, term: &str) -> Value {
        let game_info = self.make_game_info();
        match game_info.sgdb_search(term) {
            Ok(hits) => json!(hits.iter().map(|h| json!({"sgdbId": h.sgdb_id, "title": h.title, "year": h.year})).collect::<Vec<_>>()),
            Err(e) => json!({"error": e.to_string()}),
        }
    }

    pub fn sgdb_images(&self, kind: &str, id: &str) -> Value {
        let steam = self.steam_games.lock().unwrap();
        let raw = self.raw_games(&steam).into_iter().find(|g| g.get("id").and_then(Value::as_str) == Some(id));
        drop(steam);
        let Some(g) = raw else { return json!([]) };
        let info = self.make_game_info().lookup(id).unwrap_or_else(|| json!({}));
        let ref_sgdb = g.pointer("/override/sgdbId").and_then(Value::as_i64).or_else(|| info.get("sgdbId").and_then(Value::as_i64));
        let ref_steam = g.pointer("/override/steamAppId").and_then(Value::as_str).map(String::from).or_else(|| info.get("steamAppId").and_then(Value::as_str).map(String::from)).or_else(|| g.get("appid").and_then(Value::as_str).map(String::from));
        let game_info = self.make_game_info();
        let Ok(imgs) = game_info.sgdb_images(kind, ref_sgdb, ref_steam.as_deref()) else { return json!([]) };
        // Thumbnails are remote; download them so the UI (which only shows local files) can display them.
        let mut out = Vec::new();
        for i in imgs.into_iter().take(12) {
            if let Some(thumb) = game_info.download(&i.thumb) {
                out.push(json!({"url": i.url, "thumb": file_url(Path::new(&thumb))}));
            }
        }
        json!(out)
    }

    pub fn set_game_art(&self, id: &str, kind: &str, url: &str) -> Value {
        let game_info = self.make_game_info();
        let Some(p) = game_info.download(url) else { return json!({"ok": false}) };
        let mut art = Map::new();
        art.insert(kind.to_string(), json!(p));
        self.edit_game(id, json!({"art": Value::Object(art)}));
        json!({"ok": true})
    }

    pub fn screenshot(&self, url: &str) -> Value {
        // Only Steam's CDN domains, like the JS version's allowlist.
        if !is_steam_cdn(url) {
            return Value::Null;
        }
        let game_info = self.make_game_info();
        match game_info.download(url) {
            Some(p) => file_url_opt(p.to_str()),
            None => Value::Null,
        }
    }

    pub fn show_game_folder(&self, id: &str) {
        let steam = self.steam_games.lock().unwrap();
        let g = self.raw_games(&steam).into_iter().find(|g| g.get("id").and_then(Value::as_str) == Some(id));
        drop(steam);
        if let Some(g) = g {
            let dir = g.get("installDir").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from).or_else(|| g.get("exe").and_then(Value::as_str).and_then(|e| Path::new(e).parent().map(|p| p.to_string_lossy().into_owned())));
            if let Some(dir) = dir {
                if Path::new(&dir).is_dir() {
                    let _ = open_path_in_shell(Path::new(&dir));
                }
            }
        }
    }
}

fn is_steam_cdn(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let Some((host, _)) = rest.split_once('/') else { return false };
    let host = host.to_lowercase();
    let ok_tld = host.ends_with(".com") || host.ends_with(".net");
    ok_tld && ["steamstatic", "steampowered", "akamaihd"].iter().any(|d| host.contains(d))
}

#[cfg(test)]
mod tests {
    use super::is_steam_cdn;

    #[test]
    fn screenshot_allowlist_is_steam_cdns_only() {
        assert!(is_steam_cdn("https://cdn.akamai.steamstatic.com/a.jpg"));
        assert!(is_steam_cdn("https://shared.fastly.steamstatic.com/x.png"));
        assert!(!is_steam_cdn("http://cdn.steamstatic.com/a.jpg"));
        assert!(!is_steam_cdn("https://example.org/steamstatic.com/a.jpg"));
    }
}
