'use strict';

const { app, BrowserWindow, ipcMain, dialog, shell, powerSaveBlocker } = require('electron');
const fs = require('fs');
const path = require('path');
const { pathToFileURL } = require('url');
const { JsonStore } = require('./src/store');
const { scanLibraries } = require('./src/library');
const { Metadata } = require('./src/metadata');
const { findVlc, VlcSession } = require('./src/vlc');
const { scanSteam } = require('./src/steam');
const { GameInfo } = require('./src/gameinfo');
const { GameSession, titleFromExe, manualId } = require('./src/games');
const { SystemHelper, wifi, power, setPriority, hasBattery } = require('./src/system');

const WATCHED_RATIO = 0.9;
const MIN_RESUME_SECONDS = 60;
const SUSPEND_DELAY_MS = 4000;

const DEFAULT_SETTINGS = {
  libraries: [], // { path, type: 'movies' | 'tv' }
  vlcPath: '',
  vlcFullscreen: true,
  vlcExtraArgs: '',
  autoplayNext: true,
  tmdbKey: '',
  sgdbKey: '',
  steamEnabled: true,
  steamPath: '',
  uiLanguage: 'auto', // 'auto' | 'en' | 'fr'
  startFullscreen: true,
  launchAtLogin: false,
  uiScale: 1,
  haptics: true,
  sounds: true,
  freeWhilePlaying: true
};

if (!app.requestSingleInstanceLock()) {
  app.quit();
}

let win = null;
let settings;
let libraryStore;
let progressStore;
let metaStore;
let gamesStore; // { manual: [], overrides: {}, stats: {} }
let gameInfoStore;
let prefsStore; // { favorites: {}, hidden: {} }
let metadata;
let gameInfo;
let library = { movies: [], shows: [], games: [], scannedAt: 0 };
let session = null;
let nowPlaying = null;
let gameSession = null;
let gameState = null;
let scanning = false;
let rescanQueued = false;
let metaStatus = { running: false, error: null };
let gameInfoStatus = { running: false, error: null };
const ui = { suspended: false, state: null };
const helper = new SystemHelper();
let batteryPresent = null;

const userFile = (name) => path.join(app.getPath('userData'), name);
const fileUrl = (p) => (p ? pathToFileURL(p).href : null);
const progressKey = (p) => (process.platform === 'win32' ? p.toLowerCase() : p);

function send(channel, payload) {
  if (win && !win.isDestroyed() && !ui.suspended) win.webContents.send(channel, payload);
}

function uiLang() {
  const l = settings.get('uiLanguage');
  if (l === 'en' || l === 'fr') return l;
  return /^fr/i.test(app.getLocale()) ? 'fr' : 'en';
}

/** Foyer used to be called Marquee: carry settings, progress and caches over on first launch. */
function migrateFromMarquee() {
  const oldDir = path.join(app.getPath('appData'), 'Marquee');
  const dir = app.getPath('userData');
  if (!fs.existsSync(oldDir) || fs.existsSync(path.join(dir, 'settings.json'))) return;
  try {
    fs.mkdirSync(dir, { recursive: true });
    for (const f of ['settings.json', 'library.json', 'progress.json', 'metadata.json']) {
      if (fs.existsSync(path.join(oldDir, f))) fs.copyFileSync(path.join(oldDir, f), path.join(dir, f));
    }
    if (fs.existsSync(path.join(oldDir, 'artwork'))) fs.cpSync(path.join(oldDir, 'artwork'), path.join(dir, 'artwork'), { recursive: true });
  } catch (err) {
    console.error('migration failed', err);
  }
}

// ---------------------------------------------------------------------------
// View model: the library merged with metadata, progress and preferences, shaped for the UI.

function progressFor(p) {
  const e = progressStore.get('items')[progressKey(p)];
  if (!e) return { time: 0, length: 0, watched: false, updatedAt: 0 };
  return e;
}

function resumable(pr) {
  return !pr.watched && pr.time >= MIN_RESUME_SECONDS && (!pr.length || pr.time < pr.length * WATCHED_RATIO);
}

function prefsOf(id) {
  return { favorite: Boolean(prefsStore.get('favorites')[id]), hidden: Boolean(prefsStore.get('hidden')[id]) };
}

/** Raw games (Steam scan + manual), with overrides applied, as fed to both the UI and GameInfo. */
function rawGames() {
  const overrides = gamesStore.get('overrides');
  const all = [...(library.games || []), ...gamesStore.get('manual')];
  return all.map((g) => {
    const o = overrides[g.id] || {};
    return { ...g, title: o.title || g.title, art: { ...(g.art || {}), ...(o.art || {}) }, override: o };
  });
}

function buildGames() {
  const stats = gamesStore.get('stats');
  return rawGames().map((g) => {
    const info = gameInfo.lookup(g.id) || {};
    const fetched = info.art || {};
    const st = stats[g.id] || {};
    const art = (k) => fileUrl(g.art[k] || fetched[k]);
    return {
      id: g.id,
      type: 'game',
      source: g.source,
      appid: g.appid || null,
      title: g.title,
      poster: art('poster'),
      hero: art('hero') || art('header'),
      logo: art('logo'),
      header: art('header'),
      icon: art('icon'),
      overview: info.overview || '',
      about: info.about || '',
      genres: info.genres || [],
      developers: info.developers || [],
      publishers: info.publishers || [],
      releaseDate: info.releaseDate || null,
      metacritic: info.metacritic ?? null,
      controller: info.controller || null,
      screenshots: (info.screenshots || []).map((s) => ({ thumb: fileUrl(s.thumb), full: s.full })),
      steamAppId: info.steamAppId || g.appid || null,
      playtime: g.source === 'steam' ? Math.max(g.playtime || 0, st.playtime || 0) : st.playtime || 0,
      lastPlayed: Math.max(g.lastPlayed || 0, st.lastPlayed || 0),
      addedAt: g.addedAt || 0,
      exe: g.exe || null,
      args: g.args || '',
      installDir: g.installDir || (g.exe ? path.dirname(g.exe) : null),
      override: g.override,
      ...prefsOf(g.id)
    };
  });
}

function buildViewModel() {
  const movies = library.movies.map((m) => {
    const meta = metadata.lookup(Metadata.movieKey(m)) || {};
    const pr = progressFor(m.path);
    return {
      id: m.id,
      type: 'movie',
      title: m.title,
      year: m.year,
      overview: meta.overview || '',
      tagline: meta.tagline || '',
      rating: meta.rating ?? null,
      runtime: meta.runtime ?? (pr.length ? Math.round(pr.length / 60) : null),
      genres: meta.genres || [],
      poster: fileUrl(m.poster || meta.poster),
      backdrop: fileUrl(m.backdrop || meta.backdrop),
      path: m.path,
      addedAt: m.addedAt,
      progress: { time: pr.time, length: pr.length, watched: pr.watched, updatedAt: pr.updatedAt, resumable: resumable(pr) },
      ...prefsOf(m.id)
    };
  });

  const shows = library.shows.map((s) => {
    const meta = metadata.lookup(Metadata.showKey(s)) || {};
    const epMeta = meta.episodes || {};
    let lastWatched = null;
    const episodes = s.episodes.map((e) => {
      const em = epMeta[`${e.season}x${e.episode}`] || {};
      const pr = progressFor(e.path);
      if (pr.updatedAt && (!lastWatched || pr.updatedAt > lastWatched.updatedAt)) lastWatched = { id: e.id, updatedAt: pr.updatedAt };
      return {
        id: e.id,
        season: e.season,
        episode: e.episode,
        episodeEnd: e.episodeEnd,
        title: em.title || e.title || null,
        overview: em.overview || '',
        airDate: em.airDate || null,
        runtime: em.runtime ?? (pr.length ? Math.round(pr.length / 60) : null),
        thumb: fileUrl(e.thumb || em.still),
        path: e.path,
        addedAt: e.addedAt,
        progress: { time: pr.time, length: pr.length, watched: pr.watched, updatedAt: pr.updatedAt, resumable: resumable(pr) }
      };
    });

    // "Next up": resume the most recently touched episode, or the one after it if it was finished,
    // otherwise the first unwatched episode.
    let nextUp = null;
    if (lastWatched) {
      const i = episodes.findIndex((e) => e.id === lastWatched.id);
      const ep = episodes[i];
      if (ep && !ep.progress.watched) nextUp = ep;
      else nextUp = episodes.slice(i + 1).find((e) => !e.progress.watched) || null;
    }
    if (!nextUp && !lastWatched) nextUp = episodes.find((e) => !e.progress.watched) || null;

    return {
      id: s.id,
      type: 'show',
      title: s.title,
      year: s.year || (meta.firstAirDate ? Number(meta.firstAirDate.slice(0, 4)) : null),
      overview: meta.overview || '',
      rating: meta.rating ?? null,
      genres: meta.genres || [],
      status: meta.status || null,
      poster: fileUrl(s.poster || meta.poster),
      backdrop: fileUrl(s.backdrop || meta.backdrop),
      addedAt: s.addedAt,
      episodes,
      seasons: [...new Set(episodes.map((e) => e.season))].sort((a, b) => a - b),
      watchedCount: episodes.filter((e) => e.progress.watched).length,
      nextUp: nextUp ? nextUp.id : null,
      lastActivity: lastWatched ? lastWatched.updatedAt : 0,
      ...prefsOf(s.id)
    };
  });

  // Continue watching: part-watched movies, plus shows with activity and something left to watch.
  const continueWatching = [
    ...movies.filter((m) => m.progress.resumable && !m.hidden).map((m) => ({ kind: 'movie', id: m.id, at: m.progress.updatedAt })),
    ...shows.filter((s) => s.lastActivity && s.nextUp && !s.hidden).map((s) => ({ kind: 'episode', showId: s.id, id: s.nextUp, at: s.lastActivity }))
  ]
    .sort((a, b) => b.at - a.at)
    .slice(0, 20);

  return { movies, shows, games: buildGames(), continueWatching, scannedAt: library.scannedAt, steamFound: Boolean(library.steamPath) };
}

function state() {
  return {
    settings: settings.data,
    lang: uiLang(),
    library: buildViewModel(),
    scanning,
    metaStatus,
    gameInfoStatus,
    nowPlaying,
    game: gameState,
    uiState: ui.state,
    toasts: pendingToasts.splice(0),
    platform: process.platform,
    packaged: app.isPackaged,
    fsePackage: Boolean(process.windowsStore),
    systemControls: helper.supported,
    hasBattery: batteryPresent,
    version: app.getVersion()
  };
}

let pushTimer = null;
function pushLibrary() {
  clearTimeout(pushTimer);
  pushTimer = setTimeout(() => send('state', state()), 250);
}

// Toasts raised while the UI is unloaded or reloading (e.g. right after a game) wait for the next page load.
const pendingToasts = [];
function toast(key, vars, kind = 'info') {
  const t = { key, vars, kind };
  if (!win || ui.suspended || win.webContents.isLoading()) pendingToasts.push(t);
  else send('toast', t);
}

// ---------------------------------------------------------------------------
// Scanning & enrichment

async function scanGamesOnly() {
  if (!settings.get('steamEnabled')) {
    library.games = [];
    library.steamPath = null;
    return;
  }
  const steam = await scanSteam(settings.get('steamPath')).catch((err) => {
    console.error('steam scan failed', err);
    return null;
  });
  library.games = steam ? steam.games : [];
  library.steamPath = steam ? steam.steamPath : null;
}

async function rescan() {
  if (scanning) {
    // Folders changed mid-scan: run again once this pass finishes so the change isn't lost.
    rescanQueued = true;
    return;
  }
  if (gameSession) return; // never compete with a running game for disk and CPU
  scanning = true;
  send('state', state());
  try {
    const media = await scanLibraries(settings.get('libraries'));
    library = { ...library, ...media };
    await scanGamesOnly();
    libraryStore.data = library;
    libraryStore.save();
  } catch (err) {
    console.error('scan failed', err);
  } finally {
    scanning = false;
    send('state', state());
  }
  if (rescanQueued) {
    rescanQueued = false;
    return rescan();
  }
  enrich();
  enrichGames();
}

async function enrich() {
  metadata = new Metadata({
    store: metaStore,
    imageDir: userFile('artwork'),
    apiKey: settings.get('tmdbKey'),
    language: uiLang() === 'fr' ? 'fr-FR' : 'en-US'
  });
  if (!metadata.enabled || metaStatus.running || gameSession) return;
  metaStatus = { running: true, error: null };
  pushLibrary();
  try {
    await metadata.enrich(library, pushLibrary);
  } catch (err) {
    metaStatus.error = err.message;
  } finally {
    metaStatus.running = false;
    pushLibrary();
  }
}

function makeGameInfo() {
  gameInfo = new GameInfo({
    store: gameInfoStore,
    imageDir: userFile('artwork'),
    sgdbKey: settings.get('sgdbKey'),
    language: uiLang()
  });
}

async function enrichGames() {
  if (gameInfoStatus.running) return;
  gameInfoStatus = { running: true, error: null };
  pushLibrary();
  try {
    await gameInfo.enrich(rawGames(), pushLibrary, () => Boolean(gameSession));
  } catch (err) {
    gameInfoStatus.error = err.message;
  } finally {
    gameInfoStatus.running = false;
    pushLibrary();
  }
}

// ---------------------------------------------------------------------------
// Media playback (VLC)

function recordProgress({ path: p, time, length }) {
  const items = progressStore.get('items');
  const key = progressKey(p);
  const prev = items[key] || {};
  const watched = prev.watched || (length > 0 && time >= length * WATCHED_RATIO);
  items[key] = { time: watched ? 0 : time, length, watched, updatedAt: Date.now() };
  progressStore.save();
}

function setWatched(paths, watched) {
  const items = progressStore.get('items');
  for (const p of paths) {
    const key = progressKey(p);
    items[key] = { ...(items[key] || { length: 0 }), time: 0, watched, updatedAt: Date.now() };
  }
  progressStore.save();
  pushLibrary();
}

function buildQueue(req) {
  if (req.kind === 'movie') {
    const m = library.movies.find((x) => x.id === req.id);
    if (!m) throw new Error('notFound');
    const pr = progressFor(m.path);
    return { title: m.title, queue: [{ path: m.path, startTime: req.resume && resumable(pr) ? pr.time : 0 }] };
  }
  const show = library.shows.find((s) => s.id === req.showId);
  const idx = show ? show.episodes.findIndex((e) => e.id === req.id) : -1;
  if (idx < 0) throw new Error('notFound');
  const first = show.episodes[idx];
  const pr = progressFor(first.path);
  const rest = settings.get('autoplayNext') ? show.episodes.slice(idx + 1) : [];
  const label = `${show.title} · S${String(first.season).padStart(2, '0')}E${String(first.episode).padStart(2, '0')}`;
  return {
    title: label,
    queue: [{ path: first.path, startTime: req.resume && resumable(pr) ? pr.time : 0 }, ...rest.map((e) => ({ path: e.path }))]
  };
}

function splitArgs(s) {
  return (String(s || '').match(/"[^"]*"|\S+/g) || []).map((a) => a.replace(/^"|"$/g, ''));
}

async function play(req) {
  if (session) session.kill();
  const vlcPath = await findVlc(settings.get('vlcPath'));
  if (!vlcPath) return { ok: false, errorKey: 'err.vlcNotFound' };

  let job;
  try {
    job = buildQueue(req);
  } catch {
    return { ok: false, errorKey: 'err.notFound' };
  }

  const s = new VlcSession(vlcPath, job.queue, {
    fullscreen: settings.get('vlcFullscreen'),
    extraArgs: splitArgs(settings.get('vlcExtraArgs'))
  });
  session = s;
  nowPlaying = { title: job.title, request: req, current: job.queue[0].path, time: 0, length: 0 };
  const blocker = powerSaveBlocker.start('prevent-display-sleep');

  s.on('progress', (p) => {
    if (session !== s) return;
    recordProgress(p);
    nowPlaying = { ...nowPlaying, current: p.path, time: p.time, length: p.length };
    send('now-playing', nowPlaying);
  });
  const finish = (errorKey, vars) => {
    if (powerSaveBlocker.isStarted(blocker)) powerSaveBlocker.stop(blocker);
    if (session !== s) return;
    session = null;
    nowPlaying = null;
    send('now-playing', null);
    if (errorKey) toast(errorKey, vars, 'error');
    pushLibrary();
    bringToFront();
  };
  const startedAt = Date.now();
  s.on('exit', ({ code, last }) => {
    // VLC that dies within seconds without ever reporting a position failed to start or open the file.
    if (code && !last && Date.now() - startedAt < 15000) finish('err.vlcExited', { code });
    else finish();
  });
  s.on('error', (err) => finish('err.vlcStart', { message: err.message }));

  try {
    await s.start();
  } catch (err) {
    finish('err.vlcStart', { message: err.message });
    return { ok: false };
  }
  send('now-playing', nowPlaying);
  return { ok: true };
}

// ---------------------------------------------------------------------------
// Games

let suspendTimer = null;

/**
 * While a game runs, Foyer gets out of the way: the UI is unloaded (freeing the renderer's memory and GPU
 * work), the window is minimised, every Foyer process drops to low CPU priority, and background work
 * (scans, artwork downloads, the system helper) stops. It all comes back when the game exits or when you
 * switch back to Foyer.
 */
function suspendUi() {
  if (ui.suspended || !win || win.isDestroyed()) return;
  ui.suspended = true;
  helper.stop();
  win.webContents.loadURL('data:text/html,<body style="background:%2307080c"></body>');
  win.minimize();
  setPriority(app.getAppMetrics().map((m) => m.pid), true);
}

function resumeUi() {
  clearTimeout(suspendTimer);
  if (ui.suspended) {
    ui.suspended = false;
    setPriority(app.getAppMetrics().map((m) => m.pid), false);
    win.loadFile(path.join(__dirname, 'renderer', 'index.html'));
  }
  bringToFront();
}

function recordPlay(id, playedMs) {
  const stats = gamesStore.get('stats');
  const s = stats[id] || { playtime: 0, lastPlayed: 0 };
  s.lastPlayed = Date.now();
  s.playtime = (s.playtime || 0) + Math.round(playedMs / 60000);
  stats[id] = s;
  gamesStore.save();
}

async function playGame(id) {
  if (gameSession) return { ok: false, errorKey: 'err.alreadyPlaying' };
  const game = rawGames().find((g) => g.id === id);
  if (!game) return { ok: false, errorKey: 'err.notFound' };
  if (game.source === 'manual' && !fs.existsSync(game.exe)) return { ok: false, errorKey: 'err.exeMissing' };

  const s = new GameSession(game, { openExternal: (url) => shell.openExternal(url), openPath: (p) => shell.openPath(p) });
  gameSession = s;
  gameState = { id, title: game.title, phase: 'launching', startedAt: Date.now() };
  send('game', gameState);

  s.on('running', () => {
    if (gameSession !== s) return;
    gameState = { ...gameState, phase: 'running' };
    send('game', gameState);
    if (settings.get('freeWhilePlaying')) suspendTimer = setTimeout(suspendUi, SUSPEND_DELAY_MS);
  });
  s.on('exit', ({ reason, playedMs, error }) => {
    if (gameSession !== s) return;
    if (reason === 'stub') {
      // The exe was a launcher that handed off to the real game: we can't see when that one ends,
      // so stay out of the way until the user comes back to Foyer and says they're done.
      gameState = { ...gameState, phase: 'untracked' };
      return;
    }
    gameSession = null;
    gameState = null;
    if (playedMs > 0 || reason === 'exited') recordPlay(id, playedMs);
    resumeUi();
    send('game', null);
    if (reason === 'error') toast('err.gameStart', { message: error ? error.message : '' }, 'error');
    if (reason === 'timeout') toast('err.gameTimeout', { title: game.title }, 'error');
    // Steam updates its own playtime when a game closes; pick that up.
    if (game.source === 'steam') setTimeout(() => scanGamesOnly().then(pushLibrary), 3000);
    else pushLibrary();
  });

  try {
    await s.start();
  } catch (err) {
    gameSession = null;
    gameState = null;
    send('game', null);
    return { ok: false, errorKey: 'err.gameStart', vars: { message: err.message } };
  }
  return { ok: true };
}

function endGame() {
  if (gameSession) {
    const s = gameSession;
    if (gameState && gameState.phase === 'untracked') {
      // Already counted as a launch; the exit event was swallowed above.
      // We couldn't watch the game itself, so count the time until the user said they were done.
      const played = Date.now() - gameState.startedAt;
      gameSession = null;
      gameState = null;
      recordPlay(s.game.id, played);
      send('game', null);
      pushLibrary();
      return;
    }
    s.stopTracking();
  }
}

async function iconFor(exe) {
  try {
    const img = await app.getFileIcon(exe, { size: 'large' });
    const dest = userFile(path.join('artwork', `icon-${Buffer.from(exe.toLowerCase()).toString('base64url').slice(-40)}.png`));
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.writeFileSync(dest, img.toPNG());
    return dest;
  } catch {
    return null;
  }
}

async function addManualGame(exe) {
  const manual = gamesStore.get('manual');
  if (manual.some((g) => g.exe.toLowerCase() === exe.toLowerCase())) return { ok: false, errorKey: 'err.gameExists' };
  const game = {
    id: manualId(exe),
    source: 'manual',
    title: titleFromExe(exe),
    exe,
    args: '',
    cwd: '',
    addedAt: Date.now(),
    art: { icon: await iconFor(exe) }
  };
  manual.push(game);
  gamesStore.save();
  pushLibrary();
  enrichGames();
  return { ok: true, id: game.id };
}

function editGame(id, patch) {
  const overrides = gamesStore.get('overrides');
  const o = { ...(overrides[id] || {}) };
  const manual = gamesStore.get('manual').find((g) => g.id === id);
  let refetch = false;
  if (patch.title !== undefined) {
    const t = String(patch.title).trim();
    if (manual) manual.title = t || manual.title;
    else if (t) o.title = t;
    else delete o.title;
    refetch = true;
  }
  if (manual && patch.args !== undefined) manual.args = String(patch.args);
  if (patch.steamAppId !== undefined) {
    o.steamAppId = patch.steamAppId ? String(patch.steamAppId) : undefined;
    o.noSteamMatch = patch.steamAppId === null ? true : undefined;
    refetch = true;
  }
  if (patch.sgdbId !== undefined) {
    o.sgdbId = patch.sgdbId || undefined;
    refetch = true;
  }
  if (patch.art) o.art = { ...(o.art || {}), ...patch.art };
  for (const k of Object.keys(o)) if (o[k] === undefined) delete o[k];
  overrides[id] = o;
  gamesStore.save();
  if (refetch) {
    gameInfo.forget(id);
    enrichGames();
  }
  pushLibrary();
}

function removeGame(id) {
  gamesStore.set('manual', gamesStore.get('manual').filter((g) => g.id !== id));
  delete gamesStore.get('overrides')[id];
  gameInfo.forget(id);
  pushLibrary();
}

// ---------------------------------------------------------------------------
// Window & IPC

function bringToFront() {
  if (!win || win.isDestroyed()) return;
  if (win.isMinimized()) win.restore();
  // Windows refuses focus changes from background processes; briefly going topmost gets around it.
  win.setAlwaysOnTop(true);
  win.show();
  win.focus();
  win.setAlwaysOnTop(false);
  if (settings.get('startFullscreen')) win.setFullScreen(true);
}

function createWindow() {
  win = new BrowserWindow({
    width: 1600,
    height: 900,
    minWidth: 960,
    minHeight: 540,
    fullscreen: settings.get('startFullscreen'),
    autoHideMenuBar: true,
    backgroundColor: '#07080c',
    title: 'Foyer',
    icon: path.join(__dirname, 'build', 'icon.png'),
    show: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      autoplayPolicy: 'no-user-gesture-required', // UI sounds
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true
    }
  });
  win.removeMenu();
  win.once('ready-to-show', () => win.show());
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));

  // Switching back to Foyer while a game runs brings the UI back.
  win.on('focus', () => {
    if (ui.suspended) resumeUi();
  });

  // Never navigate away from the app or open windows; send links to the browser.
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:/.test(url)) shell.openExternal(url);
    return { action: 'deny' };
  });
  win.webContents.on('will-navigate', (e) => e.preventDefault());
}

function pathsFor({ kind, id, showId }) {
  const show = (sid) => library.shows.find((s) => s.id === sid);
  if (kind === 'movie') return library.movies.filter((m) => m.id === id).map((m) => m.path);
  if (kind === 'episode') return (show(showId)?.episodes || []).filter((e) => e.id === id).map((e) => e.path);
  if (kind === 'season') return (show(showId)?.episodes || []).filter((e) => e.season === id).map((e) => e.path);
  if (kind === 'show') return (show(id)?.episodes || []).map((e) => e.path);
  return [];
}

function registerIpc() {
  ipcMain.handle('get-state', () => state());
  ipcMain.handle('save-ui-state', (_e, s) => {
    ui.state = s;
  });
  ipcMain.handle('rescan', () => rescan());
  ipcMain.handle('play', (_e, req) => play(req));
  ipcMain.handle('stop', () => {
    if (session) session.kill();
  });
  ipcMain.handle('set-watched', (_e, req) => setWatched(pathsFor(req), req.watched));
  ipcMain.handle('set-pref', (_e, { id, key, value }) => {
    if (key !== 'favorites' && key !== 'hidden') return;
    const map = prefsStore.get(key);
    if (value) map[id] = true;
    else delete map[id];
    prefsStore.save();
    pushLibrary();
  });

  // Games
  ipcMain.handle('play-game', (_e, id) => playGame(id));
  ipcMain.handle('end-game', () => endGame());
  ipcMain.handle('back-to-game', () => {
    if (gameSession) suspendUi();
  });
  ipcMain.handle('add-game', async () => {
    const r = await dialog.showOpenDialog(win, {
      properties: ['openFile'],
      filters: process.platform === 'win32' ? [{ name: 'Games', extensions: ['exe', 'bat', 'cmd', 'lnk', 'url'] }] : []
    });
    if (r.canceled || !r.filePaths[0]) return { ok: false };
    return addManualGame(r.filePaths[0]);
  });
  ipcMain.handle('edit-game', (_e, { id, patch }) => editGame(id, patch));
  ipcMain.handle('remove-game', (_e, id) => removeGame(id));
  ipcMain.handle('search-steam', async (_e, term) => gameInfo.storeSearch(term).catch(() => []));
  ipcMain.handle('search-sgdb', async (_e, term) => gameInfo.sgdbSearch(term).catch((e) => ({ error: e.message })));
  ipcMain.handle('sgdb-images', async (_e, { kind, id }) => {
    const g = rawGames().find((x) => x.id === id);
    if (!g) return [];
    const info = gameInfo.lookup(id) || {};
    const ref = { sgdbId: g.override.sgdbId || info.sgdbId, steamAppId: g.override.steamAppId || info.steamAppId || g.appid };
    const imgs = await gameInfo.sgdbImages(kind, ref).catch(() => []);
    // Thumbnails are remote; download them so the page (which only shows local files) can display them.
    const out = [];
    for (const i of imgs.slice(0, 12)) {
      const thumb = await gameInfo.download(i.thumb);
      if (thumb) out.push({ url: i.url, thumb: fileUrl(thumb) });
    }
    return out;
  });
  ipcMain.handle('set-game-art', async (_e, { id, kind, url }) => {
    const p = await gameInfo.download(url);
    if (!p) return { ok: false };
    editGame(id, { art: { [kind]: p } });
    return { ok: true };
  });
  ipcMain.handle('screenshot', async (_e, url) => {
    if (!/^https:\/\/[\w.-]*(steamstatic|steampowered|akamaihd)\.(com|net)\//.test(url)) return null;
    return fileUrl(await gameInfo.download(url));
  });
  ipcMain.handle('show-game-folder', (_e, id) => {
    const g = rawGames().find((x) => x.id === id);
    const dir = g && (g.installDir || (g.exe && path.dirname(g.exe)));
    if (dir && fs.existsSync(dir)) shell.openPath(dir);
  });

  ipcMain.handle('save-settings', async (_e, patch) => {
    const before = { ...settings.data };
    const prevLang = uiLang();
    const allowed = Object.keys(DEFAULT_SETTINGS);
    for (const [k, v] of Object.entries(patch || {})) if (allowed.includes(k)) settings.data[k] = v;
    settings.flush();
    const changed = (k) => JSON.stringify(before[k]) !== JSON.stringify(settings.data[k]);

    if (patch.launchAtLogin !== undefined && app.isPackaged) {
      app.setLoginItemSettings({ openAtLogin: Boolean(patch.launchAtLogin) });
    }
    if (patch.startFullscreen !== undefined && win) win.setFullScreen(Boolean(patch.startFullscreen));
    const langChanged = uiLang() !== prevLang;
    if (langChanged) {
      // Synopses and game descriptions are per language: refetch them, but keep showing what we have
      // (artwork doesn't depend on language) until the new text arrives.
      for (const e of Object.values(metaStore.get('entries') || {})) e.stale = true;
      for (const e of Object.values(gameInfoStore.get('games') || {})) e.stale = true;
      metaStore.save();
      gameInfoStore.save();
    }
    if (changed('sgdbKey') || langChanged) {
      makeGameInfo();
      enrichGames();
    }
    if (changed('libraries') || changed('steamEnabled') || changed('steamPath')) rescan();
    else if (changed('tmdbKey') || langChanged) enrich();
    pushLibrary();
    return settings.data;
  });
  ipcMain.handle('pick-folder', async () => {
    const r = await dialog.showOpenDialog(win, { properties: ['openDirectory'] });
    return r.canceled ? null : r.filePaths[0];
  });
  ipcMain.handle('pick-vlc', async () => {
    const r = await dialog.showOpenDialog(win, {
      properties: ['openFile'],
      filters: process.platform === 'win32' ? [{ name: 'VLC', extensions: ['exe'] }] : []
    });
    return r.canceled ? null : r.filePaths[0];
  });
  ipcMain.handle('detect-vlc', () => findVlc(settings.get('vlcPath')));
  ipcMain.handle('clear-metadata', () => {
    metaStore.set('entries', {});
    gameInfoStore.set('games', {});
    enrich();
    enrichGames();
  });
  ipcMain.handle('show-in-folder', (_e, p) => {
    const known = library.movies.some((m) => m.path === p) || library.shows.some((s) => s.episodes.some((e) => e.path === p));
    if (known) shell.showItemInFolder(p);
  });

  // System (quick menu & status bar)
  ipcMain.handle('system-get', async () => {
    if (!helper.supported) return { supported: false };
    const [volume, muted, brightness] = await Promise.all([
      helper.call('getVolume').catch(() => null),
      helper.call('getMute').catch(() => null),
      helper.call('getBrightness').catch(() => null)
    ]);
    return { supported: true, volume, muted, brightness };
  });
  ipcMain.handle('system-set', (_e, { key, value }) => {
    const cmd = { volume: 'setVolume', muted: 'setMute', brightness: 'setBrightness' }[key];
    return cmd ? helper.call(cmd, value).catch(() => null) : null;
  });
  ipcMain.handle('wifi', () => wifi());
  ipcMain.handle('power', async (_e, action) => {
    if (action === 'desktop') return win.minimize();
    if (!['sleep', 'restart', 'shutdown'].includes(action)) return;
    for (const s of [settings, libraryStore, progressStore, metaStore, gamesStore, gameInfoStore, prefsStore]) if (s.timer) s.flush();
    return power(action, helper).catch(() => null);
  });
  ipcMain.handle('open-external', (_e, url) => {
    if (/^(https:\/\/|ms-settings:)/.test(url)) shell.openExternal(url);
  });

  ipcMain.handle('toggle-fullscreen', () => win && win.setFullScreen(!win.isFullScreen()));
  ipcMain.handle('minimize', () => win && win.minimize());
  ipcMain.handle('quit', () => app.quit());
}

app.on('second-instance', () => {
  if (ui.suspended) resumeUi();
  else bringToFront();
});

app.whenReady().then(() => {
  migrateFromMarquee();
  settings = new JsonStore(userFile('settings.json'), DEFAULT_SETTINGS);
  libraryStore = new JsonStore(userFile('library.json'), library);
  progressStore = new JsonStore(userFile('progress.json'), { items: {} });
  metaStore = new JsonStore(userFile('metadata.json'), { entries: {} });
  gamesStore = new JsonStore(userFile('games.json'), { manual: [], overrides: {}, stats: {} });
  gameInfoStore = new JsonStore(userFile('gameinfo.json'), { games: {} });
  prefsStore = new JsonStore(userFile('prefs.json'), { favorites: {}, hidden: {} });
  library = { games: [], ...libraryStore.data };
  metadata = new Metadata({ store: metaStore, imageDir: userFile('artwork'), apiKey: settings.get('tmdbKey') });
  makeGameInfo();
  hasBattery().then((v) => {
    batteryPresent = v;
    pushLibrary();
  });

  registerIpc();
  createWindow();
  // Show the cached library immediately, then refresh it in the background.
  win.webContents.once('did-finish-load', () => rescan());
});

app.on('before-quit', () => {
  if (session) session.kill();
  helper.stop();
  for (const s of [settings, libraryStore, progressStore, metaStore, gamesStore, gameInfoStore, prefsStore]) if (s && s.timer) s.flush();
});

app.on('window-all-closed', () => app.quit());
