//! Where downloaded files go: decides whether a remote file or folder is a film or a show, and lays it
//! out the way the library scanner reads best ("Movies/Title (Year)/…", "TV/Show/Season 02/…"). Pure —
//! no filesystem/network access — so it's a direct, fully-tested port of `src/transfer-plan.js`.
//!
//! Remote relative paths (`rel`) are always `/`-separated regardless of platform (matching the JS
//! version's explicit use of `path.posix` for them); destination paths use whatever separator this is
//! compiled for, same as the JS version's plain (platform-native) `path` module.

use crate::parse;
use fancy_regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static SUB_EXTENSIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| [".srt", ".ass", ".ssa", ".sub", ".idx", ".vtt", ".sup"].into_iter().collect());
static SAMPLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(^|[\s._\-])(sample|trailer)([\s._\-]|$)").unwrap());
static IGNORED_DIR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(sample|samples|extras?|featurettes?|trailers?)$").unwrap());

fn posix_basename(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

fn posix_dirname(rel: &str) -> &str {
    match rel.rfind('/') {
        Some(i) => &rel[..i],
        None => ".",
    }
}

fn ext(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i..].to_lowercase(),
        _ => String::new(),
    }
}

fn is_sub(name: &str) -> bool {
    SUB_EXTENSIONS.contains(ext(name).as_str())
}

fn is_reserved_windows_name(s: &str) -> bool {
    let lower = s.to_lowercase();
    for r in ["con", "prn", "aux", "nul"] {
        if lower == r || lower.starts_with(&format!("{r}.")) {
            return true;
        }
    }
    for prefix in ["com", "lpt"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let mut chars = rest.chars();
            if let Some(d) = chars.next() {
                if d.is_ascii_digit() {
                    let after = chars.as_str();
                    if after.is_empty() || after.starts_with('.') {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Make a name safe as a Windows file or folder name (and never a path).
pub fn safe_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_space = false;
    for c in name.chars() {
        let collapses = matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20 || c.is_whitespace();
        if collapses {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            last_was_space = false;
        }
    }
    let s = out.trim().trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() || s.chars().all(|c| c == '.') {
        return "_".to_string();
    }
    if is_reserved_windows_name(&s) {
        format!("_{s}")
    } else {
        s
    }
}

#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub remote: String,
    /// `/`-separated, relative to the selection.
    pub rel: String,
    pub size: u64,
}

/// Keep the video files and subtitles worth copying; drop samples, trailers and extras.
fn relevant(files: &[RemoteFile]) -> Vec<&RemoteFile> {
    files
        .iter()
        .filter(|f| {
            let parts: Vec<&str> = f.rel.split('/').collect();
            if parts[..parts.len().saturating_sub(1)].iter().any(|d| IGNORED_DIR_RE.is_match(d).unwrap_or(false)) {
                return false;
            }
            let name = parts[parts.len() - 1];
            if parse::is_video_file(name) {
                !SAMPLE_RE.is_match(parse::strip_extension(name)).unwrap_or(false)
            } else {
                is_sub(name)
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Movie,
    Tv,
}

/// Film or show? Episode markers in the files, or a season in the folder name, mean a show.
fn detect_kind(selection_name: &str, files: &[&RemoteFile]) -> Kind {
    let videos: Vec<&&RemoteFile> = files.iter().filter(|f| parse::is_video_file(&f.rel)).collect();
    if videos.iter().any(|f| parse::parse_episode_name(posix_basename(&f.rel)).is_some()) {
        return Kind::Tv;
    }
    if parse::parse_show_folder(selection_name).season.is_some() {
        return Kind::Tv;
    }
    let any_season_dir = files.iter().any(|f| {
        let parts: Vec<&str> = f.rel.split('/').collect();
        parts[..parts.len().saturating_sub(1)].iter().any(|d| parse::parse_season_folder(d).is_some())
    });
    if any_season_dir {
        return Kind::Tv;
    }
    Kind::Movie
}

/// The video a subtitle belongs to: the one whose name it starts with ("Film.mkv" ← "Film.en.srt"), if any.
fn owner_of<'a>(sub: &RemoteFile, videos: &[&'a RemoteFile]) -> Option<&'a RemoteFile> {
    let base = parse::strip_extension(posix_basename(&sub.rel)).to_lowercase();
    let dir = posix_dirname(&sub.rel);
    let mut best: Option<(&RemoteFile, usize)> = None;
    for &v in videos {
        let vb = parse::strip_extension(posix_basename(&v.rel)).to_lowercase();
        if base.starts_with(&vb) && best.map(|(_, len)| vb.len() > len).unwrap_or(true) {
            best = Some((v, vb.len()));
        }
    }
    if let Some((v, _)) = best {
        return Some(v);
    }
    // "Subs/English.srt" next to a single film belongs to that film.
    let near: Vec<&&RemoteFile> = videos
        .iter()
        .filter(|v| {
            let vd = posix_dirname(&v.rel);
            dir == vd || dir.starts_with(&format!("{vd}/")) || vd == "."
        })
        .collect();
    if near.len() == 1 {
        Some(near[0])
    } else if videos.len() == 1 {
        Some(videos[0])
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub remote: String,
    pub rel: String,
    pub size: u64,
    pub dest: PathBuf,
}

pub struct Selection<'a> {
    pub name: &'a str,
    pub is_dir: bool,
    pub parent: Option<&'a str>,
}

fn plan_movies(selection: &Selection, files: &[&RemoteFile], root: &Path) -> Vec<PlanItem> {
    let videos: Vec<&RemoteFile> = files.iter().filter(|f| parse::is_video_file(&f.rel)).copied().collect();
    let subs: Vec<&RemoteFile> = files.iter().filter(|f| is_sub(&f.rel)).copied().collect();
    let folder_info = if selection.is_dir { Some(parse::parse_movie_name(selection.name)) } else { None };
    let mut items = Vec::new();
    let mut folder_for: HashMap<String, String> = HashMap::new();

    for v in &videos {
        let from_file = parse::parse_movie_name(posix_basename(&v.rel));
        // A folder holding one film usually has the cleaner name ("Heat (1995)"), unless the file has the year.
        let info = if videos.len() == 1 {
            match &folder_info {
                Some(fi) if fi.year.is_some() || from_file.year.is_none() => fi.clone(),
                _ => from_file.clone(),
            }
        } else {
            from_file.clone()
        };
        let folder_name = match info.year {
            Some(y) => format!("{} ({y})", info.title),
            None => info.title.clone(),
        };
        let folder = safe_name(&folder_name);
        folder_for.insert(v.rel.clone(), folder.clone());
        items.push(PlanItem { remote: v.remote.clone(), rel: v.rel.clone(), size: v.size, dest: root.join(&folder).join(safe_name(posix_basename(&v.rel))) });
    }
    for s in &subs {
        if let Some(owner) = owner_of(s, &videos) {
            if let Some(folder) = folder_for.get(&owner.rel) {
                items.push(PlanItem { remote: s.remote.clone(), rel: s.rel.clone(), size: s.size, dest: root.join(folder).join(safe_name(posix_basename(&s.rel))) });
            }
        }
    }
    items
}

/// Name of the show's folder: an existing one with the same title if there is one, so episodes merge in.
fn show_folder(title: &str, year: Option<i32>, existing_dirs: &[String]) -> String {
    let key = parse::normalize_key(title);
    if let Some(hit) = existing_dirs.iter().find(|d| parse::normalize_key(&parse::parse_show_folder(d).title) == key) {
        return hit.clone();
    }
    safe_name(&match year {
        Some(y) => format!("{title} ({y})"),
        None => title.to_string(),
    })
}

fn plan_shows(selection: &Selection, files: &[&RemoteFile], root: &Path, existing_dirs: &[String]) -> Vec<PlanItem> {
    let sel_source = if selection.is_dir { selection.name.to_string() } else { parse::strip_extension(selection.name).to_string() };
    let sel_info = parse::parse_show_folder(&sel_source);
    let videos: Vec<&RemoteFile> = files.iter().filter(|f| parse::is_video_file(&f.rel)).copied().collect();
    let subs: Vec<&RemoteFile> = files.iter().filter(|f| is_sub(&f.rel)).copied().collect();
    let mut items = Vec::new();
    let mut place_of: HashMap<String, PathBuf> = HashMap::new();

    let place = |f: &RemoteFile| -> PathBuf {
        let name = posix_basename(&f.rel);
        let all_parts: Vec<&str> = f.rel.split('/').collect();
        let dirs: Vec<&str> = all_parts[..all_parts.len() - 1].to_vec();
        let ep = parse::parse_episode_name(name);

        // Show name: from a folder that names it (the selected one, or the first below it), else from the file.
        let mut title: Option<String> = None;
        let mut year: Option<i32> = None;
        if selection.is_dir && !sel_info.title.is_empty() && parse::parse_season_folder(selection.name).is_none() {
            title = Some(sel_info.title.clone());
            year = sel_info.year;
        } else if !dirs.is_empty() && parse::parse_season_folder(dirs[0]).is_none() {
            let d = parse::parse_show_folder(dirs[0]);
            title = Some(d.title);
            year = d.year;
        }
        if title.is_none() && selection.is_dir {
            if let Some(parent) = selection.parent {
                if parse::parse_season_folder(selection.name).is_some() {
                    // A "Season 2" folder picked on its own: the show is the folder above it.
                    let p = parse::parse_show_folder(parent);
                    title = Some(p.title);
                    year = p.year;
                }
            }
        }
        let title = title.unwrap_or_else(|| ep.as_ref().and_then(|e| e.show.clone()).unwrap_or_else(|| parse::parse_movie_name(name).title));

        // Season: from the file, a "Season N" folder, or a season-pack folder name; specials land in season 0.
        let mut season = ep.as_ref().map(|e| e.season);
        if season.is_none() {
            for i in (0..dirs.len()).rev() {
                season = parse::parse_season_folder(dirs[i]);
                if season.is_none() {
                    season = parse::parse_show_folder(dirs[i]).season;
                }
                if season.is_some() {
                    break;
                }
            }
        }
        if season.is_none() && selection.is_dir {
            season = parse::parse_season_folder(selection.name).or(sel_info.season);
        }
        let season = season.unwrap_or(1);
        let season_dir = if season == 0 { "Specials".to_string() } else { format!("Season {season:02}") };
        root.join(show_folder(&title, year, existing_dirs)).join(season_dir)
    };

    for v in &videos {
        let dir = place(v);
        place_of.insert(v.rel.clone(), dir.clone());
        items.push(PlanItem { remote: v.remote.clone(), rel: v.rel.clone(), size: v.size, dest: dir.join(safe_name(posix_basename(&v.rel))) });
    }
    for s in &subs {
        let own = if parse::parse_episode_name(posix_basename(&s.rel)).is_some() { Some(place(s)) } else { owner_of(s, &videos).and_then(|o| place_of.get(&o.rel).cloned()) };
        if let Some(dir) = own {
            items.push(PlanItem { remote: s.remote.clone(), rel: s.rel.clone(), size: s.size, dest: dir.join(safe_name(posix_basename(&s.rel))) });
        }
    }
    items
}

#[derive(Default)]
pub struct PlanOptions<'a> {
    pub kind: Option<Kind>,
    pub movie_root: Option<&'a Path>,
    pub tv_root: Option<&'a Path>,
    pub existing_show_dirs: &'a [String],
}

pub struct Plan {
    pub kind: Kind,
    pub root: Option<PathBuf>,
    pub items: Vec<PlanItem>,
    pub total_size: u64,
    pub folders: Vec<PathBuf>,
}

/// Plan a download: `selection` is the remote file or folder picked, `files` every file in it.
pub fn plan_transfer(selection: &Selection, files: &[RemoteFile], opts: &PlanOptions) -> Plan {
    let keep = relevant(files);
    let kind = opts.kind.unwrap_or_else(|| detect_kind(selection.name, &keep));
    let root = match kind {
        Kind::Tv => opts.tv_root,
        Kind::Movie => opts.movie_root,
    };
    let Some(root) = root else {
        return Plan { kind, root: None, items: vec![], total_size: 0, folders: vec![] };
    };
    let mut items = match kind {
        Kind::Tv => plan_shows(selection, &keep, root, opts.existing_show_dirs),
        Kind::Movie => plan_movies(selection, &keep, root),
    };
    items.sort_by(|a, b| a.dest.cmp(&b.dest));
    let total_size: u64 = items.iter().map(|i| i.size).sum();
    let mut folders: Vec<PathBuf> = Vec::new();
    for i in &items {
        if let Some(parent) = i.dest.parent() {
            let parent = parent.to_path_buf();
            if !folders.contains(&parent) {
                folders.push(parent);
            }
        }
    }
    Plan { kind, root: Some(root.to_path_buf()), items, total_size, folders }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> PathBuf {
        Path::new("/lib").join("Movies")
    }
    fn t() -> PathBuf {
        Path::new("/lib").join("TV")
    }

    fn files(list: &[&str]) -> Vec<RemoteFile> {
        list.iter().map(|rel| RemoteFile { remote: format!("/remote/{rel}"), rel: rel.to_string(), size: 100 }).collect()
    }

    fn dests(plan: &Plan) -> Vec<String> {
        plan.items.iter().map(|i| i.dest.strip_prefix("/lib").unwrap().to_string_lossy().replace('\\', "/")).collect()
    }

    fn sel<'a>(name: &'a str, is_dir: bool) -> Selection<'a> {
        Selection { name, is_dir, parent: None }
    }

    // Parity with "a film folder goes to Movies/Title (Year) with its subtitles, without samples".
    #[test]
    fn a_film_folder_goes_to_movies_title_year_with_subtitles_without_samples() {
        let root_m = m();
        let root_t = t();
        let plan = plan_transfer(
            &sel("Heat.1995.1080p.BluRay.x264-GRP", true),
            &files(&["Heat.1995.1080p.BluRay.x264-GRP.mkv", "Heat.1995.1080p.BluRay.x264-GRP.en.srt", "Subs/French.srt", "Sample/sample.mkv", "heat.nfo"]),
            &PlanOptions { movie_root: Some(&root_m), tv_root: Some(&root_t), ..Default::default() },
        );
        assert!(matches!(plan.kind, Kind::Movie));
        assert_eq!(dests(&plan), vec!["Movies/Heat (1995)/French.srt", "Movies/Heat (1995)/Heat.1995.1080p.BluRay.x264-GRP.en.srt", "Movies/Heat (1995)/Heat.1995.1080p.BluRay.x264-GRP.mkv"]);
        assert_eq!(plan.total_size, 300);
    }

    // Parity with "a single film file, and a folder of several films".
    #[test]
    fn a_single_film_file_and_a_folder_of_several_films() {
        let root_m = m();
        let root_t = t();
        let opts = PlanOptions { movie_root: Some(&root_m), tv_root: Some(&root_t), ..Default::default() };
        let one = plan_transfer(&sel("The.Matrix.1999.mkv", false), &files(&["The.Matrix.1999.mkv"]), &opts);
        assert_eq!(dests(&one), vec!["Movies/The Matrix (1999)/The.Matrix.1999.mkv"]);
        let many = plan_transfer(&sel("Films", true), &files(&["Alien.1979.mkv", "Aliens.1986.mkv", "Aliens.1986.srt"]), &opts);
        assert_eq!(dests(&many), vec!["Movies/Alien (1979)/Alien.1979.mkv", "Movies/Aliens (1986)/Aliens.1986.mkv", "Movies/Aliens (1986)/Aliens.1986.srt"]);
    }

    // Parity with "a season pack goes to TV/Show/Season NN and merges into an existing show folder".
    #[test]
    fn a_season_pack_merges_into_an_existing_show_folder() {
        let root_m = m();
        let root_t = t();
        let existing = vec!["Dark (2017)".to_string(), "Fallout (2024)".to_string()];
        let plan = plan_transfer(
            &sel("Fallout S02 MULTi VFF 1080p WEBrip x265-GRP", true),
            &files(&["Fallout.S02E01.MULTi.1080p.mkv", "Fallout.S02E02.MULTi.1080p.mkv", "Fallout.S02E02.MULTi.1080p.fr.srt"]),
            &PlanOptions { movie_root: Some(&root_m), tv_root: Some(&root_t), existing_show_dirs: &existing, ..Default::default() },
        );
        assert!(matches!(plan.kind, Kind::Tv));
        assert_eq!(
            dests(&plan),
            vec!["TV/Fallout (2024)/Season 02/Fallout.S02E01.MULTi.1080p.mkv", "TV/Fallout (2024)/Season 02/Fallout.S02E02.MULTi.1080p.fr.srt", "TV/Fallout (2024)/Season 02/Fallout.S02E02.MULTi.1080p.mkv"]
        );
        assert_eq!(plan.folders, vec![t().join("Fallout (2024)").join("Season 02")]);
    }

    // Parity with "a whole show with season folders, a lone season folder, and a single episode".
    #[test]
    fn a_whole_show_a_lone_season_folder_and_a_single_episode() {
        let root_m = m();
        let root_t = t();
        let opts = PlanOptions { movie_root: Some(&root_m), tv_root: Some(&root_t), ..Default::default() };
        let show = plan_transfer(&sel("Planet Earth (2006)", true), &files(&["Season 1/01 From Pole to Pole.mkv", "Season 2/01 Mountains.mkv", "Specials/Making Of.mkv"]), &opts);
        assert!(matches!(show.kind, Kind::Tv));
        assert_eq!(
            dests(&show),
            vec!["TV/Planet Earth (2006)/Season 01/01 From Pole to Pole.mkv", "TV/Planet Earth (2006)/Season 02/01 Mountains.mkv", "TV/Planet Earth (2006)/Specials/Making Of.mkv"]
        );
        let season = plan_transfer(&Selection { name: "Season 3", is_dir: true, parent: Some("Dark (2017)") }, &files(&["Dark.S03E01.mkv"]), &opts);
        assert_eq!(dests(&season), vec!["TV/Dark (2017)/Season 03/Dark.S03E01.mkv"]);
        let ep = plan_transfer(&sel("breaking.bad.s01e02.720p.mkv", false), &files(&["breaking.bad.s01e02.720p.mkv"]), &opts);
        assert_eq!(dests(&ep), vec!["TV/Breaking Bad/Season 01/breaking.bad.s01e02.720p.mkv"]);
    }

    // Parity with "the kind can be forced, and a missing library folder yields no plan".
    #[test]
    fn kind_can_be_forced_and_a_missing_library_folder_yields_no_plan() {
        let root_m = m();
        let root_t = t();
        let forced = plan_transfer(&sel("Home videos", true), &files(&["Holiday.mkv"]), &PlanOptions { kind: Some(Kind::Tv), movie_root: Some(&root_m), tv_root: Some(&root_t), ..Default::default() });
        assert_eq!(dests(&forced), vec!["TV/Home videos/Season 01/Holiday.mkv"]);
        let none = plan_transfer(&sel("Dark.S01E01.mkv", false), &files(&["Dark.S01E01.mkv"]), &PlanOptions { movie_root: Some(&root_m), ..Default::default() });
        assert_eq!(none.root, None);
        assert_eq!(none.items.len(), 0);
    }

    // Parity with "remote names can never escape the library or break Windows paths".
    #[test]
    fn remote_names_can_never_escape_the_library_or_break_windows_paths() {
        assert_eq!(safe_name(".."), "_");
        assert_eq!(safe_name("a/../../b"), "a .. .. b");
        assert_eq!(safe_name("What? Why: \"Now\" "), "What Why Now");
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("Title."), "Title");

        let root_m = m();
        let root_t = t();
        let evil = vec![RemoteFile { remote: "/r/..\\..\\evil.mkv".into(), rel: "..\\..\\evil.mkv".into(), size: 1 }];
        let plan = plan_transfer(&sel("x", true), &evil, &PlanOptions { movie_root: Some(&root_m), tv_root: Some(&root_t), ..Default::default() });
        for i in &plan.items {
            assert!(i.dest.starts_with(&root_m), "{:?} should stay under {:?}", i.dest, root_m);
        }
    }
}
