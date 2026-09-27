//! Local media library scanning: walking library folders for video files, parsing titles/episodes out
//! of their names (via [`crate::parse`]), and matching up local poster/backdrop/thumbnail art. Direct
//! port of `src/library.js`.

use crate::locale;
use crate::parse;
use fancy_regex::Regex;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_DEPTH: u32 = 8;
static IGNORED_DIRS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(\$recycle\.bin|system volume information|\..*|@eadir|#recycle|extras?|featurettes?|behind the scenes|deleted scenes|interviews|scenes|shorts|trailers?|samples?|subs|subtitles)$")
        .unwrap()
});
static SAMPLE_FILE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(^|[\s._\-])(sample|trailer)([\s._\-]|$)").unwrap());
const IMAGE_EXTS: [&str; 4] = [".jpg", ".jpeg", ".png", ".webp"];
const POSTER_NAMES: [&str; 6] = ["poster", "folder", "cover", "movie", "show", "default"];
const BACKDROP_NAMES: [&str; 4] = ["fanart", "backdrop", "background", "art"];

fn make_id(prefix: &str, p: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(p.to_lowercase());
    let digest = hasher.finalize();
    format!("{prefix}{}", &hex::encode(digest)[..16])
}

fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn mtime_millis(meta: &fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn read_dir_safe(dir: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(dir).map(|rd| rd.flatten().collect()).unwrap_or_default()
}

/// Recursively collect video files below `root`.
fn walk(root: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    for e in read_dir_safe(root) {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            if IGNORED_DIRS_RE.is_match(&name).unwrap_or(false) {
                continue;
            }
            walk(&e.path(), depth + 1, out);
        } else if ft.is_file() && parse::is_video_file(&name) && !SAMPLE_FILE_RE.is_match(parse::strip_extension(&name)).unwrap_or(false) {
            out.push(e.path());
        }
    }
}

/// Return the first file in `files` whose name (minus image extension) matches a candidate, in
/// priority order.
fn find_image(files: &[String], candidates: &[String]) -> Option<String> {
    let lower: HashMap<String, &String> = files.iter().map(|f| (f.to_lowercase(), f)).collect();
    for c in candidates {
        for ext in IMAGE_EXTS {
            if let Some(hit) = lower.get(&format!("{}{ext}", c.to_lowercase())) {
                return Some((*hit).clone());
            }
        }
    }
    None
}

/// Caches a folder's file listing for the lifetime of one [`scan_libraries`] call (the JS version's
/// module-level `dirCache`, threaded explicitly here instead of living in global state — the JS map is
/// cleared at the start and end of every `scanLibraries` call anyway, so this is the same lifetime,
/// just without the global-mutable-state downside of sharing one cache across parallel test runs).
#[derive(Default)]
pub struct DirCache(HashMap<PathBuf, Vec<String>>);

impl DirCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn list_files(&mut self, dir: &Path) -> Vec<String> {
        self.0
            .entry(dir.to_path_buf())
            .or_insert_with(|| read_dir_safe(dir).into_iter().filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .clone()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Art {
    pub poster: Option<PathBuf>,
    pub backdrop: Option<PathBuf>,
}

/// Art for a folder dedicated to one title: "<file>-poster.jpg", "<file>.jpg", then generic
/// "poster.jpg" etc.
fn local_art(dir: &Path, prefix: Option<&str>, cache: &mut DirCache) -> Art {
    let files = cache.list_files(dir);
    let prefixed = |names: &[&str]| -> Vec<String> {
        match prefix {
            Some(p) => names.iter().map(|n| format!("{p}-{n}")).collect(),
            None => Vec::new(),
        }
    };
    let mut poster_candidates = prefixed(&POSTER_NAMES);
    if let Some(p) = prefix {
        poster_candidates.push(p.to_string());
    }
    poster_candidates.extend(POSTER_NAMES.iter().map(|s| s.to_string()));
    let mut backdrop_candidates = prefixed(&BACKDROP_NAMES);
    backdrop_candidates.extend(BACKDROP_NAMES.iter().map(|s| s.to_string()));

    Art {
        poster: find_image(&files, &poster_candidates).map(|p| dir.join(p)),
        backdrop: find_image(&files, &backdrop_candidates).map(|p| dir.join(p)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Movie {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub path: PathBuf,
    pub dir: PathBuf,
    pub poster: Option<PathBuf>,
    pub backdrop: Option<PathBuf>,
    pub size: u64,
    pub added_at: i64,
}

pub fn scan_movies(root: &Path, cache: &mut DirCache) -> Vec<Movie> {
    let mut files = Vec::new();
    walk(root, 0, &mut files);

    // Count videos per directory so a folder holding a single film can lend it its (usually cleaner) name.
    let mut per_dir: HashMap<PathBuf, u32> = HashMap::new();
    for f in &files {
        *per_dir.entry(f.parent().unwrap().to_path_buf()).or_insert(0) += 1;
    }

    let mut movies = Vec::new();
    for file in &files {
        let dir = file.parent().unwrap().to_path_buf();
        let file_name = file.file_name().unwrap().to_string_lossy().into_owned();
        let base = parse::strip_extension(&file_name).to_string();
        let own_folder = dir != root && per_dir.get(&dir).copied().unwrap_or(0) == 1;
        let from_file = parse::parse_movie_name(&file_name);
        let from_folder = if own_folder {
            let dir_name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Some(parse::parse_movie_name(&dir_name))
        } else {
            None
        };

        let parsed = match &from_folder {
            Some(ff) if ff.year.is_some() => ff.clone(),
            Some(ff) => {
                if from_file.year.is_some() {
                    from_file.clone()
                } else {
                    ff.clone()
                }
            }
            None => from_file.clone(),
        };

        let art = if own_folder {
            local_art(&dir, Some(&base), cache)
        } else {
            // In a shared folder, generic names like poster.jpg belong to nobody; only accept art
            // named after this file.
            let dir_files = cache.list_files(&dir);
            let poster_candidates = vec![format!("{base}-poster"), format!("{base}-cover"), base.clone()];
            let backdrop_candidates: Vec<String> = BACKDROP_NAMES.iter().map(|n| format!("{base}-{n}")).collect();
            Art {
                poster: find_image(&dir_files, &poster_candidates).map(|p| dir.join(p)),
                backdrop: find_image(&dir_files, &backdrop_candidates).map(|p| dir.join(p)),
            }
        };

        let st = fs::metadata(file).ok();
        movies.push(Movie {
            id: make_id("m", &file.to_string_lossy()),
            title: parsed.title,
            year: parsed.year,
            path: file.clone(),
            dir: dir.clone(),
            poster: art.poster,
            backdrop: art.backdrop,
            size: st.as_ref().map(|m| m.len()).unwrap_or(0),
            added_at: st.as_ref().map(mtime_millis).unwrap_or(0),
        });
    }
    movies
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    pub id: String,
    pub season: i32,
    /// Guaranteed `Some` once [`scan_shows`] returns — unmarked episodes are numbered by position
    /// within their season as a last step, same as the JS version.
    pub episode: Option<i32>,
    pub episode_end: Option<i32>,
    pub title: Option<String>,
    pub path: PathBuf,
    pub thumb: Option<PathBuf>,
    pub size: u64,
    pub added_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Show {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub dir: PathBuf,
    pub poster: Option<PathBuf>,
    pub backdrop: Option<PathBuf>,
    pub added_at: i64,
    pub episodes: Vec<Episode>,
}

/// Collapse runs of `.`/`_` into a single space each — just that, unlike `parse`'s fuller `tidy`, which
/// also collapses whitespace runs and trims. Matches library.js's inline `.replace(/[._]+/g, ' ')` for
/// deriving a fallback episode title from a filename with no parseable episode marker.
fn dots_underscores_to_space(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_run = false;
    for c in s.chars() {
        if c == '.' || c == '_' {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

pub fn scan_shows(root: &Path, cache: &mut DirCache) -> Vec<Show> {
    let mut files = Vec::new();
    walk(root, 0, &mut files);

    // Grouped by title (via the key below) rather than by folder, so output order doesn't reflect
    // discovery order — harmless, since scan_libraries always re-sorts by title anyway.
    let mut shows: HashMap<String, Show> = HashMap::new();

    for file in &files {
        let rel = file.strip_prefix(root).unwrap();
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let file_name = parts.last().unwrap().clone();
        let ep = parse::parse_episode_name(&file_name);

        struct ShowInfo {
            title: String,
            year: Option<i32>,
            season: Option<i32>,
        }
        let (show_dir, show_info) = if parts.len() > 1 {
            let sf = parse::parse_show_folder(&parts[0]);
            (root.join(&parts[0]), ShowInfo { title: sf.title, year: sf.year, season: sf.season })
        } else {
            // Loose files in the library root: group by the show name in the filename.
            let title = ep.as_ref().and_then(|e| e.show.clone()).unwrap_or_else(|| parse::parse_movie_name(&file_name).title);
            (root.to_path_buf(), ShowInfo { title, year: None, season: None })
        };

        // Group by title rather than folder so season packs ("Show S01 ...", "Show S02 ...") merge
        // into one show.
        let key = format!("{}::{}", root.to_string_lossy().to_lowercase(), parse::normalize_key(&show_info.title));

        if !shows.contains_key(&key) {
            let mut show = Show {
                id: make_id("s", &key),
                title: show_info.title.clone(),
                year: show_info.year,
                dir: show_dir.clone(),
                poster: None,
                backdrop: None,
                added_at: 0,
                episodes: Vec::new(),
            };
            if parts.len() > 1 {
                let art = local_art(&show_dir, None, cache);
                show.poster = art.poster;
                show.backdrop = art.backdrop;
            }
            shows.insert(key.clone(), show);
        } else if parts.len() > 1 && shows.get(&key).unwrap().poster.is_none() {
            let art = local_art(&show_dir, None, cache);
            let show = shows.get_mut(&key).unwrap();
            show.poster = art.poster;
            show.backdrop = art.backdrop;
        }
        let show = shows.get_mut(&key).unwrap();
        if show.year.is_none() {
            if let Some(y) = show_info.year {
                show.year = Some(y);
            }
        }

        // Season: filename marker wins, then the nearest "Season N" folder, then 1.
        let mut season = ep.as_ref().map(|e| e.season);
        if season.is_none() && parts.len() >= 3 {
            for i in (1..=parts.len() - 2).rev() {
                if let Some(s) = parse::parse_season_folder(&parts[i]) {
                    season = Some(s);
                    break;
                }
            }
        }
        if season.is_none() {
            season = show_info.season;
        }
        let season = season.unwrap_or(1);

        let st = fs::metadata(file).ok();
        let added_at = st.as_ref().map(mtime_millis).unwrap_or(0);
        show.added_at = show.added_at.max(added_at);

        let ep_dir = file.parent().unwrap();
        let thumb_files = cache.list_files(ep_dir);
        let ep_base = parse::strip_extension(&file_name).to_string();
        let thumb = find_image(&thumb_files, &[format!("{ep_base}-thumb"), ep_base.clone()]);

        let ep_title = match &ep {
            Some(e) if e.title.is_some() => e.title.clone(),
            Some(_) => None,
            None => Some(dots_underscores_to_space(&ep_base)),
        };

        show.episodes.push(Episode {
            id: make_id("e", &file.to_string_lossy()),
            season,
            episode: ep.as_ref().map(|e| e.episode),
            episode_end: ep.as_ref().and_then(|e| e.episode_end),
            title: ep_title,
            path: file.clone(),
            thumb: thumb.map(|t| ep_dir.join(t)),
            size: st.as_ref().map(|m| m.len()).unwrap_or(0),
            added_at,
        });
    }

    for show in shows.values_mut() {
        show.episodes.sort_by(|a, b| {
            a.season
                .cmp(&b.season)
                .then_with(|| a.episode.unwrap_or(i32::MAX).cmp(&b.episode.unwrap_or(i32::MAX)))
                .then_with(|| locale::natural_cmp(&a.path.to_string_lossy(), &b.path.to_string_lossy()))
        });
        // Number unmarked episodes by their position within the season.
        let mut counters: HashMap<i32, i32> = HashMap::new();
        for e in show.episodes.iter_mut() {
            let n = counters.get(&e.season).copied().unwrap_or(0) + 1;
            counters.insert(e.season, e.episode.unwrap_or(n));
            if e.episode.is_none() {
                e.episode = Some(n);
            }
        }
    }
    shows.into_values().collect()
}

pub enum LibraryKind {
    Movies,
    Tv,
}

pub struct LibraryDef<'a> {
    pub path: &'a Path,
    pub kind: LibraryKind,
}

pub struct LibraryScan {
    pub movies: Vec<Movie>,
    pub shows: Vec<Show>,
    pub scanned_at: i64,
}

/// Scan every configured library folder.
pub fn scan_libraries(libraries: &[LibraryDef]) -> LibraryScan {
    let mut cache = DirCache::new();
    let mut movies = Vec::new();
    let mut shows = Vec::new();
    for lib in libraries {
        let is_dir = fs::metadata(lib.path).map(|m| m.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        match lib.kind {
            LibraryKind::Tv => shows.extend(scan_shows(lib.path, &mut cache)),
            LibraryKind::Movies => movies.extend(scan_movies(lib.path, &mut cache)),
        }
    }
    movies.sort_by(|a, b| locale::compare_base_numeric(&a.title, &b.title));
    shows.sort_by(|a, b| locale::compare_base_numeric(&a.title, &b.title));
    LibraryScan { movies, shows, scanned_at: now_millis() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn tree(root: &Path, files: &[&str]) {
        for f in files {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
        }
    }

    // Parity with the JS test "scans movies and shows with local artwork" (test/library.test.js).
    #[test]
    fn scans_movies_and_shows_with_local_artwork() {
        let root = tempdir().unwrap();
        let movies_dir = root.path().join("Movies");
        let tv_dir = root.path().join("TV");
        tree(
            &movies_dir,
            &[
                "Heat (1995)/Heat.1995.1080p.mkv",
                "Heat (1995)/poster.jpg",
                "Heat (1995)/fanart.jpg",
                "Heat (1995)/Sample/sample.mkv",
                "Loose/The.Matrix.1999.mkv",
                "Loose/The.Matrix.1999-poster.jpg",
                "Loose/Alien.1979.mkv",
                "Loose/poster.jpg",
            ],
        );
        tree(
            &tv_dir,
            &[
                "Dark (2017)/poster.jpg",
                "Dark (2017)/Season 1/Dark.S01E02.mkv",
                "Dark (2017)/Season 1/Dark.S01E01.mkv",
                "Dark (2017)/Season 2/Dark.S02E01.mkv",
                "Dark (2017)/Season 2/Dark.S02E01.srt",
                "Planet Earth/Season 1/01 From Pole to Pole.mkv",
                "Planet Earth/Season 1/02 Mountains.mkv",
                "Planet Earth/Specials/Making Of.mkv",
                "Fallout S01 MULTi 1080p WEB x265-GRP/Fallout.S01E01.MULTi.1080p.WEB.x265-GRP.mkv",
                "Fallout S02 MULTi VFF 1080p WEBrip 10 bits x265-Tyrell/Fallout.S02E01.MULTi.VFF.1080p.mkv",
                "Fallout S02 MULTi VFF 1080p WEBrip 10 bits x265-Tyrell/Fallout.S02E02.MULTi.VFF.1080p.mkv",
                "Fallout S02 MULTi VFF 1080p WEBrip 10 bits x265-Tyrell/poster.jpg",
            ],
        );

        let libs = [
            LibraryDef { path: &movies_dir, kind: LibraryKind::Movies },
            LibraryDef { path: &tv_dir, kind: LibraryKind::Tv },
            LibraryDef { path: &root.path().join("missing"), kind: LibraryKind::Movies },
        ];
        let lib = scan_libraries(&libs);

        let titles_years: Vec<(String, Option<i32>)> = lib.movies.iter().map(|m| (m.title.clone(), m.year)).collect();
        assert_eq!(titles_years, vec![("Alien".into(), Some(1979)), ("Heat".into(), Some(1995)), ("The Matrix".into(), Some(1999))]);

        let heat = lib.movies.iter().find(|m| m.title == "Heat").unwrap();
        assert_eq!(heat.poster.as_ref().unwrap().file_name().unwrap(), "poster.jpg");
        assert_eq!(heat.backdrop.as_ref().unwrap().file_name().unwrap(), "fanart.jpg");

        let matrix = lib.movies.iter().find(|m| m.title == "The Matrix").unwrap();
        assert_eq!(matrix.poster.as_ref().unwrap().file_name().unwrap(), "The.Matrix.1999-poster.jpg");

        // A generic poster.jpg in a shared folder must not be attributed to an arbitrary film.
        assert!(lib.movies.iter().find(|m| m.title == "Alien").unwrap().poster.is_none());

        let show_titles: Vec<&str> = lib.shows.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(show_titles, vec!["Dark", "Fallout", "Planet Earth"]);

        let dark = &lib.shows[0];
        let fallout = &lib.shows[1];
        assert_eq!(fallout.episodes.iter().map(|e| (e.season, e.episode)).collect::<Vec<_>>(), vec![(1, Some(1)), (2, Some(1)), (2, Some(2))]);
        assert!(fallout.poster.as_ref().unwrap().ends_with("poster.jpg"));
        assert_eq!(dark.year, Some(2017));
        assert!(dark.poster.as_ref().unwrap().ends_with("poster.jpg"));
        assert_eq!(dark.episodes.iter().map(|e| (e.season, e.episode)).collect::<Vec<_>>(), vec![(1, Some(1)), (1, Some(2)), (2, Some(1))]);

        let earth = &lib.shows[2];
        assert_eq!(earth.episodes.iter().map(|e| (e.season, e.episode)).collect::<Vec<_>>(), vec![(0, Some(1)), (1, Some(1)), (1, Some(2))]);
    }
}
