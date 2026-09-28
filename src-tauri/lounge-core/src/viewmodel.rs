//! The view model: the scanned library merged with progress, preferences, TMDB metadata and cached
//! game info, shaped exactly as the UI's `state.library` expects. Port of the view-model half of
//! `main.js` (its `progressFor`/`resumable`/`prefsOf`/`rawGames`/`buildGames`/`buildViewModel`/
//! `pathsFor`/`setWatched`/`setLanguages`/`recordProgress`/`logSession`/`statsData` functions).
//!
//! `main.js` had no test file of its own, so — like `store.rs` — the tests below are new, covering
//! the behaviour read from that file. Everything here is pure over the [`JsonStore`]s plus the scan
//! results; the `app` crate supplies the stores and a `file_url` helper (the JS version calls
//! `pathToFileURL(p).href` inline) and renders nothing itself.
//!
//! Game entries stay dynamic `serde_json::Value`s rather than typed structs: they flow in from two
//! shapes (the Steam scan and manually added games), get user overrides merged over them as plain
//! objects, and the `app` crate persists them — the JS version does all of that on plain objects,
//! and a struct would re-litigate every optional field for no gain.

use crate::library::{LibraryScan, Movie, Show};
use crate::steam;
use crate::store::JsonStore;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const WATCHED_RATIO: f64 = 0.9;
pub const MIN_RESUME_SECONDS: f64 = 60.0;
/// The play/watch log behind the stats page (~1 MB at most).
pub const MAX_SESSIONS: usize = 20000;

fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The six stores the view model reads (and the mutators below write). The `app` crate owns one
/// instance of each for the process's lifetime.
pub struct Stores<'a> {
    /// `{ items: { [path]: { time, length, watched, updatedAt } } }`
    pub progress: &'a JsonStore,
    /// `{ entries: { [metadata key]: entry } }` — TMDB lookups (see `metadata.rs`).
    pub meta: &'a JsonStore,
    /// `{ manual: [], overrides: {}, stats: {} }` — manual games, per-game overrides and playtime.
    pub games: &'a JsonStore,
    /// `{ games: { [game id]: info } }` — cached Steam/SteamGridDB lookups (see `gameinfo.rs`).
    pub game_info: &'a JsonStore,
    /// `{ favorites: {}, hidden: {}, languages: {} }`
    pub prefs: &'a JsonStore,
    /// `{ sessions: [{ kind, id, start, minutes }] }`
    pub stats: &'a JsonStore,
}

/// The Steam half of the library: the latest scan's games plus where they came from. Kept as JSON
/// entries straight away (via [`steam_game_json`]) so they merge with manual games and overrides on
/// equal terms, exactly as the JS version's plain objects do.
#[derive(Debug, Clone, Default)]
pub struct SteamGames {
    pub steam_path: Option<PathBuf>,
    pub steam_user: Option<String>,
    pub entries: Vec<Value>,
}

impl SteamGames {
    pub fn disabled() -> Self {
        Self::default()
    }
}

/// One Steam scan result as it's carried in the library and persisted, matching `scanSteam`'s object
/// shape in `src/steam.js` (`{ id, source, appid, title, installDir, size, addedAt, lastPlayed,
/// playtime, art }`).
pub fn steam_game_json(g: &steam::Game) -> Value {
    let art = json!({
        "poster": path_string(g.art.poster.as_deref()),
        "hero": path_string(g.art.hero.as_deref()),
        "logo": path_string(g.art.logo.as_deref()),
        "header": path_string(g.art.header.as_deref()),
        "icon": path_string(g.art.icon.as_deref()),
    });
    json!({
        "id": g.id,
        "source": g.source,
        "appid": g.appid,
        "title": g.title,
        "installDir": g.install_dir.to_string_lossy(),
        "size": g.size,
        "addedAt": g.added_at,
        "lastPlayed": g.last_played,
        "playtime": g.playtime,
        "art": art,
    })
}

fn path_string(p: Option<&Path>) -> Value {
    match p {
        Some(p) if !p.as_os_str().is_empty() => json!(p.to_string_lossy()),
        _ => Value::Null,
    }
}

/// Progress keys are file paths; on Windows they're lowercased first so `D:\Film.mkv` and
/// `d:\film.mkv` share an entry, same as `main.js`'s `progressKey`.
pub fn progress_key(p: &str) -> String {
    if cfg!(windows) {
        p.to_lowercase()
    } else {
        p.to_string()
    }
}

fn progress_default() -> Value {
    json!({"time": 0, "length": 0, "watched": false, "updatedAt": 0})
}

pub fn progress_for(stores: &Stores, path: &Path) -> Value {
    let items = stores.progress.get("items").unwrap_or_else(|| json!({}));
    progress_entry(&items, &path.to_string_lossy()).cloned().unwrap_or_else(progress_default)
}

fn progress_entry<'a>(items: &'a Value, path: &str) -> Option<&'a Value> {
    items.get(progress_key(path))
}

pub fn resumable(pr: &Value) -> bool {
    let time = num(pr.get("time"));
    let length = num(pr.get("length"));
    let watched = pr.get("watched").and_then(Value::as_bool).unwrap_or(false);
    !watched && time >= MIN_RESUME_SECONDS && (length == 0.0 || time < length * WATCHED_RATIO)
}

/// `null`/absent numbers read as 0, matching how the JS version's arithmetic treats `undefined`.
fn num(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(0.0)
}

fn prefs_of(stores: &Stores, id: &str) -> (bool, bool) {
    let favorite = stores.prefs.get("favorites").and_then(|m| m.get(id).and_then(Value::as_bool)).unwrap_or(false);
    let hidden = stores.prefs.get("hidden").and_then(|m| m.get(id).and_then(Value::as_bool)).unwrap_or(false);
    (favorite, hidden)
}

/// Audio/subtitle languages chosen for one film or show, overriding the defaults in Settings (`None` if none).
fn languages_of(stores: &Stores, id: &str) -> Value {
    stores.prefs.get("languages").and_then(|m| m.get(id).cloned()).unwrap_or(Value::Null)
}

/// Same "skip cached misses" rule as `GameInfo::lookup`, which the JS version calls from `buildGames`.
fn game_info_of(stores: &Stores, id: &str) -> Option<Value> {
    let hit = stores.game_info.get("games").and_then(|m| m.get(id).cloned())?;
    if hit.get("miss").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    Some(hit)
}

/// Raw games (Steam scan + manual), with overrides applied, as fed to both the view model and GameInfo.
pub fn raw_games(steam: &SteamGames, stores: &Stores) -> Vec<Value> {
    let overrides = stores.games.get("overrides").unwrap_or_else(|| json!({}));
    let manual = stores.games.get("manual").and_then(|v| v.as_array().cloned()).unwrap_or_default();
    steam.entries.iter().chain(manual.iter()).map(|g| {
        let mut merged = g.clone();
        if let (Value::Object(m), Some(id)) = (&mut merged, g.get("id").and_then(Value::as_str)) {
            let o = overrides.get(id).cloned().unwrap_or_else(|| json!({}));
            if let Some(t) = o.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                m.insert("title".into(), json!(t));
            }
            if let Some(Value::Object(oa)) = o.get("art") {
                let mut art = match m.get("art") {
                    Some(Value::Object(a)) => a.clone(),
                    _ => Map::new(),
                };
                for (k, v) in oa {
                    art.insert(k.clone(), v.clone());
                }
                m.insert("art".into(), Value::Object(art));
            }
            m.insert("override".into(), o);
        }
        merged
    }).collect()
}

fn url_of(file_url: &dyn Fn(&Path) -> String, v: Option<&Value>) -> Value {
    match v.and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(s) => json!(file_url(Path::new(s))),
        None => Value::Null,
    }
}

/// First non-empty of two candidate values, then through `file_url` — `fileUrl(m.poster || meta.poster)`.
fn url_pick(file_url: &dyn Fn(&Path) -> String, a: Option<&Path>, b: Option<&Value>) -> Value {
    if let Some(p) = a {
        if !p.as_os_str().is_empty() {
            return json!(file_url(p));
        }
    }
    url_of(file_url, b)
}

fn progress_json(pr: &Value) -> Value {
    let mut o = match pr {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    for (k, v) in [
        ("time", json!(num(o.get("time")))),
        ("length", json!(num(o.get("length")))),
        ("watched", json!(o.get("watched").and_then(Value::as_bool).unwrap_or(false))),
        ("updatedAt", json!(num(o.get("updatedAt")))),
        ("resumable", json!(resumable(pr))),
    ] {
        o.insert(k.into(), v);
    }
    Value::Object(o)
}

fn build_games(steam: &SteamGames, stores: &Stores, file_url: &dyn Fn(&Path) -> String) -> Vec<Value> {
    let stats = stores.games.get("stats").unwrap_or_else(|| json!({}));
    raw_games(steam, stores).into_iter().map(|g| {
        let id = g.get("id").and_then(Value::as_str).unwrap_or("");
        let info = game_info_of(stores, id).unwrap_or_else(|| json!({}));
        let fetched = info.get("art").cloned().unwrap_or_else(|| json!({}));
        let st = stats.get(id).cloned().unwrap_or_else(|| json!({}));
        let art = |k: &str| {
            let own = g.get("art").and_then(|a| a.get(k)).cloned().filter(|v| !v.is_null());
            let v = own.or_else(|| fetched.get(k).cloned().filter(|v| !v.is_null()));
            url_of(file_url, v.as_ref())
        };
        let playtime_raw = num(g.get("playtime"));
        let last_played_raw = num(g.get("lastPlayed"));
        let st_playtime = num(st.get("playtime"));
        let st_last_played = num(st.get("lastPlayed"));
        let playtime = if g.get("source").and_then(Value::as_str) == Some("steam") {
            playtime_raw.max(st_playtime)
        } else {
            st_playtime
        };
        let install_dir = g.get("installDir").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from).or_else(|| {
            g.get("exe").and_then(Value::as_str).filter(|s| !s.is_empty()).map(win_dirname)
        });
        let (favorite, hidden) = prefs_of(stores, id);
        json!({
            "id": id,
            "type": "game",
            "source": g.get("source").and_then(Value::as_str).unwrap_or(""),
            "appid": g.get("appid").cloned().unwrap_or(Value::Null),
            "title": g.get("title").cloned().unwrap_or(Value::Null),
            "poster": art("poster"),
            "hero": if art("hero").is_null() { art("header") } else { art("hero") },
            "logo": art("logo"),
            "header": art("header"),
            "icon": art("icon"),
            "overview": str_or_empty(info.get("overview")),
            "about": str_or_empty(info.get("about")),
            "genres": arr_or_empty(info.get("genres")),
            "developers": arr_or_empty(info.get("developers")),
            "publishers": arr_or_empty(info.get("publishers")),
            "releaseDate": info.get("releaseDate").cloned().filter(|v| !v.is_null()).unwrap_or(Value::Null),
            "metacritic": info.get("metacritic").cloned().filter(|v| !v.is_null()).unwrap_or(Value::Null),
            "controller": info.get("controller").cloned().filter(|v| !v.is_null()).unwrap_or(Value::Null),
            "screenshots": info.get("screenshots").and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().map(|s| json!({
                "thumb": url_of(file_url, s.get("thumb")),
                "full": s.get("full").cloned().unwrap_or(Value::Null),
            })).collect::<Vec<_>>(),
            "steamAppId": first_present(&[info.get("steamAppId"), g.get("appid")]),
            "playtime": playtime,
            "lastPlayed": last_played_raw.max(st_last_played),
            "addedAt": num(g.get("addedAt")),
            "exe": g.get("exe").cloned().unwrap_or(Value::Null),
            "args": str_or_empty(g.get("args")),
            "installDir": install_dir.map(|d| json!(d)).unwrap_or(Value::Null),
            "override": g.get("override").cloned().unwrap_or_else(|| json!({})),
            "favorite": favorite,
            "hidden": hidden,
        })
    }).collect()
}

fn str_or_empty(v: Option<&Value>) -> Value {
    v.and_then(Value::as_str).map(|s| json!(s)).unwrap_or_else(|| json!(""))
}

fn arr_or_empty(v: Option<&Value>) -> Value {
    v.and_then(Value::as_array).cloned().map(Value::Array).unwrap_or_else(|| json!([]))
}

/// `a || b || null` for possibly-absent JSON values.
fn first_present(candidates: &[Option<&Value>]) -> Value {
    for c in candidates.iter().flatten() {
        let c: &Value = c;
        let truthy = match c {
            Value::Null => false,
            Value::String(s) => !s.is_empty(),
            Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
            Value::Bool(b) => *b,
            _ => true,
        };
        if truthy {
            return c.clone();
        }
    }
    Value::Null
}

/// `path.dirname` of a Windows-style path, used for manual games' install folders; `std::path::Path`
/// doesn't understand `\` as a separator unless compiled for Windows (same helper as `tailscale.rs`).
fn win_dirname(p: &str) -> String {
    match p.rfind(['\\', '/']) {
        Some(i) => p[..i].to_string(),
        None => ".".to_string(),
    }
}

fn movie_json(m: &Movie, stores: &Stores, file_url: &dyn Fn(&Path) -> String, meta_entries: &Value) -> Value {
    let key = crate::metadata::movie_key(&m.title, m.year);
    let meta = meta_entries.get(&key).cloned().unwrap_or_else(|| json!({}));
    let pr = progress_for(stores, &m.path);
    let runtime = runtime_of(meta.get("runtime"), &pr);
    let (favorite, hidden) = prefs_of(stores, &m.id);
    json!({
        "id": m.id,
        "type": "movie",
        "title": m.title,
        "year": m.year,
        "overview": str_or_empty(meta.get("overview")),
        "tagline": str_or_empty(meta.get("tagline")),
        "rating": meta.get("rating").cloned().filter(|v| !v.is_null()).unwrap_or(Value::Null),
        "runtime": runtime,
        "genres": arr_or_empty(meta.get("genres")),
        "poster": url_pick(file_url, m.poster.as_deref(), meta.get("poster")),
        "backdrop": url_pick(file_url, m.backdrop.as_deref(), meta.get("backdrop")),
        "path": m.path.to_string_lossy(),
        "addedAt": m.added_at,
        "progress": progress_json(&pr),
        "languages": languages_of(stores, &m.id),
        "favorite": favorite,
        "hidden": hidden,
    })
}

/// `meta.runtime ?? (pr.length ? Math.round(pr.length / 60) : null)`
fn runtime_of(meta_runtime: Option<&Value>, pr: &Value) -> Value {
    match meta_runtime {
        Some(v) if !v.is_null() => v.clone(),
        _ => {
            let length = num(pr.get("length"));
            if length > 0.0 {
                json!((length / 60.0).round() as i64)
            } else {
                Value::Null
            }
        }
    }
}

fn show_json(s: &Show, stores: &Stores, file_url: &dyn Fn(&Path) -> String, meta_entries: &Value) -> Value {
    let key = crate::metadata::show_key(&s.title, s.year);
    let meta = meta_entries.get(&key).cloned().unwrap_or_else(|| json!({}));
    let ep_meta = meta.get("episodes").cloned().unwrap_or_else(|| json!({}));

    let mut last_watched_updated_at: Option<i64> = None;
    let mut last_watched_id: Option<String> = None;
    let episodes: Vec<Value> = s.episodes.iter().map(|e| {
        let em = ep_meta.get(format!("{}x{}", e.season, e.episode.unwrap_or(0))).cloned().unwrap_or_else(|| json!({}));
        let pr = progress_for(stores, &e.path);
        let updated_at = num(pr.get("updatedAt"));
        if updated_at > 0.0 && last_watched_updated_at.map(|l| updated_at > l as f64).unwrap_or(true) {
            last_watched_updated_at = Some(updated_at as i64);
            last_watched_id = Some(e.id.clone());
        }
        let title = match em.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            Some(t) => json!(t),
            None => match e.title.as_deref().filter(|t| !t.is_empty()) {
                Some(t) => json!(t),
                None => Value::Null,
            },
        };
        json!({
            "id": e.id,
            "season": e.season,
            "episode": e.episode.unwrap_or(0),
            "episodeEnd": e.episode_end,
            "title": title,
            "overview": str_or_empty(em.get("overview")),
            "airDate": em.get("airDate").cloned().filter(|v| !v.is_null()).unwrap_or(Value::Null),
            "runtime": runtime_of(em.get("runtime"), &pr),
            "thumb": url_pick(file_url, e.thumb.as_deref(), em.get("still")),
            "path": e.path.to_string_lossy(),
            "addedAt": e.added_at,
            "progress": progress_json(&pr),
        })
    }).collect();

    // "Next up": resume the most recently touched episode, or the one after it if it was finished,
    // otherwise the first unwatched episode.
    let watched = |e: &Value| e.pointer("/progress/watched").and_then(Value::as_bool).unwrap_or(false);
    let mut next_up: Option<&Value> = None;
    if let Some(id) = &last_watched_id {
        if let Some(i) = episodes.iter().position(|e| e.get("id").and_then(Value::as_str) == Some(id)) {
            let ep = &episodes[i];
            if !watched(ep) {
                next_up = Some(ep);
            } else {
                next_up = episodes[i + 1..].iter().find(|e| !watched(e));
            }
        }
    }
    if next_up.is_none() && last_watched_id.is_none() {
        next_up = episodes.iter().find(|e| !watched(e));
    }

    let year = match s.year {
        Some(y) if y != 0 => json!(y),
        _ => meta.get("firstAirDate").and_then(Value::as_str).and_then(|d| d.get(0..4)).and_then(|y| y.parse::<i32>().ok()).map(|y| json!(y)).unwrap_or(Value::Null),
    };
    let mut seasons: Vec<i64> = episodes.iter().filter_map(|e| e.get("season").and_then(Value::as_i64)).collect();
    seasons.sort_unstable();
    seasons.dedup();
    let watched_count = episodes.iter().filter(|e| watched(e)).count();
    let (favorite, hidden) = prefs_of(stores, &s.id);
    json!({
        "id": s.id,
        "type": "show",
        "title": s.title,
        "year": year,
        "overview": str_or_empty(meta.get("overview")),
        "rating": meta.get("rating").cloned().filter(|v| !v.is_null()).unwrap_or(Value::Null),
        "genres": arr_or_empty(meta.get("genres")),
        "status": meta.get("status").cloned().filter(|v| !v.is_null()).unwrap_or(Value::Null),
        "poster": url_pick(file_url, s.poster.as_deref(), meta.get("poster")),
        "backdrop": url_pick(file_url, s.backdrop.as_deref(), meta.get("backdrop")),
        "addedAt": s.added_at,
        "episodes": episodes,
        "seasons": seasons,
        "watchedCount": watched_count,
        "nextUp": next_up.and_then(|e| e.get("id").cloned()).unwrap_or(Value::Null),
        "lastActivity": last_watched_updated_at.unwrap_or(0),
        "languages": languages_of(stores, &s.id),
        "favorite": favorite,
        "hidden": hidden,
    })
}

/// The whole `state.library` payload: movies, shows, games, the Continue watching row and the Steam
/// provenance, all with progress/prefs/metadata merged in.
pub fn build_view_model(lib: &LibraryScan, steam: &SteamGames, stores: &Stores, file_url: &dyn Fn(&Path) -> String) -> Value {
    let meta_entries = stores.meta.get("entries").unwrap_or_else(|| json!({}));
    let movies: Vec<Value> = lib.movies.iter().map(|m| movie_json(m, stores, file_url, &meta_entries)).collect();
    let shows: Vec<Value> = lib.shows.iter().map(|s| show_json(s, stores, file_url, &meta_entries)).collect();
    let games = build_games(steam, stores, file_url);

    // Continue watching: part-watched movies, plus shows with activity and something left to watch.
    let mut continue_watching: Vec<(i64, Value)> = Vec::new();
    for m in &movies {
        let hidden = m.get("hidden").and_then(Value::as_bool).unwrap_or(false);
        let resumable = m.pointer("/progress/resumable").and_then(Value::as_bool).unwrap_or(false);
        if resumable && !hidden {
            continue_watching.push((num(m.pointer("/progress/updatedAt")) as i64, json!({
                "kind": "movie", "id": m.get("id"), "at": m.pointer("/progress/updatedAt"),
            })));
        }
    }
    for s in &shows {
        let hidden = s.get("hidden").and_then(Value::as_bool).unwrap_or(false);
        let last_activity = num(s.get("lastActivity"));
        let has_next_up = !s.get("nextUp").map(Value::is_null).unwrap_or(true);
        if last_activity > 0.0 && has_next_up && !hidden {
            continue_watching.push((last_activity as i64, json!({
                "kind": "episode", "showId": s.get("id"), "id": s.get("nextUp"), "at": s.get("lastActivity"),
            })));
        }
    }
    continue_watching.sort_by_key(|e| std::cmp::Reverse(e.0));
    let continue_watching: Vec<Value> = continue_watching.into_iter().take(20).map(|(_, v)| v).collect();

    json!({
        "movies": movies,
        "shows": shows,
        "games": games,
        "continueWatching": continue_watching,
        "scannedAt": lib.scanned_at,
        "steamFound": steam.steam_path.is_some(),
        "steamUser": steam.steam_user.clone().map(|u| json!(u)).unwrap_or(Value::Null),
    })
}

/// The paths a *mark watched* request covers: one movie, one episode, a whole season or a whole show.
/// A season's `id` arrives as a JSON number from the UI; the others are strings.
pub fn paths_for(lib: &LibraryScan, req: &Value) -> Vec<String> {
    let kind = req.get("kind").and_then(Value::as_str).unwrap_or("");
    let id = req.get("id");
    let show_id = req.get("showId").and_then(Value::as_str).unwrap_or("");
    let find_show = |sid: &str| lib.shows.iter().find(|s| s.id == sid);
    match kind {
        "movie" => lib.movies.iter().filter(|m| Some(m.id.as_str()) == id.and_then(Value::as_str)).map(|m| m.path.to_string_lossy().into_owned()).collect(),
        "episode" => find_show(show_id).map(|s| s.episodes.iter().filter(|e| Some(e.id.as_str()) == id.and_then(Value::as_str)).map(|e| e.path.to_string_lossy().into_owned()).collect()).unwrap_or_default(),
        "season" => find_show(show_id).map(|s| s.episodes.iter().filter(|e| id.and_then(Value::as_i64) == Some(e.season as i64)).map(|e| e.path.to_string_lossy().into_owned()).collect()).unwrap_or_default(),
        "show" => find_show(id.and_then(Value::as_str).unwrap_or("")).map(|s| s.episodes.iter().map(|e| e.path.to_string_lossy().into_owned()).collect()).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// What *play* records as VLC reports position changes: watched sticks once passed, and a watched
/// item's time resets so it doesn't show a stale resume point.
pub fn record_progress(stores: &Stores, path: &Path, time: f64, length: f64) {
    let key = progress_key(&path.to_string_lossy());
    stores.progress.update("items", json!({}), |items| {
        let Value::Object(m) = items else { return };
        let prev_watched = m.get(&key).and_then(|e| e.get("watched")).and_then(Value::as_bool).unwrap_or(false);
        let watched = prev_watched || (length > 0.0 && time >= length * WATCHED_RATIO);
        m.insert(key, json!({
            "time": if watched { 0.0 } else { time },
            "length": length,
            "watched": watched,
            "updatedAt": now_millis(),
        }));
    });
}

/// Marking watched explicitly: one entry per path, time reset, previous `length` kept.
pub fn set_watched(stores: &Stores, paths: &[String], watched: bool) {
    stores.progress.update("items", json!({}), |items| {
        let Value::Object(m) = items else { return };
        for p in paths {
            let key = progress_key(p);
            let mut entry = match m.get(&key) {
                Some(e) => e.clone(),
                None => json!({"length": 0}),
            };
            if let Value::Object(o) = &mut entry {
                o.insert("time".into(), json!(0));
                o.insert("watched".into(), json!(watched));
                o.insert("updatedAt".into(), json!(now_millis()));
            }
            m.insert(key, entry);
        }
    });
}

/// Per-item audio/subtitle language override; dropping both languages removes the override.
pub fn set_languages(stores: &Stores, id: &str, languages: Option<&Value>) {
    let mut clean = Map::new();
    if let Some(l) = languages {
        if let Some(audio) = l.get("audio").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            clean.insert("audio".into(), json!(audio));
        }
        if let Some(subs) = l.get("subs").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            clean.insert("subs".into(), json!(subs));
        }
    }
    stores.prefs.update("languages", json!({}), |map| {
        let Value::Object(m) = map else { return };
        if clean.is_empty() {
            m.remove(id);
        } else {
            m.insert(id.to_string(), Value::Object(clean.clone()));
        }
    });
}

/// Favourite / hidden flags. Other keys are ignored, same as `main.js`'s `set-pref` guard.
pub fn set_pref(stores: &Stores, id: &str, key: &str, value: bool) {
    if key != "favorites" && key != "hidden" {
        return;
    }
    stores.prefs.update(key, json!({}), |map| {
        let Value::Object(m) = map else { return };
        if value {
            m.insert(id.to_string(), json!(true));
        } else {
            m.remove(id);
        }
    });
}

/// One entry in the log behind the stats page: a game played or a film/show watched, from `start` (ms).
pub fn log_session(stores: &Stores, kind: &str, id: &str, start: i64, minutes: f64) {
    let m = minutes.round() as i64;
    if m < 1 {
        return;
    }
    stores.stats.update("sessions", json!([]), |list| {
        let Value::Array(list) = list else { return };
        list.push(json!({"kind": kind, "id": id, "start": start, "minutes": m}));
        if list.len() > MAX_SESSIONS {
            let excess = list.len() - MAX_SESSIONS;
            list.drain(0..excess);
        }
    });
}

/// Everything the stats page needs: the session log, plus names and art for whatever it mentions.
pub fn stats_data(lib: &LibraryScan, steam: &SteamGames, stores: &Stores, file_url: &dyn Fn(&Path) -> String) -> Value {
    let sessions = stores.stats.get("sessions").and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let ids: HashSet<String> = sessions.iter().filter_map(|x| x.get("id").and_then(Value::as_str)).map(String::from).collect();
    let vm = build_view_model(lib, steam, stores, file_url);
    let mut items = Map::new();
    for g in vm.get("games").and_then(Value::as_array).into_iter().flatten() {
        let id = g.get("id").and_then(Value::as_str).unwrap_or("");
        let playtime = num(g.get("playtime"));
        if ids.contains(id) || playtime != 0.0 {
            items.insert(id.to_string(), json!({
                "type": "game", "title": g.get("title"), "poster": g.get("poster"), "icon": g.get("icon"), "playtime": g.get("playtime"),
            }));
        }
    }
    for (key, kind) in [("movies", "movie"), ("shows", "show")] {
        for x in vm.get(key).and_then(Value::as_array).into_iter().flatten() {
            let id = x.get("id").and_then(Value::as_str).unwrap_or("");
            if ids.contains(id) {
                items.insert(id.to_string(), json!({
                    "type": kind, "title": x.get("title"), "poster": x.get("poster"),
                }));
            }
        }
    }
    json!({"sessions": sessions, "items": items, "now": now_millis()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn url(p: &Path) -> String {
        format!("url:{}", p.to_string_lossy())
    }

    fn store(dir: &Path, name: &str, defaults: Value) -> JsonStore {
        JsonStore::new(dir.join(name), defaults.as_object().cloned().unwrap_or_default())
    }

    /// The six stores with the exact defaults `main.js` gives each one.
    fn mk_stores(dir: &Path) -> [JsonStore; 6] {
        [
            store(dir, "progress.json", json!({"items": {}})),
            store(dir, "metadata.json", json!({"entries": {}})),
            store(dir, "games.json", json!({"manual": [], "overrides": {}, "stats": {}})),
            store(dir, "gameinfo.json", json!({"games": {}})),
            store(dir, "prefs.json", json!({"favorites": {}, "hidden": {}, "languages": {}})),
            store(dir, "stats.json", json!({"sessions": []})),
        ]
    }

    fn stores_ref(s: &[JsonStore; 6]) -> Stores<'_> {
        Stores { progress: &s[0], meta: &s[1], games: &s[2], game_info: &s[3], prefs: &s[4], stats: &s[5] }
    }

    fn scan(movies: Vec<Movie>, shows: Vec<Show>) -> LibraryScan {
        LibraryScan { movies, shows, scanned_at: 1000 }
    }

    fn movie(id: &str, title: &str, path: &str) -> Movie {
        Movie {
            id: id.into(),
            title: title.into(),
            year: Some(2001),
            path: PathBuf::from(path),
            dir: PathBuf::from("/lib"),
            poster: None,
            backdrop: None,
            size: 0,
            added_at: 5,
        }
    }

    fn episode(id: &str, n: i32, path: String) -> crate::library::Episode {
        crate::library::Episode {
            id: id.into(),
            season: 1,
            episode: Some(n),
            episode_end: None,
            title: None,
            path: PathBuf::from(path),
            thumb: None,
            size: 0,
            added_at: 5,
        }
    }

    fn manual_game(id: &str, title: &str, exe: &str) -> Value {
        json!({"id": id, "source": "manual", "title": title, "exe": exe, "args": "", "cwd": "", "addedAt": 9, "art": {"icon": "/art/icon.png"}})
    }

    fn steam_fixture() -> SteamGames {
        SteamGames {
            steam_path: Some(PathBuf::from("/steam")),
            steam_user: Some("tom".into()),
            entries: vec![json!({
                "id": "steam-440", "source": "steam", "appid": "440", "title": "TF2",
                "installDir": "/steam/common/TF2", "size": 1, "addedAt": 7, "lastPlayed": 111, "playtime": 60,
                "art": {"poster": "/steam/p.jpg", "hero": null, "logo": null, "header": "/steam/h.jpg", "icon": null}
            })],
        }
    }

    #[test]
    fn resumable_matches_the_js_thresholds() {
        let pr = json!({"time": 30, "length": 0, "watched": false, "updatedAt": 1});
        assert!(!resumable(&pr), "under the 60s minimum");
        let pr = json!({"time": 100, "length": 0, "watched": false, "updatedAt": 1});
        assert!(resumable(&pr), "no known length: resumable from 60s on");
        let pr = json!({"time": 905, "length": 1000, "watched": false, "updatedAt": 1});
        assert!(!resumable(&pr), "past 90% of the length");
        let pr = json!({"time": 895, "length": 1000, "watched": false, "updatedAt": 1});
        assert!(resumable(&pr));
        let pr = json!({"time": 100, "length": 0, "watched": true, "updatedAt": 1});
        assert!(!resumable(&pr), "already watched");
        let pr = json!({});
        assert!(!resumable(&pr), "an absent entry has no progress at all");
    }

    #[test]
    fn set_watched_marks_whole_paths_and_the_view_model_reflects_it() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        let lib = scan(vec![movie("m1", "Film", "/lib/Film (2001)/Film.mkv")], vec![]);
        let req = json!({"kind": "movie", "id": "m1"});
        let paths = paths_for(&lib, &req);
        assert_eq!(paths, vec!["/lib/Film (2001)/Film.mkv"]);

        set_watched(st, &paths, true);
        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        let m = &vm["movies"][0];
        assert_eq!(m["progress"]["watched"], json!(true));
        assert_eq!(m["progress"]["time"], json!(0.0));
        assert_eq!(m["progress"]["resumable"], json!(false));

        set_watched(st, &paths, false);
        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        assert_eq!(vm["movies"][0]["progress"]["watched"], json!(false));
    }

    #[test]
    fn set_watched_keeps_the_known_length_when_unmarking() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        record_progress(st, Path::new("/lib/Film.mkv"), 300.0, 1000.0);

        set_watched(st, &["/lib/Film.mkv".into()], true);
        set_watched(st, &["/lib/Film.mkv".into()], false);

        let items = st.progress.get("items").unwrap();
        let entry = &items["/lib/Film.mkv"];
        assert_eq!(entry["length"], json!(1000.0), "the length from playback survives marking");
        assert_eq!(entry["watched"], json!(false));
    }

    #[test]
    fn record_progress_becomes_watched_at_ninety_percent_and_resets_time() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        record_progress(st, Path::new("/lib/Film.mkv"), 899.0, 1000.0);
        let items = st.progress.get("items").unwrap();
        assert_eq!(items["/lib/Film.mkv"]["watched"], json!(false));
        assert_eq!(items["/lib/Film.mkv"]["time"], json!(899.0));

        record_progress(st, Path::new("/lib/Film.mkv"), 910.0, 1000.0);
        let items = st.progress.get("items").unwrap();
        assert_eq!(items["/lib/Film.mkv"]["watched"], json!(true));
        assert_eq!(items["/lib/Film.mkv"]["time"], json!(0.0), "a watched item's time resets");
    }

    #[test]
    fn shows_get_watched_counts_next_up_and_last_activity() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        let show = Show {
            id: "s1".into(),
            title: "Show".into(),
            year: Some(2020),
            dir: PathBuf::from("/lib/Show"),
            poster: None,
            backdrop: None,
            added_at: 5,
            episodes: vec![episode("e1", 1, "/lib/Show/s01e01.mkv".into()), episode("e2", 2, "/lib/Show/s01e02.mkv".into())],
        };
        let lib = scan(vec![], vec![show]);

        // Watch episode 1 fully.
        set_watched(st, &["/lib/Show/s01e01.mkv".into()], true);
        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        let s = &vm["shows"][0];
        assert_eq!(s["watchedCount"], json!(1));
        assert_eq!(s["nextUp"], json!("e2"), "finished episode: next up is the one after");
        assert!(s["lastActivity"].as_i64().unwrap() > 0);

        // Only part-way through episode 1: resume it instead.
        set_watched(st, &["/lib/Show/s01e01.mkv".into()], false);
        record_progress(st, Path::new("/lib/Show/s01e01.mkv"), 500.0, 2000.0);
        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        assert_eq!(vm["shows"][0]["nextUp"], json!("e1"));

        // Nothing touched yet: first unwatched episode.
        let fresh = mk_stores(tempdir().unwrap().path());
        let vm = build_view_model(&lib, &SteamGames::disabled(), &stores_ref(&fresh), &url);
        assert_eq!(vm["shows"][0]["nextUp"], json!("e1"));
        assert_eq!(vm["shows"][0]["lastActivity"], json!(0));
    }

    #[test]
    fn continue_watching_lists_resumable_movies_newest_first_capped_at_20() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        let movies: Vec<Movie> = (0..25).map(|i| movie(&format!("m{i}"), &format!("F{i}"), &format!("/lib/f{i}.mkv"))).collect();
        let lib = scan(movies, vec![]);
        for i in 0..25 {
            record_progress(st, Path::new(&format!("/lib/f{i}.mkv")), 500.0, 1000.0);
        }
        // Make the *later* entries the most recent, so recency ordering is observable.
        let mut items = st.progress.get("items").unwrap();
        if let Value::Object(m) = &mut items {
            for (i, k) in (0..25).enumerate() {
                let key = format!("/lib/f{k}.mkv");
                if let Some(e) = m.get_mut(&key).and_then(|v| v.as_object_mut()) {
                    e.insert("updatedAt".into(), json!(1_000_000 + i as i64));
                }
            }
        }
        st.progress.set("items", items);

        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        let cw = vm["continueWatching"].as_array().unwrap();
        assert_eq!(cw.len(), 20, "capped at 20");
        assert_eq!(cw[0]["id"], json!("m24"), "most recent first");
        assert_eq!(cw[0]["kind"], json!("movie"));
        assert_eq!(cw[19]["id"], json!("m5"));
    }

    #[test]
    fn prefs_and_languages_flow_into_the_view_model_and_back_out() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        let lib = scan(vec![movie("m1", "Film", "/lib/Film.mkv")], vec![]);

        set_pref(st, "m1", "favorites", true);
        set_pref(st, "m1", "nope", true); // not a real pref key: ignored
        set_languages(st, "m1", Some(&json!({"audio": "ja", "subs": "en", "junk": 1})));
        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        assert_eq!(vm["movies"][0]["favorite"], json!(true));
        assert_eq!(vm["movies"][0]["hidden"], json!(false));
        assert_eq!(vm["movies"][0]["languages"], json!({"audio": "ja", "subs": "en"}), "unknown fields dropped");

        set_pref(st, "m1", "favorites", false);
        set_languages(st, "m1", Some(&json!({})));
        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        assert_eq!(vm["movies"][0]["favorite"], json!(false));
        assert_eq!(vm["movies"][0]["languages"], json!(null), "an empty override is removed");
    }

    #[test]
    fn hidden_items_leave_continue_watching() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        let lib = scan(vec![movie("m1", "Film", "/lib/Film.mkv")], vec![]);
        record_progress(st, Path::new("/lib/Film.mkv"), 500.0, 1000.0);
        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        assert_eq!(vm["continueWatching"].as_array().unwrap().len(), 1);

        set_pref(st, "m1", "hidden", true);
        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        assert_eq!(vm["continueWatching"].as_array().unwrap().len(), 0, "hidden movies don't resume");
    }

    #[test]
    fn raw_games_applies_title_and_art_overrides_without_touching_the_rest() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        stores[2].set("manual", json!([manual_game("manual-c:/game/exe", "My Game", "C:\\Game\\game.exe")]));
        stores[2].set("overrides", json!({"manual-c:/game/exe": {"title": "Renamed", "art": {"poster": "/art/custom.jpg"}}}));

        let raw = raw_games(&SteamGames::disabled(), st);
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0]["title"], json!("Renamed"));
        assert_eq!(raw[0]["art"]["icon"], json!("/art/icon.png"), "base art kept");
        assert_eq!(raw[0]["art"]["poster"], json!("/art/custom.jpg"), "override art merged over");
        assert_eq!(raw[0]["override"]["title"], json!("Renamed"));
    }

    #[test]
    fn build_games_merges_cached_info_playtime_and_prefs() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        stores[3].set("games", json!({
            "steam-440": {
                "overview": "Hats.", "about": "More hats.", "genres": ["Action"], "developers": ["Valve"],
                "publishers": ["Valve"], "releaseDate": "Oct 10, 2007", "metacritic": 92, "controller": "full",
                "steamAppId": "440",
                "art": {"hero": "/cache/hero.jpg", "logo": "/cache/logo.jpg"},
                "screenshots": [{"thumb": "/cache/s1.jpg", "full": "https://cdn/s1_full.jpg"}],
            }
        }));
        stores[2].set("stats", json!({"steam-440": {"playtime": 30, "lastPlayed": 222}}));
        stores[2].set("overrides", json!({"steam-440": {"title": "Team Fortress 2"}}));

        let vm = build_view_model(&scan(vec![], vec![]), &steam_fixture(), st, &url);
        let game = &vm["games"][0];
        assert_eq!(game["id"], json!("steam-440"));
        assert_eq!(game["type"], json!("game"));
        assert_eq!(game["title"], json!("Team Fortress 2"), "override wins");
        assert_eq!(game["poster"], json!("url:/steam/p.jpg"), "own art before cached art");
        assert_eq!(game["hero"], json!("url:/cache/hero.jpg"), "cached hero fills the gap");
        assert_eq!(game["logo"], json!("url:/cache/logo.jpg"));
        assert_eq!(game["header"], json!("url:/steam/h.jpg"));
        assert_eq!(game["overview"], json!("Hats."));
        assert_eq!(game["metacritic"], json!(92));
        assert_eq!(game["screenshots"][0]["thumb"], json!("url:/cache/s1.jpg"));
        assert_eq!(game["screenshots"][0]["full"], json!("https://cdn/s1_full.jpg"), "screenshots stay remote");
        assert_eq!(game["steamAppId"], json!("440"));
        assert_eq!(game["playtime"], json!(60.0), "steam games: max(scan, tracked here)");
        assert_eq!(game["lastPlayed"], json!(222.0));
        assert_eq!(game["installDir"], json!("/steam/common/TF2"));
        assert_eq!(vm["steamFound"], json!(true));
        assert_eq!(vm["steamUser"], json!("tom"));
    }

    #[test]
    fn manual_games_derive_their_install_dir_from_the_exe_and_count_only_tracked_playtime() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        stores[2].set("manual", json!([manual_game("manual-c:/game/exe", "My Game", "C:\\Game\\game.exe")]));
        stores[2].set("stats", json!({"manual-c:/game/exe": {"playtime": 45, "lastPlayed": 333}}));

        let vm = build_view_model(&scan(vec![], vec![]), &SteamGames::disabled(), st, &url);
        let game = &vm["games"][0];
        assert_eq!(game["source"], json!("manual"));
        assert_eq!(game["installDir"], json!("C:\\Game"), "dirname of the exe");
        assert_eq!(game["icon"], json!("url:/art/icon.png"));
        assert_eq!(game["playtime"], json!(45.0), "manual games: tracked playtime only");
        assert_eq!(game["steamAppId"], json!(null));
        assert_eq!(vm["steamFound"], json!(false));
        assert_eq!(vm["steamUser"], json!(null));
    }

    #[test]
    fn a_cached_miss_is_treated_as_no_info() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        stores[3].set("games", json!({"steam-440": {"miss": true, "at": 1}}));
        let vm = build_view_model(&scan(vec![], vec![]), &steam_fixture(), st, &url);
        assert_eq!(vm["games"][0]["overview"], json!(""));
        assert_eq!(vm["games"][0]["metacritic"], json!(null));
    }

    #[test]
    fn metadata_entries_fill_movies_including_fallbacks() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        let lib = scan(vec![movie("m1", "Film", "/lib/Film.mkv")], vec![]);
        stores[1].set("entries", json!({
            "movie:film:2001": {
                "overview": "A film.", "tagline": "Tag.", "rating": 7.5, "runtime": 96,
                "genres": ["Drama"], "poster": "/cache/poster.jpg", "backdrop": "/cache/backdrop.jpg",
            }
        }));
        let vm = build_view_model(&lib, &SteamGames::disabled(), st, &url);
        let mv = &vm["movies"][0];
        assert_eq!(mv["overview"], json!("A film."));
        assert_eq!(mv["tagline"], json!("Tag."));
        assert_eq!(mv["rating"], json!(7.5));
        assert_eq!(mv["runtime"], json!(96), "metadata runtime wins");
        assert_eq!(mv["poster"], json!("url:/cache/poster.jpg"));

        // No metadata, but a known file length: runtime falls back to length/60.
        let fresh = mk_stores(tempdir().unwrap().path());
        let st2 = &stores_ref(&fresh);
        record_progress(st2, Path::new("/lib/Film.mkv"), 100.0, 4000.0);
        let vm = build_view_model(&lib, &SteamGames::disabled(), st2, &url);
        assert_eq!(vm["movies"][0]["runtime"], json!(67), "4000s / 60, rounded");
        assert_eq!(vm["movies"][0]["overview"], json!(""));
        assert_eq!(vm["movies"][0]["poster"], json!(null));
    }

    #[test]
    fn log_session_rounds_drops_sub_minute_and_caps_the_log() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        log_session(st, "watch", "m1", 1000, 0.4);
        assert_eq!(st.stats.get("sessions").and_then(|v| v.as_array().map(|a| a.len())), Some(0), "sub-minute sessions dropped");

        log_session(st, "watch", "m1", 1000, 1.6);
        let sessions = st.stats.get("sessions").unwrap();
        assert_eq!(sessions[0]["minutes"], json!(2), "rounded");
        assert_eq!(sessions[0]["start"], json!(1000));

        for i in 0..(MAX_SESSIONS as i64) {
            log_session(st, "game", &format!("g{i}"), i, 5.0);
        }
        let sessions = st.stats.get("sessions").unwrap().as_array().unwrap().clone();
        assert_eq!(sessions.len(), MAX_SESSIONS);
        assert_eq!(sessions[0]["id"], json!("g0"), "the earlier 'm1' session was trimmed as the oldest");
    }

    #[test]
    fn stats_data_maps_session_ids_to_titles_and_art() {
        let dir = tempdir().unwrap();
        let stores = mk_stores(dir.path());
        let st = &stores_ref(&stores);
        let lib = scan(vec![movie("m1", "Film", "/lib/Film.mkv")], vec![]);
        stores[2].set("manual", json!([manual_game("manual-c:/game/exe", "My Game", "C:\\Game\\game.exe")]));

        log_session(st, "watch", "m1", 10, 90.0);
        log_session(st, "game", "manual-c:/game/exe", 20, 30.0);
        let data = stats_data(&lib, &SteamGames::disabled(), st, &url);
        assert_eq!(data["sessions"].as_array().unwrap().len(), 2);
        assert_eq!(data["items"]["m1"]["type"], json!("movie"));
        assert_eq!(data["items"]["m1"]["title"], json!("Film"));
        assert_eq!(data["items"]["manual-c:/game/exe"]["type"], json!("game"));
        assert_eq!(data["items"]["manual-c:/game/exe"]["icon"], json!("url:/art/icon.png"));
        assert!(data["now"].as_i64().unwrap() > 0);

        // A game with playtime but no sessions still shows up on the stats page.
        stores[2].set("stats", json!({"manual-c:/game/exe": {"playtime": 10, "lastPlayed": 1}}));
        let data = stats_data(&lib, &SteamGames::disabled(), st, &url);
        assert_eq!(data["items"]["manual-c:/game/exe"]["playtime"], json!(10.0));
    }

    #[test]
    fn paths_for_covers_every_request_kind() {
        let ep = |id: &str, n: i32| crate::library::Episode {
            id: id.into(), season: 2, episode: Some(n), episode_end: None, title: None,
            path: PathBuf::from(format!("/lib/Show/s02e0{n}.mkv")), thumb: None, size: 0, added_at: 5,
        };
        let show = Show {
            id: "s1".into(), title: "Show".into(), year: None, dir: PathBuf::from("/lib/Show"),
            poster: None, backdrop: None, added_at: 5, episodes: vec![ep("e1", 1), ep("e2", 2), ep("e3", 3)],
        };
        let lib = scan(vec![movie("m1", "Film", "/lib/Film.mkv")], vec![show]);

        assert_eq!(paths_for(&lib, &json!({"kind": "movie", "id": "m1"})), vec!["/lib/Film.mkv"]);
        assert_eq!(paths_for(&lib, &json!({"kind": "episode", "showId": "s1", "id": "e2"})), vec!["/lib/Show/s02e02.mkv"]);
        assert_eq!(paths_for(&lib, &json!({"kind": "season", "showId": "s1", "id": 2})), vec!["/lib/Show/s02e01.mkv", "/lib/Show/s02e02.mkv", "/lib/Show/s02e03.mkv"]);
        assert_eq!(paths_for(&lib, &json!({"kind": "show", "id": "s1"})).len(), 3);
        assert_eq!(paths_for(&lib, &json!({"kind": "show", "id": "gone"})), Vec::<String>::new());
        assert_eq!(paths_for(&lib, &json!({"kind": "bogus", "id": "x"})), Vec::<String>::new());
    }

    #[test]
    fn steam_games_convert_to_the_persisted_shape() {
        let g = steam::Game {
            id: "steam-440".into(),
            source: "steam",
            appid: "440".into(),
            title: "TF2".into(),
            install_dir: PathBuf::from("/steam/common/TF2"),
            size: 123,
            added_at: 7,
            last_played: 111,
            playtime: 60,
            art: steam::Art { poster: Some(PathBuf::from("/p.jpg")), hero: None, logo: None, header: Some(PathBuf::from("/h.jpg")), icon: None },
        };
        let v = steam_game_json(&g);
        assert_eq!(v, json!({
            "id": "steam-440", "source": "steam", "appid": "440", "title": "TF2",
            "installDir": "/steam/common/TF2", "size": 123, "addedAt": 7, "lastPlayed": 111, "playtime": 60,
            "art": {"poster": "/p.jpg", "hero": null, "logo": null, "header": "/h.jpg", "icon": null},
        }));
    }
}
