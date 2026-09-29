//! Platform glue: opening things in the OS shell, detached spawns, the display-awake request, exe icons
//! and launch-at-login. Everything Windows-specific in here mirrors what the Tauri shell did.

use std::path::Path;

pub(super) fn open_url_in_shell(url: &str) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        lounge_core::hidden_command("cmd").args(["/C", "start", "", url]).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ())
    }
}

pub(super) fn open_path_in_shell(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe").arg(path).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
}

pub(super) fn reveal_in_shell(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        // `explorer /select,` highlights the item in Explorer.
        std::process::Command::new("explorer.exe").arg(format!("/select,{}", path.to_string_lossy())).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").args(["-R", &path.to_string_lossy()]).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(parent) = path.parent() {
            std::process::Command::new("xdg-open").arg(parent).spawn().map(|_| ())
        } else {
            Err(std::io::Error::other("no parent"))
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) fn spawn_detached(command: &str, args: &[String]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(command)
        .args(args)
        .creation_flags(0x00000008 | 0x08000000) // DETACHED_PROCESS | CREATE_NO_WINDOW
        .spawn()
        .map(|_| ())
}

#[cfg(not(target_os = "windows"))]
pub(super) fn spawn_detached(command: &str, args: &[String]) -> std::io::Result<()> {
    std::process::Command::new(command).args(args).spawn().map(|_| ())
}

/// Keep the display awake during playback (Electron's powerSaveBlocker). Windows: one call per
/// process is enough; stop only fires when playback is truly done.
pub(super) fn power_save_blocker_start() {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
        }
    }
}

pub(super) fn power_save_blocker_stop() {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

/// The icon of a manual game's exe, saved as a PNG in the artwork folder (`app.getFileIcon` was the
/// Electron way; here a one-shot PowerShell `ExtractAssociatedIcon` call does the same job).
#[cfg(target_os = "windows")]
pub(super) fn icon_for(artwork_dir: &Path, exe: &str) -> Option<String> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(exe.to_lowercase());
    let hash: String = b64.chars().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
    let _ = std::fs::create_dir_all(artwork_dir);
    let dest = artwork_dir.join(format!("icon-{hash}.png"));
    let script = format!(
        "Add-Type -AssemblyName System.Drawing; $i = [System.Drawing.Icon]::ExtractAssociatedIcon('{}'); if ($i) {{ $i.ToBitmap().Save('{}', [System.Drawing.Imaging.ImageFormat]::Png) }}",
        exe.replace('\'', "''"),
        dest.to_string_lossy().replace('\'', "''")
    );
    let ok = lounge_core::hidden_command("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    (ok && dest.is_file()).then(|| dest.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "windows"))]
pub(super) fn icon_for(_artwork_dir: &Path, _exe: &str) -> Option<String> {
    None
}

// ---------------------------------------------------------------------------
// Launch at login

/// The `Run` value's name under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
const AUTOSTART_VALUE: &str = "Lounge";
const AUTOSTART_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// `reg.exe` arguments that enable (`Some(exe)`) or disable (`None`) launch at login.
fn autostart_args(exe: Option<&Path>) -> Vec<String> {
    match exe {
        Some(exe) => vec![
            "add".into(),
            AUTOSTART_KEY.into(),
            "/v".into(),
            AUTOSTART_VALUE.into(),
            "/t".into(),
            "REG_SZ".into(),
            "/d".into(),
            format!("\"{}\"", exe.to_string_lossy()),
            "/f".into(),
        ],
        None => vec!["delete".into(), AUTOSTART_KEY.into(), "/v".into(), AUTOSTART_VALUE.into(), "/f".into()],
    }
}

/// Register or remove Lounge in the current user's Run key. Windows only; a no-op elsewhere.
/// (Electron's `app.setLoginItemSettings({ openAtLogin })`.)
pub(super) fn set_autostart(enabled: bool) {
    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe().ok();
        let args = autostart_args(if enabled { exe.as_deref() } else { None });
        let _ = lounge_core::hidden_command("reg.exe").args(args).output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_commands_target_the_run_key() {
        let on = autostart_args(Some(Path::new(r"C:\Program Files\Lounge\lounge.exe")));
        assert_eq!(on[0], "add");
        assert!(on.contains(&r#""C:\Program Files\Lounge\lounge.exe""#.to_string()));
        assert!(on.iter().any(|a| a.ends_with(r"CurrentVersion\Run")));
        let off = autostart_args(None);
        assert_eq!(off[0], "delete");
        assert_eq!(off.last().map(String::as_str), Some("/f"));
    }
}
