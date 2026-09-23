'use strict';

const { spawn, execFile } = require('child_process');
const os = require('os');

const IDLE_MS = 60 * 1000;

// Runs inside a PowerShell process that stays alive while the quick menu is in use. Volume goes through the
// Core Audio COM API (IAudioEndpointVolume); brightness through WMI, which drives built-in panels like the
// Legion Go's. One JSON request per stdin line, one JSON reply per stdout line.
const HELPER = String.raw`
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
public class FoyerAudio {
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
      'getVolume' { $v = [int][math]::Round([FoyerAudio]::Volume * 100) }
      'setVolume' {
        $n = [math]::Max(0, [math]::Min(100, [double]$req.arg))
        [FoyerAudio]::Volume = [float]($n / 100)
        if ($n -gt 0 -and [FoyerAudio]::Mute) { [FoyerAudio]::Mute = $false }
        $v = [int]$n
      }
      'getMute' { $v = [FoyerAudio]::Mute }
      'setMute' { [FoyerAudio]::Mute = [bool]$req.arg; $v = [bool]$req.arg }
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
`;

class SystemHelper {
  constructor() {
    this.proc = null;
    this.pending = new Map();
    this.seq = 0;
    this.buf = '';
    this.ready = null;
    this.idle = null;
  }

  get supported() {
    return process.platform === 'win32';
  }

  start() {
    if (this.proc) return this.ready;
    const encoded = Buffer.from(HELPER, 'utf16le').toString('base64');
    this.proc = spawn('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', encoded], {
      windowsHide: true,
      stdio: ['pipe', 'pipe', 'ignore']
    });
    this.ready = new Promise((resolve, reject) => {
      this.onReady = resolve;
      this.proc.once('error', reject);
    });
    this.proc.stdout.setEncoding('utf8');
    this.proc.stdout.on('data', (chunk) => {
      this.buf += chunk;
      let nl;
      while ((nl = this.buf.indexOf('\n')) >= 0) {
        const line = this.buf.slice(0, nl).trim();
        this.buf = this.buf.slice(nl + 1);
        if (!line) continue;
        let msg;
        try {
          msg = JSON.parse(line);
        } catch {
          continue;
        }
        if (msg.ready) this.onReady();
        const p = msg.id !== undefined && this.pending.get(msg.id);
        if (p) {
          this.pending.delete(msg.id);
          msg.ok ? p.resolve(msg.value) : p.reject(new Error(msg.error));
        }
      }
    });
    this.proc.on('exit', () => {
      for (const p of this.pending.values()) p.reject(new Error('helper exited'));
      this.pending.clear();
      this.proc = null;
    });
    return this.ready;
  }

  async call(cmd, arg) {
    if (!this.supported) throw new Error('not supported on this platform');
    await this.start();
    clearTimeout(this.idle);
    this.idle = setTimeout(() => this.stop(), IDLE_MS);
    const id = ++this.seq;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.proc.stdin.write(JSON.stringify({ id, cmd, arg }) + '\n');
      setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error('helper timeout'));
      }, 15000);
    });
  }

  stop() {
    clearTimeout(this.idle);
    if (this.proc) {
      this.proc.stdin.end();
      this.proc.kill();
      this.proc = null;
    }
  }
}

/** Wi-Fi state from `netsh` (works on any Windows display language: we only rely on "SSID" and "%"). */
function wifi() {
  return new Promise((resolve) => {
    if (process.platform !== 'win32') return resolve(null);
    execFile('netsh', ['wlan', 'show', 'interfaces'], { windowsHide: true, timeout: 5000 }, (err, stdout) => {
      if (err || !stdout) return resolve(null);
      const ssid = /^\s*SSID\s*:\s*(.+)$/m.exec(stdout);
      const signal = /^\s*Signal\s*:\s*(\d+)\s*%/im.exec(stdout);
      resolve(ssid && signal ? { connected: true, ssid: ssid[1].trim(), signal: Number(signal[1]) } : { connected: false });
    });
  });
}

function power(action, helper) {
  if (process.platform !== 'win32') return Promise.reject(new Error('not supported'));
  if (action === 'sleep') return helper.call('sleep');
  const flag = action === 'restart' ? '/r' : '/s';
  return new Promise((resolve, reject) => execFile('shutdown', [flag, '/t', '0'], { windowsHide: true }, (err) => (err ? reject(err) : resolve())));
}

/** Set the CPU priority of every Foyer process (main, renderer, GPU…). */
function setPriority(pids, low) {
  const prio = low ? os.constants.priority.PRIORITY_LOW : os.constants.priority.PRIORITY_NORMAL;
  for (const pid of pids) {
    try {
      os.setPriority(pid, prio);
    } catch {}
  }
}

/** Whether the machine has a real battery (true/false), or null when unknown. Asked once at startup. */
function hasBattery() {
  return new Promise((resolve) => {
    if (process.platform !== 'win32') return resolve(null);
    execFile(
      'powershell.exe',
      ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', '@(Get-CimInstance -ClassName Win32_Battery).Count'],
      { windowsHide: true, timeout: 10000 },
      (err, stdout) => resolve(err ? null : Number(String(stdout).trim()) > 0)
    );
  });
}

module.exports = { SystemHelper, wifi, power, setPriority, hasBattery, HELPER };
