//! Game details and artwork. Steam store data needs no key; SteamGridDB (covers, backgrounds and logos
//! for anything, including non-Steam games) needs a free API key. Everything is cached in a
//! [`JsonStore`] and images are saved under `image_dir`. Direct port of `src/gameinfo.js`.
//!
//! Uses `ureq` (blocking) instead of `fetch`; `enrich` processes games strictly one at a time (like the
//! JS version — no worker pool here, so unlike `metadata.rs` there's no concurrent-write hazard to
//! design around).

use crate::parse::normalize_key;
use crate::store::JsonStore;
use chrono::Datelike;
use fancy_regex::Regex;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_STORE_BASE: &str = "https://store.steampowered.com";
const DEFAULT_CDNS: [&str; 2] = ["https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps", "https://cdn.cloudflare.steamstatic.com/steam/apps"];
const DEFAULT_SGDB_BASE: &str = "https://www.steamgriddb.com/api/v2";
const STORE_SPACING_MS: u64 = 1600; // the store API allows ~200 requests per 5 minutes
const RETRY_MS: i64 = 7 * 24 * 3600 * 1000;

const CDN_POSTER: &str = "library_600x900_2x.jpg";
const CDN_POSTER_SMALL: &str = "library_600x900.jpg";
const CDN_HERO: &str = "library_hero.jpg";
const CDN_LOGO: &str = "logo.png";
const CDN_HEADER: &str = "header.jpg";

static BR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

pub fn strip_html(s: &str) -> String {
    let s = BR_RE.replace_all(s, "\n");
    let s = TAG_RE.replace_all(&s, "");
    s.replace("&quot;", "\"").replace("&amp;", "&").replace("&#39;", "'").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").trim().to_string()
}

fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn json_is_falsy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => !b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f == 0.0).unwrap_or(false),
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => false,
    }
}

#[derive(Debug, Clone)]
pub enum GiError {
    /// SteamGridDB rejected the API key (401/403) — enrich() stops and surfaces this.
    Fatal(String),
    /// Steam's store API rate limit (429) — enrich() backs off 60s and continues.
    Retry(String),
    /// Anything else: leave the entry uncached so the next scan retries.
    Other(String),
}

impl std::fmt::Display for GiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GiError::Fatal(m) | GiError::Retry(m) | GiError::Other(m) => write!(f, "{m}"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExistingArt {
    pub poster: bool,
    pub hero: bool,
    pub logo: bool,
    pub header: bool,
}

impl ExistingArt {
    fn get(&self, key: &str) -> bool {
        match key {
            "poster" => self.poster,
            "hero" => self.hero,
            "logo" => self.logo,
            _ => unreachable!("kind is always one of the three checked in the loop"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GameOverride {
    pub steam_app_id: Option<String>,
    pub sgdb_id: Option<i64>,
    pub no_steam_match: bool,
}

pub struct GameRef<'a> {
    pub id: &'a str,
    pub source: &'a str,
    pub appid: Option<&'a str>,
    pub title: &'a str,
    pub art: ExistingArt,
    pub r#override: GameOverride,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoreHit {
    pub steam_app_id: String,
    pub title: String,
    pub thumb: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SgdbHit {
    pub sgdb_id: i64,
    pub title: String,
    pub year: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SgdbImage {
    pub url: String,
    pub thumb: String,
}

pub struct GameInfo {
    store: JsonStore,
    image_dir: PathBuf,
    sgdb_key: String,
    lang: &'static str,
    last_store_call: i64,
    running: bool,
    store_base: String,
    cdns: Vec<String>,
    sgdb_base: String,
    store_spacing_ms: u64,
}

impl GameInfo {
    pub fn new(store: JsonStore, image_dir: impl Into<PathBuf>, sgdb_key: impl Into<String>, language: Option<&str>) -> Self {
        GameInfo {
            store,
            image_dir: image_dir.into(),
            sgdb_key: sgdb_key.into().trim().to_string(),
            lang: if language == Some("fr") { "french" } else { "english" },
            last_store_call: 0,
            running: false,
            store_base: DEFAULT_STORE_BASE.to_string(),
            cdns: DEFAULT_CDNS.iter().map(|s| s.to_string()).collect(),
            sgdb_base: DEFAULT_SGDB_BASE.to_string(),
            store_spacing_ms: STORE_SPACING_MS,
        }
    }

    #[cfg(test)]
    fn with_bases(mut self, store_base: impl Into<String>, cdns: Vec<String>, sgdb_base: impl Into<String>) -> Self {
        self.store_base = store_base.into();
        self.cdns = cdns;
        self.sgdb_base = sgdb_base.into();
        self.store_spacing_ms = 0; // skip rate-limit waits in tests
        self
    }

    fn entries_snapshot(&self) -> Map<String, Value> {
        match self.store.get("games") {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }

    pub fn lookup(&self, id: &str) -> Option<Value> {
        let hit = self.entries_snapshot().get(id).cloned()?;
        if hit.get("miss").and_then(Value::as_bool).unwrap_or(false) {
            None
        } else {
            Some(hit)
        }
    }

    pub fn needs(&self, id: &str) -> bool {
        let entries = self.entries_snapshot();
        let Some(hit) = entries.get(id) else { return true };
        if hit.get("stale").and_then(Value::as_bool).unwrap_or(false) {
            return true;
        }
        if hit.get("miss").and_then(Value::as_bool).unwrap_or(false) {
            let at = hit.get("at").and_then(Value::as_i64).unwrap_or(0);
            return now_millis() - at > RETRY_MS;
        }
        false
    }

    /// Forget cached info for one game (after "Edit info") so the next enrich refetches it.
    pub fn forget(&self, id: &str) {
        self.store.update("games", json!({}), |entries| {
            if let Value::Object(m) = entries {
                m.remove(id);
            }
        });
    }

    // ------------------------------------------------------------------ HTTP

    fn store_api(&mut self, pathname: &str, params: &[(&str, &str)]) -> Result<Value, GiError> {
        let wait = self.last_store_call + self.store_spacing_ms as i64 - now_millis();
        if wait > 0 {
            std::thread::sleep(Duration::from_millis(wait as u64));
        }
        self.last_store_call = now_millis();
        let mut req = ureq::get(format!("{}{pathname}", self.store_base));
        for (k, v) in params {
            req = req.query(*k, *v);
        }
        match req.call() {
            Ok(mut resp) => resp.body_mut().read_json::<Value>().map_err(|e| GiError::Other(e.to_string())),
            Err(ureq::Error::StatusCode(429)) => Err(GiError::Retry("Steam store rate limit".into())),
            Err(ureq::Error::StatusCode(code)) => Err(GiError::Other(format!("Steam store {code}"))),
            Err(e) => Err(GiError::Other(e.to_string())),
        }
    }

    fn sgdb(&self, pathname: &str) -> Result<Option<Value>, GiError> {
        if self.sgdb_key.is_empty() {
            return Ok(None);
        }
        let url = format!("{}{pathname}", self.sgdb_base);
        match ureq::get(&url).header("Authorization", format!("Bearer {}", self.sgdb_key)).call() {
            Ok(mut resp) => {
                let body: Value = resp.body_mut().read_json().map_err(|e| GiError::Other(e.to_string()))?;
                if body.get("success").and_then(Value::as_bool).unwrap_or(false) {
                    Ok(body.get("data").cloned())
                } else {
                    Ok(None)
                }
            }
            Err(ureq::Error::StatusCode(401)) | Err(ureq::Error::StatusCode(403)) => Err(GiError::Fatal("SteamGridDB rejected the API key".into())),
            Err(ureq::Error::StatusCode(404)) => Ok(None),
            Err(ureq::Error::StatusCode(code)) => Err(GiError::Other(format!("SteamGridDB {code}"))),
            Err(e) => Err(GiError::Other(e.to_string())),
        }
    }

    /// Download `url` once into the image cache and return the local path (or `None`).
    pub fn download(&self, url: &str) -> Option<PathBuf> {
        if url.is_empty() {
            return None;
        }
        let parsed = url::Url::parse(url).ok()?;
        let ext: String = Path::new(parsed.path()).extension().map(|e| format!(".{}", e.to_string_lossy())).filter(|e| e.len() > 1).unwrap_or_else(|| ".jpg".to_string()).chars().take(5).collect();
        let mut hasher = Sha1::new();
        hasher.update(url);
        let dest = self.image_dir.join(format!("{}{ext}", &hex::encode(hasher.finalize())[..20]));
        if dest.is_file() {
            return Some(dest);
        }
        let mut resp = ureq::get(url).call().ok()?;
        let bytes = resp.body_mut().read_to_vec().ok()?;
        if bytes.len() < 200 {
            return None; // error placeholders
        }
        std::fs::create_dir_all(&self.image_dir).ok()?;
        std::fs::write(&dest, bytes).ok()?;
        Some(dest)
    }

    fn cdn(&self, appid: &str, file: &str) -> Option<PathBuf> {
        for base in &self.cdns.clone() {
            if let Some(p) = self.download(&format!("{base}/{appid}/{file}")) {
                return Some(p);
            }
        }
        None
    }

    // ------------------------------------------------------------------ Steam store

    fn store_details(&mut self, appid: &str) -> Result<Option<Value>, GiError> {
        let lang = self.lang;
        let r = self.store_api("/api/appdetails", &[("appids", appid), ("l", lang)])?;
        let d = r.get(appid).filter(|e| e.get("success").and_then(Value::as_bool).unwrap_or(false)).and_then(|e| e.get("data"));
        let Some(d) = d else { return Ok(None) };

        let shots: Vec<Value> = d.get("screenshots").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().take(8).collect();
        let mut screenshots = Vec::new();
        for s in &shots {
            let thumb_url = s.get("path_thumbnail").and_then(Value::as_str).unwrap_or("");
            if let Some(thumb) = self.download(thumb_url) {
                screenshots.push(json!({"thumb": thumb, "full": s.get("path_full")}));
            }
        }

        let genres: Vec<&str> = d.get("genres").and_then(Value::as_array).map(|a| a.iter().filter_map(|g| g.get("description").and_then(Value::as_str)).collect()).unwrap_or_default();
        let about = strip_html(d.get("about_the_game").and_then(Value::as_str).unwrap_or(""));
        let about: String = about.chars().take(3000).collect();

        Ok(Some(json!({
            "steamAppId": appid,
            "title": d.get("name"),
            "overview": strip_html(d.get("short_description").and_then(Value::as_str).unwrap_or("")),
            "about": about,
            "genres": genres,
            "developers": d.get("developers").cloned().unwrap_or_else(|| json!([])),
            "publishers": d.get("publishers").cloned().unwrap_or_else(|| json!([])),
            "releaseDate": d.get("release_date").and_then(|r| r.get("date")),
            "metacritic": d.get("metacritic").and_then(|m| m.get("score")),
            "controller": d.get("controller_support"),
            "screenshots": screenshots,
        })))
    }

    /// Steam store search, e.g. to find a manually added game's Steam page.
    pub fn store_search(&mut self, term: &str) -> Result<Vec<StoreHit>, GiError> {
        let lang = self.lang;
        let r = self.store_api("/api/storesearch/", &[("term", term), ("l", lang), ("cc", "US")])?;
        let items = r.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(items
            .into_iter()
            .filter(|i| i.get("type").and_then(Value::as_str) == Some("app") || json_is_falsy(i.get("type")))
            .map(|i| StoreHit {
                steam_app_id: i.get("id").map(|v| v.to_string()).unwrap_or_default(),
                title: i.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                thumb: i.get("tiny_image").and_then(Value::as_str).map(String::from),
            })
            .collect())
    }

    // ------------------------------------------------------------------ SteamGridDB

    pub fn sgdb_search(&self, term: &str) -> Result<Vec<SgdbHit>, GiError> {
        let encoded = utf8_percent_encode(term, NON_ALPHANUMERIC).to_string();
        let data = self.sgdb(&format!("/search/autocomplete/{encoded}"))?;
        Ok(data
            .and_then(|d| d.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|g| SgdbHit {
                sgdb_id: g.get("id").and_then(Value::as_i64).unwrap_or(0),
                title: g.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                year: g.get("release_date").and_then(Value::as_i64).and_then(|secs| chrono::DateTime::from_timestamp(secs, 0)).map(|dt| dt.year()),
            })
            .collect())
    }

    /// Candidate images of one kind ("grids" | "heroes" | "logos") for a game.
    pub fn sgdb_images(&self, kind: &str, sgdb_id: Option<i64>, steam_app_id: Option<&str>) -> Result<Vec<SgdbImage>, GiError> {
        if self.sgdb_key.is_empty() {
            return Ok(vec![]);
        }
        let query = if kind == "grids" { "?dimensions=600x900,342x482,660x930&types=static" } else { "?types=static" };
        let target = match (sgdb_id, steam_app_id) {
            (Some(id), _) => format!("game/{id}"),
            (None, Some(id)) => format!("steam/{id}"),
            (None, None) => return Ok(vec![]),
        };
        let data = self.sgdb(&format!("/{kind}/{target}{query}"))?;
        Ok(data
            .and_then(|d| d.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .take(24)
            .filter_map(|i| {
                let url = i.get("url").and_then(Value::as_str)?.to_string();
                let thumb = i.get("thumb").and_then(Value::as_str).unwrap_or(&url).to_string();
                Some(SgdbImage { url, thumb })
            })
            .collect())
    }

    // ------------------------------------------------------------------ Enrichment

    /// Fill in details and missing artwork for one game. The override (from "Edit info") can pin a
    /// Steam app id or a SteamGridDB id.
    pub fn fetch_game(&mut self, game: &GameRef) -> Result<Value, GiError> {
        let o = &game.r#override;
        let mut steam_app_id = o.steam_app_id.clone().or_else(|| if game.source == "steam" { game.appid.map(String::from) } else { None });

        if steam_app_id.is_none() && game.source != "steam" && !o.no_steam_match {
            // Manual game: take the store's top hit only if its name really matches.
            let hits = self.store_search(game.title).unwrap_or_default();
            let key = normalize_key(game.title);
            let hit = hits.iter().find(|h| normalize_key(&h.title) == key).or_else(|| hits.iter().find(|h| normalize_key(&h.title).starts_with(&key) && key.len() > 4));
            if let Some(h) = hit {
                steam_app_id = Some(h.steam_app_id.clone());
            }
        }

        let info = match &steam_app_id {
            Some(id) => match self.store_details(id) {
                Ok(v) => v,
                Err(GiError::Retry(m)) => return Err(GiError::Retry(m)),
                Err(_) => None,
            },
            None => None,
        };

        let mut out = match info {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        out.insert("steamAppId".into(), json!(steam_app_id));

        let mut sgdb_id = o.sgdb_id;
        if !self.sgdb_key.is_empty() && sgdb_id.is_none() && steam_app_id.is_none() {
            match self.sgdb_search(game.title) {
                Ok(hits) => {
                    if let Some(h) = hits.first() {
                        sgdb_id = Some(h.sgdb_id);
                    }
                }
                Err(GiError::Fatal(m)) => return Err(GiError::Fatal(m)),
                Err(_) => {}
            }
        }

        let mut art = Map::new();
        let kinds: [(&str, &str, &str); 3] = [("poster", "grids", CDN_POSTER), ("hero", "heroes", CDN_HERO), ("logo", "logos", CDN_LOGO)];
        for (key, sgdb_kind, cdn_file) in kinds {
            if game.art.get(key) {
                continue;
            }
            let mut p: Option<PathBuf> = None;
            if !self.sgdb_key.is_empty() && (sgdb_id.is_some() || steam_app_id.is_some()) {
                match self.sgdb_images(sgdb_kind, sgdb_id, steam_app_id.as_deref()) {
                    Ok(imgs) => {
                        if let Some(first) = imgs.first() {
                            p = self.download(&first.url);
                        }
                    }
                    Err(GiError::Fatal(m)) => return Err(GiError::Fatal(m)),
                    Err(_) => {}
                }
            }
            if p.is_none() {
                if let Some(id) = &steam_app_id {
                    p = self.cdn(id, cdn_file).or_else(|| if key == "poster" { self.cdn(id, CDN_POSTER_SMALL) } else { None });
                }
            }
            if let Some(path) = p {
                art.insert(key.to_string(), json!(path));
            }
        }
        if !game.art.header {
            if let Some(id) = &steam_app_id {
                if let Some(p) = self.cdn(id, CDN_HEADER) {
                    art.insert("header".into(), json!(p));
                }
            }
        }
        out.insert("art".into(), Value::Object(art));
        out.insert("sgdbId".into(), json!(sgdb_id));
        Ok(Value::Object(out))
    }

    fn record_result(&self, key: &str, result: Value) {
        let now = now_millis();
        let found = result.get("title").map(|v| !json_is_falsy(Some(v))).unwrap_or(false)
            || result.get("overview").map(|v| !json_is_falsy(Some(v))).unwrap_or(false)
            || result.get("art").and_then(Value::as_object).map(|m| !m.is_empty()).unwrap_or(false);
        self.store.update("games", json!({}), |entries| {
            let Value::Object(entries) = entries else { return };
            let prev = entries.get(key).cloned();
            let new_entry = if found {
                let mut m = match result.clone() {
                    Value::Object(m) => m,
                    _ => Map::new(),
                };
                m.insert("at".into(), json!(now));
                Value::Object(m)
            } else {
                match prev {
                    Some(Value::Object(mut m)) if !m.get("miss").and_then(Value::as_bool).unwrap_or(false) => {
                        // Keep what we had.
                        m.insert("stale".into(), json!(false));
                        m.insert("at".into(), json!(now));
                        Value::Object(m)
                    }
                    _ => json!({"miss": true, "at": now}),
                }
            };
            entries.insert(key.to_string(), new_entry);
        });
    }

    /// Enrich every game that isn't cached yet, one at a time. `should_pause` lets playback stop the
    /// work.
    pub fn enrich(&mut self, games: &[GameRef], on_update: impl Fn(), mut should_pause: impl FnMut() -> bool) -> Result<(), GiError> {
        if self.running {
            return Ok(());
        }
        self.running = true;
        let result = self.enrich_inner(games, &on_update, &mut should_pause);
        self.running = false;
        result
    }

    fn enrich_inner(&mut self, games: &[GameRef], on_update: &impl Fn(), should_pause: &mut impl FnMut() -> bool) -> Result<(), GiError> {
        for g in games {
            while should_pause() {
                std::thread::sleep(Duration::from_millis(5000));
            }
            if !self.needs(g.id) {
                continue;
            }
            match self.fetch_game(g) {
                Ok(r) => {
                    self.record_result(g.id, r);
                    on_update();
                }
                Err(GiError::Fatal(m)) => return Err(GiError::Fatal(m)),
                Err(GiError::Retry(_)) => std::thread::sleep(Duration::from_millis(60000)),
                Err(GiError::Other(_)) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use tempfile::tempdir;

    fn new_store(dir: &Path) -> JsonStore {
        JsonStore::new(dir.join("s.json"), Map::new())
    }

    #[test]
    fn strip_html_test() {
        // Parity with the JS test "stripHtml".
        assert_eq!(strip_html("<p>A &quot;b&quot;<br>c</p>"), "A \"b\"\nc");
    }

    // Parity with the JS test "game info: Steam store details, CDN art and manual-game matching".
    #[test]
    fn steam_store_details_cdn_art_and_manual_game_matching() {
        let store_server = MockServer::start();
        let cdn_server = MockServer::start();
        let dir = tempdir().unwrap();

        store_server.mock(|when, then| {
            when.method(GET).path("/api/storesearch/");
            then.status(200).json_body(json!({"items": [{"id": 1145360, "name": "Hades", "type": "app"}, {"id": 1, "name": "Hades Star"}]}));
        });
        store_server.mock(|when, then| {
            when.method(GET).path("/api/appdetails").query_param("l", "french");
            then.status(200).json_body(json!({
                "1145360": {
                    "success": true,
                    "data": {
                        "name": "Hades",
                        "short_description": "D\u{e9}fiez le dieu des morts &amp; frappez&nbsp;fort.",
                        "genres": [{"description": "Action"}, {"description": "Ind\u{e9}pendant"}],
                        "developers": ["Supergiant Games"],
                        "publishers": ["Supergiant Games"],
                        "release_date": {"date": "17 sept. 2020"},
                        "metacritic": {"score": 93},
                        "controller_support": "full",
                        "screenshots": [{"path_thumbnail": cdn_server.url("/s1_600.jpg"), "path_full": cdn_server.url("/s1_1920.jpg")}]
                    }
                }
            }));
        });
        // The 2x poster is missing; the 1x fallback and everything else on the CDN succeeds.
        cdn_server.mock(|when, then| {
            when.method(GET).path(format!("/1145360/{CDN_POSTER}"));
            then.status(404);
        });
        cdn_server.mock(|when, then| {
            when.method(GET);
            then.status(200).body(vec![0u8; 500]);
        });

        let mut gi = GameInfo::new(new_store(dir.path()), dir.path(), "", Some("fr")).with_bases(store_server.base_url(), vec![cdn_server.base_url()], String::new());

        let r = gi
            .fetch_game(&GameRef { id: "game-1", source: "manual", appid: None, title: "Hades", art: ExistingArt::default(), r#override: GameOverride::default() })
            .unwrap();
        assert_eq!(r["steamAppId"], "1145360");
        assert_eq!(r["overview"], "Défiez le dieu des morts & frappez fort.");
        assert_eq!(r["genres"], json!(["Action", "Indépendant"]));
        assert_eq!(r["metacritic"], 93);
        assert_eq!(r["controller"], "full");
        assert_eq!(r["screenshots"].as_array().unwrap().len(), 1);
        assert!(Path::new(r["art"]["poster"].as_str().unwrap()).exists(), "falls back to the 1x poster when the 2x one is missing");
        assert!(r["art"]["hero"].is_string() && r["art"]["logo"].is_string() && r["art"]["header"].is_string());

        // A title that doesn't really match a store result isn't matched.
        let miss = gi
            .fetch_game(&GameRef { id: "game-2", source: "manual", appid: None, title: "Hadés Fan Remake Deluxe", art: ExistingArt::default(), r#override: GameOverride::default() })
            .unwrap();
        assert_eq!(miss["steamAppId"], Value::Null);
    }

    #[test]
    fn store_search_keeps_apps_and_typeless_hits_but_drops_other_types() {
        let store_server = MockServer::start();
        let dir = tempdir().unwrap();
        store_server.mock(|when, then| {
            when.method(GET).path("/api/storesearch/");
            then.status(200).json_body(json!({"items": [
                {"id": 1, "name": "A Game", "type": "app", "tiny_image": "http://x/a.jpg"},
                {"id": 2, "name": "No Type Field"},
                {"id": 3, "name": "A Bundle", "type": "bundle"},
            ]}));
        });
        let mut gi = GameInfo::new(new_store(dir.path()), dir.path(), "", None).with_bases(store_server.base_url(), vec![], String::new());
        let hits = gi.store_search("query").unwrap();
        assert_eq!(hits, vec![
            StoreHit { steam_app_id: "1".into(), title: "A Game".into(), thumb: Some("http://x/a.jpg".into()) },
            StoreHit { steam_app_id: "2".into(), title: "No Type Field".into(), thumb: None },
        ]);
    }

    #[test]
    fn sgdb_search_derives_the_year_from_a_unix_timestamp() {
        let sgdb_server = MockServer::start();
        let dir = tempdir().unwrap();
        sgdb_server.mock(|when, then| {
            when.method(GET).path("/search/autocomplete/Hades");
            then.status(200).json_body(json!({"success": true, "data": [{"id": 42, "name": "Hades", "release_date": 1_600_000_000}]}));
        });
        let gi = GameInfo::new(new_store(dir.path()), dir.path(), "key", None).with_bases(String::new(), vec![], sgdb_server.base_url());
        let hits = gi.sgdb_search("Hades").unwrap();
        assert_eq!(hits, vec![SgdbHit { sgdb_id: 42, title: "Hades".into(), year: Some(2020) }]);
    }

    #[test]
    fn sgdb_rejects_a_bad_key_as_fatal() {
        let sgdb_server = MockServer::start();
        let dir = tempdir().unwrap();
        sgdb_server.mock(|when, then| {
            when.method(GET);
            then.status(401).json_body(json!({}));
        });
        let gi = GameInfo::new(new_store(dir.path()), dir.path(), "bad", None).with_bases(String::new(), vec![], sgdb_server.base_url());
        assert!(matches!(gi.sgdb_search("x"), Err(GiError::Fatal(_))));
    }

    // Parity with the JS test "a refetch that finds nothing keeps the info we already had".
    #[test]
    fn a_refetch_that_finds_nothing_keeps_the_info_we_already_had() {
        let dir = tempdir().unwrap();
        let store = new_store(dir.path());
        store.set("games", json!({"steam-1": {"title": "Old", "overview": "Kept", "art": {}, "stale": true, "at": 1}}));
        let gi = GameInfo::new(store, dir.path(), "", None);
        // fetch_game would need real network; instead exercise record_result/enrich's bookkeeping
        // directly the way the JS test stubs `gi.fetchGame`, by recording a "found nothing" result.
        gi.record_result("steam-1", json!({"steamAppId": "1", "art": {}}));
        assert_eq!(gi.lookup("steam-1").unwrap()["overview"], "Kept");
        assert!(!gi.needs("steam-1"));
    }
}
