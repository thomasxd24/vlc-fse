'use strict';

const { app, BrowserWindow, ipcMain, dialog, shell, powerSaveBlocker } = require('electron');
const path = require('path');
const { pathToFileURL } = require('url');
const { JsonStore } = require('./src/store');
const { scanLibraries } = require('./src/library');
const { Metadata } = require('./src/metadata');
const { findVlc, VlcSession } = require('./src/vlc');

const WATCHED_RATIO = 0.9;
const MIN_RESUME_SECONDS = 60;

const DEFAULT_SETTINGS = {
  libraries: [], // { path, type: 'movies' | 'tv' }
  vlcPath: '',
  vlcFullscreen: true,
  vlcExtraArgs: '',
  autoplayNext: true,
  tmdbKey: '',
  language: 'en-US',
  startFullscreen: true,
  launchAtLogin: false,
  uiScale: 1
};

if (!app.requestSingleInstanceLock()) {
  app.quit();
}

let win = null;
let settings;
let libraryStore;
let progressStore;
let metaStore;
let metadata;
let library = { movies: [], shows: [], scannedAt: 0 };
let session = null;
let nowPlaying = null;
let scanning = false;
let rescanQueued = false;
let metaStatus = { running: false, error: null };

const userFile = (name) => path.join(app.getPath('userData'), name);
const fileUrl = (p) => (p ? pathToFileURL(p).href : null);
const progressKey = (p) => (process.platform === 'win32' ? p.toLowerCase() : p);

function send(channel, payload) {
  if (win && !win.isDestroyed()) win.webContents.send(channel, payload);
}

// ---------------------------------------------------------------------------
// View model: the library merged with metadata and watch progress, shaped for the UI.

function progressFor(p) {
  const e = progressStore.get('items')[progressKey(p)];
  if (!e) return { time: 0, length: 0, watched: false, updatedAt: 0 };
  return e;
}

function resumable(pr) {
  return !pr.watched && pr.time >= MIN_RESUME_SECONDS && (!pr.length || pr.time < pr.length * WATCHED_RATIO);
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
      progress: { time: pr.time, length: pr.length, watched: pr.watched, updatedAt: pr.updatedAt, resumable: resumable(pr) }
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
      lastActivity: lastWatched ? lastWatched.updatedAt : 0
    };
  });

  // Continue watching: part-watched movies, plus shows with activity and something left to watch.
  const continueWatching = [
    ...movies.filter((m) => m.progress.resumable).map((m) => ({ kind: 'movie', id: m.id, at: m.progress.updatedAt })),
    ...shows.filter((s) => s.lastActivity && s.nextUp).map((s) => ({ kind: 'episode', showId: s.id, id: s.nextUp, at: s.lastActivity }))
  ]
    .sort((a, b) => b.at - a.at)
    .slice(0, 20);

  return { movies, shows, continueWatching, scannedAt: library.scannedAt };
}

function state() {
  return {
    settings: settings.data,
    library: buildViewModel(),
    scanning,
    metaStatus,
    nowPlaying,
    platform: process.platform,
    version: app.getVersion()
  };
}

let pushTimer = null;
function pushLibrary() {
  clearTimeout(pushTimer);
  pushTimer = setTimeout(() => send('state', state()), 250);
}

// ---------------------------------------------------------------------------
// Scanning

async function rescan() {
  if (scanning) {
    // Folders changed mid-scan: run again once this pass finishes so the change isn't lost.
    rescanQueued = true;
    return;
  }
  scanning = true;
  send('state', state());
  try {
    library = await scanLibraries(settings.get('libraries'));
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
}

async function enrich() {
  metadata = new Metadata({
    store: metaStore,
    imageDir: userFile('artwork'),
    apiKey: settings.get('tmdbKey'),
    language: settings.get('language')
  });
  if (!metadata.enabled || metaStatus.running) return;
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

// ---------------------------------------------------------------------------
// Playback

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
    if (!m) throw new Error('Movie not found');
    const pr = progressFor(m.path);
    return { title: m.title, queue: [{ path: m.path, startTime: req.resume && resumable(pr) ? pr.time : 0 }] };
  }
  const show = library.shows.find((s) => s.id === req.showId);
  const idx = show ? show.episodes.findIndex((e) => e.id === req.id) : -1;
  if (idx < 0) throw new Error('Episode not found');
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
  if (!vlcPath) return { ok: false, error: 'VLC was not found. Install VLC or set its location in Settings.' };

  let job;
  try {
    job = buildQueue(req);
  } catch (err) {
    return { ok: false, error: err.message };
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
  const finish = (error) => {
    if (powerSaveBlocker.isStarted(blocker)) powerSaveBlocker.stop(blocker);
    if (session !== s) return;
    session = null;
    nowPlaying = null;
    send('now-playing', null);
    if (error) send('toast', { kind: 'error', text: `Could not start VLC: ${error.message}` });
    pushLibrary();
    bringToFront();
  };
  s.on('exit', () => finish());
  s.on('error', (err) => finish(err));

  try {
    await s.start();
  } catch (err) {
    finish(err);
    return { ok: false, error: err.message };
  }
  send('now-playing', nowPlaying);
  return { ok: true };
}

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

// ---------------------------------------------------------------------------
// Window & IPC

function createWindow() {
  win = new BrowserWindow({
    width: 1600,
    height: 900,
    minWidth: 960,
    minHeight: 540,
    fullscreen: settings.get('startFullscreen'),
    autoHideMenuBar: true,
    backgroundColor: '#07080c',
    title: 'Cinema',
    icon: path.join(__dirname, 'build', 'icon.png'),
    show: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true
    }
  });
  win.removeMenu();
  win.once('ready-to-show', () => win.show());
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));

  // Never navigate away from the app or open windows; send links to the browser.
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:/.test(url)) shell.openExternal(url);
    return { action: 'deny' };
  });
  win.webContents.on('will-navigate', (e) => e.preventDefault());
}

function registerIpc() {
  ipcMain.handle('get-state', () => state());
  ipcMain.handle('rescan', () => rescan());
  ipcMain.handle('play', (_e, req) => play(req));
  ipcMain.handle('stop', () => {
    if (session) session.kill();
  });
  ipcMain.handle('set-watched', (_e, { kind, id, showId, watched }) => {
    let paths = [];
    if (kind === 'movie') paths = library.movies.filter((m) => m.id === id).map((m) => m.path);
    else if (kind === 'episode') paths = (library.shows.find((s) => s.id === showId)?.episodes || []).filter((e) => e.id === id).map((e) => e.path);
    else if (kind === 'season') paths = (library.shows.find((s) => s.id === showId)?.episodes || []).filter((e) => e.season === id).map((e) => e.path);
    else if (kind === 'show') paths = (library.shows.find((s) => s.id === id)?.episodes || []).map((e) => e.path);
    setWatched(paths, watched);
  });
  ipcMain.handle('save-settings', async (_e, patch) => {
    const before = { ...settings.data };
    const allowed = Object.keys(DEFAULT_SETTINGS);
    for (const [k, v] of Object.entries(patch || {})) if (allowed.includes(k)) settings.data[k] = v;
    settings.flush();

    if (patch.launchAtLogin !== undefined && app.isPackaged) {
      app.setLoginItemSettings({ openAtLogin: Boolean(patch.launchAtLogin) });
    }
    if (patch.startFullscreen !== undefined && win) win.setFullScreen(Boolean(patch.startFullscreen));
    if (JSON.stringify(before.libraries) !== JSON.stringify(settings.data.libraries)) rescan();
    else if (before.tmdbKey !== settings.data.tmdbKey || before.language !== settings.data.language) {
      if (before.language !== settings.data.language) metaStore.set('entries', {});
      enrich();
    }
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
    enrich();
  });
  ipcMain.handle('show-in-folder', (_e, p) => {
    const known = library.movies.some((m) => m.path === p) || library.shows.some((s) => s.episodes.some((e) => e.path === p));
    if (known) shell.showItemInFolder(p);
  });
  ipcMain.handle('toggle-fullscreen', () => win && win.setFullScreen(!win.isFullScreen()));
  ipcMain.handle('minimize', () => win && win.minimize());
  ipcMain.handle('quit', () => app.quit());
}

app.on('second-instance', () => bringToFront());

app.whenReady().then(() => {
  settings = new JsonStore(userFile('settings.json'), DEFAULT_SETTINGS);
  libraryStore = new JsonStore(userFile('library.json'), library);
  progressStore = new JsonStore(userFile('progress.json'), { items: {} });
  metaStore = new JsonStore(userFile('metadata.json'), { entries: {} });
  library = libraryStore.data;
  metadata = new Metadata({ store: metaStore, imageDir: userFile('artwork'), apiKey: settings.get('tmdbKey') });

  registerIpc();
  createWindow();
  // Show the cached library immediately, then refresh it in the background.
  win.webContents.once('did-finish-load', () => rescan());
});

app.on('before-quit', () => {
  if (session) session.kill();
  for (const s of [settings, libraryStore, progressStore, metaStore]) if (s && s.timer) s.flush();
});

app.on('window-all-closed', () => app.quit());
