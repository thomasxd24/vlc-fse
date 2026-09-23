'use strict';

const fs = require('fs/promises');
const fss = require('fs');
const os = require('os');
const path = require('path');
const { execFile } = require('child_process');
const vdf = require('./vdf');

// Tools that show up as "installed apps" but aren't games.
const NOT_GAMES = new Set(['228980', '1070560', '1391110', '1628350', '1493710', '2180100', '1887720', '961940']);
const NOT_GAMES_RE = /^(proton\b|steam linux runtime|steamworks common redistributables|steamvr\b)/i;

function regQueryValue(key, value) {
  return new Promise((resolve) => {
    if (process.platform !== 'win32') return resolve(null);
    execFile('reg', ['query', key, '/v', value], { windowsHide: true }, (err, stdout) => {
      if (err) return resolve(null);
      const m = new RegExp(`${value}\\s+(REG_\\w+)\\s+(.*)`, 'i').exec(stdout);
      if (!m) return resolve(null);
      const raw = m[2].trim();
      resolve(m[1] === 'REG_DWORD' ? parseInt(raw, 16) : raw);
    });
  });
}

function isDir(p) {
  try {
    return fss.statSync(p).isDirectory();
  } catch {
    return false;
  }
}

/** Locate the Steam install folder, or null. */
async function findSteam(preferred) {
  const candidates = [preferred, process.env.FOYER_STEAM_PATH];
  if (process.platform === 'win32') {
    const reg = await regQueryValue('HKCU\\Software\\Valve\\Steam', 'SteamPath');
    if (reg) candidates.push(path.normalize(reg));
    candidates.push(
      path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Steam'),
      path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Steam')
    );
  } else if (process.platform === 'darwin') {
    candidates.push(path.join(os.homedir(), 'Library', 'Application Support', 'Steam'));
  } else {
    candidates.push(path.join(os.homedir(), '.steam', 'steam'), path.join(os.homedir(), '.local', 'share', 'Steam'));
  }
  return candidates.find((c) => c && isDir(path.join(c, 'steamapps'))) || null;
}

async function readVdf(p) {
  try {
    return vdf.parse(await fs.readFile(p, 'utf8'));
  } catch {
    return null;
  }
}

/** Every steamapps folder: the main one plus extra library folders on other drives. */
async function libraryFolders(steamPath) {
  const out = [path.join(steamPath, 'steamapps')];
  const data = await readVdf(path.join(steamPath, 'steamapps', 'libraryfolders.vdf'));
  const root = vdf.get(data, 'libraryfolders') || vdf.get(data, 'LibraryFolders') || {};
  for (const [key, entry] of Object.entries(root)) {
    if (!/^\d+$/.test(key)) continue;
    // New format: { "path": "D:\\SteamLibrary", ... }; old format: "1" "D:\\SteamLibrary".
    const p = typeof entry === 'string' ? entry : vdf.get(entry, 'path');
    if (p) out.push(path.join(path.normalize(p), 'steamapps'));
  }
  const seen = new Set();
  return out.filter((p) => {
    const k = p.toLowerCase();
    if (seen.has(k) || !isDir(p)) return false;
    seen.add(k);
    return true;
  });
}

/** The account that last signed in to Steam, as its 32-bit id used by the userdata folder. */
async function currentUser(steamPath) {
  const data = await readVdf(path.join(steamPath, 'config', 'loginusers.vdf'));
  const users = vdf.get(data, 'users') || {};
  let best = null;
  for (const [id64, u] of Object.entries(users)) {
    const recent = vdf.get(u, 'MostRecent') === '1';
    const ts = Number(vdf.get(u, 'Timestamp') || 0);
    if (!best || recent > best.recent || (recent === best.recent && ts > best.ts)) best = { id64, recent, ts, name: vdf.get(u, 'PersonaName') };
  }
  if (best) {
    try {
      return { accountId: String(BigInt(best.id64) & 0xffffffffn), name: best.name || null };
    } catch {}
  }
  // Fall back to the most recently modified userdata folder.
  const dirs = await fs.readdir(path.join(steamPath, 'userdata')).catch(() => []);
  let newest = null;
  for (const d of dirs.filter((x) => /^\d+$/.test(x) && x !== '0')) {
    const st = await fs.stat(path.join(steamPath, 'userdata', d, 'config', 'localconfig.vdf')).catch(() => null);
    if (st && (!newest || st.mtimeMs > newest.t)) newest = { accountId: d, t: st.mtimeMs };
  }
  return newest ? { accountId: newest.accountId, name: null } : null;
}

/** Playtime (minutes) and last-played time (ms) per app id, from the user's localconfig.vdf. */
async function playStats(steamPath, accountId) {
  const stats = {};
  if (!accountId) return stats;
  const data = await readVdf(path.join(steamPath, 'userdata', accountId, 'config', 'localconfig.vdf'));
  const apps = vdf.get(data, 'UserLocalConfigStore', 'Software', 'Valve', 'Steam', 'apps') || {};
  for (const [appid, a] of Object.entries(apps)) {
    const lastPlayed = Number(vdf.get(a, 'LastPlayed') || 0) * 1000;
    const playtime = Number(vdf.get(a, 'Playtime') || 0);
    if (lastPlayed || playtime) stats[appid] = { lastPlayed, playtime };
  }
  return stats;
}

async function listDir(p) {
  return fs.readdir(p, { withFileTypes: true }).catch(() => []);
}

/**
 * Artwork Steam already has on disk. Priority: custom images the user set in Steam (userdata/.../grid),
 * then Steam's library cache, in both the old flat layout ("620_library_600x900.jpg") and the newer
 * per-app folders ("620/library_600x900.jpg", sometimes one level deeper).
 */
async function localArt(steamPath, accountId, appid, cache) {
  const art = { poster: null, hero: null, logo: null, header: null, icon: null };
  const want = {
    poster: ['library_600x900.jpg', 'library_600x900_2x.jpg', 'library_capsule.jpg'],
    hero: ['library_hero.jpg', 'library_hero_2x.jpg'],
    logo: ['logo.png', 'logo_2x.png'],
    header: ['header.jpg', 'library_header.jpg', 'header_2x.jpg']
  };

  const lc = path.join(steamPath, 'appcache', 'librarycache');
  if (!cache.flat) cache.flat = new Set((await listDir(lc)).filter((e) => e.isFile()).map((e) => e.name));
  for (const [kind, names] of Object.entries(want)) {
    const hit = names.map((n) => `${appid}_${n}`).find((n) => cache.flat.has(n));
    if (hit) art[kind] = path.join(lc, hit);
  }
  const appDir = path.join(lc, String(appid));
  const found = new Map();
  for (const e of await listDir(appDir)) {
    if (e.isFile()) found.set(e.name, path.join(appDir, e.name));
    else if (e.isDirectory()) for (const f of await listDir(path.join(appDir, e.name))) if (f.isFile() && !found.has(f.name)) found.set(f.name, path.join(appDir, e.name, f.name));
  }
  for (const [kind, names] of Object.entries(want)) {
    const hit = names.find((n) => found.has(n));
    if (hit) art[kind] = found.get(hit);
  }
  // Steam's small icon is named by a hash; any other .jpg in the app folder is it.
  const icon = [...found.keys()].find((n) => /^[0-9a-f]{40}\.jpg$/i.test(n));
  if (icon) art.icon = found.get(icon);

  if (accountId) {
    const grid = path.join(steamPath, 'userdata', accountId, 'config', 'grid');
    if (!cache.grid) cache.grid = new Map((await listDir(grid)).filter((e) => e.isFile()).map((e) => [e.name.toLowerCase(), path.join(grid, e.name)]));
    const pick = (base) => ['.png', '.jpg', '.jpeg', '.webp'].map((x) => cache.grid.get(`${appid}${base}${x}`)).find(Boolean);
    art.poster = pick('p') || art.poster;
    art.hero = pick('_hero') || art.hero;
    art.logo = pick('_logo') || art.logo;
    art.header = pick('') || art.header;
  }
  return art;
}

/** Scan installed Steam games. Returns { steamPath, user, games } or null when Steam isn't installed. */
async function scanSteam(preferredPath) {
  const steamPath = await findSteam(preferredPath);
  if (!steamPath) return null;
  const user = await currentUser(steamPath);
  const stats = await playStats(steamPath, user && user.accountId);
  const cache = {};
  const games = [];
  const seen = new Set();

  for (const lib of await libraryFolders(steamPath)) {
    for (const e of await listDir(lib)) {
      const m = /^appmanifest_(\d+)\.acf$/i.exec(e.name);
      if (!m || !e.isFile()) continue;
      const data = await readVdf(path.join(lib, e.name));
      const st = vdf.get(data, 'AppState');
      if (!st) continue;
      const appid = String(vdf.get(st, 'appid') || m[1]);
      const name = vdf.get(st, 'name') || `App ${appid}`;
      const flags = Number(vdf.get(st, 'StateFlags') || 0);
      if (seen.has(appid) || NOT_GAMES.has(appid) || NOT_GAMES_RE.test(name)) continue;
      if (!(flags & 4)) continue; // not fully installed
      seen.add(appid);
      const installDir = path.join(lib, 'common', vdf.get(st, 'installdir') || '');
      const fstat = await fs.stat(path.join(lib, e.name)).catch(() => null);
      const s = stats[appid] || {};
      games.push({
        id: `steam-${appid}`,
        source: 'steam',
        appid,
        title: name,
        installDir,
        size: Number(vdf.get(st, 'SizeOnDisk') || 0),
        addedAt: fstat ? Math.round(fstat.birthtimeMs || fstat.mtimeMs) : 0,
        lastPlayed: s.lastPlayed || Number(vdf.get(st, 'LastPlayed') || 0) * 1000,
        playtime: s.playtime || 0,
        art: await localArt(steamPath, user && user.accountId, appid, cache)
      });
    }
  }
  return { steamPath, user, games };
}

/** The app id Steam reports as running right now (0 when none), or null if it can't be read. */
async function runningAppId() {
  const v = await regQueryValue('HKCU\\Software\\Valve\\Steam', 'RunningAppID');
  return typeof v === 'number' ? v : null;
}

module.exports = { findSteam, scanSteam, runningAppId, libraryFolders, playStats, currentUser };
