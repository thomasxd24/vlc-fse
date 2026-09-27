//! Optional TMDB lookups. Results (including misses) are cached in a [`JsonStore`] under `"entries"`,
//! and images are downloaded once into `image_dir` so the UI works offline afterwards. Direct port of
//! `src/metadata.js`.
//!
//! Uses `ureq` (blocking) rather than the JS version's `fetch`, and real OS threads rather than async
//! workers for `enrich`'s 3-way concurrency — same "blocking core, Tauri wraps in `spawn_blocking`"
//! approach as `steam.rs`/`library.rs`. That concurrency change is not cosmetic: the JS version mutates
//! its cache object directly and only calls `store.save()` afterwards, which is safe because JS's
//! workers only ever interleave at `await` points, never truly in parallel. Real OS threads doing the
//! same plain get-then-mutate-then-set would race and could lose an update, so `enrich` here goes
//! through [`crate::store::JsonStore::update`] instead, which holds the store's lock across the whole
//! read-modify-write.

use crate::parse::normalize_key;
use crate::store::JsonStore;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_API_BASE: &str = "https://api.themoviedb.org/3";
const DEFAULT_IMG_BASE: &str = "https://image.tmdb.org/t/p";
const MISS_RETRY_MS: i64 = 7 * 24 * 3600 * 1000;

fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Debug, Clone)]
pub enum ApiError {
    /// The API key itself was rejected (TMDB returned 401) — enrich() stops everything and surfaces
    /// this, same as the JS version's `err.fatal`.
    Fatal(String),
    /// A network hiccup or bad response: leave the entry uncached so the next scan retries.
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Fatal(m) | ApiError::Other(m) => write!(f, "{m}"),
        }
    }
}

pub fn movie_key(title: &str, year: Option<i32>) -> String {
    format!("movie:{}:{}", normalize_key(title), year.map(|y| y.to_string()).unwrap_or_default())
}

pub fn show_key(title: &str, year: Option<i32>) -> String {
    format!("tv:{}:{}", normalize_key(title), year.map(|y| y.to_string()).unwrap_or_default())
}

pub struct MovieRef<'a> {
    pub title: &'a str,
    pub year: Option<i32>,
}

pub struct EpisodeRef {
    pub season: i32,
    pub episode: i32,
    pub has_thumb: bool,
}

pub struct ShowRef<'a> {
    pub title: &'a str,
    pub year: Option<i32>,
    pub episodes: &'a [EpisodeRef],
}

pub struct LibraryRef<'a> {
    pub movies: &'a [MovieRef<'a>],
    pub shows: &'a [ShowRef<'a>],
}

pub struct Metadata {
    store: JsonStore,
    image_dir: PathBuf,
    api_key: String,
    language: String,
    api_base: String,
    img_base: String,
}

impl Metadata {
    pub fn new(store: JsonStore, image_dir: impl Into<PathBuf>, api_key: impl Into<String>, language: Option<String>) -> Self {
        Metadata {
            store,
            image_dir: image_dir.into(),
            api_key: api_key.into().trim().to_string(),
            language: language.unwrap_or_else(|| "en-US".to_string()),
            api_base: DEFAULT_API_BASE.to_string(),
            img_base: DEFAULT_IMG_BASE.to_string(),
        }
    }

    #[cfg(test)]
    fn with_bases(mut self, api_base: impl Into<String>, img_base: impl Into<String>) -> Self {
        self.api_base = api_base.into();
        self.img_base = img_base.into();
        self
    }

    pub fn enabled(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn entries_snapshot(&self) -> Map<String, Value> {
        match self.store.get("entries") {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }

    pub fn lookup(&self, key: &str) -> Option<Value> {
        let hit = self.entries_snapshot().get(key).cloned()?;
        if hit.get("miss").and_then(Value::as_bool).unwrap_or(false) {
            None
        } else {
            Some(hit)
        }
    }

    pub fn needs(&self, key: &str) -> bool {
        let entries = self.entries_snapshot();
        let Some(hit) = entries.get(key) else { return true };
        if hit.get("stale").and_then(Value::as_bool).unwrap_or(false) {
            return true;
        }
        if hit.get("miss").and_then(Value::as_bool).unwrap_or(false) {
            let at = hit.get("at").and_then(Value::as_i64).unwrap_or(0);
            return now_millis() - at > MISS_RETRY_MS;
        }
        false
    }

    fn api(&self, pathname: &str, params: &[(&str, String)]) -> Result<Value, ApiError> {
        let url = format!("{}{pathname}", self.api_base);
        let mut req = ureq::get(&url);
        // v4 "read access tokens" are JWTs; v3 keys are 32 hex chars.
        if self.api_key.starts_with("eyJ") {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        } else {
            req = req.query("api_key", &self.api_key);
        }
        req = req.query("language", &self.language);
        for (k, v) in params {
            if !v.is_empty() {
                req = req.query(*k, v);
            }
        }
        match req.call() {
            Ok(mut resp) => resp.body_mut().read_json::<Value>().map_err(|e| ApiError::Other(e.to_string())),
            Err(ureq::Error::StatusCode(401)) => Err(ApiError::Fatal("TMDB rejected the API key".into())),
            Err(ureq::Error::StatusCode(code)) => Err(ApiError::Other(format!("TMDB {code}"))),
            Err(e) => Err(ApiError::Other(e.to_string())),
        }
    }

    fn image(&self, file_path: Option<&str>, size: &str) -> Option<PathBuf> {
        let file_path = file_path?;
        let mut hasher = Sha1::new();
        hasher.update(format!("{size}{file_path}"));
        let digest = hasher.finalize();
        let ext = Path::new(file_path).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let dest = self.image_dir.join(format!("{}{ext}", &hex::encode(digest)[..20]));
        if dest.is_file() {
            return Some(dest);
        }
        let url = format!("{}/{size}{file_path}", self.img_base);
        let mut resp = ureq::get(&url).call().ok()?;
        std::fs::create_dir_all(&self.image_dir).ok()?;
        let bytes = resp.body_mut().read_to_vec().ok()?;
        std::fs::write(&dest, bytes).ok()?;
        Some(dest)
    }

    pub fn fetch_movie(&self, movie: &MovieRef) -> Result<Option<Value>, ApiError> {
        let mut params: Vec<(&str, String)> = vec![("query", movie.title.to_string())];
        if let Some(y) = movie.year {
            params.push(("year", y.to_string()));
        }
        let mut r = self.api("/search/movie", &params)?;
        if r.get("results").and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(true) && movie.year.is_some() {
            r = self.api("/search/movie", &[("query", movie.title.to_string())])?;
        }
        let Some(best) = r.get("results").and_then(Value::as_array).and_then(|a| a.first()) else { return Ok(None) };
        let Some(id) = best.get("id").and_then(Value::as_i64) else { return Ok(None) };
        let d = self.api(&format!("/movie/{id}"), &[])?;

        let genres: Vec<String> = d.get("genres").and_then(Value::as_array).map(|a| a.iter().filter_map(|g| g.get("name").and_then(Value::as_str).map(String::from)).collect()).unwrap_or_default();
        let vote_average = d.get("vote_average").and_then(Value::as_f64);
        let rating = match vote_average {
            Some(v) if v != 0.0 => Some((v * 10.0).round() / 10.0),
            _ => None,
        };
        let poster = self.image(d.get("poster_path").and_then(Value::as_str), "w500");
        let backdrop = self.image(d.get("backdrop_path").and_then(Value::as_str), "w1280");

        Ok(Some(json!({
            "tmdbId": d.get("id"),
            "title": d.get("title"),
            "overview": d.get("overview").and_then(Value::as_str).unwrap_or(""),
            "tagline": d.get("tagline").and_then(Value::as_str).unwrap_or(""),
            "rating": rating,
            "runtime": d.get("runtime").and_then(Value::as_i64),
            "genres": genres,
            "releaseDate": d.get("release_date"),
            "poster": poster,
            "backdrop": backdrop,
        })))
    }

    pub fn fetch_show(&self, show: &ShowRef) -> Result<Option<Value>, ApiError> {
        let mut params: Vec<(&str, String)> = vec![("query", show.title.to_string())];
        if let Some(y) = show.year {
            params.push(("first_air_date_year", y.to_string()));
        }
        let mut r = self.api("/search/tv", &params)?;
        if r.get("results").and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(true) && show.year.is_some() {
            r = self.api("/search/tv", &[("query", show.title.to_string())])?;
        }
        let Some(best) = r.get("results").and_then(Value::as_array).and_then(|a| a.first()) else { return Ok(None) };
        let Some(id) = best.get("id").and_then(Value::as_i64) else { return Ok(None) };
        let d = self.api(&format!("/tv/{id}"), &[])?;

        let mut seasons: Vec<i32> = show.episodes.iter().map(|e| e.season).collect();
        seasons.sort_unstable();
        seasons.dedup();

        let mut episodes: Map<String, Value> = Map::new();
        for n in &seasons {
            match self.api(&format!("/tv/{id}/season/{n}"), &[]) {
                Ok(s) => {
                    for ep in s.get("episodes").and_then(Value::as_array).into_iter().flatten() {
                        let Some(number) = ep.get("episode_number").and_then(Value::as_i64) else { continue };
                        episodes.insert(
                            format!("{n}x{number}"),
                            json!({
                                "title": ep.get("name").and_then(Value::as_str),
                                "overview": ep.get("overview").and_then(Value::as_str).unwrap_or(""),
                                "airDate": ep.get("air_date"),
                                "runtime": ep.get("runtime"),
                                "still": ep.get("still_path"),
                            }),
                        );
                    }
                }
                Err(ApiError::Fatal(m)) => return Err(ApiError::Fatal(m)),
                Err(ApiError::Other(_)) => {} // this season's episode list is just missing; keep going
            }
        }
        // Only download stills for episodes we actually have.
        for e in show.episodes {
            let key = format!("{}x{}", e.season, e.episode);
            let Some(info) = episodes.get_mut(&key) else { continue };
            let still_path = info.get("still").and_then(Value::as_str).map(String::from);
            let new_still = match still_path {
                Some(p) if !e.has_thumb => self.image(Some(&p), "w300").map(|path| json!(path)).unwrap_or(Value::Null),
                _ => Value::Null,
            };
            info["still"] = new_still;
        }

        let genres: Vec<String> = d.get("genres").and_then(Value::as_array).map(|a| a.iter().filter_map(|g| g.get("name").and_then(Value::as_str).map(String::from)).collect()).unwrap_or_default();
        let vote_average = d.get("vote_average").and_then(Value::as_f64);
        let rating = match vote_average {
            Some(v) if v != 0.0 => Some((v * 10.0).round() / 10.0),
            _ => None,
        };
        let poster = self.image(d.get("poster_path").and_then(Value::as_str), "w500");
        let backdrop = self.image(d.get("backdrop_path").and_then(Value::as_str), "w1280");

        Ok(Some(json!({
            "tmdbId": d.get("id"),
            "title": d.get("name"),
            "overview": d.get("overview").and_then(Value::as_str).unwrap_or(""),
            "rating": rating,
            "genres": genres,
            "firstAirDate": d.get("first_air_date"),
            "status": d.get("status"),
            "poster": poster,
            "backdrop": backdrop,
            "episodes": episodes,
        })))
    }

    /// Record the outcome of one fetch into the cache, exactly matching the JS version's
    /// keep-what-we-had-on-a-non-fatal-miss logic — but via `JsonStore::update` so concurrent workers
    /// can't race each other's writes (see the module doc).
    fn record_result(&self, key: &str, result: Option<Value>) {
        let now = now_millis();
        self.store.update("entries", json!({}), |entries| {
            let Value::Object(entries) = entries else { return };
            let prev = entries.get(key).cloned();
            let new_entry = match result {
                Some(Value::Object(mut map)) => {
                    map.insert("at".into(), json!(now));
                    Value::Object(map)
                }
                Some(other) => other, // fetch_* always returns an object; defensive fallback only
                None => match prev {
                    Some(Value::Object(mut map)) if !map.get("miss").and_then(Value::as_bool).unwrap_or(false) => {
                        // Keep what we had.
                        map.insert("stale".into(), json!(false));
                        map.insert("at".into(), json!(now));
                        Value::Object(map)
                    }
                    _ => json!({"miss": true, "at": now}),
                },
            };
            entries.insert(key.to_string(), new_entry);
        });
    }

    /// Fetch anything missing from the cache. Calls `on_update` after each new result so the UI can
    /// refresh. Returns `Err` only for a rejected API key.
    pub fn enrich(&self, library: &LibraryRef, on_update: impl Fn() + Sync) -> Result<(), ApiError> {
        if !self.enabled() {
            return Ok(());
        }

        enum Kind<'a> {
            Movie(&'a MovieRef<'a>),
            Show(&'a ShowRef<'a>),
        }
        struct Job<'a> {
            key: String,
            kind: Kind<'a>,
        }

        let mut jobs: Vec<Job> = Vec::new();
        for m in library.movies {
            let key = movie_key(m.title, m.year);
            if self.needs(&key) {
                jobs.push(Job { key, kind: Kind::Movie(m) });
            }
        }
        for s in library.shows {
            let key = show_key(s.title, s.year);
            if self.needs(&key) {
                jobs.push(Job { key, kind: Kind::Show(s) });
            }
        }

        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let fatal: Mutex<Option<ApiError>> = Mutex::new(None);
        let jobs = &jobs;
        let next = &next;
        let stop = &stop;
        let fatal = &fatal;
        let on_update = &on_update;

        std::thread::scope(|scope| {
            for _ in 0..3 {
                scope.spawn(move || loop {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= jobs.len() {
                        break;
                    }
                    let job = &jobs[i];
                    let result = match &job.kind {
                        Kind::Movie(m) => self.fetch_movie(m),
                        Kind::Show(s) => self.fetch_show(s),
                    };
                    match result {
                        Ok(value) => {
                            self.record_result(&job.key, value);
                            on_update();
                        }
                        Err(ApiError::Fatal(msg)) => {
                            *fatal.lock().unwrap() = Some(ApiError::Fatal(msg));
                            stop.store(true, Ordering::SeqCst);
                            break;
                        }
                        Err(ApiError::Other(_)) => {
                            // Network hiccup: leave uncached so the next scan retries.
                        }
                    }
                });
            }
        });

        let result = fatal.lock().unwrap().take();
        match result {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::Map;
    use std::sync::atomic::AtomicUsize as Counter;
    use tempfile::tempdir;

    fn store(dir: &Path) -> JsonStore {
        JsonStore::new(dir.join("settings.json"), Map::new())
    }

    // Parity with the JS test "fetches, caches and reuses TMDB metadata" (test/metadata.test.js).
    #[test]
    fn fetches_caches_and_reuses_tmdb_metadata() {
        let api = MockServer::start();
        let img = MockServer::start();
        let image_dir = tempdir().unwrap();

        let search_movie = api.mock(|when, then| {
            when.method(GET).path("/search/movie").query_param("query", "Heat").query_param("api_key", "k");
            then.status(200).json_body(json!({"results": [{"id": 949}]}));
        });
        let movie_detail = api.mock(|when, then| {
            when.method(GET).path("/movie/949");
            then.status(200).json_body(json!({
                "id": 949, "title": "Heat", "overview": "Cops and robbers.", "vote_average": 7.94,
                "runtime": 170, "genres": [{"name": "Crime"}], "poster_path": "/p.jpg", "backdrop_path": "/b.jpg"
            }));
        });
        let search_tv = api.mock(|when, then| {
            when.method(GET).path("/search/tv");
            then.status(200).json_body(json!({"results": []}));
        });
        img.mock(|when, then| {
            when.method(GET);
            then.status(200).body([1u8, 2, 3]);
        });

        let meta = Metadata::new(store(image_dir.path()), image_dir.path(), "k", None).with_bases(api.base_url(), img.base_url());
        let movies = [MovieRef { title: "Heat", year: Some(1995) }];
        let shows = [ShowRef { title: "Nope", year: None, episodes: &[] }];
        let library = LibraryRef { movies: &movies, shows: &shows };

        let updates = Counter::new(0);
        meta.enrich(&library, || {
            updates.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

        let hit = meta.lookup(&movie_key("Heat", Some(1995))).unwrap();
        assert_eq!(hit["overview"], "Cops and robbers.");
        assert_eq!(hit["rating"], 7.9);
        assert_eq!(hit["genres"], json!(["Crime"]));
        assert!(Path::new(hit["poster"].as_str().unwrap()).exists());
        assert!(meta.lookup(&show_key("Nope", None)).is_none(), "misses are cached but not returned");
        assert_eq!(updates.load(Ordering::SeqCst), 2);

        search_movie.assert();
        movie_detail.assert();
        search_tv.assert();

        // Second run: everything is cached, so no network at all.
        meta.enrich(&library, || panic!("should not fetch again")).unwrap();
    }

    // Parity with the JS test "a rejected API key stops enrichment with an error".
    #[test]
    fn a_rejected_api_key_stops_enrichment_with_an_error() {
        let api = MockServer::start();
        api.mock(|when, then| {
            when.method(GET);
            then.status(401).json_body(json!({}));
        });
        let dir = tempdir().unwrap();
        let meta = Metadata::new(store(dir.path()), dir.path(), "bad", None).with_bases(api.base_url(), api.base_url());
        let movies = [MovieRef { title: "A", year: None }, MovieRef { title: "B", year: None }];
        let library = LibraryRef { movies: &movies, shows: &[] };
        let err = meta.enrich(&library, || {}).unwrap_err();
        assert!(matches!(err, ApiError::Fatal(_)));
    }

    #[test]
    fn movie_and_show_keys_normalize_title_and_include_year() {
        assert_eq!(movie_key("The Matrix", Some(1999)), "movie:the matrix:1999");
        assert_eq!(movie_key("Amélie", None), "movie:amelie:");
        assert_eq!(show_key("Dark", Some(2017)), "tv:dark:2017");
    }
}
