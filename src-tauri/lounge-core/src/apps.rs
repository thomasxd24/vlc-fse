//! Installed applications, as the Start menu knows them: `Get-StartApps` lists desktop programs and
//! Store apps alike, each with an AppID that `shell:AppsFolder\<AppID>` launches exactly like a click
//! in Start. Direct port of `src/apps.js` — parsing, junk filtering, known-folder resolution, Store
//! logo picking and Start-menu shortcut matching are all pure here and tested; the two process calls
//! (the PowerShell listing, `explorer.exe` launching) are `cfg(windows)` and verified by the Windows
//! CI build.

use crate::locale;
use fancy_regex::Regex;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

/// One PowerShell run: the Start menu's apps, plus where each Store package lives (for its logo).
/// Verbatim from `src/apps.js`.
pub const LIST_SCRIPT: &str = r#"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'SilentlyContinue'
$apps = @(Get-StartApps | Select-Object Name, AppID)
$pkgs = @{}
foreach ($p in @(Get-AppxPackage)) { if ($p.InstallLocation) { $pkgs[$p.PackageFamilyName] = $p.InstallLocation } }
@{ apps = $apps; packages = $pkgs } | ConvertTo-Json -Depth 4 -Compress
"#;

/// The environment variables the known-folder GUIDs resolve against (see [`KNOWN_FOLDERS`]).
#[derive(Debug, Clone, Default)]
pub struct AppEnv {
    pub program_data: Option<String>,
    pub program_w6432: Option<String>,
    pub program_files: Option<String>,
    pub program_files_x86: Option<String>,
    pub common_program_files: Option<String>,
    pub system_root: Option<String>,
    pub local_app_data: Option<String>,
    pub appdata: Option<String>,
    pub userprofile: Option<String>,
}

impl AppEnv {
    pub fn from_process_env() -> Self {
        let v = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        AppEnv {
            program_data: v("ProgramData"),
            program_w6432: v("ProgramW6432"),
            program_files: v("ProgramFiles"),
            program_files_x86: v("ProgramFiles(x86)"),
            common_program_files: v("CommonProgramFiles"),
            system_root: v("SystemRoot"),
            local_app_data: v("LOCALAPPDATA"),
            appdata: v("APPDATA"),
            userprofile: v("USERPROFILE"),
        }
    }
}

/// Windows-style path join, so tests get Windows-shaped results on any host (same reasoning as
/// `tailscale.rs`'s `win_join`).
fn win_join(parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<&str>>().join("\\")
}

/// Known-folder GUIDs that prefix desktop-program AppIDs (`{6D809377-…}\Tailscale\tailscale-ipn.exe`).
fn known_folder(guid: &str, env: &AppEnv) -> Option<String> {
    match guid {
        "{6d809377-6af0-444b-8957-a3773f02200e}" => env.program_w6432.clone().or_else(|| env.program_files.clone()),
        "{905e63b6-c1bf-494e-b29c-65b732d3d21a}" => env.program_files.clone(),
        "{7c5a40ef-a0fb-4bfc-874a-c0f2e0b9fa8e}" => env.program_files_x86.clone(),
        "{f7f1ed05-9f6d-47a2-aaae-29d317c6f066}" => env.common_program_files.clone(),
        "{1ac14e77-02e7-4e5d-b744-2eb1ae5198b7}" => env.system_root.as_ref().map(|r| win_join(&[r, "System32"])),
        "{d65231b0-b2f1-4857-a4ce-a8e7c6ea7d27}" => env.system_root.as_ref().map(|r| win_join(&[r, "SysWOW64"])),
        "{f38bf404-1d43-42f2-9305-67de0b28fc23}" => env.system_root.clone(),
        "{f1b32785-6fba-4fcf-9d55-7b8e7f157091}" => env.local_app_data.clone(),
        "{3eb685db-65f9-4cf6-a03a-e3ef65729f3d}" => env.appdata.clone(),
        "{5e6c858f-0e22-4760-9afe-ea3317b67173}" => env.userprofile.clone(),
        _ => None,
    }
}

// Start menu clutter: uninstallers, readmes, manuals, websites. English, French and German wording.
static JUNK_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(uninstall|uninstaller|d[ée]sinstaller|deinstallieren|readme|read me|lisez-moi|release notes|notes de version|changelog|license|licence|manual|manuel|handbuch|documentation|user guide|help|aide|hilfe|website|site web|web site)\b").unwrap());
static JUNK_TARGET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(txt|chm|pdf|html?|url|rtf|ini|log|md|xml|hlp)$").unwrap());
static STORE_PARTS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^([^\\/!]+_[a-z0-9]{13})!([^\\/!]+)$").unwrap());
static KNOWN_FOLDER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(\{[0-9a-f-]{36}\})\\(.+)$").unwrap());
static APPLICATION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<Application\b([^>]*)>(.*?)</Application>").unwrap());
static VISUAL_ELEMENTS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[\w:]*VisualElements\b([^>]*)>?").unwrap());

/// Store apps have AppIDs like `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`.
pub fn store_parts(app_id: &str) -> Option<(String, String)> {
    let m = STORE_PARTS_RE.captures(app_id).ok().flatten()?;
    Some((m[1].to_string(), m[2].to_string()))
}

/// A desktop program's executable, when its AppID is a known-folder path.
pub fn resolve_known_folder(app_id: &str, env: &AppEnv) -> Option<String> {
    let m = KNOWN_FOLDER_RE.captures(app_id).ok().flatten()?;
    let root = known_folder(m[1].to_lowercase().as_str(), env)?;
    Some(win_join(&[&root, &m[2]]))
}

pub fn is_junk(name: &str, app_id: &str) -> bool {
    let lower = app_id.to_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") {
        return true;
    }
    if JUNK_TARGET.is_match(app_id).unwrap_or(false) {
        return true;
    }
    JUNK_NAME.is_match(name).unwrap_or(false)
}

pub fn id_of(app_id: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(app_id.to_lowercase());
    let digest = hasher.finalize();
    format!("a{}", &hex::encode(digest)[..16])
}

/// One launchable app as stored in `apps.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub app_id: String,
    /// `"store"` or `"desktop"`.
    pub kind: &'static str,
    pub exe: Option<String>,
    pub package_dir: Option<String>,
    pub store_app: Option<String>,
    /// A `.lnk` in the Start menu with the same name (desktop apps whose AppID isn't a path).
    pub shortcut: Option<String>,
}

/// Turn Get-StartApps output into the app list: junk dropped, duplicates (same name) merged, sorted by name.
pub fn parse_start_apps(raw: &Value, env: &AppEnv) -> Vec<AppEntry> {
    let list: Vec<Value> = match raw.get("apps") {
        Some(Value::Array(a)) => a.clone(),
        Some(a) if !a.is_null() => vec![a.clone()],
        _ => Vec::new(),
    };
    let packages = raw.get("packages").cloned().unwrap_or_else(|| json!({}));
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for a in &list {
        let name = a.get("Name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let app_id = a.get("AppID").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if name.is_empty() || app_id.is_empty() || is_junk(&name, &app_id) {
            continue;
        }
        let key = name.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let store = store_parts(&app_id);
        out.push(AppEntry {
            id: id_of(&app_id),
            name,
            app_id: app_id.clone(),
            kind: if store.is_some() { "store" } else { "desktop" },
            exe: if store.is_none() { resolve_known_folder(&app_id, env) } else { None },
            package_dir: store.as_ref().and_then(|(family, _)| packages.get(family)).and_then(Value::as_str).map(String::from),
            store_app: store.as_ref().map(|(_, app)| app.clone()),
            shortcut: None,
        });
    }
    out.sort_by(|x, y| locale::compare_base_numeric(&x.name, &y.name));
    out
}

// ---------------------------------------------------------------------------- Store app logos

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r".*+?^${}()|[]\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn visual_element_attr(attrs: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"\b{}="([^"]+)""#, regex_escape(name))).ok()?;
    re.captures(attrs).ok().flatten()?.get(1).map(|m| m.as_str().to_string())
}

/// The logo a Store app's manifest names for one of its `<Application>`s: Square44x44Logo (the
/// app-list icon) or, failing that, Square150x150Logo. Returns the manifest's relative path, e.g.
/// `Assets\AppList.png`.
pub fn manifest_logo(xml: &str, app_name: &str) -> Option<String> {
    let apps: Vec<(String, String)> = APPLICATION_RE.captures_iter(xml).filter_map(|c| {
        let c = c.ok()?;
        Some((c.get(1)?.as_str().to_string(), c.get(2)?.as_str().to_string()))
    }).collect();
    let id_re = Regex::new(&format!(r#"\bId="{}""#, regex_escape(app_name))).ok()?;
    let hit = apps.iter().find(|(attrs, _)| id_re.is_match(attrs).unwrap_or(false)).or_else(|| apps.first())?;
    let ve = VISUAL_ELEMENTS_RE.captures(&hit.1).ok().flatten()?;
    let attrs = ve.get(1)?.as_str();
    visual_element_attr(attrs, "Square44x44Logo").or_else(|| visual_element_attr(attrs, "Square150x150Logo"))
}

/// Store logos are stored per scale (`AppList.scale-200.png`, `AppList.targetsize-256_altform-unplated.png`).
/// Pick the best one for a large tile: unplated target sizes first (no coloured plate), biggest up to 256.
pub fn pick_logo_file(files: &[String], logo_name: &str) -> Option<String> {
    let logo = logo_name.replace('\\', "/");
    let ext = logo.rsplit('.').next().unwrap_or("").to_lowercase();
    let base = {
        let stem = match logo.rfind('/') {
            Some(i) => &logo[i + 1..],
            None => logo.as_str(),
        };
        match stem.rfind('.') {
            Some(i) => stem[..i].to_lowercase(),
            None => stem.to_lowercase(),
        }
    };
    let mut candidates: Vec<(String, f64)> = Vec::new();
    for f in files {
        let lower = f.to_lowercase();
        if !lower.ends_with(&format!(".{ext}")) {
            continue;
        }
        let stem = lower[..lower.len() - ext.len() - 1].to_string();
        if stem == base {
            candidates.push((f.clone(), 100.0));
            continue;
        }
        let Some(q) = stem.strip_prefix(&format!("{base}.")) else { continue };
        if q.contains("contrast-black") || q.contains("contrast-white") || q.contains("theme-light") {
            continue; // high-contrast and light-theme variants
        }
        let targetsize = Regex::new(r"targetsize-(\d+)").ok()?.captures(q).ok().flatten().and_then(|c| c.get(1)).and_then(|m| m.as_str().parse::<f64>().ok());
        let scale = Regex::new(r"scale-(\d+)").ok()?.captures(q).ok().flatten().and_then(|c| c.get(1)).and_then(|m| m.as_str().parse::<f64>().ok());
        let mut score = 0.0;
        if let Some(n) = targetsize {
            let capped = if n <= 256.0 { n } else { 256.0 - (n - 256.0) / 4.0 };
            score = capped + if q.contains("altform-unplated") || q.contains("altform-lightunplated") { 1000.0 } else { 500.0 };
        } else if let Some(n) = scale {
            score = 200.0 + n.min(400.0) / 4.0;
        }
        candidates.push((f.clone(), score));
    }
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates.first().map(|(f, _)| f.clone())
}

/// Absolute path of a Store app's best logo file, or `None`.
pub fn store_logo(package_dir: &str, store_app: Option<&str>) -> Option<String> {
    let xml = std::fs::read_to_string(Path::new(package_dir).join("AppxManifest.xml")).ok()?;
    let rel = manifest_logo(&xml, store_app.unwrap_or(""))?;
    let rel_unix = rel.replace('\\', "/");
    let file_name = rel_unix.rsplit('/').next().unwrap_or(&rel_unix);
    let dir_parts: Vec<&str> = rel_unix.rsplit_once('/').map(|(d, _)| d.split('/').collect()).unwrap_or_default();
    let mut dir = Path::new(package_dir).to_path_buf();
    for part in dir_parts {
        dir = dir.join(part);
    }
    let files: Vec<String> = std::fs::read_dir(&dir).ok()?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    pick_logo_file(&files, file_name).map(|f| dir.join(f).to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------- Listing & launching

/// Start menu shortcuts by lowercase name (for desktop apps whose AppID isn't a path, e.g. "Chrome").
pub fn start_menu_shortcuts(program_data: Option<&str>, appdata: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let roots = [program_data, appdata].into_iter().flatten().map(|r| Path::new(r).join("Microsoft").join("Windows").join("Start Menu").join("Programs"));
    for root in roots {
        walk_dir(&root, 0, &mut map);
    }
    map
}

fn walk_dir(dir: &Path, depth: u32, map: &mut HashMap<String, String>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.filter_map(|e| e.ok()) {
        let p = e.path();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            walk_dir(&p, depth + 1, map);
        } else {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.to_lowercase().ends_with(".lnk") {
                let key = name[..name.len() - 4].to_lowercase();
                map.entry(key).or_insert_with(|| p.to_string_lossy().into_owned());
            }
        }
    }
}

/// Run a PowerShell script (base64-encoded UTF-16LE, like the JS `runPowerShell`) and return its stdout.
#[cfg(target_os = "windows")]
fn run_power_shell(script: &str, timeout: std::time::Duration) -> Result<String, String> {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>());
    let mut child = std::process::Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut out = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    use std::io::Read;
                    let _ = stdout.read_to_string(&mut out);
                }
                return Ok(out);
            }
            Ok(Some(status)) => return Err(format!("powershell exited with code {}", status.code().unwrap_or(-1))),
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("powershell timed out".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Every launchable app on this PC (Windows only; elsewhere an empty list).
pub fn list_apps(env: &AppEnv) -> Result<Vec<AppEntry>, String> {
    if !cfg!(target_os = "windows") {
        return Ok(Vec::new());
    }
    list_apps_impl(env)
}

#[cfg(target_os = "windows")]
fn list_apps_impl(env: &AppEnv) -> Result<Vec<AppEntry>, String> {
    let out = run_power_shell(LIST_SCRIPT, std::time::Duration::from_secs(30))?;
    let start = out.find('{').unwrap_or(0);
    let raw: Value = serde_json::from_str(&out[start..]).map_err(|e| e.to_string())?;
    let mut apps = parse_start_apps(&raw, env);
    let shortcuts = start_menu_shortcuts(env.program_data.as_deref(), env.appdata.as_deref());
    for a in &mut apps {
        if a.kind == "desktop" {
            a.shortcut = shortcuts.get(&a.name.to_lowercase()).cloned();
        }
    }
    Ok(apps)
}

#[cfg(not(target_os = "windows"))]
fn list_apps_impl(_env: &AppEnv) -> Result<Vec<AppEntry>, String> {
    Ok(Vec::new())
}

/// Open an app the way the Start menu does (`explorer.exe shell:AppsFolder\<AppID>`).
pub fn launch_app(app_id: &str) -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Err("not supported".into());
    }
    launch_app_impl(app_id)
}

#[cfg(target_os = "windows")]
fn launch_app_impl(app_id: &str) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(format!("shell:AppsFolder\\{app_id}"))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "windows"))]
fn launch_app_impl(_app_id: &str) -> Result<(), String> {
    Err("not supported".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn env() -> AppEnv {
        AppEnv {
            program_data: Some(r"C:\ProgramData".into()),
            program_w6432: Some(r"C:\Program Files".into()),
            program_files: Some(r"C:\Program Files".into()),
            program_files_x86: Some(r"C:\Program Files (x86)".into()),
            common_program_files: Some(r"C:\Program Files\Common Files".into()),
            system_root: Some(r"C:\WINDOWS".into()),
            local_app_data: Some(r"C:\Users\me\AppData\Local".into()),
            appdata: Some(r"C:\Users\me\AppData\Roaming".into()),
            userprofile: Some(r"C:\Users\me".into()),
        }
    }

    #[test]
    fn store_app_ids_split_into_family_and_app() {
        assert_eq!(store_parts("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), Some(("Microsoft.WindowsCalculator_8wekyb3d8bbwe".into(), "App".into())));
        assert_eq!(store_parts("not an appid"), None);
        assert_eq!(store_parts("Short_8wekyb3d8bb"), None, "the publisher hash must be 13 chars");
        assert_eq!(store_parts(r"C:\path\to\app.exe"), None);
    }

    #[test]
    fn known_folder_appids_resolve_through_the_guid_table() {
        assert_eq!(resolve_known_folder(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Tailscale\tailscale-ipn.exe", &env()), Some(r"C:\Program Files\Tailscale\tailscale-ipn.exe".into()));
        assert_eq!(resolve_known_folder(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe", &env()), Some(r"C:\WINDOWS\System32\notepad.exe".into()));
        assert_eq!(resolve_known_folder(r"{00000000-0000-0000-0000-000000000000}\x.exe", &env()), None, "unknown GUID");
        assert_eq!(resolve_known_folder("Chrome", &env()), None, "not a path AppID");
    }

    #[test]
    fn junk_filtering_covers_three_languages_and_targets() {
        assert!(is_junk("Uninstall Tailscale", "{6D809377-…}\\x.exe"));
        assert!(is_junk("Désinstaller Foo", "{6D809377-…}\\x.exe"));
        assert!(is_junk("Deinstallieren Foo", "{6D809377-…}\\x.exe"));
        assert!(is_junk("Read Me", "{6D809377-…}\\x.exe"));
        assert!(is_junk("Lisez-moi", "{6D809377-…}\\x.exe"));
        assert!(is_junk("Handbuch", "{6D809377-…}\\x.exe"));
        assert!(is_junk("Anything", "https://example.com"));
        assert!(is_junk("Anything", r"{6D809377-…}\docs\readme.txt"));
        assert!(is_junk("Anything", r"{6D809377-…}\site.url"));
        assert!(!is_junk("Hades II", "{6D809377-…}\\Hades II\\Hades.exe"));
        assert!(!is_junk("VLC media player", r"{6D809377-…}\vlc.exe"));
    }

    #[test]
    fn start_apps_output_becomes_a_sorted_deduped_app_list() {
        let raw = json!({
            "apps": [
                { "Name": "VLC media player", "AppID": r"{6D809377-6AF0-444B-8957-A3773F02200E}\VideoLAN\VLC\vlc.exe" },
                { "Name": "VLC media player", "AppID": "duplicate-ignored" },
                { "Name": "Uninstall Tailscale", "AppID": r"{6D809377-6AF0-444B-8957-A3773F02200E}\Tailscale\Uninst.exe" },
                { "Name": "Tailscale", "AppID": r"{6D809377-6AF0-444B-8957-A3773F02200E}\Tailscale\tailscale-ipn.exe" },
                { "Name": "Calculator", "AppID": "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App" },
                { "Name": "", "AppID": "noname" },
                { "Name": "Web Page", "AppID": "https://example.com" }
            ],
            "packages": { "Microsoft.WindowsCalculator_8wekyb3d8bbwe": r"C:\Program Files\WindowsApps\CalcPkg" }
        });
        let apps = parse_start_apps(&raw, &env());
        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Calculator", "Tailscale", "VLC media player"], "sorted by name, junk and duplicates gone");
        let calc = &apps[0];
        assert_eq!(calc.kind, "store");
        assert_eq!(calc.package_dir.as_deref(), Some(r"C:\Program Files\WindowsApps\CalcPkg"));
        assert_eq!(calc.store_app.as_deref(), Some("App"));
        assert_eq!(calc.exe, None);
        assert_eq!(calc.id, id_of("microsoft.windowscalculator_8wekyb3d8bbwe!app"), "ids hash the lowercased AppID");
        let tailscale = &apps[1];
        assert_eq!(tailscale.kind, "desktop");
        assert_eq!(tailscale.exe.as_deref(), Some(r"C:\Program Files\Tailscale\tailscale-ipn.exe"));
    }

    #[test]
    fn a_single_app_object_is_treated_as_a_one_item_list() {
        let raw = json!({ "apps": { "Name": "Solo", "AppID": "solo-app" } });
        let apps = parse_start_apps(&raw, &env());
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Solo");
    }

    #[test]
    fn manifest_logo_picks_the_named_application_then_the_smallest_logo() {
        let xml = r#"
<Package>
  <Applications>
    <Application Id="Other" Executable="other.exe">
      <uap:VisualElements DisplayName="Other" Square150x150Logo="Assets\Other150.png"/>
    </Application>
    <Application Id="MyApp" Executable="myapp.exe">
      <uap:VisualElements DisplayName="MyApp" Square44x44Logo="Assets\AppList.png" Square150x150Logo="Assets\Tile150.png"/>
    </Application>
  </Applications>
</Package>"#;
        assert_eq!(manifest_logo(xml, "MyApp"), Some(r"Assets\AppList.png".into()));
        // Falls back to the 150 logo when no 44 exists, and to the first <Application> when the name misses.
        let xml2 = r#"<Application Id="Only"><uap:VisualElements Square150x150Logo="Assets\Only150.png"/></Application>"#;
        assert_eq!(manifest_logo(xml2, "Only"), Some(r"Assets\Only150.png".into()));
        assert_eq!(manifest_logo(xml2, "Missing"), Some(r"Assets\Only150.png".into()));
        assert_eq!(manifest_logo("<NoApplications/>", "X"), None);
    }

    #[test]
    fn logo_files_favour_unplated_target_sizes_up_to_256() {
        let files: Vec<String> = [
            "AppList.png",
            "AppList.scale-100.png",
            "AppList.scale-200.png",
            "AppList.targetsize-24.png",
            "AppList.targetsize-256_altform-unplated.png",
            "AppList.targetsize-512_altform-unplated.png",
            "AppList.contrast-black_targetsize-256.png",
            "AppList.theme-light_scale-200.png",
        ].iter().map(|s| s.to_string()).collect();
        let pick = pick_logo_file(&files, "AppList.png").unwrap();
        assert_eq!(pick, "AppList.targetsize-256_altform-unplated.png", "unplated 256 beats everything");

        let fewer: Vec<String> = ["AppList.scale-100.png", "AppList.scale-200.png", "AppList.targetsize-24.png"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick_logo_file(&fewer, "AppList.png").as_deref(), Some("AppList.targetsize-24.png"), "any targetsize outscores any scale, unplated or not");
        assert_eq!(pick_logo_file(&["Other.png".to_string()], "AppList.png"), None);
    }

    #[test]
    fn store_logos_are_found_on_disk_next_to_the_manifest() {
        let dir = tempdir().unwrap();
        let pkg = dir.path().join("CalcPkg");
        std::fs::create_dir_all(pkg.join("Assets")).unwrap();
        std::fs::write(pkg.join("AppxManifest.xml"), r#"<Application Id="App"><uap:VisualElements Square44x44Logo="Assets\AppList.png"/></Application>"#).unwrap();
        std::fs::write(pkg.join("Assets").join("AppList.scale-200.png"), "png").unwrap();
        std::fs::write(pkg.join("Assets").join("AppList.png"), "png").unwrap();

        let logo = store_logo(pkg.to_str().unwrap(), Some("App")).unwrap();
        assert!(logo.ends_with("AppList.scale-200.png"));
        assert!(store_logo(pkg.to_str().unwrap(), None).is_some(), "a missing app name falls back to the first <Application>");
        assert!(store_logo(dir.path().to_str().unwrap(), Some("App")).is_none(), "no manifest, no logo");
    }

    #[test]
    fn start_menu_shortcuts_index_lnk_files_by_lowercase_name() {
        let dir = tempdir().unwrap();
        let programs = dir.path().join("Microsoft").join("Windows").join("Start Menu").join("Programs");
        std::fs::create_dir_all(programs.join("Nested").join("Deeper")).unwrap();
        std::fs::write(programs.join("VLC media player.lnk"), "").unwrap();
        std::fs::write(programs.join("Nested").join("Hades II.lnk"), "").unwrap();
        std::fs::write(programs.join("Nested").join("Deeper").join("Deep Game.lnk"), "").unwrap();
        std::fs::write(programs.join("readme.txt"), "").unwrap();

        let root = dir.path().to_string_lossy().into_owned();
        let map = start_menu_shortcuts(Some(&root), None);
        assert!(map.get("vlc media player").map(String::as_str).unwrap_or_default().ends_with("VLC media player.lnk"));
        assert!(map.contains_key("hades ii"));
        assert!(map.contains_key("deep game"));
        assert_eq!(map.len(), 3, "only .lnk files are indexed");
    }

    #[test]
    fn off_windows_there_is_nothing_to_list_or_launch() {
        if cfg!(target_os = "windows") {
            return;
        }
        assert_eq!(list_apps(&AppEnv::from_process_env()).unwrap(), Vec::<AppEntry>::new());
        assert!(launch_app("solo-app").is_err());
    }
}
