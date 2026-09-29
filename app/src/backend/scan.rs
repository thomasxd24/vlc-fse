//! Scanning (library + Steam) and background enrichment (TMDB, Steam store, SteamGridDB).

use super::state::{build_state, path_is_file};
use super::Backend;
use lounge_core::store::JsonStore;
use lounge_core::viewmodel::{self, SteamGames};
use lounge_core::{gameinfo, library, metadata, steam};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

pub(super) fn scan_games_blocking(settings: &JsonStore) -> SteamGames {
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

pub(super) fn scan_media_blocking(settings: &JsonStore) -> library::LibraryScan {
    let libs_json = settings.get("libraries").and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let defs: Vec<library::LibraryDef> = libs_json.iter().filter_map(|l| {
        let p = l.get("path").and_then(Value::as_str)?;
        let kind = if l.get("type").and_then(Value::as_str) == Some("tv") { library::LibraryKind::Tv } else { library::LibraryKind::Movies };
        Some(library::LibraryDef { path: Path::new(p), kind })
    }).collect();
    library::scan_libraries(&defs)
}

impl Backend {
    pub fn rescan(&self) -> Value {
        self.do_rescan();
        build_state(self)
    }

    pub(super) fn do_rescan(&self) {
        self.scanning.store(true, Ordering::SeqCst);
        self.push_state();
        // Blocking disk walks; the caller is already off the UI thread.
        let media = scan_media_blocking(&self.settings);
        let steam_games = scan_games_blocking(&self.settings);
        *self.library.lock().unwrap() = media;
        *self.steam_games.lock().unwrap() = steam_games;
        self.scanning.store(false, Ordering::SeqCst);
        self.push_state();
        self.enrich_metadata();
        self.enrich_games();
    }

    /// Refetch synopses/descriptions in the background (TMDB), pushing state as entries arrive.
    pub(super) fn enrich_metadata(&self) {
        let metadata = {
            let m = self.make_metadata();
            if !m.enabled() {
                return;
            }
            let mut status = self.meta_status.lock().unwrap();
            if status.0 {
                return;
            }
            *status = (true, None);
            m
        };
        self.push_state();
        let this = self.clone();
        std::thread::spawn(move || {
            let lib = this.library.lock().unwrap().clone();
            let movies: Vec<metadata::MovieRef> = lib.movies.iter().map(|m| metadata::MovieRef { title: &m.title, year: m.year }).collect();
            let show_eps: Vec<Vec<metadata::EpisodeRef>> = lib.shows.iter().map(|s| {
                s.episodes.iter().map(|e| metadata::EpisodeRef { season: e.season, episode: e.episode.unwrap_or(0), has_thumb: e.thumb.is_some() }).collect()
            }).collect();
            let shows: Vec<metadata::ShowRef> = lib.shows.iter().zip(&show_eps).map(|(s, eps)| metadata::ShowRef { title: &s.title, year: s.year, episodes: eps }).collect();
            let lib_ref = metadata::LibraryRef { movies: &movies, shows: &shows };
            let on_update = || {
                this.push_state();
            };
            let result = metadata.enrich(&lib_ref, on_update);
            let error = match result {
                Err(e) => Some(e.to_string()),
                Ok(_) => None,
            };
            *this.meta_status.lock().unwrap() = (false, error);
            this.push_state();
        });
    }

    /// Refetch game descriptions/art in the background (Steam store + SteamGridDB).
    pub(super) fn enrich_games(&self) {
        {
            let mut status = self.game_info_status.lock().unwrap();
            if status.0 {
                return;
            }
            *status = (true, None);
        }
        // (The status guard must be gone before this: the state payload reads it too.)
        self.push_state();
        let mut game_info = self.make_game_info();
        let this = self.clone();
        std::thread::spawn(move || {
            let raw = {
                let steam = this.steam_games.lock().unwrap();
                this.raw_games(&steam)
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
                this.push_state();
            };
            let result = game_info.enrich(&refs, on_update, || false);
            let error = match result {
                Err(e) => Some(e.to_string()),
                Ok(_) => None,
            };
            *this.game_info_status.lock().unwrap() = (false, error);
            this.push_state();
        });
    }
}
