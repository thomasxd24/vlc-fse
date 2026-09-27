'use strict';

const { app, BrowserWindow, ipcMain, dialog, shell, powerSaveBlocker, safeStorage } = require('electron');
const fs = require('fs');
const path = require('path');
const { pathToFileURL } = require('url');
const { JsonStore } = require('./src/store');
const { scanLibraries } = require('./src/library');
const { Metadata } = require('./src/metadata');
const { findVlc, VlcSession } = require('./src/vlc');
const { scanSteam, launchQuietly } = require('./src/steam');
const { GameInfo } = require('./src/gameinfo');
const { GameSession, titleFromExe, manualId } = require('./src/games');
const { SystemHelper, wifi, power, setPriority, hasBattery } = require('./src/system');
const { Updater, detectInstallType } = require('./src/updater');
const { migrateUserData } = require('./src/migrate');
const remote = require('./src/remote');
const { planTransfer, SUB_EXTENSIONS } = require('./src/transfer-plan');
const { TransferQueue } = require('./src/transfers');
const { isVideoFile, stripExtension } = require('./src/parse');
const apps = require('./src/apps');
const { Tailscale } = require('./src/tailscale');

const REPO = 'thomasxd24/vlc-fse';
const UPDATE_FIRST_CHECK_MS = 30 * 1000;
const UPDATE_INTERVAL_MS = 6 * 3600 * 1000;

const WATCHED_RATIO = 0.9;
const MIN_RESUME_SECONDS = 60;
const SUSPEND_DELAY_MS = 4000;
const MAX_SESSIONS = 20000; // the play/watch log behind the stats page (~1 MB at most)
const MAX_WATCH_STEP = 5; // seconds of playback credited per VLC poll; bigger jumps are seeks, not watching

const DEFAULT_SETTINGS = {
  libraries: [], // { path, type: 'movies' | 'tv' }
  vlcPath: '',
  vlcFullscreen: true,
  vlcExtraArgs: '',
  autoplayNext: true,
  audioLanguage: 'en', // a language code, or 'original' for the file's default track
  subLanguage: 'en', // a language code, or 'off'
  tmdbKey: '',
  sgdbKey: '',
  steamEnabled: true,
  steamPath: '',
  quietSteam: true, // launch Steam games without Steam's own window coming up
  uiLanguage: 'auto', // 'auto' | 'en' | 'fr'
  startFullscreen: true,
  launchAtLogin: false,
  uiScale: 1,
  haptics: true,
  sounds: true,
  animations: 'full', // 'full' | 'reduced'
  freeWhilePlaying: true,
  autoCheckUpdates: true,
  skippedVersion: ''
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
let serversStore; // { servers: [{ id, name, protocol, host, port, username, authType, keyPath, secret, root, hostKey, insecureTls }] }
let transfers = null;
let appsStore; // { apps: [...], icons: { [id]: file }, hidden: { [id]: true }, recent: { [id]: ms }, scannedAt }
let appsScanning = false;
const tailscale = new Tailscale();
let statsStore; // { sessions: [{ kind: 'game' | 'watch', id, start, minutes }] }
let prefsStore; // { favorites: {}, hidden: {}, languages: { [movie or show id]: {audio, subs} } }
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
let updater = null;
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

/** Earlier names (Lounge, Marquee) kept their data in their own folders: carry it over on first launch. */
function migrateOldData() {
  try {
    migrateUserData({ appData: app.getPath('appData'), userData: app.getPath('userData') });
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

/** Audio/subtitle languages chosen for one film or show, overriding the defaults in Settings (null if none). */
function languagesOf(id) {
  return prefsStore.get('languages')[id] || null;
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
      languages: languagesOf(m.id),
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
      languages: languagesOf(s.id),
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

  return { movies, shows, games: buildGames(), continueWatching, scannedAt: library.scannedAt, steamFound: Boolean(library.steamPath), steamUser: library.steamUser || null };
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
    update: updater ? updater.state : null,
    hasBattery: batteryPresent,
    servers: serversStore.get('servers').map(publicServer),
    apps: appsView(),
    appsScanning,
    appsScannedAt: appsStore.get('scannedAt'),
    tailscaleInstalled: tailscale.installed,
    transfers: transfers ? transfers.state : [],
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
  library.steamUser = steam && steam.user ? steam.user.name : null;
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
    return { title: m.title, queue: [{ path: m.path, startTime: req.resume && resumable(pr) ? pr.time : 0, languages: languagesOf(m.id), itemId: m.id }] };
  }
  const show = library.shows.find((s) => s.id === req.showId);
  const idx = show ? show.episodes.findIndex((e) => e.id === req.id) : -1;
  if (idx < 0) throw new Error('notFound');
  const first = show.episodes[idx];
  const pr = progressFor(first.path);
  const rest = settings.get('autoplayNext') ? show.episodes.slice(idx + 1) : [];
  const label = (e) => `${show.title} · S${String(e.season).padStart(2, '0')}E${String(e.episode).padStart(2, '0')}`;
  const languages = languagesOf(show.id);
  return {
    title: label(first),
    queue: [
      { path: first.path, startTime: req.resume && resumable(pr) ? pr.time : 0, languages, label: label(first), itemId: show.id },
      ...rest.map((e) => ({ path: e.path, languages, label: label(e), itemId: show.id }))
    ]
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

  const watched = new Map(); // item id -> seconds actually spent watching during this session
  const watchStart = Date.now();
  let lastPos = null;
  const s = new VlcSession(vlcPath, job.queue, {
    fullscreen: settings.get('vlcFullscreen'),
    languages: { audio: settings.get('audioLanguage'), subs: settings.get('subLanguage') },
    extraArgs: splitArgs(settings.get('vlcExtraArgs'))
  });
  session = s;
  nowPlaying = { title: job.title, request: req, current: job.queue[0].path, time: 0, length: 0, paused: false, queueSize: job.queue.length, index: 0 };
  const blocker = powerSaveBlocker.start('prevent-display-sleep');

  s.on('progress', (p) => {
    if (session !== s) return;
    recordProgress(p);
    const q = job.queue.find((x) => x.path === p.path);
    if (lastPos && lastPos.path === p.path && !p.paused && q && q.itemId) {
      const step = p.time - lastPos.time;
      if (step > 0) watched.set(q.itemId, (watched.get(q.itemId) || 0) + Math.min(step, MAX_WATCH_STEP));
    }
    lastPos = { path: p.path, time: p.time };
    const index = Math.max(0, job.queue.findIndex((q) => q.path === p.path));
    const title = job.queue[index].label || nowPlaying.title;
    nowPlaying = { ...nowPlaying, title, current: p.path, time: p.time, length: p.length, paused: p.paused ?? nowPlaying.paused, index };
    send('now-playing', nowPlaying);
  });
  const finish = (errorKey, vars) => {
    if (powerSaveBlocker.isStarted(blocker)) powerSaveBlocker.stop(blocker);
    for (const [id, sec] of watched) logSession('watch', id, watchStart, sec / 60);
    watched.clear();
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

// Playback controls on the "Playing in VLC" screen, mapped to VLC's HTTP commands.
const NP_COMMANDS = {
  pause: ['pl_pause'],
  back: ['seek', '-10'],
  forward: ['seek', '+30'],
  next: ['pl_next'],
  audio: ['key', 'audio-track'],
  subs: ['key', 'subtitle-track']
};

async function nowPlayingCommand(name) {
  const cmd = NP_COMMANDS[name];
  if (!session || !cmd) return;
  const status = await session.command(...cmd).catch(() => null);
  if (status && nowPlaying) {
    nowPlaying = { ...nowPlaying, paused: status.state === 'paused', time: status.time ?? nowPlaying.time };
    send('now-playing', nowPlaying);
  }
}

function setLanguages(id, languages) {
  const map = prefsStore.get('languages');
  const clean = {};
  if (languages && languages.audio) clean.audio = String(languages.audio);
  if (languages && languages.subs) clean.subs = String(languages.subs);
  if (Object.keys(clean).length) map[id] = clean;
  else delete map[id];
  prefsStore.save();
  pushLibrary();
}

// ---------------------------------------------------------------------------
// Games

let suspendTimer = null;

/**
 * While a game runs, Lounge gets out of the way: the UI is unloaded (freeing the renderer's memory and GPU
 * work), the window is minimised, every Lounge process drops to low CPU priority, and background work
 * (scans, artwork downloads, the system helper) stops. It all comes back when the game exits or when you
 * switch back to Lounge.
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
    win.loadFile(path.join(__dirname, 'renderer', 'index.html'), { query: { resume: '1' } }); // no startup intro
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
  logSession('game', id, Date.now() - playedMs, playedMs / 60000);
}

/** One entry in the log behind the stats page: a game played or a film/show watched, from `start` (ms). */
function logSession(kind, id, start, minutes) {
  const m = Math.round(minutes);
  if (m < 1) return;
  const list = statsStore.get('sessions');
  list.push({ kind, id, start: Math.round(start), minutes: m });
  if (list.length > MAX_SESSIONS) list.splice(0, list.length - MAX_SESSIONS);
  statsStore.save();
}

/** Everything the stats page needs: the session log, plus names and art for whatever it mentions. */
function statsData() {
  const sessions = statsStore.get('sessions');
  const ids = new Set(sessions.map((x) => x.id));
  const vm = buildViewModel();
  const items = {};
  for (const g of vm.games) if (ids.has(g.id) || g.playtime) items[g.id] = { type: 'game', title: g.title, poster: g.poster, icon: g.icon, playtime: g.playtime };
  for (const m of vm.movies) if (ids.has(m.id)) items[m.id] = { type: 'movie', title: m.title, poster: m.poster };
  for (const x of vm.shows) if (ids.has(x.id)) items[x.id] = { type: 'show', title: x.title, poster: x.poster };
  return { sessions, items, now: Date.now() };
}

async function playGame(id) {
  if (gameSession) return { ok: false, errorKey: 'err.alreadyPlaying' };
  const game = rawGames().find((g) => g.id === id);
  if (!game) return { ok: false, errorKey: 'err.notFound' };
  if (game.source === 'manual' && !fs.existsSync(game.exe)) return { ok: false, errorKey: 'err.exeMissing' };

  const s = new GameSession(game, {
    openExternal: (url) => shell.openExternal(url),
    openPath: (p) => shell.openPath(p),
    launchSteam: settings.get('quietSteam') ? (appid) => launchQuietly(appid, library.steamPath) : null
  });
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
      // so stay out of the way until the user comes back to Lounge and says they're done.
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
// Servers & transfers: browse an SFTP/FTP server and copy films and shows into the library folders

const BROWSE_IDLE_MS = 60000;
const browsing = new Map(); // server id -> { client: Promise, timer }

function findServer(id) {
  return serversStore.get('servers').find((x) => x.id === id) || null;
}

/** A server as the page sees it: never the password. */
function publicServer(x) {
  const { secret, ...rest } = x;
  return { ...rest, hasSecret: Boolean(secret) };
}

// Passwords are encrypted with the OS's user key (DPAPI on Windows) when available.
function sealSecret(plain) {
  if (!plain) return '';
  if (safeStorage.isEncryptionAvailable()) return `enc:${safeStorage.encryptString(plain).toString('base64')}`;
  return `plain:${Buffer.from(plain, 'utf8').toString('base64')}`;
}

function openSecret(sealed) {
  if (!sealed) return '';
  try {
    if (sealed.startsWith('enc:')) return safeStorage.decryptString(Buffer.from(sealed.slice(4), 'base64'));
    if (sealed.startsWith('plain:')) return Buffer.from(sealed.slice(6), 'base64').toString('utf8');
  } catch {}
  return '';
}

/** Open a fresh connection to a saved server, remembering its SSH host key the first time. */
function connectServer(id) {
  const srv = findServer(id);
  if (!srv) return Promise.reject(new remote.RemoteError('noServer'));
  return remote.connect(srv, openSecret(srv.secret), {
    onHostKey: (key) => {
      srv.hostKey = key;
      serversStore.save();
      pushLibrary();
    }
  });
}

/** The connection used for browsing, kept open for a minute between folders. */
function browseClient(id) {
  let entry = browsing.get(id);
  if (!entry) {
    entry = { client: connectServer(id), timer: null };
    browsing.set(id, entry);
    entry.client.catch(() => browsing.get(id) === entry && browsing.delete(id));
  }
  clearTimeout(entry.timer);
  entry.timer = setTimeout(() => dropBrowse(id), BROWSE_IDLE_MS);
  return entry.client;
}

function dropBrowse(id) {
  const entry = browsing.get(id);
  if (!entry) return;
  browsing.delete(id);
  clearTimeout(entry.timer);
  entry.client.then((c) => c.close()).catch(() => {});
}

function remoteError(err) {
  const code = err && typeof err.code === 'string' && /^[a-zA-Z]+$/.test(err.code) ? err.code : null;
  const known = ['auth', 'hostKeyChanged', 'notFound', 'keyUnreadable', 'noHost', 'noServer', 'ENOTFOUND', 'ECONNREFUSED', 'ETIMEDOUT', 'EHOSTUNREACH'];
  return { ok: false, errorKey: known.includes(code) ? `err.remote.${code}` : 'err.remote.other', vars: { message: (err && err.message) || String(err) } };
}

async function remoteList(id, dir) {
  try {
    const c = await browseClient(id);
    const p = dir || findServer(id)?.root || (await c.home());
    return { ok: true, path: p, entries: await c.list(p) };
  } catch (err) {
    dropBrowse(id);
    return remoteError(err);
  }
}

function libraryRoots(kind, preferred) {
  const libs = settings.get('libraries').filter((l) => l.type === (kind === 'tv' ? 'tv' : 'movies'));
  return libs.find((l) => l.path === preferred) ? [preferred, ...libs.map((l) => l.path).filter((p) => p !== preferred)] : libs.map((l) => l.path);
}

function subdirs(dir) {
  try {
    return fs.readdirSync(dir, { withFileTypes: true }).filter((e) => e.isDirectory()).map((e) => e.name);
  } catch {
    return [];
  }
}

/** Work out what a remote file or folder contains and where each file will go. */
async function planRemote({ serverId, path: p, isDir, kind, library }) {
  const c = await browseClient(serverId);
  const posix = path.posix;
  const name = posix.basename(p);
  let files;
  if (isDir) {
    files = await c.walk(p);
  } else {
    // A single video brings the subtitles sitting next to it.
    const base = stripExtension(name).toLowerCase();
    const siblings = await c.list(posix.dirname(p)).catch(() => []);
    const me = siblings.find((e) => e.name === name);
    files = [{ remote: p, rel: name, size: me ? me.size : 0 }];
    if (isVideoFile(name)) {
      for (const e of siblings) {
        if (!e.isDir && e.name !== name && SUB_EXTENSIONS.has(posix.extname(e.name).toLowerCase()) && e.name.toLowerCase().startsWith(base)) {
          files.push({ remote: posix.join(posix.dirname(p), e.name), rel: e.name, size: e.size });
        }
      }
    }
  }
  const selection = { name, isDir, parent: posix.basename(posix.dirname(p)) };
  const first = planTransfer(selection, files, { kind, movieRoot: '/', tvRoot: '/' });
  const roots = libraryRoots(first.kind, library);
  const root = roots[0] || null;
  const plan = planTransfer(selection, files, {
    kind: first.kind,
    movieRoot: first.kind === 'movie' ? root : null,
    tvRoot: first.kind === 'tv' ? root : null,
    existingShowDirs: first.kind === 'tv' && root ? subdirs(root) : []
  });
  return { plan, roots, name };
}

async function remotePlan(req) {
  try {
    const { plan, roots } = await planRemote(req);
    return {
      ok: true,
      kind: plan.kind,
      root: plan.root,
      roots,
      files: plan.items.length,
      videos: plan.items.filter((i) => isVideoFile(i.rel)).length,
      totalSize: plan.totalSize,
      folders: plan.folders
    };
  } catch (err) {
    dropBrowse(req.serverId);
    return remoteError(err);
  }
}

async function remoteDownload(req) {
  try {
    const { plan, name } = await planRemote(req);
    if (!plan.root) return { ok: false, errorKey: plan.kind === 'tv' ? 'err.remote.noTvLibrary' : 'err.remote.noMovieLibrary' };
    if (!plan.items.length) return { ok: false, errorKey: 'err.remote.nothing' };
    const id = transfers.add({ serverId: req.serverId, title: req.isDir ? name : stripExtension(name), plan });
    return { ok: true, id };
  } catch (err) {
    dropBrowse(req.serverId);
    return remoteError(err);
  }
}

const PROTOCOLS = ['sftp', 'ftp', 'ftps'];

/** Add or update a saved server. `secret` replaces the stored password only when given. */
function saveServer(input) {
  const list = serversStore.get('servers');
  const existing = input.id ? list.find((x) => x.id === input.id) : null;
  const host = String(input.host || '').trim();
  const port = Number(input.port) || remote.DEFAULT_PORTS[input.protocol] || 22;
  if (!PROTOCOLS.includes(input.protocol)) return { ok: false, errorKey: 'err.remote.protocol' };
  if (!host || /\s/.test(host)) return { ok: false, errorKey: 'err.remote.noHost' };
  if (port < 1 || port > 65535) return { ok: false, errorKey: 'err.remote.port' };
  const srv = existing || { id: require('crypto').randomBytes(6).toString('hex'), secret: '', hostKey: '' };
  // Pointing the entry at another machine means its old host key no longer applies.
  if (existing && (existing.host !== host || Number(existing.port) !== port || existing.protocol !== input.protocol)) srv.hostKey = '';
  Object.assign(srv, {
    name: String(input.name || '').trim() || host,
    protocol: input.protocol,
    host,
    port,
    username: String(input.username || '').trim(),
    authType: input.protocol === 'sftp' && input.authType === 'key' ? 'key' : 'password',
    keyPath: input.protocol === 'sftp' && input.authType === 'key' ? String(input.keyPath || '') : '',
    root: String(input.root || '').trim(),
    insecureTls: input.protocol === 'ftps' && Boolean(input.insecureTls)
  });
  if (input.secret !== undefined && input.secret !== null) srv.secret = sealSecret(String(input.secret));
  if (!existing) list.push(srv);
  serversStore.save();
  dropBrowse(srv.id);
  pushLibrary();
  return { ok: true, id: srv.id };
}

function setupTransfers() {
  transfers = new TransferQueue({ connect: connectServer });
  transfers.on('update', (st) => send('transfers', st));
  transfers.on('finished', (job) => {
    if (job.status === 'done') {
      toast('xfer.doneToast', { title: job.title });
      if (job.skipped < job.items.length) rescan();
    } else if (job.status === 'error') {
      toast('xfer.failedToast', { title: job.title, message: job.error }, 'error');
    }
  });
}

// ---------------------------------------------------------------------------
// Apps: everything in the Start menu, launchable from the Apps tab

const APP_STEP_ASIDE_MS = 1200;
const isTailscaleApp = (a) => /(^|\\)tailscale-ipn\.exe$/i.test(a.appId) || /^tailscale$/i.test(a.name);

function appsView() {
  const icons = appsStore.get('icons');
  const hidden = appsStore.get('hidden');
  const recent = appsStore.get('recent');
  return appsStore.get('apps').map((a) => ({
    id: a.id,
    name: a.name,
    kind: a.kind,
    icon: icons[a.id] && fs.existsSync(icons[a.id]) ? fileUrl(icons[a.id]) : null,
    hidden: Boolean(hidden[a.id]),
    lastLaunched: recent[a.id] || 0,
    tailscale: isTailscaleApp(a)
  }));
}

/** Save an app's icon as a PNG in the artwork folder (Store logo, or the program's own icon). */
async function appIcon(a) {
  const dest = userFile(path.join('artwork', 'apps', `${a.id}.png`));
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  if (a.kind === 'store') {
    const logo = apps.storeLogo(a);
    if (!logo) return null;
    fs.copyFileSync(logo, dest);
    return dest;
  }
  let target = a.exe && fs.existsSync(a.exe) ? a.exe : null;
  if (!target && a.shortcut) {
    try {
      const link = shell.readShortcutLink(a.shortcut);
      target = [link.icon, link.target].find((p) => p && fs.existsSync(p)) || a.shortcut;
    } catch {
      target = a.shortcut;
    }
  }
  if (!target) return null;
  const img = await app.getFileIcon(target, { size: 'large' });
  if (img.isEmpty()) return null;
  fs.writeFileSync(dest, img.toPNG());
  return dest;
}

async function scanApps() {
  if (appsScanning || gameSession || process.platform !== 'win32') return;
  appsScanning = true;
  pushLibrary();
  try {
    const list = await apps.listApps();
    appsStore.set('apps', list);
    appsStore.set('scannedAt', Date.now());
    pushLibrary();
    const icons = appsStore.get('icons');
    for (const a of list) {
      if (gameSession) break;
      if (icons[a.id] && fs.existsSync(icons[a.id])) continue;
      const file = await appIcon(a).catch(() => null);
      if (file) {
        icons[a.id] = file;
        appsStore.save();
        pushLibrary();
      }
    }
  } catch (err) {
    console.error('app scan failed', err);
  } finally {
    appsScanning = false;
    pushLibrary();
  }
}

async function launchInstalledApp(id) {
  const a = appsStore.get('apps').find((x) => x.id === id);
  if (!a) return { ok: false, errorKey: 'err.notFound' };
  try {
    await apps.launchApp(a.appId);
  } catch (err) {
    return { ok: false, errorKey: 'err.appLaunch', vars: { message: err.message } };
  }
  appsStore.get('recent')[id] = Date.now();
  appsStore.save();
  pushLibrary();
  // Step aside so the app comes up in front; the home button (or Alt+Tab) brings Lounge back.
  setTimeout(() => win && !win.isDestroyed() && win.minimize(), APP_STEP_ASIDE_MS);
  return { ok: true };
}

// Tailscale (see src/tailscale.js)

async function tailscaleAction({ action, node }) {
  if (!tailscale.installed) return { ok: false, errorKey: 'err.tsMissing' };
  try {
    if (action === 'up') await tailscale.up();
    else if (action === 'down') await tailscale.down();
    else if (action === 'exitNode') await tailscale.setExitNode(node || null);
    else return { ok: false };
  } catch (err) {
    return { ok: false, errorKey: 'err.tailscale', vars: { message: err.message }, status: await tailscale.status() };
  }
  return { ok: true, status: await tailscale.status() };
}

async function tailscaleLogin() {
  if (!tailscale.installed) return { ok: false, errorKey: 'err.tsMissing' };
  try {
    const url = await tailscale.startLogin(async (ok) => {
      const st = await tailscale.status();
      send('tailscale', st);
      if (ok && st.state === 'connected') toast('ts.signedIn', { name: st.hostName ? ` · ${st.hostName}` : '' });
    });
    if (!url) return { ok: true, url: null, status: await tailscale.status() };
    const qr = await require('qrcode').toDataURL(url, { margin: 1, width: 360, errorCorrectionLevel: 'M' });
    return { ok: true, url, qr };
  } catch (err) {
    return { ok: false, errorKey: 'err.tailscale', vars: { message: err.message } };
  }
}

// ---------------------------------------------------------------------------
// Updates (always the user's call: we only check, then ask)

function setupUpdater() {
  updater = new Updater({
    repo: REPO,
    version: app.getVersion(),
    installType: process.env.LOUNGE_UPDATE_TYPE || detectInstallType({ packaged: app.isPackaged, windowsStore: process.windowsStore, exePath: process.execPath }),
    dir: userFile('updates')
  });
  updater.on('state', (st) => send('update', st));
  if (!updater.supported) return;
  const auto = () => {
    if (settings.get('autoCheckUpdates') && !gameSession && updater.state.status !== 'downloading') updater.check();
  };
  setTimeout(auto, UPDATE_FIRST_CHECK_MS);
  setInterval(auto, UPDATE_INTERVAL_MS);
}

async function installUpdate() {
  if (!updater || !updater.supported) return { ok: false };
  if (gameSession) return { ok: false, errorKey: 'err.updateWhilePlaying' };
  try {
    if (updater.state.status !== 'ready') await updater.download();
    if (gameSession) return { ok: false, errorKey: 'err.updateWhilePlaying' };
    const { command, args } = await updater.installCommand({ pid: process.pid, exePath: process.execPath });
    for (const s of [settings, libraryStore, progressStore, metaStore, gamesStore, gameInfoStore, prefsStore, statsStore, serversStore, appsStore]) if (s.timer) s.flush();
    if (session) session.kill();
    const child = require('child_process').spawn(command, args, { detached: true, stdio: 'ignore', windowsHide: true });
    await new Promise((resolve, reject) => {
      child.once('spawn', resolve);
      child.once('error', reject);
    });
    child.unref();
    // Give the UI a moment to show "Installing…", then get out of the installer's way.
    setTimeout(() => app.quit(), 1200);
    return { ok: true };
  } catch (err) {
    updater.set({ status: 'error', error: err.message });
    return { ok: false, errorKey: 'err.update', vars: { message: err.message } };
  }
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
    title: 'Lounge',
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

  // Switching back to Lounge while a game runs brings the UI back.
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
  ipcMain.handle('get-stats', () => statsData());
  ipcMain.handle('np-command', (_e, name) => nowPlayingCommand(name));
  ipcMain.handle('set-languages', (_e, { id, languages }) => setLanguages(id, languages));
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
    for (const s of [settings, libraryStore, progressStore, metaStore, gamesStore, gameInfoStore, prefsStore, statsStore, serversStore, appsStore]) if (s.timer) s.flush();
    return power(action, helper).catch(() => null);
  });
  ipcMain.handle('open-external', (_e, url) => {
    if (/^(https:\/\/|ms-settings:)/.test(url)) shell.openExternal(url);
  });

  // Servers & transfers
  ipcMain.handle('server-save', (_e, input) => saveServer(input || {}));
  ipcMain.handle('server-remove', (_e, id) => {
    dropBrowse(id);
    serversStore.set('servers', serversStore.get('servers').filter((x) => x.id !== id));
    pushLibrary();
  });
  ipcMain.handle('server-forget-key', (_e, id) => {
    const srv = findServer(id);
    if (srv) {
      srv.hostKey = '';
      serversStore.save();
      dropBrowse(id);
      pushLibrary();
    }
  });
  ipcMain.handle('server-test', async (_e, id) => {
    dropBrowse(id);
    return remoteList(id, null);
  });
  ipcMain.handle('pick-key-file', async () => {
    const r = await dialog.showOpenDialog(win, { properties: ['openFile', 'showHiddenFiles'], defaultPath: path.join(app.getPath('home'), '.ssh') });
    return r.canceled ? null : r.filePaths[0];
  });
  ipcMain.handle('remote-list', (_e, { serverId, path: p }) => remoteList(serverId, p));
  ipcMain.handle('remote-plan', (_e, req) => remotePlan(req));
  ipcMain.handle('remote-download', (_e, req) => remoteDownload(req));
  ipcMain.handle('transfer-cancel', (_e, id) => transfers.cancel(id));
  ipcMain.handle('transfer-clear', (_e, id) => transfers.clear(id));
  ipcMain.handle('transfer-retry', (_e, id) => transfers.retry(id));

  // Apps & Tailscale
  ipcMain.handle('apps-rescan', () => scanApps());
  ipcMain.handle('app-launch', (_e, id) => launchInstalledApp(id));
  ipcMain.handle('app-hide', (_e, { id, hidden }) => {
    const map = appsStore.get('hidden');
    if (hidden) map[id] = true;
    else delete map[id];
    appsStore.save();
    pushLibrary();
  });
  ipcMain.handle('tailscale-status', () => tailscale.status());
  ipcMain.handle('tailscale-action', (_e, req) => tailscaleAction(req || {}));
  ipcMain.handle('tailscale-login', () => tailscaleLogin());
  ipcMain.handle('tailscale-cancel-login', () => tailscale.cancelLogin());
  ipcMain.handle('tailscale-open-app', () => {
    const a = appsStore.get('apps').find(isTailscaleApp);
    return a ? launchInstalledApp(a.id) : { ok: false, errorKey: 'err.tsMissing' };
  });

  ipcMain.handle('update-check', () => (updater ? updater.check() : null));
  ipcMain.handle('update-install', () => installUpdate());
  ipcMain.handle('update-skip', (_e, version) => {
    settings.set('skippedVersion', String(version || ''));
    pushLibrary();
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
  migrateOldData();
  settings = new JsonStore(userFile('settings.json'), DEFAULT_SETTINGS);
  libraryStore = new JsonStore(userFile('library.json'), library);
  progressStore = new JsonStore(userFile('progress.json'), { items: {} });
  metaStore = new JsonStore(userFile('metadata.json'), { entries: {} });
  gamesStore = new JsonStore(userFile('games.json'), { manual: [], overrides: {}, stats: {} });
  gameInfoStore = new JsonStore(userFile('gameinfo.json'), { games: {} });
  prefsStore = new JsonStore(userFile('prefs.json'), { favorites: {}, hidden: {}, languages: {} });
  statsStore = new JsonStore(userFile('stats.json'), { sessions: [] });
  serversStore = new JsonStore(userFile('servers.json'), { servers: [] });
  appsStore = new JsonStore(userFile('apps.json'), { apps: [], icons: {}, hidden: {}, recent: {}, scannedAt: 0 });
  library = { games: [], ...libraryStore.data };
  metadata = new Metadata({ store: metaStore, imageDir: userFile('artwork'), apiKey: settings.get('tmdbKey') });
  makeGameInfo();
  hasBattery().then((v) => {
    batteryPresent = v;
    pushLibrary();
  });

  setupTransfers();
  registerIpc();
  createWindow();
  setupUpdater();
  // Show the cached library immediately, then refresh it in the background.
  win.webContents.once('did-finish-load', () => {
    rescan();
    // The installed-apps list refreshes a little later, so it doesn't compete with the library scan.
    setTimeout(scanApps, 8000);
  });
});

app.on('before-quit', () => {
  if (session) session.kill();
  tailscale.cancelLogin();
  for (const id of [...browsing.keys()]) dropBrowse(id);
  helper.stop();
  for (const s of [settings, libraryStore, progressStore, metaStore, gamesStore, gameInfoStore, prefsStore, statsStore, serversStore, appsStore]) if (s && s.timer) s.flush();
});

app.on('window-all-closed', () => app.quit());
