//! Self-updater driven by GitHub Releases. Works for all three ways Lounge can be installed:
//!   - `nsis` — `Lounge-Setup-<v>.exe`, run silently over the existing install, which relaunches Lounge
//!   - `zip`  — `Lounge-<v>-win-x64.zip`, unpacked over the current folder by a small PowerShell script
//!   - `fse`  — `Lounge-FSE-<v>.zip`, the package's own installer updates the MSIX (one UAC prompt, which
//!     Windows only shows on the desktop, not in the full screen experience)
//!
//! Nothing is installed without the user saying so; downloads are verified against the SHA-256 digest
//! GitHub publishes for each release asset. Direct, fully-ported port of `src/updater.js` — this is the
//! one channel real users actually update through, so unlike most of this migration's stateful classes,
//! it's ported and tested in full using the same `ureq` + local `httpmock` approach as `metadata.rs` and
//! `gameinfo.rs`, rather than deferred.

use fancy_regex::Regex;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::{Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CHECKSUMS: &str = "SHA256SUMS.txt";
const DEFAULT_API_BASE: &str = "https://api.github.com";
const DEFAULT_WEB_BASE: &str = "https://github.com";

// JS's `encodeURIComponent` leaves `A-Za-z0-9 - _ . ! ~ * ' ( )` unescaped (the ECMA-262 "uriUnescaped"
// set); `percent_encoding`'s NON_ALPHANUMERIC is more aggressive, so a plain filename like
// "Lounge-FSE-2.2.0.zip" would come out mangled unless we carve those characters back out.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~').remove(b'!').remove(b'*').remove(b'\'').remove(b'(').remove(b')');

fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

static NSIS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:Lounge|Foyer)-Setup-\d+\.\d+\.\d+\.exe$").unwrap());
static ZIP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:Lounge|Foyer)-\d+\.\d+\.\d+-win-x64\.zip$").unwrap());
static FSE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:Lounge|Foyer)-FSE-\d+\.\d+\.\d+\.zip$").unwrap());
static VERSION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:-([\w.]+))?$").unwrap());
static LOCATION_TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/releases/tag/v?([^/?#]+)").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallType {
    Nsis,
    Zip,
    Fse,
}

fn asset_matches(t: InstallType, name: &str) -> bool {
    let re = match t {
        InstallType::Nsis => &*NSIS_RE,
        InstallType::Zip => &*ZIP_RE,
        InstallType::Fse => &*FSE_RE,
    };
    re.is_match(name).unwrap_or(false)
}

/// The file each install type downloads, by version (used when the release list comes from github.com
/// rather than the API — see [`Updater::check_without_api`]).
pub fn asset_name(t: InstallType, v: &str) -> String {
    match t {
        InstallType::Nsis => format!("Lounge-Setup-{v}.exe"),
        InstallType::Zip => format!("Lounge-{v}-win-x64.zip"),
        InstallType::Fse => format!("Lounge-FSE-{v}.zip"),
    }
}

/// "<sha256>  <file name>" lines, as `sha256sum` writes them, to `{ name: "sha256:<hex>" }`.
pub fn parse_checksums(text: &str) -> HashMap<String, String> {
    static LINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^([0-9a-f]{64})\s+\*?(.+?)\s*$").unwrap());
    let mut out = HashMap::new();
    for line in text.replace("\r\n", "\n").split('\n') {
        if let Ok(Some(caps)) = LINE_RE.captures(line) {
            out.insert(caps[2].to_string(), format!("sha256:{}", caps[1].to_lowercase()));
        }
    }
    out
}

struct ParsedVersion {
    nums: [i64; 3],
    pre: Option<String>,
}

fn parse_version(v: &str) -> Option<ParsedVersion> {
    let caps = VERSION_RE.captures(v.trim()).ok().flatten()?;
    Some(ParsedVersion {
        nums: [
            caps.get(1)?.as_str().parse().ok()?,
            caps.get(2).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0),
            caps.get(3).map(|m| m.as_str().parse().unwrap_or(0)).unwrap_or(0),
        ],
        pre: caps.get(4).map(|m| m.as_str().to_string()),
    })
}

/// `> 0` when `a` is newer than `b`. Pre-releases sort before the release they precede.
pub fn compare_versions(a: &str, b: &str) -> i64 {
    let (Some(x), Some(y)) = (parse_version(a), parse_version(b)) else { return 0 };
    for i in 0..3 {
        if x.nums[i] != y.nums[i] {
            return x.nums[i] - y.nums[i];
        }
    }
    match (&x.pre, &y.pre) {
        (None, None) => 0,
        (None, Some(_)) => 1,
        (Some(_), None) => -1,
        (Some(xp), Some(yp)) if xp == yp => 0,
        (Some(xp), Some(yp)) => {
            if xp < yp {
                -1
            } else {
                1
            }
        }
    }
}

pub struct DetectInstallOpts<'a> {
    pub packaged: bool,
    pub windows_store: bool,
    pub exe_path: &'a Path,
    pub is_windows: bool,
}

/// How this copy of Lounge was installed, which decides the update file and method.
pub fn detect_install_type(opts: &DetectInstallOpts) -> Option<InstallType> {
    if !opts.packaged || !opts.is_windows {
        return None;
    }
    if opts.windows_store {
        return Some(InstallType::Fse);
    }
    let dir = opts.exe_path.parent()?;
    for f in ["Uninstall Lounge.exe", "Uninstall Foyer.exe"] {
        if dir.join(f).is_file() {
            return Some(InstallType::Nsis);
        }
    }
    Some(InstallType::Zip)
}

// Unpacks a zip update over the install folder once Lounge has quit, then starts it again.
// Re-launches itself elevated if the folder isn't writable (e.g. it lives in Program Files).
const ZIP_APPLY: &str = r#"param([int]$ParentPid, [string]$Zip, [string]$Target, [string]$Exe, [string]$Log)
$ErrorActionPreference = 'Stop'
function Log($m) { Add-Content -Path $Log -Value ("{0:u} {1}" -f (Get-Date), $m) }
try {
  Log "waiting for $ParentPid"
  try { Wait-Process -Id $ParentPid -Timeout 90 -ErrorAction SilentlyContinue } catch {}
  Start-Sleep -Milliseconds 700
  $probe = Join-Path $Target ('.lounge-write-test-' + [guid]::NewGuid())
  try { Set-Content -Path $probe -Value 'x'; Remove-Item -Force $probe }
  catch {
    Log 'folder not writable, asking for admin rights'
    $a = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"' + $MyInvocation.MyCommand.Path + '"'), '-ParentPid', '0', '-Zip', ('"' + $Zip + '"'), '-Target', ('"' + $Target + '"'), '-Exe', ('"' + $Exe + '"'), '-Log', ('"' + $Log + '"'))
    Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $a
    exit
  }
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ('lounge-update-' + [guid]::NewGuid())
  Expand-Archive -Path $Zip -DestinationPath $tmp -Force
  $found = Get-ChildItem -Path $tmp -Filter $Exe -Recurse | Select-Object -First 1
  if (-not $found) {
    # Renamed since this copy was installed (Foyer.exe -> Lounge.exe): restart whichever the update contains.
    $found = Get-ChildItem -Path $tmp -Include 'Lounge.exe', 'Foyer.exe' -Recurse | Select-Object -First 1
    if ($found) { $Exe = $found.Name }
  }
  if (-not $found) { throw "$Exe not found in the update" }
  Log "copying $($found.DirectoryName) -> $Target"
  Copy-Item -Path (Join-Path $found.DirectoryName '*') -Destination $Target -Recurse -Force
  Remove-Item -Recurse -Force $tmp
  Remove-Item -Force $Zip -ErrorAction SilentlyContinue
  Log 'done, restarting'
} catch {
  Log "failed: $_"
}
Start-Process -FilePath (Join-Path $Target $Exe)
"#;

// FSE: unpacks the update and starts the package's installer with admin rights, reporting each step in
// a status file that Lounge watches (see wait_for_fse). Windows doesn't show its permission prompt in
// the full screen experience, so the script switches to the desktop first, and the installer switches
// back after. Lounge starts it through WMI so that it runs outside the MSIX package.
const FSE_APPLY: &str = r#"param([string]$Zip, [string]$Status, [string]$Log)
$ErrorActionPreference = 'Stop'
function Log($m) { Add-Content -Path $Log -Value ("{0:u} {1}" -f (Get-Date), $m) }
function Report($s) { Set-Content -Path $Status -Value $s }
Add-Type -Namespace LoungeUpd -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr FindWindow(string c, string t);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
'@
# The full screen experience hides the taskbar; Win+F11 switches between it and the desktop.
function DesktopShown { $t = [LoungeUpd.Win]::FindWindow('Shell_TrayWnd', $null); ($t -ne [IntPtr]::Zero) -and [LoungeUpd.Win]::IsWindowVisible($t) }
function ToggleFse {
  [LoungeUpd.Win]::keybd_event(0x5B, 0, 0, [UIntPtr]::Zero); [LoungeUpd.Win]::keybd_event(0x7A, 0, 0, [UIntPtr]::Zero)
  [LoungeUpd.Win]::keybd_event(0x7A, 0, 2, [UIntPtr]::Zero); [LoungeUpd.Win]::keybd_event(0x5B, 0, 2, [UIntPtr]::Zero)
}
try {
  Report 'unpacking'
  Log "unpacking $Zip"
  $tmp = Join-Path (Split-Path -Parent $Zip) 'fse'
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  Expand-Archive -Path $Zip -DestinationPath $tmp -Force
  $installer = Get-ChildItem -Path $tmp -Filter 'Install-Lounge-FSE.ps1' -Recurse | Select-Object -First 1
  if (-not $installer) { throw 'installer not found in the update' }
  $a = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden', '-File', ('"' + $installer.FullName + '"'), '-Quiet', '-Update', '-Launch', '-Log', ('"' + $Log + '"'))
  if (-not (DesktopShown)) {
    Report 'desktop'
    Log 'switching to the desktop for the permission prompt'
    ToggleFse
    for ($i = 0; $i -lt 20 -and -not (DesktopShown); $i++) { Start-Sleep -Milliseconds 250 }
    if (DesktopShown) { $a += '-ReturnToFse' } else { Log 'the desktop did not appear' }
  }
  Report 'asking'
  Log "asking for admin rights to run $($installer.FullName)"
  try { Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $a }
  catch {
    Log "admin rights not given: $_"
    Report 'declined'
    exit
  }
  Log 'installer started'
  Report 'elevated'
} catch {
  Log "failed: $_"
  Report "failed: $_"
}
"#;

/// The one-liner that starts a command through WMI (`Win32_Process.Create`), outside Lounge's package.
pub fn wmi_launch(command_line: &str) -> String {
    let quoted = command_line.replace('\'', "''");
    format!("$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{{ CommandLine = '{quoted}' }}; exit [int]$r.ReturnValue")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unsupported,
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Ready,
    Installing,
    Elevating,
    Error,
}

#[derive(Debug, Clone)]
pub struct UpdateState {
    pub status: Status,
    pub current: String,
    pub version: Option<String>,
    pub notes: String,
    pub progress: u8,
    pub size: u64,
    pub error: Option<String>,
    pub checked_at: i64,
}

#[derive(Debug, Clone)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
    digest: Option<String>,
}

// Mirrors the JS version's `this.release`: kept for parity and potential future use (e.g. a "view full
// release notes" command), but like the JS version, nothing reads it back yet — `notes` in UpdateState
// is what the UI actually sees.
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct Release {
    tag_name: String,
    body: String,
}

pub struct UpdaterConfig {
    pub repo: String,
    pub version: String,
    pub install_type: Option<InstallType>,
    pub dir: PathBuf,
}

pub struct InstallCommand {
    pub command: String,
    pub args: Vec<String>,
    pub script: Option<PathBuf>,
    pub status: Option<PathBuf>,
}

pub struct Updater {
    repo: String,
    version: String,
    install_type: Option<InstallType>,
    dir: PathBuf,
    api_base: String,
    web_base: String,
    state: RwLock<UpdateState>,
    release: Mutex<Option<Release>>,
    asset: Mutex<Option<Asset>>,
    file: Mutex<Option<PathBuf>>,
    on_state: Box<dyn Fn(&UpdateState) + Send + Sync>,
}

impl Updater {
    pub fn new(cfg: UpdaterConfig, on_state: impl Fn(&UpdateState) + Send + Sync + 'static) -> Self {
        let status = if cfg.install_type.is_some() { Status::Idle } else { Status::Unsupported };
        Updater {
            repo: cfg.repo,
            version: cfg.version.clone(),
            install_type: cfg.install_type,
            dir: cfg.dir,
            api_base: DEFAULT_API_BASE.to_string(),
            web_base: DEFAULT_WEB_BASE.to_string(),
            state: RwLock::new(UpdateState { status, current: cfg.version, version: None, notes: String::new(), progress: 0, size: 0, error: None, checked_at: 0 }),
            release: Mutex::new(None),
            asset: Mutex::new(None),
            file: Mutex::new(None),
            on_state: Box::new(on_state),
        }
    }

    #[cfg(test)]
    fn with_bases(mut self, api_base: impl Into<String>, web_base: impl Into<String>) -> Self {
        self.api_base = api_base.into();
        self.web_base = web_base.into();
        self
    }

    pub fn supported(&self) -> bool {
        self.install_type.is_some()
    }

    pub fn state(&self) -> UpdateState {
        self.state.read().unwrap().clone()
    }

    pub fn file(&self) -> Option<PathBuf> {
        self.file.lock().unwrap().clone()
    }

    fn mutate(&self, f: impl FnOnce(&mut UpdateState)) {
        {
            let mut st = self.state.write().unwrap();
            f(&mut st);
        }
        (self.on_state)(&self.state());
    }

    /// Look for a newer release. Resolves with the state.
    pub fn check(&self) -> UpdateState {
        if !self.supported() {
            return self.state();
        }
        if matches!(self.state().status, Status::Downloading | Status::Elevating | Status::Installing) {
            return self.state();
        }
        self.mutate(|s| {
            s.status = Status::Checking;
            s.error = None;
        });

        if let Err(e) = self.check_via_api() {
            self.mutate(|s| {
                s.status = Status::Error;
                s.error = Some(e);
                s.checked_at = now_millis();
            });
        }
        self.state()
    }

    fn check_via_api(&self) -> Result<(), String> {
        let install_type = self.install_type.ok_or_else(|| "not supported".to_string())?;
        let url = format!("{}/repos/{}/releases/latest", self.api_base, self.repo);
        let mut resp = ureq::get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "Lounge-updater")
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        if status == 404 {
            self.mutate(|s| {
                s.status = Status::UpToDate;
                s.checked_at = now_millis();
            });
            return Ok(());
        }
        if status == 403 || status == 429 {
            return self.check_without_api();
        }
        if !(200..300).contains(&status) {
            return Err(format!("GitHub {status}"));
        }
        let rel: Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
        let latest = rel.get("tag_name").and_then(Value::as_str).unwrap_or("").trim_start_matches('v').to_string();
        // Releases may also carry Foyer-named copies (for updating Foyer 2.0.0): prefer the Lounge files.
        let assets: Vec<Asset> = rel.get("assets").and_then(Value::as_array).cloned().unwrap_or_default().iter().filter_map(asset_from_json).collect();
        let matching: Vec<&Asset> = assets.iter().filter(|a| asset_matches(install_type, &a.name)).collect();
        let chosen = matching.iter().find(|a| a.name.to_lowercase().starts_with("lounge-")).or_else(|| matching.first()).map(|a| (*a).clone());

        if compare_versions(&latest, &self.version) > 0 {
            if let Some(asset) = chosen {
                let notes: String = rel.get("body").and_then(Value::as_str).unwrap_or("").chars().take(4000).collect();
                *self.release.lock().unwrap() = Some(Release { tag_name: format!("v{latest}"), body: notes.clone() });
                let size = asset.size;
                *self.asset.lock().unwrap() = Some(asset);
                self.mutate(|s| {
                    s.status = Status::Available;
                    s.version = Some(latest.clone());
                    s.notes = notes.clone();
                    s.size = size;
                    s.checked_at = now_millis();
                });
                return Ok(());
            }
        }
        self.mutate(|s| {
            s.status = Status::UpToDate;
            s.version = if latest.is_empty() { None } else { Some(latest.clone()) };
            s.checked_at = now_millis();
        });
        Ok(())
    }

    /// The same check without the API: github.com/<repo>/releases/latest redirects to the latest tag,
    /// the file name follows from the version, and the release's SHA256SUMS.txt supplies the checksum
    /// to verify against.
    fn check_without_api(&self) -> Result<(), String> {
        let install_type = self.install_type.ok_or_else(|| "not supported".to_string())?;
        let url = format!("{}/{}/releases/latest", self.web_base, self.repo);
        let resp = ureq::get(&url).header("User-Agent", "Lounge-updater").config().max_redirects(0).http_status_as_error(false).build().call().map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let tag = LOCATION_TAG_RE.captures(location.as_str()).ok().flatten().map(|c| c[1].to_string());
        let Some(tag) = tag else {
            return Err(if status == 404 { "GitHub 404".to_string() } else { format!("GitHub {status} (rate limited)") });
        };
        let latest = percent_encoding::percent_decode_str(&tag).decode_utf8_lossy().to_string();
        if compare_versions(&latest, &self.version) <= 0 {
            self.mutate(|s| {
                s.status = Status::UpToDate;
                s.version = Some(latest.clone());
                s.checked_at = now_millis();
            });
            return Ok(());
        }
        let base = format!("{}/{}/releases/download/v{latest}", self.web_base, self.repo);
        let sums_resp = ureq::get(format!("{base}/{CHECKSUMS}")).header("User-Agent", "Lounge-updater").config().http_status_as_error(false).build().call();
        let digests = match sums_resp {
            Ok(mut r) if (200..300).contains(&r.status().as_u16()) => parse_checksums(&r.body_mut().read_to_string().unwrap_or_default()),
            _ => HashMap::new(),
        };
        let name = asset_name(install_type, &latest);
        // Without a checksum to verify against, don't install; the API route will work again within the hour.
        let Some(digest) = digests.get(&name).cloned() else {
            return Err("GitHub 403 (rate limited)".to_string());
        };
        let download_url = format!("{base}/{}", utf8_percent_encode(&name, URI_COMPONENT));
        *self.release.lock().unwrap() = Some(Release { tag_name: format!("v{latest}"), body: String::new() });
        *self.asset.lock().unwrap() = Some(Asset { name, browser_download_url: download_url, digest: Some(digest), size: 0 });
        self.mutate(|s| {
            s.status = Status::Available;
            s.version = Some(latest.clone());
            s.notes = String::new();
            s.size = 0;
            s.checked_at = now_millis();
        });
        Ok(())
    }

    /// Download the update for this install type, verifying its digest.
    pub fn download(&self) -> Result<PathBuf, String> {
        let asset = self.asset.lock().unwrap().clone().ok_or_else(|| "no update available".to_string())?;
        self.mutate(|s| {
            s.status = Status::Downloading;
            s.progress = 0;
            s.error = None;
        });
        let result = self.download_inner(&asset);
        if let Err(e) = &result {
            self.mutate(|s| {
                s.status = Status::Error;
                s.error = Some(e.clone());
            });
        }
        result
    }

    fn download_inner(&self, asset: &Asset) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let base_name = Path::new(&asset.name).file_name().and_then(|n| n.to_str()).unwrap_or(&asset.name).to_string();
        // Clear out older downloads.
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy() != base_name {
                    let p = entry.path();
                    if p.is_dir() {
                        let _ = std::fs::remove_dir_all(&p);
                    } else {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
        let dest = self.dir.join(&base_name);
        let mut resp = ureq::get(&asset.browser_download_url).header("User-Agent", "Lounge-updater").config().http_status_as_error(false).build().call().map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("download failed ({status})"));
        }
        let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok()).unwrap_or(asset.size);

        let partial = suffixed(&dest, ".partial");
        let mut file = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut got: u64 = 0;
        let mut last_emit = Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(Instant::now);
        let mut buf = [0u8; 65536];
        let mut reader = resp.body_mut().as_reader();
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            got += n as u64;
            if total > 0 && last_emit.elapsed() > Duration::from_millis(300) {
                last_emit = Instant::now();
                let pct = ((got as f64 / total as f64) * 100.0).round().min(99.0) as u8;
                self.mutate(|s| s.progress = pct);
            }
        }
        drop(file);
        drop(reader);

        let digest_hex = hex::encode(hasher.finalize());
        if let Some(d) = &asset.digest {
            if let Some(expected) = d.strip_prefix("sha256:") {
                if expected.to_lowercase() != digest_hex {
                    let _ = std::fs::remove_file(&partial);
                    return Err("the download was corrupted (checksum mismatch)".to_string());
                }
            }
        }
        std::fs::rename(&partial, &dest).map_err(|e| e.to_string())?;
        *self.file.lock().unwrap() = Some(dest.clone());
        self.mutate(|s| {
            s.status = Status::Ready;
            s.progress = 100;
        });
        Ok(dest)
    }

    /// The command that installs the downloaded update. The caller spawns it detached and quits Lounge.
    pub fn install_command(&self, pid: u32, exe_path: &Path) -> Result<InstallCommand, String> {
        let file = self.file.lock().unwrap().clone().ok_or_else(|| "nothing downloaded".to_string())?;
        let install_type = self.install_type.ok_or_else(|| "not supported".to_string())?;
        let log = self.dir.join("update.log");

        if install_type != InstallType::Fse {
            self.mutate(|s| s.status = Status::Installing);
        }

        match install_type {
            InstallType::Nsis => {
                // electron-builder's NSIS installer: /S = silent, --force-run = start Lounge when done.
                Ok(InstallCommand { command: file.to_string_lossy().to_string(), args: vec!["/S".into(), "--updated".into(), "--force-run".into()], script: None, status: None })
            }
            InstallType::Zip => {
                let exe_dir = exe_path.parent().unwrap_or_else(|| Path::new("."));
                let exe_name = exe_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                self.write_ps1(
                    ZIP_APPLY,
                    "apply-zip.ps1",
                    vec!["-ParentPid".into(), pid.to_string(), "-Zip".into(), file.to_string_lossy().to_string(), "-Target".into(), exe_dir.to_string_lossy().to_string(), "-Exe".into(), exe_name.to_string(), "-Log".into(), log.to_string_lossy().to_string()],
                )
            }
            InstallType::Fse => {
                // Run the script outside the package, then wait_for_fse() follows it. Lounge stays open
                // until the installer has its admin rights, so a declined (or invisible) prompt leaves
                // it running with an error.
                let status_file = self.dir.join("fse-status.txt");
                let _ = std::fs::remove_file(&status_file);
                let cmd = self.write_ps1(FSE_APPLY, "apply-fse.ps1", vec![])?;
                let script = cmd.script.clone().unwrap();
                let line = format!("powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\" -Zip \"{}\" -Status \"{}\" -Log \"{}\"", script.display(), file.display(), status_file.display(), log.display());
                self.mutate(|s| s.status = Status::Elevating);
                Ok(InstallCommand { command: "powershell.exe".into(), args: vec!["-NoProfile".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-Command".into(), wmi_launch(&line)], script: None, status: Some(status_file) })
            }
        }
    }

    fn write_ps1(&self, script: &str, name: &str, extra_args: Vec<String>) -> Result<InstallCommand, String> {
        let p = self.dir.join(name);
        let content = format!("\u{feff}{}", script.replace('\n', "\r\n"));
        std::fs::write(&p, content.as_bytes()).map_err(|e| e.to_string())?;
        let mut args = vec!["-NoProfile".to_string(), "-ExecutionPolicy".to_string(), "Bypass".to_string(), "-WindowStyle".to_string(), "Hidden".to_string(), "-File".to_string(), p.to_string_lossy().to_string()];
        args.extend(extra_args);
        Ok(InstallCommand { command: "powershell.exe".to_string(), args, script: Some(p), status: None })
    }

    /// Follow the FSE install script through its status file until the installer has admin rights.
    /// Returns "elevated", "declined", "noprompt" (nobody answered the prompt: in the full screen
    /// experience Windows doesn't show it) or "failed: <reason>".
    pub fn wait_for_fse(&self, status_file: &Path, opts: &WaitForFseOpts) -> String {
        wait_for_fse_impl(status_file, opts)
    }
}

fn asset_from_json(a: &Value) -> Option<Asset> {
    Some(Asset {
        name: a.get("name")?.as_str()?.to_string(),
        size: a.get("size").and_then(Value::as_u64).unwrap_or(0),
        browser_download_url: a.get("browser_download_url").and_then(Value::as_str).unwrap_or("").to_string(),
        digest: a.get("digest").and_then(Value::as_str).map(String::from),
    })
}

fn suffixed(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

#[derive(Debug, Clone, Copy)]
pub struct WaitForFseOpts {
    pub unpack: Duration,
    pub prompt: Duration,
    pub every: Duration,
}

impl Default for WaitForFseOpts {
    fn default() -> Self {
        WaitForFseOpts { unpack: Duration::from_secs(5 * 60), prompt: Duration::from_secs(90), every: Duration::from_millis(500) }
    }
}

fn wait_for_fse_impl(status_file: &Path, opts: &WaitForFseOpts) -> String {
    let read = || std::fs::read_to_string(status_file).unwrap_or_default().trim().to_string();
    let mut since = Instant::now();
    let mut last = String::new();
    loop {
        let now = read();
        if now != last {
            last = now.clone();
            since = Instant::now();
        }
        if now == "elevated" || now == "declined" || now.starts_with("failed") {
            return now;
        }
        if now == "asking" && since.elapsed() > opts.prompt {
            return "noprompt".to_string();
        }
        if now != "asking" && since.elapsed() > opts.unpack {
            return "failed: the update script didn't start".to_string();
        }
        thread::sleep(opts.every);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::tempdir;

    // Parity with the JS test "version comparison".
    #[test]
    fn version_comparison() {
        assert!(compare_versions("2.1.0", "2.0.9") > 0);
        assert!(compare_versions("v2.0.10", "2.0.9") > 0);
        assert_eq!(compare_versions("2.0.0", "2.0.0"), 0);
        assert!(compare_versions("2.0.0", "2.0.0-beta.1") > 0);
        assert!(compare_versions("1.9.9", "2.0.0") < 0);
        assert!(compare_versions("2.0.0", "0.0") > 0);
        assert_eq!(compare_versions("2", "2.0.0"), 0);
        assert_eq!(compare_versions("garbage", "1.0.0"), 0);
    }

    // Parity with the JS test "install type detection".
    #[test]
    fn install_type_detection() {
        let dir = tempdir().unwrap();
        let exe_path = dir.path().join("Lounge.exe");
        assert_eq!(detect_install_type(&DetectInstallOpts { packaged: false, windows_store: false, exe_path: &exe_path, is_windows: true }), None, "dev builds never update");
        assert_eq!(detect_install_type(&DetectInstallOpts { packaged: true, windows_store: false, exe_path: &exe_path, is_windows: false }), None);
        assert_eq!(detect_install_type(&DetectInstallOpts { packaged: true, windows_store: true, exe_path: &exe_path, is_windows: true }), Some(InstallType::Fse));
        assert_eq!(detect_install_type(&DetectInstallOpts { packaged: true, windows_store: false, exe_path: &exe_path, is_windows: true }), Some(InstallType::Zip));
        std::fs::write(dir.path().join("Uninstall Lounge.exe"), "").unwrap();
        assert_eq!(detect_install_type(&DetectInstallOpts { packaged: true, windows_store: false, exe_path: &exe_path, is_windows: true }), Some(InstallType::Nsis));

        // Installed back when the app was called Foyer.
        let old = tempdir().unwrap();
        std::fs::write(old.path().join("Uninstall Foyer.exe"), "").unwrap();
        let foyer_exe = old.path().join("Foyer.exe");
        assert_eq!(detect_install_type(&DetectInstallOpts { packaged: true, windows_store: false, exe_path: &foyer_exe, is_windows: true }), Some(InstallType::Nsis));
    }

    fn release_payload(payload: &[u8], digest_of: &[u8], prefix: &str, asset_base: &str) -> Value {
        let sha = hex::encode(Sha256::digest(digest_of));
        let names = [format!("{prefix}-Setup-2.1.0.exe"), format!("{prefix}-2.1.0-win-x64.zip"), format!("{prefix}-FSE-2.1.0.zip"), format!("{prefix}-Setup-2.1.0.exe.blockmap")];
        let assets: Vec<Value> = names
            .iter()
            .map(|name| json!({"name": name, "size": payload.len(), "digest": format!("sha256:{sha}"), "browser_download_url": format!("{asset_base}/{name}")}))
            .collect();
        json!({"tag_name": "v2.1.0", "body": "## What's Changed\n* Faster things", "assets": assets})
    }

    // Parity with "finds the right asset for each install type, downloads and verifies it".
    #[test]
    fn finds_the_right_asset_downloads_and_verifies_it() {
        let payload = vec![7u8; 300000];
        for (install_type, name) in [(InstallType::Nsis, "Lounge-Setup-2.1.0.exe"), (InstallType::Zip, "Lounge-2.1.0-win-x64.zip"), (InstallType::Fse, "Lounge-FSE-2.1.0.zip")] {
            let dir = tempdir().unwrap();
            let api = MockServer::start();
            let rel = release_payload(&payload, &payload, "Lounge", &api.base_url());
            api.mock(|when, then| {
                when.method(GET).path("/repos/o/r/releases/latest");
                then.status(200).json_body(rel.clone());
            });
            api.mock(|when, then| {
                when.method(GET).path(format!("/{name}"));
                then.status(200).body(payload.clone());
            });

            let updates = Arc::new(AtomicUsize::new(0));
            let updates_clone = Arc::clone(&updates);
            let u = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.0.0".into(), install_type: Some(install_type), dir: dir.path().to_path_buf() }, move |_| {
                updates_clone.fetch_add(1, Ordering::SeqCst);
            })
            .with_bases(api.base_url(), api.base_url());

            let st = u.check();
            assert_eq!(st.status, Status::Available, "{install_type:?}");
            assert_eq!(st.version.as_deref(), Some("2.1.0"));
            let file = u.download().unwrap();
            assert_eq!(file.file_name().unwrap().to_str().unwrap(), name);
            assert_eq!(std::fs::metadata(&file).unwrap().len(), payload.len() as u64);
            assert_eq!(u.state().status, Status::Ready);

            let cmd = u.install_command(123, &dir.path().join("Lounge.exe")).unwrap();
            match install_type {
                InstallType::Nsis => assert_eq!(cmd.args, vec!["/S", "--updated", "--force-run"]),
                InstallType::Fse => {
                    assert_eq!(u.state().status, Status::Elevating);
                    let i = cmd.args.iter().position(|a| a == "-Command").unwrap();
                    let launcher = &cmd.args[i + 1];
                    assert!(launcher.contains("Invoke-CimMethod -ClassName Win32_Process -MethodName Create"));
                    assert!(launcher.contains(&format!("-Zip \"{}\"", file.display())));
                    assert!(launcher.contains(&format!("-Status \"{}\"", cmd.status.as_ref().unwrap().display())));
                    let applied = std::fs::read_to_string(dir.path().join("apply-fse.ps1")).unwrap();
                    assert!(applied.starts_with("\u{feff}param("));
                    // Windows only shows the permission prompt on the desktop: the script leaves the FSE.
                    assert!(applied.find("    ToggleFse\r\n").unwrap() < applied.find("Report 'asking'").unwrap());
                    assert!(applied.contains("$a += '-ReturnToFse'"));
                }
                InstallType::Zip => {
                    assert_eq!(cmd.command, "powershell.exe");
                    let i = cmd.args.iter().position(|a| a == "-File").unwrap();
                    let script = std::fs::read_to_string(&cmd.args[i + 1]).unwrap();
                    assert!(script.starts_with("\u{feff}param("), "script written with a BOM for Windows PowerShell");
                    assert!(cmd.args.contains(&"123".to_string()));
                }
            }
        }
    }

    // Parity with "rejects a corrupted download".
    #[test]
    fn rejects_a_corrupted_download() {
        let dir = tempdir().unwrap();
        let payload = vec![1u8; 1000];
        let api = MockServer::start();
        let rel = release_payload(&payload, b"something else", "Lounge", &api.base_url());
        api.mock(|when, then| {
            when.method(GET).path("/repos/o/r/releases/latest");
            then.status(200).json_body(rel);
        });
        api.mock(|when, then| {
            when.method(GET);
            then.status(200).body(payload.clone());
        });
        let u = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.0.0".into(), install_type: Some(InstallType::Zip), dir: dir.path().to_path_buf() }, |_| {}).with_bases(api.base_url(), api.base_url());
        u.check();
        let err = u.download().unwrap_err();
        assert!(err.contains("checksum"));
        assert_eq!(u.state().status, Status::Error);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "the bad file is removed");
    }

    // Parity with "up to date, no release yet, and unsupported installs".
    #[test]
    fn up_to_date_no_release_yet_and_unsupported_installs() {
        let dir = tempdir().unwrap();
        let api = MockServer::start();
        api.mock(|when, then| {
            when.method(GET).path("/repos/o/r/releases/latest");
            then.status(200).json_body(json!({"tag_name": "v2.0.0", "assets": []}));
        });
        let u = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.0.0".into(), install_type: Some(InstallType::Zip), dir: dir.path().to_path_buf() }, |_| {}).with_bases(api.base_url(), api.base_url());
        assert_eq!(u.check().status, Status::UpToDate);

        let api2 = MockServer::start();
        api2.mock(|when, then| {
            when.method(GET);
            then.status(404);
        });
        let u2 = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.0.0".into(), install_type: Some(InstallType::Zip), dir: dir.path().to_path_buf() }, |_| {}).with_bases(api2.base_url(), api2.base_url());
        assert_eq!(u2.check().status, Status::UpToDate);

        let none = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.0.0".into(), install_type: None, dir: dir.path().to_path_buf() }, |_| {});
        assert_eq!(none.state().status, Status::Unsupported);
        assert_eq!(none.check().status, Status::Unsupported);
    }

    // Parity with "releases published under the old name (Foyer) are still found".
    #[test]
    fn releases_published_under_the_old_name_are_still_found() {
        let dir = tempdir().unwrap();
        let payload = vec![3u8; 1000];
        let api = MockServer::start();
        let rel = release_payload(&payload, &payload, "Foyer", &api.base_url());
        api.mock(|when, then| {
            when.method(GET).path("/repos/o/r/releases/latest");
            then.status(200).json_body(rel);
        });
        let u = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.0.0".into(), install_type: Some(InstallType::Fse), dir: dir.path().to_path_buf() }, |_| {}).with_bases(api.base_url(), api.base_url());
        let st = u.check();
        assert_eq!(st.status, Status::Available);
    }

    // Parity with "with both names in a release, the Lounge files are preferred".
    #[test]
    fn with_both_names_the_lounge_files_are_preferred() {
        let payload = vec![5u8; 1000];
        let api = MockServer::start();
        let mut rel = release_payload(&payload, &payload, "Foyer", &api.base_url());
        let lounge_rel = release_payload(&payload, &payload, "Lounge", &api.base_url());
        rel["assets"].as_array_mut().unwrap().extend(lounge_rel["assets"].as_array().unwrap().clone());
        api.mock(|when, then| {
            when.method(GET).path("/repos/o/r/releases/latest");
            then.status(200).json_body(rel);
        });
        for (install_type, name) in [(InstallType::Nsis, "Lounge-Setup-2.1.0.exe"), (InstallType::Zip, "Lounge-2.1.0-win-x64.zip"), (InstallType::Fse, "Lounge-FSE-2.1.0.zip")] {
            let dir = tempdir().unwrap();
            let u = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.0.0".into(), install_type: Some(install_type), dir: dir.path().to_path_buf() }, |_| {}).with_bases(api.base_url(), api.base_url());
            u.check();
            assert_eq!(u.asset.lock().unwrap().as_ref().unwrap().name, name);
        }
    }

    // Parity with "rate-limited API: falls back to github.com and verifies against SHA256SUMS.txt".
    #[test]
    fn rate_limited_api_falls_back_to_github_com() {
        let sha = hex::encode(Sha256::digest([9u8; 5000]));
        let checks = parse_checksums(&format!("{sha}  Lounge-FSE-2.2.0.zip\r\n{} *Lounge-Setup-2.2.0.exe\n", "a".repeat(64)));
        let mut expected = HashMap::new();
        expected.insert("Lounge-FSE-2.2.0.zip".to_string(), format!("sha256:{sha}"));
        expected.insert("Lounge-Setup-2.2.0.exe".to_string(), format!("sha256:{}", "a".repeat(64)));
        assert_eq!(checks, expected);

        let payload = vec![9u8; 5000];
        let dir = tempdir().unwrap();

        let api = MockServer::start();
        api.mock(|when, then| {
            when.method(GET);
            then.status(403).json_body(json!({"message": "API rate limit exceeded"}));
        });
        let web = MockServer::start();
        web.mock(|when, then| {
            when.method(GET).path("/o/r/releases/latest");
            then.status(302).header("location", format!("{}/o/r/releases/tag/v2.2.0", web.base_url()));
        });
        web.mock(|when, then| {
            when.method(GET).path("/o/r/releases/download/v2.2.0/SHA256SUMS.txt");
            then.status(200).body(format!("{sha}  Lounge-FSE-2.2.0.zip\n"));
        });
        web.mock(|when, then| {
            when.method(GET).path("/o/r/releases/download/v2.2.0/Lounge-FSE-2.2.0.zip");
            then.status(200).body(payload.clone());
        });

        let u = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.1.2".into(), install_type: Some(InstallType::Fse), dir: dir.path().to_path_buf() }, |_| {}).with_bases(api.base_url(), web.base_url());
        let st = u.check();
        assert_eq!(st.status, Status::Available);
        assert_eq!(st.version.as_deref(), Some("2.2.0"));
        u.download().unwrap();
        assert_eq!(u.state().status, Status::Ready);
        assert_eq!(std::fs::read(u.file().unwrap()).unwrap(), payload);

        // Already up to date: no downloads at all.
        let web2 = MockServer::start();
        web2.mock(|when, then| {
            when.method(GET).path("/o/r/releases/latest");
            then.status(302).header("location", format!("{}/o/r/releases/tag/v2.2.0", web2.base_url()));
        });
        web2.mock(|when, then| {
            when.method(GET).path("/o/r/releases/download/v2.2.0/SHA256SUMS.txt");
            then.status(404);
        });
        let current = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.2.0".into(), install_type: Some(InstallType::Fse), dir: dir.path().to_path_buf() }, |_| {}).with_bases(api.base_url(), web2.base_url());
        assert_eq!(current.check().status, Status::UpToDate);

        // A newer release without a checksum file: nothing to verify against, so no update is offered.
        let older = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.1.2".into(), install_type: Some(InstallType::Fse), dir: dir.path().to_path_buf() }, |_| {}).with_bases(api.base_url(), web2.base_url());
        let err_state = older.check();
        assert_eq!(err_state.status, Status::Error);
        assert!(err_state.error.unwrap().contains("rate limited"));
    }

    // Parity with "FSE install: follows the script until the installer has admin rights".
    #[test]
    fn fse_install_follows_the_script_until_admin_rights() {
        let dir = tempdir().unwrap();
        let status = dir.path().join("fse-status.txt");
        let u = Updater::new(UpdaterConfig { repo: "o/r".into(), version: "2.1.2".into(), install_type: Some(InstallType::Fse), dir: dir.path().to_path_buf() }, |_| {});
        let fast = WaitForFseOpts { unpack: Duration::from_millis(200), prompt: Duration::from_millis(150), every: Duration::from_millis(10) };

        let after = |ms: u64, text: &'static str, status: PathBuf| {
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(ms));
                std::fs::write(&status, format!("{text}\r\n")).unwrap();
            });
        };
        after(20, "unpacking", status.clone());
        after(60, "asking", status.clone());
        after(100, "elevated", status.clone());
        assert_eq!(u.wait_for_fse(&status, &fast), "elevated");

        std::fs::write(&status, "declined").unwrap();
        assert_eq!(u.wait_for_fse(&status, &fast), "declined");

        std::fs::write(&status, "asking").unwrap(); // the prompt never answered (the FSE hides it)
        assert_eq!(u.wait_for_fse(&status, &fast), "noprompt");

        std::fs::write(&status, "failed: installer not found in the update").unwrap();
        assert_eq!(u.wait_for_fse(&status, &fast), "failed: installer not found in the update");

        std::fs::remove_file(&status).unwrap();
        assert!(u.wait_for_fse(&status, &fast).starts_with("failed: "));
        assert!(u.wait_for_fse(&status, &fast).ends_with("didn't start"));

        assert_eq!(wmi_launch("a 'b'"), "$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = 'a ''b''' }; exit [int]$r.ReturnValue");
    }
}
