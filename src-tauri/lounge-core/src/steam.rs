//! Steam library scanning: finding the install, reading its VDF/ACF files, matching up local artwork.
//! Direct port of `src/steam.js`.
//!
//! This crate does blocking filesystem/process I/O rather than the JS version's async — there's no
//! async runtime dependency here on purpose (see `lib.rs`); the future Tauri command layer wraps calls
//! like [`scan_steam`] in `tokio::task::spawn_blocking`, which is the standard Tauri pattern for
//! blocking work and arguably closer to correct than the JS version's fully-async walk (which still
//! blocks Node's single thread on every `fs/promises` call in practice).
//!
//! `find_steam`'s Windows registry lookup and `launch_quietly`/`running_app_id` (spawning `steam.exe`
//! and a PowerShell window-closing watcher) are Windows-only, `cfg`-gated, and **not exercised by any
//! test in this crate** — there's no Windows box in this sandbox to run them against. Everything else
//! here (library/manifest/artwork scanning) is tested against the same fixtures as
//! `test/games.test.js`'s "scans installed Steam games..." and "scanSteam returns null..." tests.

use crate::vdf::{self, Value as VdfValue};
use fancy_regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

// Tools that show up as "installed apps" but aren't games.
static NOT_GAMES: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| ["228980", "1070560", "1391110", "1628350", "1493710", "2180100", "1887720", "961940"].into_iter().collect());
static NOT_GAMES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(proton\b|steam linux runtime|steamworks common redistributables|steamvr\b)").unwrap());
static MANIFEST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^appmanifest_(\d+)\.acf$").unwrap());

/// Environment the caller resolved once from the real process environment — kept explicit (rather than
/// this module reading `std::env` itself) so tests never have to mutate global process state, which
/// `std::env::set_var` makes an `unsafe`, cross-thread-unsafe operation in current Rust anyway.
#[derive(Debug, Clone, Default)]
pub struct SteamEnv {
    pub lounge_steam_path: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub program_files_x86: Option<PathBuf>,
    pub program_files: Option<PathBuf>,
}

impl SteamEnv {
    pub fn from_process_env() -> Self {
        SteamEnv {
            lounge_steam_path: std::env::var_os("LOUNGE_STEAM_PATH").map(PathBuf::from),
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
            program_files_x86: std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
            program_files: std::env::var_os("ProgramFiles").map(PathBuf::from),
        }
    }
}

fn is_dir(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

/// Locate the Steam install folder, or `None`.
pub fn find_steam(preferred: Option<&Path>, env: &SteamEnv) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = preferred {
        candidates.push(p.to_path_buf());
    }
    if let Some(p) = &env.lounge_steam_path {
        candidates.push(p.clone());
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(reg) = registry_steam_path() {
            candidates.push(reg);
        }
        let pf86 = env.program_files_x86.clone().unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
        let pf = env.program_files.clone().unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        candidates.push(pf86.join("Steam"));
        candidates.push(pf.join("Steam"));
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = &env.home {
            candidates.push(home.join("Library").join("Application Support").join("Steam"));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(home) = &env.home {
            candidates.push(home.join(".steam").join("steam"));
            candidates.push(home.join(".local").join("share").join("Steam"));
        }
    }

    candidates.into_iter().find(|c| is_dir(&c.join("steamapps")))
}

fn read_vdf(p: &Path) -> Option<VdfValue> {
    fs::read_to_string(p).ok().map(|text| vdf::parse(&text))
}

fn list_dir(p: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(p).map(|rd| rd.flatten().collect()).unwrap_or_default()
}

fn all_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Every steamapps folder: the main one plus extra library folders on other drives.
pub fn library_folders(steam_path: &Path) -> Vec<PathBuf> {
    let mut out = vec![steam_path.join("steamapps")];
    if let Some(data) = read_vdf(&steam_path.join("steamapps").join("libraryfolders.vdf")) {
        let root = vdf::get(&data, &["libraryfolders"]).or_else(|| vdf::get(&data, &["LibraryFolders"])).and_then(VdfValue::as_object);
        if let Some(root) = root {
            let mut entries: Vec<_> = root.iter().filter(|(k, _)| all_ascii_digits(k)).collect();
            entries.sort_by_key(|(k, _)| k.parse::<u64>().unwrap_or(0));
            for (_, entry) in entries {
                // New format: { "path": "D:\\SteamLibrary", ... }; old format: "1" "D:\\SteamLibrary".
                let p = match entry {
                    VdfValue::Str(s) => Some(s.clone()),
                    VdfValue::Object(_) => vdf::get(entry, &["path"]).and_then(VdfValue::as_str).map(String::from),
                };
                if let Some(p) = p {
                    out.push(PathBuf::from(p).join("steamapps"));
                }
            }
        }
    }
    let mut seen = HashSet::new();
    out.into_iter()
        .filter(|p| {
            let k = p.to_string_lossy().to_lowercase();
            if seen.contains(&k) || !is_dir(p) {
                return false;
            }
            seen.insert(k);
            true
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamUser {
    pub account_id: String,
    pub name: Option<String>,
}

/// The account that last signed in to Steam, as its 32-bit id used by the userdata folder.
///
/// Note: if `loginusers.vdf` ever listed two accounts tied on both "most recent" and timestamp (real
/// Steam data never does), which one wins can differ from the JS version — it iterates in file order,
/// this iterates in `HashMap`'s unspecified order. Only matters on an exact tie.
pub fn current_user(steam_path: &Path) -> Option<SteamUser> {
    let data = read_vdf(&steam_path.join("config").join("loginusers.vdf"));
    let users = data.as_ref().and_then(|d| vdf::get(d, &["users"])).and_then(VdfValue::as_object);

    struct Best {
        id64: String,
        recent: bool,
        ts: i64,
        name: Option<String>,
    }
    let mut best: Option<Best> = None;
    if let Some(users) = users {
        for (id64, u) in users {
            let recent = vdf::get(u, &["MostRecent"]).and_then(VdfValue::as_str) == Some("1");
            let ts: i64 = vdf::get(u, &["Timestamp"]).and_then(VdfValue::as_str).and_then(|s| s.parse().ok()).unwrap_or(0);
            let name = vdf::get(u, &["PersonaName"]).and_then(VdfValue::as_str).map(String::from);
            let better = match &best {
                None => true,
                Some(b) => (recent && !b.recent) || (recent == b.recent && ts > b.ts),
            };
            if better {
                best = Some(Best { id64: id64.clone(), recent, ts, name });
            }
        }
    }
    if let Some(b) = best {
        if let Ok(id64) = b.id64.parse::<u64>() {
            return Some(SteamUser { account_id: (id64 & 0xffff_ffff).to_string(), name: b.name });
        }
    }
    // Fall back to the most recently modified userdata folder.
    let userdata = steam_path.join("userdata");
    let mut newest: Option<(String, std::time::SystemTime)> = None;
    for entry in list_dir(&userdata) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "0" || !all_ascii_digits(&name) {
            continue;
        }
        if let Ok(meta) = fs::metadata(userdata.join(&name).join("config").join("localconfig.vdf")) {
            if let Ok(mtime) = meta.modified() {
                if newest.as_ref().map(|(_, t)| mtime > *t).unwrap_or(true) {
                    newest = Some((name, mtime));
                }
            }
        }
    }
    newest.map(|(account_id, _)| SteamUser { account_id, name: None })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayStat {
    pub last_played: i64,
    pub playtime: i64,
}

/// Playtime (minutes) and last-played time (ms) per app id, from the user's `localconfig.vdf`.
pub fn play_stats(steam_path: &Path, account_id: Option<&str>) -> HashMap<String, PlayStat> {
    let mut stats = HashMap::new();
    let Some(account_id) = account_id else { return stats };
    let data = read_vdf(&steam_path.join("userdata").join(account_id).join("config").join("localconfig.vdf"));
    let apps = data
        .as_ref()
        .and_then(|d| vdf::get(d, &["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]))
        .and_then(VdfValue::as_object);
    if let Some(apps) = apps {
        for (appid, a) in apps {
            let last_played: i64 = vdf::get(a, &["LastPlayed"]).and_then(VdfValue::as_str).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0) * 1000;
            let playtime: i64 = vdf::get(a, &["Playtime"]).and_then(VdfValue::as_str).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            if last_played != 0 || playtime != 0 {
                stats.insert(appid.clone(), PlayStat { last_played, playtime });
            }
        }
    }
    stats
}

/// Caches the two directory listings [`local_art`] needs, shared across a whole [`scan_steam`] run so
/// each is only read from disk once (same purpose as the JS version's `cache` object).
#[derive(Debug, Default)]
pub struct ArtCache {
    flat: Option<HashSet<String>>,
    grid: Option<HashMap<String, PathBuf>>,
}

impl ArtCache {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Art {
    pub poster: Option<PathBuf>,
    pub hero: Option<PathBuf>,
    pub logo: Option<PathBuf>,
    pub header: Option<PathBuf>,
    pub icon: Option<PathBuf>,
}

fn set_art_field(art: &mut Art, kind: &str, path: PathBuf) {
    match kind {
        "poster" => art.poster = Some(path),
        "hero" => art.hero = Some(path),
        "logo" => art.logo = Some(path),
        "header" => art.header = Some(path),
        _ => unreachable!("kind is always one of the WANT keys below"),
    }
}

fn is_icon_hash_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    match lower.strip_suffix(".jpg") {
        Some(hash) => hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

const WANT: [(&str, &[&str]); 4] = [
    ("poster", &["library_600x900.jpg", "library_600x900_2x.jpg", "library_capsule.jpg"]),
    ("hero", &["library_hero.jpg", "library_hero_2x.jpg"]),
    ("logo", &["logo.png", "logo_2x.png"]),
    ("header", &["header.jpg", "library_header.jpg", "header_2x.jpg"]),
];

/// Artwork Steam already has on disk. Priority: custom images the user set in Steam
/// (`userdata/.../grid`), then Steam's library cache, in both the old flat layout
/// (`620_library_600x900.jpg`) and the newer per-app folders (`620/library_600x900.jpg`, sometimes one
/// level deeper).
pub fn local_art(steam_path: &Path, account_id: Option<&str>, appid: &str, cache: &mut ArtCache) -> Art {
    let mut art = Art::default();

    let lc = steam_path.join("appcache").join("librarycache");
    if cache.flat.is_none() {
        cache.flat = Some(
            list_dir(&lc)
                .into_iter()
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
        );
    }
    let flat = cache.flat.as_ref().unwrap();
    for (kind, names) in WANT {
        if let Some(hit) = names.iter().map(|n| format!("{appid}_{n}")).find(|n| flat.contains(n)) {
            set_art_field(&mut art, kind, lc.join(hit));
        }
    }

    let app_dir = lc.join(appid);
    let mut found: HashMap<String, PathBuf> = HashMap::new();
    for e in list_dir(&app_dir) {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_file() {
            found.insert(e.file_name().to_string_lossy().into_owned(), e.path());
        } else if ft.is_dir() {
            for f in list_dir(&e.path()) {
                if f.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    found.entry(f.file_name().to_string_lossy().into_owned()).or_insert_with(|| f.path());
                }
            }
        }
    }
    for (kind, names) in WANT {
        if let Some(hit) = names.iter().find(|n| found.contains_key(**n)) {
            set_art_field(&mut art, kind, found.get(*hit).unwrap().clone());
        }
    }
    // Steam's small icon is named by a hash; any other .jpg in the app folder is it.
    if let Some(name) = found.keys().find(|n| is_icon_hash_name(n)) {
        art.icon = found.get(name).cloned();
    }

    if let Some(account_id) = account_id {
        let grid = steam_path.join("userdata").join(account_id).join("config").join("grid");
        if cache.grid.is_none() {
            cache.grid = Some(
                list_dir(&grid)
                    .into_iter()
                    .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                    .map(|e| (e.file_name().to_string_lossy().to_lowercase(), e.path()))
                    .collect(),
            );
        }
        let grid_map = cache.grid.as_ref().unwrap();
        let pick = |base: &str| -> Option<PathBuf> { [".png", ".jpg", ".jpeg", ".webp"].iter().find_map(|ext| grid_map.get(&format!("{appid}{base}{ext}")).cloned()) };
        art.poster = pick("p").or(art.poster);
        art.hero = pick("_hero").or(art.hero);
        art.logo = pick("_logo").or(art.logo);
        art.header = pick("").or(art.header);
    }
    art
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub id: String,
    pub source: &'static str,
    pub appid: String,
    pub title: String,
    pub install_dir: PathBuf,
    pub size: i64,
    pub added_at: i64,
    pub last_played: i64,
    pub playtime: i64,
    pub art: Art,
}

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub steam_path: PathBuf,
    pub user: Option<SteamUser>,
    pub games: Vec<Game>,
}

/// Scan installed Steam games. Returns `None` when Steam isn't installed.
pub fn scan_steam(preferred: Option<&Path>, env: &SteamEnv) -> Option<ScanResult> {
    let steam_path = find_steam(preferred, env)?;
    let user = current_user(&steam_path);
    let stats = play_stats(&steam_path, user.as_ref().map(|u| u.account_id.as_str()));
    let mut cache = ArtCache::new();
    let mut games = Vec::new();
    let mut seen = HashSet::new();

    for lib in library_folders(&steam_path) {
        for e in list_dir(&lib) {
            let Ok(ft) = e.file_type() else { continue };
            if !ft.is_file() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(Some(m)) = MANIFEST_RE.captures(name.as_str()) else { continue };
            let appid_from_name = m[1].to_string();
            let Some(data) = read_vdf(&e.path()) else { continue };
            let Some(st) = vdf::get(&data, &["AppState"]) else { continue };
            let appid = vdf::get(st, &["appid"]).and_then(VdfValue::as_str).map(String::from).unwrap_or(appid_from_name);
            let title = vdf::get(st, &["name"]).and_then(VdfValue::as_str).map(String::from).unwrap_or_else(|| format!("App {appid}"));
            let flags: i64 = vdf::get(st, &["StateFlags"]).and_then(VdfValue::as_str).and_then(|s| s.parse().ok()).unwrap_or(0);
            if seen.contains(&appid) || NOT_GAMES.contains(appid.as_str()) || NOT_GAMES_RE.is_match(&title).unwrap_or(false) {
                continue;
            }
            if flags & 4 == 0 {
                continue; // not fully installed
            }
            seen.insert(appid.clone());
            let install_dir = lib.join("common").join(vdf::get(st, &["installdir"]).and_then(VdfValue::as_str).unwrap_or(""));
            let added_at = fs::metadata(e.path())
                .ok()
                .and_then(|meta| meta.created().or_else(|_| meta.modified()).ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let s = stats.get(&appid);
            let manifest_last_played: i64 = vdf::get(st, &["LastPlayed"]).and_then(VdfValue::as_str).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0) * 1000;
            games.push(Game {
                id: format!("steam-{appid}"),
                source: "steam",
                appid: appid.clone(),
                title,
                install_dir,
                size: vdf::get(st, &["SizeOnDisk"]).and_then(VdfValue::as_str).and_then(|s| s.parse().ok()).unwrap_or(0),
                added_at,
                last_played: s.map(|s| s.last_played).filter(|&v| v != 0).unwrap_or(manifest_last_played),
                playtime: s.map(|s| s.playtime).unwrap_or(0),
                art: local_art(&steam_path, user.as_ref().map(|u| u.account_id.as_str()), &appid, &mut cache),
            });
        }
    }
    Some(ScanResult { steam_path, user, games })
}

// ---------------------------------------------------------------------------- Windows-only pieces
//
// Everything below touches the Windows registry or spawns real Windows processes (steam.exe,
// PowerShell). None of it can run or be tested on this machine; treat it as an unverified direct
// translation until it's been checked on an actual Windows box.

#[cfg(target_os = "windows")]
enum RegValue {
    Dword(i64),
    Str(String),
}

#[cfg(target_os = "windows")]
fn registry_query_value(key: &str, value: &str) -> Option<RegValue> {
    let output = std::process::Command::new("reg").args(["query", key, "/v", value]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let re = Regex::new(&format!(r"(?i){}\s+(REG_\w+)\s+(.*)", fancy_regex::escape(value))).ok()?;
    let caps = re.captures(stdout.as_ref()).ok()??;
    let kind = caps.get(1)?.as_str();
    let raw = caps.get(2)?.as_str().trim();
    if kind.eq_ignore_ascii_case("REG_DWORD") {
        let hex = raw.strip_prefix("0x").unwrap_or(raw);
        i64::from_str_radix(hex, 16).ok().map(RegValue::Dword)
    } else {
        Some(RegValue::Str(raw.to_string()))
    }
}

#[cfg(target_os = "windows")]
fn registry_steam_path() -> Option<PathBuf> {
    match registry_query_value(r"HKCU\Software\Valve\Steam", "SteamPath")? {
        RegValue::Str(s) => Some(PathBuf::from(s)),
        RegValue::Dword(_) => None,
    }
}

/// The app id Steam reports as running right now (0 when none), or `None` if it can't be read.
#[cfg(target_os = "windows")]
pub fn running_app_id() -> Option<i64> {
    match registry_query_value(r"HKCU\Software\Valve\Steam", "RunningAppID")? {
        RegValue::Dword(v) => Some(v),
        RegValue::Str(_) => None,
    }
}
#[cfg(not(target_os = "windows"))]
pub fn running_app_id() -> Option<i64> {
    None
}

#[cfg(target_os = "windows")]
const QUIET_WINDOW_SECS: u64 = 40;

/// For a while after a launch, close Steam's main window whenever it shows up (to the tray, exactly
/// like its X button). Only a visible top-level window titled exactly "Steam" that belongs to Steam's
/// own processes is touched: the sign-in window, update and "preparing to launch" dialogs, and the game
/// itself have other titles.
pub fn quiet_script(seconds: u64) -> String {
    format!(
        r#"
$ErrorActionPreference = 'SilentlyContinue'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class LoungeSteamWindow {{
  delegate bool EnumProc(IntPtr hwnd, IntPtr lParam);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int max);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr w, IntPtr l);
  public static int CloseMain(uint[] pids) {{
    int closed = 0;
    EnumWindows((hwnd, l) => {{
      if (!IsWindowVisible(hwnd)) return true;
      uint pid;
      GetWindowThreadProcessId(hwnd, out pid);
      if (Array.IndexOf(pids, pid) < 0) return true;
      var title = new StringBuilder(64);
      GetWindowText(hwnd, title, 64);
      if (title.ToString() == "Steam") {{ PostMessage(hwnd, 0x0010, IntPtr.Zero, IntPtr.Zero); closed++; }}
      return true;
    }}, IntPtr.Zero);
    return closed;
  }}
}}
'@
$end = (Get-Date).AddSeconds({seconds})
while ((Get-Date) -lt $end) {{
  $pids = @(Get-Process -Name steam, steamwebhelper | ForEach-Object {{ [uint32]$_.Id }})
  if ($pids.Count) {{ [void][LoungeSteamWindow]::CloseMain($pids) }}
  Start-Sleep -Milliseconds 400
}}
"#
    )
}

/// Launch a Steam game without Steam's window coming up. `steam.exe -silent` starts Steam straight into
/// the tray when it isn't running (and hands the game over to it when it is); a short watcher then
/// closes the main window if Steam still shows it. Falls back to the plain `steam://` link when
/// `steam.exe` can't be found.
///
/// Returns whether the quiet path was used.
#[cfg(target_os = "windows")]
pub fn launch_quietly(appid: &str, steam_path: Option<&Path>) -> std::io::Result<bool> {
    use base64::Engine;
    use std::process::{Command, Stdio};

    let Some(exe) = steam_path.map(|p| p.join("steam.exe")).filter(|e| e.exists()) else {
        return Ok(false);
    };
    Command::new(&exe).args(["-silent", &format!("steam://rungameid/{appid}")]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;

    let utf16le: Vec<u8> = quiet_script(QUIET_WINDOW_SECS).encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16le);
    let _ = Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    Ok(true)
}
#[cfg(not(target_os = "windows"))]
pub fn launch_quietly(_appid: &str, _steam_path: Option<&Path>) -> std::io::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    // Parity with the JS test "scans installed Steam games with playtime and local artwork".
    #[test]
    fn scans_installed_steam_games_with_playtime_and_local_artwork() {
        let steam_dir = tempdir().unwrap();
        let lib2_dir = tempdir().unwrap();
        let steam = steam_dir.path();
        let lib2 = lib2_dir.path();

        write(
            steam,
            "steamapps/libraryfolders.vdf",
            &format!(r#""libraryfolders" {{ "0" {{ "path" "{}" }} "1" {{ "path" "{}" }} }}"#, steam.display(), lib2.display()),
        );
        write(steam, "steamapps/appmanifest_620.acf", r#""AppState" { "appid" "620" "name" "Portal 2" "installdir" "Portal 2" "StateFlags" "4" "SizeOnDisk" "123" }"#);
        write(steam, "steamapps/appmanifest_228980.acf", r#""AppState" { "appid" "228980" "name" "Steamworks Common Redistributables" "StateFlags" "4" }"#);
        write(steam, "steamapps/appmanifest_999.acf", r#""AppState" { "appid" "999" "name" "Downloading Game" "StateFlags" "1026" }"#);
        write(lib2, "steamapps/appmanifest_1145360.acf", r#""AppState" { "appid" "1145360" "name" "Hades" "installdir" "Hades" "StateFlags" "4" }"#);
        write(steam, "config/loginusers.vdf", r#""users" { "76561197960287930" { "AccountName" "gabe" "PersonaName" "Gabe" "MostRecent" "1" "Timestamp" "1" } }"#);
        // 76561197960287930 & 0xffffffff = 22202
        write(steam, "userdata/22202/config/localconfig.vdf", r#""UserLocalConfigStore" { "Software" { "Valve" { "Steam" { "apps" { "620" { "LastPlayed" "1700000000" "Playtime" "95" } } } } } }"#);
        write(steam, "userdata/22202/config/grid/620p.png", "x"); // custom cover set in Steam
        write(steam, "appcache/librarycache/620_library_hero.jpg", "x"); // old flat layout
        write(steam, "appcache/librarycache/1145360/abcdef/library_600x900.jpg", "x"); // new nested layout
        write(steam, "appcache/librarycache/1145360/logo.png", "x");

        let env = SteamEnv::default();
        let r = scan_steam(Some(steam), &env).expect("steam should be found");
        assert_eq!(r.steam_path, steam);
        assert_eq!(r.user.as_ref().unwrap().account_id, "22202");

        let mut appids: Vec<&str> = r.games.iter().map(|g| g.appid.as_str()).collect();
        appids.sort();
        assert_eq!(appids, vec!["1145360", "620"], "tools and half-installed apps are skipped");

        let by_id: HashMap<&str, &Game> = r.games.iter().map(|g| (g.appid.as_str(), g)).collect();
        let portal2 = by_id["620"];
        assert_eq!(portal2.title, "Portal 2");
        assert_eq!(portal2.playtime, 95);
        assert_eq!(portal2.last_played, 1700000000 * 1000);
        assert!(portal2.art.poster.as_ref().unwrap().ends_with("620p.png"));
        assert!(portal2.art.hero.as_ref().unwrap().ends_with("620_library_hero.jpg"));

        let hades = by_id["1145360"];
        assert!(hades.art.poster.as_ref().unwrap().ends_with(Path::new("abcdef").join("library_600x900.jpg")));
        assert!(hades.art.logo.as_ref().unwrap().ends_with("logo.png"));
        assert!(hades.install_dir.starts_with(lib2));
    }

    // Parity with the JS test "scanSteam returns null without Steam".
    #[test]
    fn scan_steam_returns_none_without_steam() {
        let empty = tempdir().unwrap();
        let env = SteamEnv { home: Some(empty.path().to_path_buf()), ..Default::default() };
        assert!(scan_steam(Some(&empty.path().join("nope")), &env).is_none());
    }
}
