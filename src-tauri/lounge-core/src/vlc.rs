//! Locating VLC and building its command line. Direct port of the pure/locate parts of `src/vlc.js`.
//!
//! `VlcSession` (the JS `EventEmitter`-based class that spawns VLC, polls its HTTP interface for
//! playback progress, and maps status back to queue entries) is **not** ported here, for the same
//! reason as `games::GameSession`: no unit test exists to verify a port against — the one test that
//! exercises it (`test/vlc-real.test.js`) launches a *real* VLC binary and is skipped when one isn't
//! installed, which is the case in this sandbox — and its event/async shape should follow the Tauri
//! `app` crate's design once that exists, not be guessed at now. `find_vlc`'s registry lookup is
//! `cfg(windows)`-gated and untested here for the same reason as `steam::find_steam`'s.

#[cfg(target_os = "windows")]
use fancy_regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// VLC matches track languages against ISO 639 codes; listing both forms catches files tagged either way.
static LANG_CODES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([("en", "eng,en"), ("fr", "fre,fra,fr"), ("de", "ger,deu,de"), ("es", "spa,es"), ("it", "ita,it"), ("ja", "jpn,ja")])
});

#[derive(Debug, Clone, Default)]
pub struct Languages<'a> {
    pub audio: Option<&'a str>,
    pub subs: Option<&'a str>,
}

/// VLC options for the preferred audio and subtitle languages. `audio` is a language code or
/// `"original"` (the file's default track); `subs` is a language code, or `"off"` to start without
/// subtitles.
pub fn language_args(languages: &Languages, prefix: &str) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(codes) = languages.audio.and_then(|a| LANG_CODES.get(a)) {
        args.push(format!("{prefix}audio-language={codes},any"));
    } else if languages.audio == Some("original") {
        // Lets one show opt out of a global preference.
        args.push(format!("{prefix}audio-language=any"));
    }
    if languages.subs == Some("off") {
        args.push(format!("{prefix}sub-language=none"));
    } else if let Some(codes) = languages.subs.and_then(|s| LANG_CODES.get(s)) {
        args.push(format!("{prefix}sub-language={codes}"));
    }
    args
}

#[derive(Debug, Clone, Default)]
pub struct QueueItem<'a> {
    pub path: &'a str,
    pub start_time: Option<f64>,
    pub languages: Option<Languages<'a>>,
}

#[derive(Debug, Clone)]
pub struct BuildArgsOptions<'a> {
    pub port: u16,
    pub password: &'a str,
    pub fullscreen: bool,
    pub languages: Languages<'a>,
    pub extra_args: &'a [&'a str],
}

impl<'a> Default for BuildArgsOptions<'a> {
    fn default() -> Self {
        BuildArgsOptions { port: 0, password: "", fullscreen: true, languages: Languages::default(), extra_args: &[] }
    }
}

/// Build VLC's command line. Each queue item may carry a start offset, applied as a per-item
/// `:start-time` option, and its own languages (e.g. a show whose tracks were picked by hand), applied
/// as per-item language options.
pub fn build_args(queue: &[QueueItem], opts: &BuildArgsOptions) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--extraintf=http".into(),
        "--http-host=127.0.0.1".into(),
        format!("--http-port={}", opts.port),
        format!("--http-password={}", opts.password),
        "--play-and-exit".into(),
        "--no-random".into(),
        "--no-loop".into(),
        "--no-repeat".into(),
        "--qt-continue=0".into(),          // this launcher owns resume; don't let VLC ask as well
        "--no-qt-privacy-ask".into(),      // VLC's first-run network-policy dialog would otherwise block fullscreen playback
        "--no-one-instance-when-started-from-file".into(), // on/off options take --no-, never =0 (VLC refuses to start)
        "--no-one-instance".into(),
    ];
    if opts.fullscreen {
        args.push("--fullscreen".into());
    }
    args.extend(language_args(&opts.languages, "--")); // before extra_args, so the user's own options still win
    args.extend(opts.extra_args.iter().map(|s| s.to_string()));
    for item in queue {
        args.push(item.path.to_string());
        if let Some(t) = item.start_time {
            if t > 5.0 {
                args.push(format!(":start-time={}", t.floor() as i64));
            }
        }
        if let Some(langs) = &item.languages {
            args.extend(language_args(langs, ":"));
        }
    }
    args
}

fn exists_file(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

fn which(cmd: &str, path_env: &str, is_windows: bool) -> Option<PathBuf> {
    let exts: &[&str] = if is_windows { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    for dir in std::env::split_paths(path_env) {
        for ext in exts {
            let p = dir.join(format!("{cmd}{ext}"));
            if exists_file(&p) {
                return Some(p);
            }
        }
    }
    None
}

/// Roots to check for a VideoLAN\VLC install under, on Windows (`%ProgramFiles%`,
/// `%ProgramFiles(x86)%`, `%ProgramW6432%`, `%LOCALAPPDATA%\Programs`) — kept explicit rather than read
/// from `std::env` inside `find_vlc` itself, same reasoning as `steam::SteamEnv`.
#[derive(Debug, Clone, Default)]
pub struct VlcEnv {
    pub path: String,
    pub program_files: Option<PathBuf>,
    pub program_files_x86: Option<PathBuf>,
    pub program_w6432: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub is_windows: bool,
    pub is_macos: bool,
}

impl VlcEnv {
    pub fn from_process_env() -> Self {
        VlcEnv {
            path: std::env::var("PATH").unwrap_or_default(),
            program_files: std::env::var_os("ProgramFiles").map(PathBuf::from),
            program_files_x86: std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
            program_w6432: std::env::var_os("ProgramW6432").map(PathBuf::from),
            local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            is_windows: cfg!(target_os = "windows"),
            is_macos: cfg!(target_os = "macos"),
        }
    }
}

/// Locate the VLC executable. Returns an absolute path or `None`.
pub fn find_vlc(preferred: Option<&Path>, env: &VlcEnv) -> Option<PathBuf> {
    if let Some(p) = preferred {
        if exists_file(p) {
            return Some(p.to_path_buf());
        }
    }

    if env.is_windows {
        #[cfg(target_os = "windows")]
        for key in ["HKLM\\SOFTWARE\\VideoLAN\\VLC", "HKLM\\SOFTWARE\\WOW6432Node\\VideoLAN\\VLC", "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\vlc.exe", "HKCU\\SOFTWARE\\VideoLAN\\VLC"] {
            if let Some(v) = registry_default_value(key) {
                if exists_file(&v) {
                    return Some(v);
                }
            }
        }
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(p) = &env.program_files {
            roots.push(p.clone());
        }
        if let Some(p) = &env.program_files_x86 {
            roots.push(p.clone());
        }
        if let Some(p) = &env.program_w6432 {
            roots.push(p.clone());
        }
        if let Some(p) = &env.local_app_data {
            roots.push(p.join("Programs"));
        }
        for r in roots {
            let p = r.join("VideoLAN").join("VLC").join("vlc.exe");
            if exists_file(&p) {
                return Some(p);
            }
        }
    } else if env.is_macos {
        let p = PathBuf::from("/Applications/VLC.app/Contents/MacOS/VLC");
        if exists_file(&p) {
            return Some(p);
        }
    }
    which("vlc", &env.path, env.is_windows)
}

#[cfg(target_os = "windows")]
fn registry_default_value(key: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("reg").args(["query", key, "/ve"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let re = Regex::new(r"REG_SZ\s+(.+)").unwrap();
    let caps = re.captures(stdout.as_ref()).ok()??;
    Some(PathBuf::from(caps.get(1)?.as_str().trim()))
}
#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
fn registry_default_value(_key: &str) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn args_to_str(args: &[String]) -> Vec<&str> {
        args.iter().map(|s| s.as_str()).collect()
    }

    // Parity with the JS test "builds VLC arguments with per-item resume" (test/library.test.js).
    #[test]
    fn builds_vlc_arguments_with_per_item_resume() {
        let queue = [QueueItem { path: "C:\\a.mkv", start_time: Some(125.7), languages: None }, QueueItem { path: "C:\\b.mkv", start_time: None, languages: None }];
        let opts = BuildArgsOptions { port: 1234, password: "pw", ..Default::default() };
        let args = build_args(&queue, &opts);
        let s = args_to_str(&args);
        assert!(s.contains(&"--fullscreen"));
        assert!(s.contains(&"--http-port=1234"));
        let i = s.iter().position(|a| *a == "C:\\a.mkv").unwrap();
        assert_eq!(s[i + 1], ":start-time=125");
        assert_eq!(s[i + 2], "C:\\b.mkv");
        assert_eq!(*s.last().unwrap(), "C:\\b.mkv");
    }

    // Parity with the JS test "builds VLC language options, with per-item overrides and the user's own
    // options last".
    #[test]
    fn builds_vlc_language_options_with_overrides_and_user_options_last() {
        let queue = [
            QueueItem { path: "a.mkv", start_time: None, languages: Some(Languages { audio: Some("original"), subs: Some("off") }) },
            QueueItem { path: "b.mkv", start_time: None, languages: None },
        ];
        let opts = BuildArgsOptions {
            port: 1,
            password: "pw",
            languages: Languages { audio: Some("en"), subs: Some("en") },
            extra_args: &["--sub-language=fre"],
            ..Default::default()
        };
        let args = build_args(&queue, &opts);
        let s = args_to_str(&args);
        let audio_pos = s.iter().position(|a| *a == "--audio-language=eng,en,any").unwrap();
        let sub_pos = s.iter().position(|a| *a == "--sub-language=fre").unwrap();
        assert!(audio_pos < sub_pos);
        assert!(s.contains(&"--sub-language=eng,en"));
        let a = s.iter().position(|a| *a == "a.mkv").unwrap();
        assert_eq!(&s[a..a + 4], ["a.mkv", ":audio-language=any", ":sub-language=none", "b.mkv"]);

        // No preference: VLC's own defaults.
        let plain_queue = [QueueItem { path: "a.mkv", start_time: None, languages: None }];
        let plain_opts = BuildArgsOptions { port: 1, password: "pw", languages: Languages { audio: Some("original"), subs: Some("") }, ..Default::default() };
        let plain = build_args(&plain_queue, &plain_opts);
        assert!(!plain.iter().any(|x| x.contains("sub-language")));
    }

    #[test]
    fn find_vlc_prefers_an_existing_preferred_path() {
        let dir = tempdir().unwrap();
        let vlc = dir.path().join("vlc.exe");
        fs::write(&vlc, "").unwrap();
        let env = VlcEnv { path: String::new(), is_windows: false, is_macos: false, ..Default::default() };
        assert_eq!(find_vlc(Some(&vlc), &env), Some(vlc));
    }

    #[test]
    fn find_vlc_returns_none_when_nothing_matches() {
        let env = VlcEnv { path: String::new(), is_windows: false, is_macos: false, ..Default::default() };
        assert_eq!(find_vlc(Some(Path::new("/nope/vlc")), &env), None);
    }

    #[test]
    fn which_finds_an_executable_on_the_path() {
        let dir = tempdir().unwrap();
        let vlc = dir.path().join("vlc");
        fs::write(&vlc, "").unwrap();
        let path_env = dir.path().to_string_lossy().into_owned();
        assert_eq!(which("vlc", &path_env, false), Some(vlc));
        assert_eq!(which("nope", &path_env, false), None);
    }
}
