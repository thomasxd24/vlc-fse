//! Wi-Fi status, sleep/restart/shutdown, process priority, and battery detection. Direct port of the
//! self-contained parts of `src/system.js`.
//!
//! `SystemHelper` — the class that spawns a long-lived PowerShell process and talks to it over a
//! JSON-line stdin/stdout protocol for volume/brightness/sleep (COM/WMI calls .NET doesn't expose any
//! other way) — is **not** ported here, for the same reason as `games::GameSession` and
//! `vlc::VlcSession`: no test exists to verify a port against, and its request/response bookkeeping
//! (matching replies to pending calls by id, timeouts, an idle-based auto-stop) is exactly the kind of
//! async state machine that should be designed against the Tauri `app` crate's actual event/command
//! shape once that exists, not guessed at now.
//!
//! Everything here is Windows-only and untestable on this Linux sandbox (no test existed for any of it
//! in the JS suite either) — cross-checked with `cargo check/clippy --target x86_64-pc-windows-gnu` only.

#[cfg(any(test, target_os = "windows"))]
use fancy_regex::Regex;
#[cfg(target_os = "windows")]
use std::process::Command;
#[cfg(any(test, target_os = "windows"))]
use std::sync::LazyLock;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiStatus {
    pub connected: bool,
    pub ssid: Option<String>,
    pub signal: Option<i32>,
}

// Only ever called from wifi_impl (Windows) or the tests below; cfg-gated so a non-Windows,
// non-test build doesn't warn (or fail clippy -D warnings) on otherwise-dead code.
#[cfg(any(test, target_os = "windows"))]
static SSID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*SSID\s*:\s*(.+)$").unwrap());
#[cfg(any(test, target_os = "windows"))]
static SIGNAL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?mi)^\s*Signal\s*:\s*(\d+)\s*%").unwrap());

/// Parse `netsh wlan show interfaces`' output. Pure and platform-independent, so it's testable here even
/// though the command that produces this text only exists on Windows.
#[cfg(any(test, target_os = "windows"))]
fn parse_netsh_output(stdout: &str) -> WifiStatus {
    let ssid = SSID_RE.captures(stdout).ok().flatten();
    let signal = SIGNAL_RE.captures(stdout).ok().flatten();
    match (ssid, signal) {
        (Some(s), Some(sig)) => WifiStatus { connected: true, ssid: Some(s[1].trim().to_string()), signal: sig[1].parse().ok() },
        _ => WifiStatus { connected: false, ssid: None, signal: None },
    }
}

/// Wi-Fi state from `netsh` (works on any Windows display language: we only rely on "SSID" and "%").
pub fn wifi() -> Option<WifiStatus> {
    wifi_impl()
}

#[cfg(target_os = "windows")]
fn wifi_impl() -> Option<WifiStatus> {
    let output = Command::new("netsh").args(["wlan", "show", "interfaces"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_netsh_output(&String::from_utf8_lossy(&output.stdout)))
}
#[cfg(not(target_os = "windows"))]
fn wifi_impl() -> Option<WifiStatus> {
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Sleep,
    Restart,
    Shutdown,
}

/// Sleep, restart or shut down the machine. `sleep` is called for [`PowerAction::Sleep`] instead of
/// shelling out directly, since that goes through `SystemHelper`'s PowerShell process (not ported here —
/// see the module doc) rather than a one-shot command.
pub fn power(action: PowerAction, sleep: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
    if !cfg!(target_os = "windows") {
        return Err(std::io::Error::other("not supported"));
    }
    match action {
        PowerAction::Sleep => sleep(),
        PowerAction::Restart => power_impl("/r"),
        PowerAction::Shutdown => power_impl("/s"),
    }
}

#[cfg(target_os = "windows")]
fn power_impl(flag: &str) -> std::io::Result<()> {
    Command::new("shutdown").args([flag, "/t", "0"]).status().map(|_| ())
}
#[cfg(not(target_os = "windows"))]
fn power_impl(_flag: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("not supported"))
}

/// Set the CPU priority of every Lounge process (main, renderer, GPU…). Uses `SetPriorityClass`
/// directly (like Node's `os.setPriority` does on Windows via libuv) rather than shelling out, since
/// this can run once per process at startup/resume for several pids.
pub fn set_priority(pids: &[u32], low: bool) {
    for &pid in pids {
        set_priority_impl(pid, low);
    }
}

#[cfg(target_os = "windows")]
fn set_priority_impl(pid: u32, low: bool) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, SetPriorityClass, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_SET_INFORMATION};
    unsafe {
        let handle = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
        if handle.is_null() {
            return;
        }
        SetPriorityClass(handle, if low { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS });
        CloseHandle(handle);
    }
}
#[cfg(not(target_os = "windows"))]
fn set_priority_impl(_pid: u32, _low: bool) {}

/// Whether the machine has a real battery, or `None` when unknown. Meant to be asked once at startup.
pub fn has_battery() -> Option<bool> {
    has_battery_impl()
}

#[cfg(target_os = "windows")]
fn has_battery_impl() -> Option<bool> {
    let output = Command::new("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "@(Get-CimInstance -ClassName Win32_Battery).Count"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse::<i64>().ok().map(|n| n > 0)
}
#[cfg(not(target_os = "windows"))]
fn has_battery_impl() -> Option<bool> {
    None
}

/// How long `SystemHelper` (not yet ported) stays alive without a call before stopping itself.
pub const HELPER_IDLE: Duration = Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use super::*;

    // No JS test exists for any of this (nothing in test/*.test.js references system.js), and it's all
    // Windows-only. These just exercise the pure regex-matching shape of wifi_impl's parsing on
    // arbitrary text, independent of actually running netsh.
    #[test]
    fn parses_connected_netsh_output() {
        let sample = "    Name                   : Wi-Fi\n    SSID                   : My Network\n    Signal                 : 87%\n";
        assert_eq!(parse_netsh_output(sample), WifiStatus { connected: true, ssid: Some("My Network".into()), signal: Some(87) });
    }

    #[test]
    fn parses_disconnected_netsh_output() {
        let sample = "There is no wireless interface on the system.\n";
        assert_eq!(parse_netsh_output(sample), WifiStatus { connected: false, ssid: None, signal: None });
    }

    #[test]
    fn power_returns_an_error_off_windows() {
        if cfg!(target_os = "windows") {
            return; // this test only documents the non-Windows guard
        }
        assert!(power(PowerAction::Restart, || Ok(())).is_err());
    }

    #[test]
    fn set_priority_is_a_no_op_off_windows_and_does_not_panic() {
        if !cfg!(target_os = "windows") {
            set_priority(&[u32::MAX], true); // an invalid pid; must not panic
        }
    }
}
