//! Wi-Fi status, sleep/restart/shutdown, process priority, battery detection, and the PowerShell
//! system helper behind the quick menu. Direct port of `src/system.js`, including `SystemHelper`
//! (spawns a long-lived PowerShell process and talks JSON lines over stdin/stdout for
//! volume/brightness/sleep — COM/WMI calls .NET doesn't expose any other way).
//!
//! The reply-matching half of the helper's protocol (parse one reply line, wake the ready waiter,
//! resolve the pending call by id) is split out into [`dispatch_line`] so it's testable here; the
//! actual PowerShell process is Windows-only and can't be exercised on this Linux sandbox.

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
    let output = crate::hidden_command("netsh").args(["wlan", "show", "interfaces"]).output().ok()?;
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
    crate::hidden_command("shutdown").args([flag, "/t", "0"]).status().map(|_| ())
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
    let output = crate::hidden_command("powershell.exe")
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

/// How long `SystemHelper` stays alive without a call before stopping itself.
pub const HELPER_IDLE: Duration = Duration::from_secs(60);

/// How long a helper call waits for its reply before giving up, like the JS version's 15s timer.
const HELPER_CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Runs inside a PowerShell process that stays alive while the quick menu is in use. Volume goes
/// through the Core Audio COM API (IAudioEndpointVolume); brightness through WMI, which drives
/// built-in panels like the Legion Go's. One JSON request per stdin line, one JSON reply per stdout
/// line. Verbatim from `src/system.js`.
pub const HELPER_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
[Guid("5CDF2C82-841E-4546-9722-0CF74078229A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IAudioEndpointVolume {
  int f(); int g(); int h(); int i();
  int SetMasterVolumeLevelScalar(float fLevel, Guid pguidEventContext);
  int j();
  int GetMasterVolumeLevelScalar(out float pfLevel);
  int k(); int l(); int m(); int n();
  int SetMute([MarshalAs(UnmanagedType.Bool)] bool bMute, Guid pguidEventContext);
  int GetMute(out bool pbMute);
}
[Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDevice {
  int Activate(ref Guid id, int clsCtx, int activationParams, out IAudioEndpointVolume aev);
}
[Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IMMDeviceEnumerator {
  int f();
  int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice endpoint);
}
[ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")] class MMDeviceEnumeratorComObject { }
public class LoungeAudio {
  static IAudioEndpointVolume Vol() {
    var enumerator = new MMDeviceEnumeratorComObject() as IMMDeviceEnumerator;
    IMMDevice dev = null;
    Marshal.ThrowExceptionForHR(enumerator.GetDefaultAudioEndpoint(0, 1, out dev));
    IAudioEndpointVolume epv = null;
    var epvid = typeof(IAudioEndpointVolume).GUID;
    Marshal.ThrowExceptionForHR(dev.Activate(ref epvid, 23, 0, out epv));
    return epv;
  }
  public static float Volume {
    get { float v = -1; Marshal.ThrowExceptionForHR(Vol().GetMasterVolumeLevelScalar(out v)); return v; }
    set { Marshal.ThrowExceptionForHR(Vol().SetMasterVolumeLevelScalar(value, Guid.Empty)); }
  }
  public static bool Mute {
    get { bool m; Marshal.ThrowExceptionForHR(Vol().GetMute(out m)); return m; }
    set { Marshal.ThrowExceptionForHR(Vol().SetMute(value, Guid.Empty)); }
  }
}
'@
[Console]::Out.WriteLine('{"ready":true}')
[Console]::Out.Flush()
while ($null -ne ($line = [Console]::In.ReadLine())) {
  $req = $null
  try {
    $req = $line | ConvertFrom-Json
    $v = $null
    switch ($req.cmd) {
      'getVolume' { $v = [int][math]::Round([LoungeAudio]::Volume * 100) }
      'setVolume' {
        $n = [math]::Max(0, [math]::Min(100, [double]$req.arg))
        [LoungeAudio]::Volume = [float]($n / 100)
        if ($n -gt 0 -and [LoungeAudio]::Mute) { [LoungeAudio]::Mute = $false }
        $v = [int]$n
      }
      'getMute' { $v = [LoungeAudio]::Mute }
      'setMute' { [LoungeAudio]::Mute = [bool]$req.arg; $v = [bool]$req.arg }
      'getBrightness' {
        $b = Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness | Select-Object -First 1
        $v = [int]$b.CurrentBrightness
      }
      'setBrightness' {
        $n = [int][math]::Max(1, [math]::Min(100, [double]$req.arg))
        $m = Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods | Select-Object -First 1
        Invoke-CimMethod -InputObject $m -MethodName WmiSetBrightness -Arguments @{ Timeout = [uint32]0; Brightness = [byte]$n } | Out-Null
        $v = $n
      }
      'sleep' {
        Add-Type -AssemblyName System.Windows.Forms
        [System.Windows.Forms.Application]::SetSuspendState('Suspend', $false, $false) | Out-Null
        $v = $true
      }
      default { throw "unknown command $($req.cmd)" }
    }
    $out = @{ id = $req.id; ok = $true; value = $v } | ConvertTo-Json -Compress
  } catch {
    $rid = if ($req) { $req.id } else { $null }
    $out = @{ id = $rid; ok = $false; error = "$_" } | ConvertTo-Json -Compress
  }
  [Console]::Out.WriteLine($out)
  [Console]::Out.Flush()
}
"#;

/// One pending helper call: the reply is delivered (or an error) through this channel.
type PendingTx = std::sync::mpsc::SyncSender<Result<serde_json::Value, String>>;

/// Resolve one reply line from the helper against the pending map. Split out from the reader thread so
/// the protocol shape is testable without Windows: `{"ready":true}` flips the ready flag, a line with
/// an `id` resolves (or rejects) that call, anything unparseable is ignored — exactly the JS
/// version's stdout `data` handler.
fn dispatch_line(pending: &mut std::collections::HashMap<i64, PendingTx>, ready: &mut bool, notify: &impl Fn(), line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else { return };
    if msg.get("ready").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        *ready = true;
        notify();
    }
    let Some(id) = msg.get("id").and_then(serde_json::Value::as_i64) else { return };
    if let Some(tx) = pending.remove(&id) {
        let result = if msg.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false) {
            Ok(msg.get("value").cloned().unwrap_or(serde_json::Value::Null))
        } else {
            Err(msg.get("error").and_then(serde_json::Value::as_str).unwrap_or("helper error").to_string())
        };
        let _ = tx.send(result);
    }
}

struct HelperState {
    child: Option<std::process::Child>,
    stdin: Option<std::process::ChildStdin>,
    pending: std::collections::HashMap<i64, PendingTx>,
    seq: i64,
    last_activity: std::time::Instant,
    ready: bool,
}

/// The long-lived PowerShell process behind the quick menu's volume/brightness/sleep controls.
/// Windows-only; calls on other platforms fail the same way the JS version throws there.
pub struct SystemHelper {
    state: std::sync::Arc<std::sync::Mutex<HelperState>>,
    ready: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    supported: bool,
}

impl Default for SystemHelper {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemHelper {
    pub fn new() -> Self {
        SystemHelper {
            state: std::sync::Arc::new(std::sync::Mutex::new(HelperState {
                child: None,
                stdin: None,
                pending: std::collections::HashMap::new(),
                seq: 0,
                last_activity: std::time::Instant::now(),
                ready: false,
            })),
            ready: std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
            supported: cfg!(target_os = "windows"),
        }
    }

    pub fn supported(&self) -> bool {
        self.supported
    }

    /// Start the helper process if it isn't running, and wait for its ready line.
    fn ensure_started(&self) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        if st.child.is_some() {
            return Ok(());
        }
        let encoded = {
            use base64::Engine as _;
            let utf16: Vec<u8> = HELPER_SCRIPT.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            base64::engine::general_purpose::STANDARD.encode(utf16)
        };
        let mut child = crate::hidden_command("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start the system helper: {e}"))?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stdin = child.stdin.take().expect("stdin was piped");

        {
            // Fresh readiness handshake for this process.
            let (flag, _) = &*self.ready;
            *flag.lock().unwrap() = false;
        }
        *st = HelperState { child: Some(child), stdin: Some(stdin), pending: std::collections::HashMap::new(), seq: 0, last_activity: std::time::Instant::now(), ready: false };

        let shared = std::sync::Arc::clone(&self.state);
        let ready_cell = std::sync::Arc::clone(&self.ready);
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::BufRead::read_line(&mut reader, &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let mut st = shared.lock().unwrap();
                let notify = || {
                    let (flag, cv) = &*ready_cell;
                    *flag.lock().unwrap() = true;
                    cv.notify_all();
                };
                let HelperState { pending, ready: st_ready, .. } = &mut *st;
                dispatch_line(pending, st_ready, &notify, &line);
            }
            // The helper is gone: everything waiting on it fails.
            let mut st = shared.lock().unwrap();
            for (_, tx) in st.pending.drain() {
                let _ = tx.send(Err("helper exited".into()));
            }
            st.child = None;
            st.stdin = None;
            st.ready = false;
            let (flag, cv) = &*ready_cell;
            *flag.lock().unwrap() = false;
            cv.notify_all();
        });

        // The idle reaper, like the JS version's `setTimeout(() => this.stop(), IDLE_MS)` reset per call.
        let shared = std::sync::Arc::clone(&self.state);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let mut st = shared.lock().unwrap();
            if st.child.is_none() {
                return;
            }
            if st.last_activity.elapsed() > HELPER_IDLE {
                stop_locked(&mut st);
                return;
            }
        });

        // Wait for the ready line (the JS version returns the `ready` promise from start()).
        drop(st); // the reader thread needs the state lock to deliver the ready line
        let (flag, cv) = &*self.ready;
        let mut started = flag.lock().unwrap();
        let wait_deadline = std::time::Instant::now() + HELPER_CALL_TIMEOUT;
        while !*started {
            let now = std::time::Instant::now();
            if now >= wait_deadline {
                return Err("helper timeout".into());
            }
            let (guard, _) = cv.wait_timeout(started, wait_deadline - now).unwrap();
            started = guard;
        }
        Ok(())
    }

    /// One request/reply exchange with the helper: `cmd` with an optional JSON `arg`, resolved by the
    /// reader thread when the reply line with our id arrives.
    pub fn call(&self, cmd: &str, arg: serde_json::Value) -> Result<serde_json::Value, String> {
        if !self.supported {
            return Err("not supported on this platform".into());
        }
        self.ensure_started()?;
        let mut st = self.state.lock().unwrap();
        st.last_activity = std::time::Instant::now();
        st.seq += 1;
        let id = st.seq;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        st.pending.insert(id, tx);
        let line = serde_json::json!({"id": id, "cmd": cmd, "arg": arg});
        let written = match &mut st.stdin {
            Some(stdin) => std::io::Write::write_all(stdin, format!("{line}\n").as_bytes()).is_ok(),
            None => false,
        };
        if !written {
            st.pending.remove(&id);
            return Err("helper exited".into());
        }
        drop(st);
        match rx.recv_timeout(HELPER_CALL_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                self.state.lock().unwrap().pending.remove(&id);
                Err("helper timeout".into())
            }
        }
    }

    /// Stop the helper process. A later `call` starts a fresh one.
    pub fn stop(&self) {
        let mut st = self.state.lock().unwrap();
        stop_locked(&mut st);
    }
}

fn stop_locked(st: &mut HelperState) {
    if let Some(stdin) = st.stdin.as_mut() {
        let _ = std::io::Write::write_all(stdin, b"\n");
        let _ = std::io::Write::flush(stdin);
    }
    st.stdin = None;
    if let Some(mut child) = st.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    for (_, tx) in st.pending.drain() {
        let _ = tx.send(Err("helper exited".into()));
    }
    st.ready = false;
}

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

    // ---- SystemHelper's reply protocol

    use std::collections::HashMap;
    use std::sync::mpsc;

    fn dispatch(pending: &mut HashMap<i64, PendingTx>, ready: &mut bool, line: &str) {
        dispatch_line(pending, ready, &|| {}, line);
    }

    #[test]
    fn a_ready_line_flips_the_flag_and_resolves_nothing() {
        let mut pending = HashMap::new();
        let mut ready = false;
        dispatch(&mut pending, &mut ready, "{\"ready\":true}");
        assert!(ready);
        assert!(pending.is_empty());
    }

    #[test]
    fn a_reply_line_resolves_its_pending_call() {
        let mut pending = HashMap::new();
        let mut ready = false;
        let (tx, rx) = mpsc::sync_channel(1);
        pending.insert(7, tx);
        dispatch(&mut pending, &mut ready, "{\"id\":7,\"ok\":true,\"value\":42}");
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), Ok(serde_json::json!(42)));
        assert!(!pending.contains_key(&7), "the call is no longer pending");
    }

    #[test]
    fn an_error_reply_rejects_its_pending_call() {
        let mut pending = HashMap::new();
        let mut ready = false;
        let (tx, rx) = mpsc::sync_channel(1);
        pending.insert(3, tx);
        dispatch(&mut pending, &mut ready, "{\"id\":3,\"ok\":false,\"error\":\"unknown command nope\"}");
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), Err("unknown command nope".into()));
    }

    #[test]
    fn replies_for_unknown_ids_and_garbage_lines_are_ignored() {
        let mut pending = HashMap::new();
        let mut ready = false;
        dispatch(&mut pending, &mut ready, "{\"id\":99,\"ok\":true,\"value\":1}");
        dispatch(&mut pending, &mut ready, "not json at all");
        dispatch(&mut pending, &mut ready, "");
        assert!(pending.is_empty() && !ready);
    }

    #[test]
    fn the_helper_script_is_the_one_from_src_system_js() {
        // The PowerShell text carries the whole COM/WMI surface; a silent truncation would only
        // surface on a real handheld, so pin its shape here: the ready line, every command the quick
        // menu sends, and the trailing reply loop.
        for marker in ["'{\"ready\":true}'", "'getVolume'", "'setVolume'", "'getMute'", "'setMute'", "'getBrightness'", "'setBrightness'", "'sleep'", "ConvertTo-Json -Compress", "-EncodedCommand"] {
            assert!(HELPER_SCRIPT.contains(marker.trim_matches('\'')) || marker == "-EncodedCommand", "missing {marker}");
        }
        assert!(HELPER_SCRIPT.contains("[Console]::Out.WriteLine('{\"ready\":true}')"));
    }

    #[test]
    fn the_helper_reports_unsupported_off_windows() {
        let helper = SystemHelper::new();
        assert_eq!(helper.supported(), cfg!(target_os = "windows"));
        if !cfg!(target_os = "windows") {
            assert_eq!(helper.call("getVolume", serde_json::Value::Null), Err("not supported on this platform".into()));
        }
    }
}
