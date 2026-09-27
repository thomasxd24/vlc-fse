'use strict';

// Installed applications, as the Start menu knows them: `Get-StartApps` lists desktop programs and Store apps
// alike, each with an AppID that `shell:AppsFolder\<AppID>` launches exactly like a click in Start.

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { execFile, spawn } = require('child_process');

// One PowerShell run: the Start menu's apps, plus where each Store package lives (for its logo).
const LIST_SCRIPT = String.raw`
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'SilentlyContinue'
$apps = @(Get-StartApps | Select-Object Name, AppID)
$pkgs = @{}
foreach ($p in @(Get-AppxPackage)) { if ($p.InstallLocation) { $pkgs[$p.PackageFamilyName] = $p.InstallLocation } }
@{ apps = $apps; packages = $pkgs } | ConvertTo-Json -Depth 4 -Compress
`;

// Known-folder GUIDs that prefix desktop-program AppIDs ("{6D809377-…}\Tailscale\tailscale-ipn.exe").
const KNOWN_FOLDERS = {
  '{6d809377-6af0-444b-8957-a3773f02200e}': (env) => env.ProgramW6432 || env.ProgramFiles, // Program Files (64-bit)
  '{905e63b6-c1bf-494e-b29c-65b732d3d21a}': (env) => env.ProgramFiles,
  '{7c5a40ef-a0fb-4bfc-874a-c0f2e0b9fa8e}': (env) => env['ProgramFiles(x86)'],
  '{f7f1ed05-9f6d-47a2-aaae-29d317c6f066}': (env) => env.CommonProgramFiles,
  '{1ac14e77-02e7-4e5d-b744-2eb1ae5198b7}': (env) => env.SystemRoot && path.win32.join(env.SystemRoot, 'System32'),
  '{d65231b0-b2f1-4857-a4ce-a8e7c6ea7d27}': (env) => env.SystemRoot && path.win32.join(env.SystemRoot, 'SysWOW64'),
  '{f38bf404-1d43-42f2-9305-67de0b28fc23}': (env) => env.SystemRoot,
  '{f1b32785-6fba-4fcf-9d55-7b8e7f157091}': (env) => env.LOCALAPPDATA,
  '{3eb685db-65f9-4cf6-a03a-e3ef65729f3d}': (env) => env.APPDATA,
  '{5e6c858f-0e22-4760-9afe-ea3317b67173}': (env) => env.USERPROFILE
};

// Start menu clutter: uninstallers, readmes, manuals, websites. English, French and German wording.
const JUNK_NAME = /\b(uninstall|uninstaller|d[ée]sinstaller|deinstallieren|readme|read me|lisez-moi|release notes|notes de version|changelog|license|licence|manual|manuel|handbuch|documentation|user guide|help|aide|hilfe|website|site web|web site)\b/i;
const JUNK_TARGET = /\.(txt|chm|pdf|html?|url|rtf|ini|log|md|xml|hlp)$/i;

/** Store apps have AppIDs like "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App". */
function storeParts(appId) {
  const m = /^([^\\/!]+_[a-z0-9]{13})!([^\\/!]+)$/i.exec(appId);
  return m ? { family: m[1], app: m[2] } : null;
}

/** A desktop program's executable, when its AppID is a known-folder path. */
function resolveKnownFolder(appId, env = process.env) {
  const m = /^(\{[0-9a-f-]{36}\})\\(.+)$/i.exec(appId);
  if (!m) return null;
  const base = KNOWN_FOLDERS[m[1].toLowerCase()];
  const root = base && base(env);
  return root ? path.win32.join(root, m[2]) : null;
}

function isJunk(name, appId) {
  if (/^https?:/i.test(appId)) return true;
  if (JUNK_TARGET.test(appId)) return true;
  return JUNK_NAME.test(name);
}

const idOf = (appId) => 'a' + crypto.createHash('sha1').update(appId.toLowerCase()).digest('hex').slice(0, 16);

/**
 * Turn Get-StartApps output into the app list: junk dropped, duplicates (same name) merged, sorted by name.
 * @param {{apps: {Name: string, AppID: string}[] | {Name: string, AppID: string}, packages?: Object<string, string>}} raw
 */
function parseStartApps(raw, env = process.env) {
  const list = Array.isArray(raw.apps) ? raw.apps : raw.apps ? [raw.apps] : [];
  const packages = raw.packages || {};
  const seen = new Set();
  const out = [];
  for (const a of list) {
    const name = String(a.Name || '').trim();
    const appId = String(a.AppID || '').trim();
    if (!name || !appId || isJunk(name, appId)) continue;
    const key = name.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    const store = storeParts(appId);
    out.push({
      id: idOf(appId),
      name,
      appId,
      kind: store ? 'store' : 'desktop',
      exe: store ? null : resolveKnownFolder(appId, env),
      packageDir: store ? packages[store.family] || null : null,
      storeApp: store ? store.app : null
    });
  }
  return out.sort((x, y) => x.name.localeCompare(y.name, undefined, { sensitivity: 'base', numeric: true }));
}

// ---------------------------------------------------------------------------- Store app logos

/**
 * The logo a Store app's manifest names for one of its <Application>s: Square44x44Logo (the app-list icon)
 * or, failing that, Square150x150Logo. Returns the manifest's relative path, e.g. "Assets\\AppList.png".
 */
function manifestLogo(xml, appName) {
  const apps = [...xml.matchAll(/<Application\b([^>]*)>([\s\S]*?)<\/Application>/g)];
  const hit = apps.find((m) => new RegExp(`\\bId="${appName.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}"`).test(m[1])) || apps[0];
  if (!hit) return null;
  const ve = /<[\w:]*VisualElements\b([^>]*)>?/.exec(hit[2]);
  if (!ve) return null;
  const attr = (n) => (new RegExp(`\\b${n}="([^"]+)"`).exec(ve[1]) || [])[1];
  return attr('Square44x44Logo') || attr('Square150x150Logo') || null;
}

/**
 * Store logos are stored per scale ("AppList.scale-200.png", "AppList.targetsize-256_altform-unplated.png").
 * Pick the best one for a large tile: unplated target sizes first (no coloured plate), biggest up to 256.
 */
function pickLogoFile(files, logoName) {
  const ext = path.extname(logoName).toLowerCase();
  const base = path.basename(logoName, path.extname(logoName)).toLowerCase();
  const candidates = [];
  for (const f of files) {
    const lower = f.toLowerCase();
    if (!lower.endsWith(ext)) continue;
    const stem = lower.slice(0, -ext.length);
    if (stem === base) {
      candidates.push({ f, score: 100 });
      continue;
    }
    if (!stem.startsWith(`${base}.`)) continue;
    const q = stem.slice(base.length + 1);
    if (/contrast-(black|white)|theme-light/.test(q)) continue; // high-contrast and light-theme variants
    const ts = /targetsize-(\d+)/.exec(q);
    const sc = /scale-(\d+)/.exec(q);
    let score = 0;
    if (ts) {
      const n = Number(ts[1]);
      score = (n <= 256 ? n : 256 - (n - 256) / 4) + (/altform-unplated|altform-lightunplated/.test(q) ? 1000 : 500);
    } else if (sc) {
      score = 200 + Math.min(Number(sc[1]), 400) / 4;
    }
    candidates.push({ f, score });
  }
  candidates.sort((a, b) => b.score - a.score);
  return candidates.length ? candidates[0].f : null;
}

/** Absolute path of a Store app's best logo file, or null. */
function storeLogo(app) {
  if (!app.packageDir) return null;
  try {
    const xml = fs.readFileSync(path.join(app.packageDir, 'AppxManifest.xml'), 'utf8');
    const rel = manifestLogo(xml, app.storeApp);
    if (!rel) return null;
    const dir = path.join(app.packageDir, path.dirname(rel.replace(/\\/g, '/')));
    const file = pickLogoFile(fs.readdirSync(dir), path.basename(rel.replace(/\\/g, '/')));
    return file ? path.join(dir, file) : null;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------- Listing & launching

/** Start menu shortcuts by name (for desktop apps whose AppID isn't a path, e.g. "Chrome"). */
function startMenuShortcuts(env = process.env) {
  const roots = [env.ProgramData && path.join(env.ProgramData, 'Microsoft', 'Windows', 'Start Menu', 'Programs'), env.APPDATA && path.join(env.APPDATA, 'Microsoft', 'Windows', 'Start Menu', 'Programs')].filter(Boolean);
  const map = new Map();
  const walk = (dir, depth) => {
    if (depth > 4) return;
    let entries = [];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) walk(p, depth + 1);
      else if (/\.lnk$/i.test(e.name)) {
        const key = e.name.slice(0, -4).toLowerCase();
        if (!map.has(key)) map.set(key, p);
      }
    }
  };
  roots.forEach((r) => walk(r, 0));
  return map;
}

function runPowerShell(script, timeout = 30000) {
  return new Promise((resolve, reject) => {
    const encoded = Buffer.from(script, 'utf16le').toString('base64');
    execFile(
      'powershell.exe',
      ['-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', encoded],
      { windowsHide: true, timeout, maxBuffer: 32 * 1024 * 1024, encoding: 'utf8' },
      (err, stdout) => (err ? reject(err) : resolve(stdout))
    );
  });
}

/** Every launchable app on this PC (Windows only; elsewhere an empty list). */
async function listApps() {
  if (process.platform !== 'win32') return [];
  const out = await runPowerShell(LIST_SCRIPT);
  const start = out.indexOf('{');
  const apps = parseStartApps(JSON.parse(out.slice(start >= 0 ? start : 0)));
  const shortcuts = startMenuShortcuts();
  for (const a of apps) if (a.kind === 'desktop') a.shortcut = shortcuts.get(a.name.toLowerCase()) || null;
  return apps;
}

/** Open an app the way the Start menu does. */
function launchApp(appId) {
  if (process.platform !== 'win32') return Promise.reject(new Error('not supported'));
  return new Promise((resolve, reject) => {
    const child = spawn('explorer.exe', [`shell:AppsFolder\\${appId}`], { detached: true, stdio: 'ignore', windowsHide: false });
    child.once('error', reject);
    child.once('spawn', () => {
      child.unref();
      resolve();
    });
  });
}

module.exports = { listApps, launchApp, parseStartApps, resolveKnownFolder, isJunk, storeParts, manifestLogo, pickLogoFile, storeLogo, LIST_SCRIPT };
