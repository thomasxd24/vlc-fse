//! Pure filename/folder parsing helpers. Kept free of filesystem/process access so they're easy to
//! unit test — a direct port of `src/parse.js`; see that file for the original comments/rationale.
//!
//! Uses `fancy-regex` rather than the `regex` crate because several patterns here rely on lookahead
//! (`(?=...)`) to check what follows a match without consuming it, which `regex` deliberately doesn't
//! support (it guarantees linear-time matching; `fancy-regex` allows backtracking for the rare patterns
//! that need it, same trade-off JS's own regex engine makes).

use fancy_regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

pub static VIDEO_EXTENSIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        ".mkv", ".mp4", ".m4v", ".avi", ".mov", ".wmv", ".mpg", ".mpeg", ".ts", ".m2ts", ".webm", ".flv", ".vob", ".ogv", ".3gp", ".divx", ".iso",
    ]
    .into_iter()
    .collect()
});

// Release-name noise that marks the end of a title.
const JUNK_TOKENS: &[&str] = &[
    "2160p", "1080p", "1080i", "720p", "576p", "480p", "4k", "uhd", "hdr", "hdr10", "dv", "dolby", "bluray", "blu-ray", "bdrip", "brrip", "bdremux", "remux", "webrip", "web-dl",
    "webdl", "web", "hdtv", "dvdrip", "dvdscr", "dvd", "hdrip", "x264", "x265", "h264", "h265", "hevc", "avc", "xvid", "divx", "aac", "ac3", "dts", "ddp5", "dd5", "atmos", "truehd",
    "flac", "10bit", "8bit", "proper", "repack", "extended", "unrated", "remastered", "directors", "imax", "multi", "subbed", "dubbed", "internal", "limited", "vff", "vfq", "vfi",
    "vf2", "vostfr", "truefrench", "french", "complete", "integrale", "intégrale",
];

static JUNK_RE: LazyLock<Regex> = LazyLock::new(|| {
    let alts = JUNK_TOKENS.iter().map(|t| fancy_regex::escape(t)).collect::<Vec<_>>().join("|");
    Regex::new(&format!(r"(?i)(^|[\s._\-\[(])({alts})(?=$|[\s._\-\])])")).unwrap()
});

static EPISODE_PATTERNS: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        // S01E02, S01E02E03, S01E02-E03, s1e2
        Regex::new(r"(?i)(?:^|[\s._\-\[(])s(\d{1,2})[\s._\-]?e(\d{1,3})(?:-?e(\d{1,3})|-(\d{1,3})(?=$|[\s._\-\])]))?").unwrap(),
        // 1x02
        Regex::new(r"(?i)(?:^|[\s._\-\[(])(\d{1,2})x(\d{2,3})(?=$|[\s._\-\])])").unwrap(),
        // Season 1 Episode 2
        Regex::new(r"(?i)season[\s._\-]*(\d{1,2})[\s._\-]*episode[\s._\-]*(\d{1,3})").unwrap(),
    ]
});

static BRACKET_TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\[[^\]]*\]\s*").unwrap());
static DOTS_UNDERSCORES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[._]+").unwrap());
static EMPTY_BRACKETS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\[(]\s*[\])]\s*").unwrap());
static TRAILING_JUNK_CHARS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s\-\[(]+$").unwrap());
static EDGE_DASH_SPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\s\-]+|[\s\-]+$").unwrap());
static MULTI_SPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").unwrap());
static WORD_START_LOWER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([a-z])").unwrap());
static YEAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[\s._\-\[(])((?:19|20)\d{2})(?=$|[\s._\-\])])").unwrap());
static SPECIALS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^specials?$").unwrap());
static SEASON_FOLDER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:season|series|saison|staffel|temporada|s)[\s._\-]*(\d{1,3})\b").unwrap());
// "S02", "Season 2", "Saison 2" etc. inside a release-style folder name, e.g. "Fallout.S02.1080p.WEBrip-GRP".
static SEASON_IN_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[\s._\-\[(])(?:s(\d{1,2})(?:e\d{1,3})?|(?:season|saison|series|staffel|temporada)[\s._\-]*(\d{1,2}))(?=$|[\s._\-\])])").unwrap());
static SHOW_YEAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*?)[\s._]*[(\[]?((?:19|20)\d{2})[)\]]?\s*$").unwrap());

pub fn is_video_file(name: &str) -> bool {
    match name.rfind('.') {
        Some(dot) => VIDEO_EXTENSIONS.contains(name[dot..].to_lowercase().as_str()),
        None => false,
    }
}

pub fn strip_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    }
}

fn tidy(s: &str) -> String {
    let s = DOTS_UNDERSCORES_RE.replace_all(s, " ");
    let s = EMPTY_BRACKETS_RE.replace_all(&s, " ");
    let s = TRAILING_JUNK_CHARS_RE.replace(&s, "");
    let s = EDGE_DASH_SPACE_RE.replace_all(&s, "");
    let s = MULTI_SPACE_RE.replace_all(&s, " ");
    s.trim().to_string()
}

fn title_case(s: &str) -> String {
    // Only fix titles that are entirely lower case (common for scene releases).
    if s != s.to_lowercase() {
        return s.to_string();
    }
    WORD_START_LOWER_RE.replace_all(s, |caps: &fancy_regex::Captures<'_, str>| caps[1].to_uppercase()).into_owned()
}

/// Strip a leading bracketed release-group tag, e.g. "[YTS] Movie" -> "Movie".
fn strip_bracket_tag(s: &str) -> std::borrow::Cow<'_, str> {
    BRACKET_TAG_RE.replace(s, "")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieName {
    pub title: String,
    pub year: Option<i32>,
}

/// Parse a movie file or folder name.
/// "The.Matrix.1999.1080p.BluRay.x264" -> { title: "The Matrix", year: Some(1999) }
/// "Heat (1995)" -> { title: "Heat", year: Some(1995) }
pub fn parse_movie_name(raw: &str) -> MovieName {
    let stripped = strip_extension(raw);
    // Drop bracketed release-group tags at the start, e.g. "[YTS] Movie".
    let name = strip_bracket_tag(stripped);
    let name = name.as_ref();

    let mut year: Option<i32> = None;
    let mut title: String = name.to_string();

    // Pick the last plausible year that is not the very first token (so "2001 A Space Odyssey 1968" works).
    let mut chosen: Option<(i32, usize)> = None;
    for caps in YEAR_RE.captures_iter(name) {
        let caps = caps.unwrap();
        let year_m = caps.get(1).unwrap();
        let whole_start = caps.get(0).unwrap().start();
        let start = whole_start + (year_m.start() - whole_start);
        if start == 0 {
            continue;
        }
        chosen = Some((year_m.as_str().parse().unwrap(), start));
    }

    if let Some((y, start)) = chosen {
        year = Some(y);
        title = name[..start].to_string();
    } else if let Ok(Some(junk)) = JUNK_RE.find(name) {
        if junk.start() > 0 {
            title = name[..junk.start()].to_string();
        }
    }

    let tidied_title = tidy(&title);
    let title = if tidied_title.is_empty() { tidy(name) } else { title_case(&tidied_title) };
    MovieName { title, year }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeName {
    pub show: Option<String>,
    pub season: i32,
    pub episode: i32,
    pub episode_end: Option<i32>,
    pub title: Option<String>,
}

/// Parse an episode filename. Returns `None` if no season/episode marker is found.
/// "Breaking.Bad.S02E05.720p.mkv" -> show "Breaking Bad", season 2, episode 5, title None
pub fn parse_episode_name(raw: &str) -> Option<EpisodeName> {
    let name = strip_extension(raw);
    for re in EPISODE_PATTERNS.iter() {
        let caps = match re.captures(name) {
            Ok(Some(c)) => c,
            _ => continue,
        };
        let whole = caps.get(0).unwrap();
        let season: i32 = caps.get(1).unwrap().as_str().parse().unwrap();
        let episode: i32 = caps.get(2).unwrap().as_str().parse().unwrap();
        let episode_end_raw = caps
            .get(3)
            .or_else(|| caps.get(4))
            .and_then(|m| m.as_str().parse::<i32>().ok());

        let before = &name[..whole.start()];
        let mut after = &name[whole.end()..];
        if let Ok(Some(junk)) = JUNK_RE.find(after) {
            after = &after[..junk.start()];
        }
        let before_no_tag = strip_bracket_tag(before);
        let show = {
            let t = title_case(&tidy(&before_no_tag));
            if t.is_empty() { None } else { Some(t) }
        };
        let after_trimmed = after.trim_start_matches([' ', '.', '_', '-']);
        let ep_title = {
            let t = tidy(after_trimmed);
            if t.is_empty() { None } else { Some(t) }
        };
        return Some(EpisodeName {
            show,
            season,
            episode,
            episode_end: episode_end_raw.filter(|e| *e > episode),
            title: ep_title,
        });
    }
    None
}

/// "Season 01", "S2", "Series 3", "Specials" -> season number (0 for specials) or `None`.
pub fn parse_season_folder(raw: &str) -> Option<i32> {
    let trimmed = raw.trim();
    if SPECIALS_RE.is_match(trimmed).unwrap_or(false) {
        return Some(0);
    }
    match SEASON_FOLDER_RE.captures(trimmed) {
        Ok(Some(caps)) => caps.get(1).and_then(|m| m.as_str().parse().ok()),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowFolder {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
}

/// Parse a TV show folder name: "The Office (US) (2005)", "Dark", or a season-pack release name like
/// "Fallout S02 MULTi VFF 1080p WEBrip x265-GRP" (-> title "Fallout", season 2).
pub fn parse_show_folder(raw: &str) -> ShowFolder {
    let mut name: String = strip_bracket_tag(raw).into_owned();
    let mut season = None;

    if let Ok(Some(caps)) = SEASON_IN_NAME_RE.captures(&name) {
        let whole = caps.get(0).unwrap();
        if whole.start() > 0 {
            season = caps.get(1).or_else(|| caps.get(2)).and_then(|m| m.as_str().parse().ok());
            name.truncate(whole.start());
        }
    }

    if let Ok(Some(junk)) = JUNK_RE.find(&name) {
        if junk.start() > 0 {
            name.truncate(junk.start());
        }
    }

    let mut year = None;
    let mut title = name.clone();
    if let Ok(Some(caps)) = SHOW_YEAR_RE.captures(&name) {
        let head = caps.get(1).unwrap().as_str();
        if !tidy(head).is_empty() {
            year = caps.get(2).and_then(|m| m.as_str().parse().ok());
            title = head.to_string();
        }
    }

    let tidied = tidy(&title);
    let title = if tidied.is_empty() { tidy(raw) } else { title_case(&tidied) };
    ShowFolder { title, year, season }
}

/// Case/punctuation-insensitive key used to group episodes into shows.
pub fn normalize_key(s: &str) -> String {
    let lower = s.to_lowercase();
    let decomposed: String = lower.nfkd().collect();
    let no_marks: String = decomposed.chars().filter(|c| !('\u{0300}'..='\u{036f}').contains(c)).collect();
    let ampersand_replaced = no_marks.replace('&', "and");
    let mut out = String::with_capacity(ampersand_replaced.len());
    let mut last_was_space = false;
    for c in ampersand_replaced.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(title: &str, year: Option<i32>) -> MovieName {
        MovieName { title: title.to_string(), year }
    }

    // Parity with the JS test "movie names" (test/parse.test.js).
    #[test]
    fn movie_names() {
        let cases: &[(&str, &str, Option<i32>)] = &[
            ("The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv", "The Matrix", Some(1999)),
            ("Heat (1995).mkv", "Heat", Some(1995)),
            ("Blade Runner 2049 (2017) [2160p].mkv", "Blade Runner 2049", Some(2017)),
            ("2001.A.Space.Odyssey.1968.mkv", "2001 A Space Odyssey", Some(1968)),
            ("1917 (2019).mp4", "1917", Some(2019)),
            ("[YTS] Parasite 2019 720p.mp4", "Parasite", Some(2019)),
            ("amelie.mkv", "Amelie", None),
            ("Some.Movie.1080p.WEB-DL.mkv", "Some Movie", None),
            ("Spider-Man Into the Spider-Verse.mkv", "Spider-Man Into the Spider-Verse", None),
        ];
        for (input, title, year) in cases {
            assert_eq!(parse_movie_name(input), movie(title, *year), "input: {input}");
        }
    }

    // Parity with the JS test "episode names".
    #[test]
    fn episode_names() {
        assert_eq!(
            parse_episode_name("Breaking.Bad.S02E05.720p.HDTV.mkv"),
            Some(EpisodeName { show: Some("Breaking Bad".into()), season: 2, episode: 5, episode_end: None, title: None })
        );
        assert_eq!(
            parse_episode_name("The Office (US) - s03e10 - A Benihana Christmas.mkv"),
            Some(EpisodeName { show: Some("The Office (US)".into()), season: 3, episode: 10, episode_end: None, title: Some("A Benihana Christmas".into()) })
        );
        assert_eq!(
            parse_episode_name("Show.S01E01E02.mkv"),
            Some(EpisodeName { show: Some("Show".into()), season: 1, episode: 1, episode_end: Some(2), title: None })
        );
        assert_eq!(
            parse_episode_name("Show - 4x07 - Title.avi"),
            Some(EpisodeName { show: Some("Show".into()), season: 4, episode: 7, episode_end: None, title: Some("Title".into()) })
        );
        assert_eq!(parse_episode_name("S01E03.mkv"), Some(EpisodeName { show: None, season: 1, episode: 3, episode_end: None, title: None }));
        assert_eq!(parse_episode_name("The.Matrix.1999.mkv"), None);
        // 1080p must not be read as 10x80.
        assert_eq!(parse_episode_name("Movie.1080p.mkv"), None);
    }

    // Parity with the JS test "season and show folders".
    #[test]
    fn season_and_show_folders() {
        assert_eq!(parse_season_folder("Season 01"), Some(1));
        assert_eq!(parse_season_folder("S2"), Some(2));
        assert_eq!(parse_season_folder("Series 3"), Some(3));
        assert_eq!(parse_season_folder("Specials"), Some(0));
        assert_eq!(parse_season_folder("Extras"), None);

        assert_eq!(parse_show_folder("The Office (US) (2005)"), ShowFolder { title: "The Office (US)".into(), year: Some(2005), season: None });
        assert_eq!(parse_show_folder("Dark"), ShowFolder { title: "Dark".into(), year: None, season: None });
        assert_eq!(
            parse_show_folder("Fallout S02 MULTi VFF 1080p WEBrip 10 bits x265-Tyrell"),
            ShowFolder { title: "Fallout".into(), year: None, season: Some(2) }
        );
        assert_eq!(parse_show_folder("Fallout.S01.MULTi.1080p.AMZN.WEB-DL"), ShowFolder { title: "Fallout".into(), year: None, season: Some(1) });
        assert_eq!(
            parse_show_folder("The.Last.of.Us.2023.S01.2160p.WEB"),
            ShowFolder { title: "The Last of Us".into(), year: Some(2023), season: Some(1) }
        );
        assert_eq!(parse_show_folder("Shogun.2024.Saison.1.FRENCH.1080p"), ShowFolder { title: "Shogun".into(), year: Some(2024), season: Some(1) });
        assert_eq!(parse_show_folder("Dark.COMPLETE.1080p.NF.WEB-DL"), ShowFolder { title: "Dark".into(), year: None, season: None });
        assert_eq!(parse_show_folder("9-1-1 (2018)"), ShowFolder { title: "9-1-1".into(), year: Some(2018), season: None });
    }

    // Parity with the JS test "video extensions".
    #[test]
    fn video_extensions() {
        assert!(is_video_file("a.MKV"));
        assert!(!is_video_file("a.srt"));
        assert!(!is_video_file("mkv"));
    }

    // normalize_key has no dedicated JS test, but is exercised indirectly via library.js/metadata.js/
    // gameinfo.js grouping keys — covered directly here instead.
    #[test]
    fn normalize_key_folds_case_diacritics_and_punctuation() {
        assert_eq!(normalize_key("Amélie"), "amelie");
        assert_eq!(normalize_key("Rock & Roll"), "rock and roll");
        assert_eq!(normalize_key("  Multiple   Spaces--and--dashes!! "), "multiple spaces and dashes");
        assert_eq!(normalize_key(""), "");
    }
}
