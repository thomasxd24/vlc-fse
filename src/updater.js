'use strict';

const { EventEmitter } = require('events');
const crypto = require('crypto');
const fs = require('fs');
const fsp = require('fs/promises');
const path = require('path');
const { Readable } = require('stream');
const { pipeline } = require('stream/promises');

/*
 * Self-updater driven by GitHub Releases. Works for all three ways Foyer can be installed:
 *   nsis  Foyer-Setup-<v>.exe      run silently over the existing install, which relaunches Foyer
 *   zip   Foyer-<v>-win-x64.zip    unpacked over the current folder by a small PowerShell script
 *   fse   Foyer-FSE-<v>.zip        the package's own installer updates the MSIX (one UAC prompt)
 * Nothing is installed without the user saying so; downloads are verified against the SHA-256 digest
 * GitHub publishes for each release asset.
 */

const ASSET_PATTERNS = {
  nsis: /^Foyer-Setup-\d+\.\d+\.\d+\.exe$/i,
  zip: /^Foyer-\d+\.\d+\.\d+-win-x64\.zip$/i,
  fse: /^Foyer-FSE-\d+\.\d+\.\d+\.zip$/i
};

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

/** How this copy of Foyer was installed, which decides the update file and method. */
function detectInstallType({ packaged, windowsStore, exePath, platform = process.platform }) {
  if (!packaged || platform !== 'win32') return null;
  if (windowsStore) return 'fse';
  if (fs.existsSync(path.join(path.dirname(exePath), 'Uninstall Foyer.exe'))) return 'nsis';
  return 'zip';
}

// Unpacks a zip update over the install folder once Foyer has quit, then starts it again.
// Re-launches itself elevated if the folder isn't writable (e.g. it lives in Program Files).
const ZIP_APPLY = String.raw`param([int]$ParentPid, [string]$Zip, [string]$Target, [string]$Exe, [string]$Log)
$ErrorActionPreference = 'Stop'
function Log($m) { Add-Content -Path $Log -Value ("{0:u} {1}" -f (Get-Date), $m) }
try {
  Log "waiting for $ParentPid"
  try { Wait-Process -Id $ParentPid -Timeout 90 -ErrorAction SilentlyContinue } catch {}
  Start-Sleep -Milliseconds 700
  $probe = Join-Path $Target ('.foyer-write-test-' + [guid]::NewGuid())
  try { Set-Content -Path $probe -Value 'x'; Remove-Item -Force $probe }
  catch {
    Log 'folder not writable, asking for admin rights'
    $a = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"' + $MyInvocation.MyCommand.Path + '"'), '-ParentPid', '0', '-Zip', ('"' + $Zip + '"'), '-Target', ('"' + $Target + '"'), '-Exe', ('"' + $Exe + '"'), '-Log', ('"' + $Log + '"'))
    Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $a
    exit
  }
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ('foyer-update-' + [guid]::NewGuid())
  Expand-Archive -Path $Zip -DestinationPath $tmp -Force
  $found = Get-ChildItem -Path $tmp -Filter $Exe -Recurse | Select-Object -First 1
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

// Runs the FSE package's installer from the downloaded zip in update mode.
const FSE_APPLY = String.raw`param([int]$ParentPid, [string]$Zip, [string]$Log)
$ErrorActionPreference = 'Stop'
function Log($m) { Add-Content -Path $Log -Value ("{0:u} {1}" -f (Get-Date), $m) }
try {
  try { Wait-Process -Id $ParentPid -Timeout 90 -ErrorAction SilentlyContinue } catch {}
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ('foyer-fse-update-' + [guid]::NewGuid())
  Expand-Archive -Path $Zip -DestinationPath $tmp -Force
  $installer = Get-ChildItem -Path $tmp -Filter 'Install-Foyer-FSE.ps1' -Recurse | Select-Object -First 1
  if (-not $installer) { throw 'installer not found in the update' }
  Log "running $($installer.FullName)"
  & $installer.FullName -Quiet -Update -Launch
  Log 'installer started'
} catch {
  Log "failed: $_"
}
`;

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
    this.api = process.env.FOYER_UPDATE_API || 'https://api.github.com'; // override only for testing
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
    if (!this.supported || ['downloading', 'installing'].includes(this.state.status)) return this.state;
    this.set({ status: 'checking', error: null });
    try {
      const res = await fetch(`${this.api}/repos/${this.repo}/releases/latest`, {
        headers: { Accept: 'application/vnd.github+json', 'User-Agent': 'Foyer-updater' },
        signal: AbortSignal.timeout(15000)
      });
      if (res.status === 404) {
        // No release published yet.
        this.set({ status: 'uptodate', checkedAt: Date.now() });
        return this.state;
      }
      if (!res.ok) throw new Error(`GitHub ${res.status}`);
      const rel = await res.json();
      const latest = String(rel.tag_name || '').replace(/^v/, '');
      const asset = (rel.assets || []).find((a) => ASSET_PATTERNS[this.installType].test(a.name));
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

  /** Download the update for this install type, verifying its digest. */
  async download() {
    if (!this.asset) throw new Error('no update available');
    this.set({ status: 'downloading', progress: 0, error: null });
    try {
      await fsp.mkdir(this.dir, { recursive: true });
      // Clear out older downloads.
      for (const f of await fsp.readdir(this.dir)) if (f !== path.basename(this.asset.name)) await fsp.rm(path.join(this.dir, f), { force: true, recursive: true });
      const dest = path.join(this.dir, path.basename(this.asset.name));
      const res = await fetch(this.asset.browser_download_url, { headers: { 'User-Agent': 'Foyer-updater' } });
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
   * The command that installs the downloaded update. The caller spawns it detached and quits Foyer.
   * @returns {{command: string, args: string[]}}
   */
  async installCommand({ pid, exePath }) {
    if (!this.file) throw new Error('nothing downloaded');
    const log = path.join(this.dir, 'update.log');
    const ps = (script, name, args) => {
      const p = path.join(this.dir, name);
      fs.writeFileSync(p, '﻿' + script.replace(/\r?\n/g, '\r\n'));
      return { command: 'powershell.exe', args: ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden', '-File', p, ...args] };
    };
    this.set({ status: 'installing' });
    if (this.installType === 'nsis') {
      // electron-builder's NSIS installer: /S = silent, --force-run = start Foyer when done.
      return { command: this.file, args: ['/S', '--updated', '--force-run'] };
    }
    if (this.installType === 'zip') {
      return ps(ZIP_APPLY, 'apply-zip.ps1', ['-ParentPid', String(pid), '-Zip', this.file, '-Target', path.dirname(exePath), '-Exe', path.basename(exePath), '-Log', log]);
    }
    return ps(FSE_APPLY, 'apply-fse.ps1', ['-ParentPid', String(pid), '-Zip', this.file, '-Log', log]);
  }
}

module.exports = { Updater, compareVersions, detectInstallType, ASSET_PATTERNS, ZIP_APPLY, FSE_APPLY };
