'use strict';

const { EventEmitter } = require('events');
const crypto = require('crypto');
const fs = require('fs');
const fsp = require('fs/promises');
const path = require('path');
const { Readable } = require('stream');
const { pipeline } = require('stream/promises');

/*
 * Self-updater driven by GitHub Releases. Works for all three ways Lounge can be installed:
 *   nsis  Lounge-Setup-<v>.exe      run silently over the existing install, which relaunches Lounge
 *   zip   Lounge-<v>-win-x64.zip    unpacked over the current folder by a small PowerShell script
 *   fse   Lounge-FSE-<v>.zip        the package's own installer updates the MSIX (one UAC prompt, which
 *                                   Windows only shows on the desktop, not in the full screen experience)
 * Nothing is installed without the user saying so; downloads are verified against the SHA-256 digest
 * GitHub publishes for each release asset.
 */

// The app used to be called Foyer: releases published under that name still count.
const ASSET_PATTERNS = {
  nsis: /^(?:Lounge|Foyer)-Setup-\d+\.\d+\.\d+\.exe$/i,
  zip: /^(?:Lounge|Foyer)-\d+\.\d+\.\d+-win-x64\.zip$/i,
  fse: /^(?:Lounge|Foyer)-FSE-\d+\.\d+\.\d+\.zip$/i
};

// The file each install type downloads, by version (used when the release list comes from github.com rather
// than the API, see checkWithoutApi).
const ASSET_NAMES = {
  nsis: (v) => `Lounge-Setup-${v}.exe`,
  zip: (v) => `Lounge-${v}-win-x64.zip`,
  fse: (v) => `Lounge-FSE-${v}.zip`
};
const CHECKSUMS = 'SHA256SUMS.txt';

/** "<sha256>  <file name>" lines, as sha256sum writes them, to { name: 'sha256:<hex>' }. */
function parseChecksums(text) {
  const out = {};
  for (const line of String(text || '').split(/\r?\n/)) {
    const m = /^([0-9a-f]{64})\s+\*?(.+?)\s*$/i.exec(line);
    if (m) out[m[2]] = `sha256:${m[1].toLowerCase()}`;
  }
  return out;
}

function parseVersion(v) {
  const m = /^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:-([\w.]+))?$/.exec(String(v || '').trim());
  return m ? { nums: [Number(m[1]), Number(m[2] || 0), Number(m[3] || 0)], pre: m[4] || null } : null;
}

/** > 0 when a is newer than b. Pre-releases sort before the release they precede. */
function compareVersions(a, b) {
  const x = parseVersion(a);
  const y = parseVersion(b);
  if (!x || !y) return 0;
  for (let i = 0; i < 3; i++) if (x.nums[i] !== y.nums[i]) return x.nums[i] - y.nums[i];
  if (x.pre === y.pre) return 0;
  if (!x.pre) return 1;
  if (!y.pre) return -1;
  return x.pre < y.pre ? -1 : 1;
}

/** How this copy of Lounge was installed, which decides the update file and method. */
function detectInstallType({ packaged, windowsStore, exePath, platform = process.platform }) {
  if (!packaged || platform !== 'win32') return null;
  if (windowsStore) return 'fse';
  const dir = path.dirname(exePath);
  if (['Uninstall Lounge.exe', 'Uninstall Foyer.exe'].some((f) => fs.existsSync(path.join(dir, f)))) return 'nsis';
  return 'zip';
}

// Unpacks a zip update over the install folder once Lounge has quit, then starts it again.
// Re-launches itself elevated if the folder isn't writable (e.g. it lives in Program Files).
const ZIP_APPLY = String.raw`param([int]$ParentPid, [string]$Zip, [string]$Target, [string]$Exe, [string]$Log)
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
`;

// FSE: unpacks the update and starts the package's installer with admin rights, reporting each step in a
// status file that Lounge watches (see waitForFse). Lounge starts it through WMI so that it runs outside the
// MSIX package: a process started directly from Lounge belongs to the package, and Windows tears it down
// with Lounge before it gets anywhere (no log line, no admin prompt, the old version back).
const FSE_APPLY = String.raw`param([string]$Zip, [string]$Status, [string]$Log)
$ErrorActionPreference = 'Stop'
function Log($m) { Add-Content -Path $Log -Value ("{0:u} {1}" -f (Get-Date), $m) }
function Report($s) { Set-Content -Path $Status -Value $s }
try {
  Report 'unpacking'
  Log "unpacking $Zip"
  $tmp = Join-Path (Split-Path -Parent $Zip) 'fse'
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  Expand-Archive -Path $Zip -DestinationPath $tmp -Force
  $installer = Get-ChildItem -Path $tmp -Filter 'Install-Lounge-FSE.ps1' -Recurse | Select-Object -First 1
  if (-not $installer) { throw 'installer not found in the update' }
  Report 'asking'
  Log "asking for admin rights to run $($installer.FullName)"
  $a = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden', '-File', ('"' + $installer.FullName + '"'), '-Quiet', '-Update', '-Launch', '-Log', ('"' + $Log + '"'))
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
`;

// The one-liner that starts a command through WMI (Win32_Process.Create), outside Lounge's package.
function wmiLaunch(commandLine) {
  const quoted = commandLine.replace(/'/g, "''");
  return `$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = '${quoted}' }; exit [int]$r.ReturnValue`;
}

class Updater extends EventEmitter {
  /**
   * @param {{repo: string, version: string, installType: string|null, dir: string}} opts
   */
  constructor({ repo, version, installType, dir }) {
    super();
    this.repo = repo;
    this.version = version;
    this.installType = installType;
    this.dir = dir;
    this.api = process.env.LOUNGE_UPDATE_API || 'https://api.github.com'; // override only for testing
    this.web = process.env.LOUNGE_UPDATE_WEB || 'https://github.com';
    this.release = null;
    this.asset = null;
    this.file = null;
    this.state = { status: installType ? 'idle' : 'unsupported', current: version, version: null, notes: '', progress: 0, size: 0, error: null, checkedAt: 0 };
  }

  get supported() {
    return Boolean(this.installType);
  }

  set(patch) {
    this.state = { ...this.state, ...patch };
    this.emit('state', this.state);
  }

  /** Look for a newer release. Resolves with the state. */
  async check() {
    if (!this.supported || ['downloading', 'elevating', 'installing'].includes(this.state.status)) return this.state;
    this.set({ status: 'checking', error: null });
    try {
      const res = await fetch(`${this.api}/repos/${this.repo}/releases/latest`, {
        headers: { Accept: 'application/vnd.github+json', 'User-Agent': 'Lounge-updater' },
        signal: AbortSignal.timeout(15000)
      });
      if (res.status === 404) {
        // No release published yet.
        this.set({ status: 'uptodate', checkedAt: Date.now() });
        return this.state;
      }
      // 403/429: the API's hourly limit for this network (60 anonymous requests, shared by every device on
      // it) is used up. The release pages on github.com aren't limited that way.
      if (res.status === 403 || res.status === 429) return await this.checkWithoutApi();
      if (!res.ok) throw new Error(`GitHub ${res.status}`);
      const rel = await res.json();
      const latest = String(rel.tag_name || '').replace(/^v/, '');
      // Releases may also carry Foyer-named copies (for updating Foyer 2.0.0): prefer the Lounge files.
      const matches = (rel.assets || []).filter((a) => ASSET_PATTERNS[this.installType].test(a.name));
      const asset = matches.find((a) => /^Lounge-/i.test(a.name)) || matches[0];
      if (compareVersions(latest, this.version) > 0 && asset) {
        this.release = rel;
        this.asset = asset;
        this.set({ status: 'available', version: latest, notes: String(rel.body || '').slice(0, 4000), size: asset.size || 0, checkedAt: Date.now() });
      } else {
        this.set({ status: 'uptodate', version: latest || null, checkedAt: Date.now() });
      }
    } catch (err) {
      this.set({ status: 'error', error: err.message, checkedAt: Date.now() });
    }
    return this.state;
  }

  /**
   * The same check without the API: github.com/<repo>/releases/latest redirects to the latest tag, the file
   * name follows from the version, and the release's SHA256SUMS.txt supplies the checksum to verify against.
   */
  async checkWithoutApi() {
    const res = await fetch(`${this.web}/${this.repo}/releases/latest`, { redirect: 'manual', headers: { 'User-Agent': 'Lounge-updater' }, signal: AbortSignal.timeout(15000) });
    const m = /\/releases\/tag\/v?([^/?#]+)/.exec(res.headers.get('location') || '');
    if (!m) throw new Error(res.status === 404 ? 'GitHub 404' : `GitHub ${res.status} (rate limited)`);
    const latest = decodeURIComponent(m[1]);
    if (compareVersions(latest, this.version) <= 0) {
      this.set({ status: 'uptodate', version: latest, checkedAt: Date.now() });
      return this.state;
    }
    const base = `${this.web}/${this.repo}/releases/download/v${latest}`;
    const sums = await fetch(`${base}/${CHECKSUMS}`, { headers: { 'User-Agent': 'Lounge-updater' }, signal: AbortSignal.timeout(15000) });
    const digests = sums.ok ? parseChecksums(await sums.text()) : {};
    const name = ASSET_NAMES[this.installType](latest);
    // Without a checksum to verify against, don't install; the API route will work again within the hour.
    if (!digests[name]) throw new Error('GitHub 403 (rate limited)');
    this.release = { tag_name: `v${latest}`, body: '' };
    this.asset = { name, browser_download_url: `${base}/${encodeURIComponent(name)}`, digest: digests[name], size: 0 };
    this.set({ status: 'available', version: latest, notes: '', size: 0, checkedAt: Date.now() });
    return this.state;
  }

  /** Download the update for this install type, verifying its digest. */
  async download() {
    if (!this.asset) throw new Error('no update available');
    this.set({ status: 'downloading', progress: 0, error: null });
    try {
      await fsp.mkdir(this.dir, { recursive: true });
      // Clear out older downloads.
      for (const f of await fsp.readdir(this.dir)) if (f !== path.basename(this.asset.name)) await fsp.rm(path.join(this.dir, f), { force: true, recursive: true });
      const dest = path.join(this.dir, path.basename(this.asset.name));
      const res = await fetch(this.asset.browser_download_url, { headers: { 'User-Agent': 'Lounge-updater' } });
      if (!res.ok || !res.body) throw new Error(`download failed (${res.status})`);
      const total = Number(res.headers.get('content-length')) || this.asset.size || 0;
      const hash = crypto.createHash('sha256');
      let got = 0;
      let lastEmit = 0;
      const body = Readable.fromWeb(res.body);
      body.on('data', (chunk) => {
        hash.update(chunk);
        got += chunk.length;
        const now = Date.now();
        if (total && now - lastEmit > 300) {
          lastEmit = now;
          this.set({ progress: Math.min(99, Math.round((got / total) * 100)) });
        }
      });
      await pipeline(body, fs.createWriteStream(dest + '.partial'));
      const digest = String(this.asset.digest || '');
      if (digest.startsWith('sha256:') && digest.slice(7).toLowerCase() !== hash.digest('hex')) {
        await fsp.rm(dest + '.partial', { force: true });
        throw new Error('the download was corrupted (checksum mismatch)');
      }
      await fsp.rename(dest + '.partial', dest);
      this.file = dest;
      this.set({ status: 'ready', progress: 100 });
      return dest;
    } catch (err) {
      this.set({ status: 'error', error: err.message });
      throw err;
    }
  }

  /**
   * The command that installs the downloaded update. The caller spawns it detached and quits Lounge.
   * @returns {{command: string, args: string[]}}
   */
  async installCommand({ pid, exePath }) {
    if (!this.file) throw new Error('nothing downloaded');
    const log = path.join(this.dir, 'update.log');
    const ps = (script, name, args) => {
      const p = path.join(this.dir, name);
      fs.writeFileSync(p, '﻿' + script.replace(/\r?\n/g, '\r\n'));
      return { command: 'powershell.exe', args: ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden', '-File', p, ...args], script: p };
    };
    if (this.installType !== 'fse') this.set({ status: 'installing' });
    if (this.installType === 'nsis') {
      // electron-builder's NSIS installer: /S = silent, --force-run = start Lounge when done.
      return { command: this.file, args: ['/S', '--updated', '--force-run'] };
    }
    if (this.installType === 'zip') {
      return ps(ZIP_APPLY, 'apply-zip.ps1', ['-ParentPid', String(pid), '-Zip', this.file, '-Target', path.dirname(exePath), '-Exe', path.basename(exePath), '-Log', log]);
    }
    // FSE: run the script outside the package, then waitForFse() follows it. Lounge stays open until the
    // installer has its admin rights, so a declined (or invisible) prompt leaves it running with an error.
    const status = path.join(this.dir, 'fse-status.txt');
    fs.rmSync(status, { force: true });
    const script = ps(FSE_APPLY, 'apply-fse.ps1', []).script;
    const line = `powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "${script}" -Zip "${this.file}" -Status "${status}" -Log "${log}"`;
    this.set({ status: 'elevating' });
    return { command: 'powershell.exe', args: ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', wmiLaunch(line)], status };
  }

  /**
   * Follow the FSE install script through its status file until the installer has admin rights.
   * Resolves 'elevated', 'declined', 'noprompt' (nobody answered the prompt: in the full screen experience
   * Windows doesn't show it) or 'failed: <reason>'.
   */
  async waitForFse(statusFile, { unpackMs = 5 * 60000, promptMs = 90000, everyMs = 500 } = {}) {
    const read = () => {
      try {
        return fs.readFileSync(statusFile, 'utf8').trim();
      } catch {
        return '';
      }
    };
    let since = Date.now();
    let last = '';
    for (;;) {
      const now = read();
      if (now !== last) {
        last = now;
        since = Date.now();
      }
      if (now === 'elevated' || now === 'declined' || now.startsWith('failed')) return now;
      if (now === 'asking' && Date.now() - since > promptMs) return 'noprompt';
      if (now !== 'asking' && Date.now() - since > unpackMs) return 'failed: the update script didn\'t start';
      await new Promise((r) => setTimeout(r, everyMs));
    }
  }
}

module.exports = { parseChecksums, ASSET_NAMES, Updater, compareVersions, detectInstallType, ASSET_PATTERNS, ZIP_APPLY, FSE_APPLY, wmiLaunch };
