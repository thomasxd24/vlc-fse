'use strict';

/*
 * Regenerates docs/*.jpg: boots the real renderer in headless Chromium against a stubbed `window.lounge`
 * and a made-up library (generated artwork, invented titles), then photographs each screen.
 *
 *   npm i --no-save playwright && npx playwright install chromium
 *   node scripts/screenshots.js
 */

const path = require('path');
const { chromium } = require('playwright');

const root = path.join(__dirname, '..');
const W = 1600;
const H = 1000;
const NOW = Date.now();
const DAY = 86400e3;

const enc = (svg) => 'data:image/svg+xml,' + encodeURIComponent(svg);
const esc = (s) => s.replace(/&/g, '&amp;');

/** Generated artwork: two blurred light blobs over a gradient, with the title set in it. */
function art(w, h, hue, title, { text = true } = {}) {
  const c = (dh, l) => `hsl(${(hue + dh) % 360} 70% ${l}%)`;
  const size = Math.round(w / 9);
  const lines = title.toUpperCase().split(' ');
  const words = text
    ? lines.map((l, i) => `<text x="${w * 0.07}" y="${h * 0.16 + i * size * 1.05}" font-family="Georgia,serif" font-style="italic" font-weight="700" font-size="${size}" fill="#fff" fill-opacity=".95">${esc(l)}</text>`).join('')
    : '';
  return enc(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${c(0, 26)}"/><stop offset="1" stop-color="${c(40, 8)}"/></linearGradient>
    <filter id="b" x="-60%" y="-60%" width="220%" height="220%"><feGaussianBlur stdDeviation="${w / 10}"/></filter></defs>
    <rect width="${w}" height="${h}" fill="url(#g)"/>
    <ellipse cx="${w * 0.7}" cy="${h * 0.4}" rx="${w * 0.2}" ry="${h * 0.22}" fill="${c(120, 58)}" filter="url(#b)"/>
    <ellipse cx="${w * 0.25}" cy="${h * 0.62}" rx="${w * 0.14}" ry="${h * 0.14}" fill="${c(200, 50)}" filter="url(#b)"/>
    <path d="M0 ${h} L0 ${h * 0.8} Q${w * 0.3} ${h * 0.6} ${w * 0.55} ${h * 0.82} T${w} ${h * 0.74} L${w} ${h}Z" fill="#000" fill-opacity=".4"/>${words}</svg>`);
}
const poster = (hue, title) => art(600, 900, hue, title);
const wide = (hue, title, text = false) => art(1920, 1080, hue, title, { text });

const progress = (time = 0, length = 0, watched = false) => ({ time, length, resumable: time > 60 && !watched, watched });

const gameDefs = [
  ['Starfall Drift', 30, 2 / 24, 4200, 'Space · Roguelike'],
  ['Ember Keep', 190, 2, 9100, 'Action RPG · Fantasy'],
  ['Tidewalker', 100, 6, 2600, 'Adventure · Exploration'],
  ['Neon Courier', 340, 21, 15000, 'Racing · Arcade'],
  ['Glass Orbit', 200, 35, 800, 'Puzzle · Space'],
  ['Mossbound', 60, 50, 6400, 'Platformer · Cozy']
];
const games = gameDefs.map(([title, hue, days, minutes, genres], i) => ({
  id: `g${i}`,
  type: 'game',
  title,
  source: i === 3 ? 'manual' : 'steam',
  poster: poster(hue, title),
  hero: wide(hue, title),
  header: wide(hue, title),
  logo: null,
  genres: genres.split(' · '),
  overview: 'Made-up sample game for the screenshots. Nothing here is a real title.',
  releaseDate: '12 Mar 2024',
  metacritic: 70 + ((i * 7) % 25),
  controller: 'full',
  developers: ['Lantern Works'],
  publishers: ['Lantern Works'],
  screenshots: [1, 2, 3, 4].map((n) => ({ thumb: art(640, 360, (hue + n * 30) % 360, '', { text: false }), full: '' })),
  playtime: minutes,
  lastPlayed: NOW - days * DAY,
  addedAt: NOW - (10 + i) * DAY,
  favorite: i === 1,
  hidden: false
}));

const movieDefs = [
  ['The Long Tide', 210, 2021, 118, 7.8],
  ['Paper Lanterns', 20, 2019, 102, 7.4],
  ['Night Freight', 280, 2023, 131, 8.1],
  ['Small Hours', 160, 2018, 96, 7.0],
  ['Copper Sky', 40, 2022, 109, 7.6]
];
const movies = movieDefs.map(([title, hue, year, runtime, rating], i) => ({
  id: `m${i}`,
  type: 'movie',
  title,
  year,
  runtime,
  rating,
  genres: ['Drama'],
  overview: 'A made-up film for the screenshots.',
  poster: poster(hue, title),
  backdrop: wide(hue, title),
  progress: i === 0 ? progress(2400, 7080) : progress(0, 0, i === 3),
  addedAt: NOW - (3 + i) * DAY,
  favorite: false,
  hidden: false
}));

const episodeTitles = ['Pilot', 'The Long Way Round', 'Static', 'Salt and Iron', 'Homecoming', 'Low Tide', 'The Quiet Room', 'Afterglow'];
function makeShow(id, title, hue, seasons, watchedThrough) {
  const episodes = [];
  for (let s = 1; s <= seasons; s++) {
    for (let e = 1; e <= 8; e++) {
      const n = (s - 1) * 8 + e;
      const watched = n <= watchedThrough;
      const partial = n === watchedThrough + 1;
      episodes.push({
        id: `${id}-s${s}e${e}`,
        season: s,
        episode: e,
        title: episodeTitles[e - 1],
        runtime: 48,
        airDate: `${2019 + s}-0${e}-14`,
        overview: 'A made-up episode for the screenshots, in which nothing in particular happens.',
        thumb: art(800, 450, (hue + e * 17) % 360, '', { text: false }),
        progress: watched ? progress(0, 0, true) : partial ? progress(1100, 2900) : progress()
      });
    }
  }
  const next = episodes.find((e) => !e.progress.watched);
  return {
    id,
    type: 'show',
    title,
    year: 2020,
    rating: 8.3,
    status: 'Returning',
    genres: ['Drama', 'Mystery'],
    overview: 'A made-up series for the screenshots: a small town, a stranded ferry, and a lot of weather.',
    poster: poster(hue, title),
    backdrop: wide(hue, title),
    seasons: Array.from({ length: seasons }, (_, i) => i + 1),
    episodes,
    watchedCount: episodes.filter((e) => e.progress.watched).length,
    nextUp: next ? next.id : null,
    addedAt: NOW - 5 * DAY,
    favorite: false,
    hidden: false
  };
}
const shows = [makeShow('sh0', 'Harbor Lights', 210, 3, 11), makeShow('sh1', 'The Salt Road', 20, 2, 16), makeShow('sh2', 'Quiet Signals', 300, 1, 3)];

function state({ lang = 'en', resume = 'game' } = {}) {
  const cont = resume === 'show' ? [{ kind: 'episode', id: shows[0].nextUp, at: NOW - 3600e3 }] : [];
  return {
    settings: { uiScale: 1, sounds: false, haptics: false, animations: 'full', libraries: [], uiLanguage: lang },
    lang,
    scanning: false,
    metaStatus: {},
    gameInfoStatus: {},
    uiState: null,
    toasts: [],
    platform: 'win32',
    servers: [],
    apps: [],
    transfers: [],
    version: '3.0.0',
    hasBattery: true,
    library: {
      steamUser: 'Thomas',
      steamFound: true,
      games: resume === 'game' ? games : games.map((g) => ({ ...g, lastPlayed: 0 })),
      movies,
      shows,
      continueWatching: cont,
      nextUp: [],
      stats: {}
    }
  };
}

async function open(browser, st) {
  const page = await browser.newPage({ viewport: { width: W, height: H } });
  page.on('pageerror', (e) => console.log('PAGEERR', e.stack.split('\n').slice(0, 3).join(' | ')));
  await page.addInitScript((s) => {
    const noop = () => () => {};
    const overrides = {
      kind: 'test',
      getState: async () => s,
      detectVlc: async () => null,
      getSystem: async () => ({ battery: { level: 1, charging: false } })
    };
    window.lounge = new Proxy(overrides, { get: (t, k) => (k in t ? t[k] : String(k).startsWith('on') ? noop : async () => null) });
    // Pretend the controller is connected so the hint bar shows pad glyphs.
    navigator.getGamepads = () => [{ id: 'Xbox 360 Controller (STANDARD GAMEPAD Vendor: 045e Product: 028e)', index: 0, connected: true, buttons: Array.from({ length: 17 }, () => ({ pressed: false, value: 0 })), axes: [0, 0, 0, 0], mapping: 'standard', timestamp: 0 }];
  }, st);
  await page.goto('file://' + path.join(root, 'renderer', 'index.html') + '?resume=1');
  await page.waitForTimeout(1200);
  return page;
}

async function shot(page, name) {
  await page.waitForTimeout(900);
  await page.screenshot({ path: path.join(root, 'docs', name), type: 'jpeg', quality: 88 });
  console.log('wrote docs/' + name);
}

(async () => {
  const browser = await chromium.launch();

  let page = await open(browser, state({ resume: 'game' }));
  await shot(page, 'home.jpg');
  await page.evaluate(() => switchTab('games'));
  await shot(page, 'games.jpg');
  await page.evaluate(() => go({ name: 'game', id: 'g1' }));
  await shot(page, 'game.jpg');
  await page.evaluate(() => { openOptions('game:g1'); });
  await shot(page, 'options.jpg');
  await page.close();

  page = await open(browser, state({ resume: 'show' }));
  await page.evaluate(() => go({ name: 'show', id: 'sh0' }));
  await shot(page, 'show.jpg');
  await page.evaluate(() => {
    S.nowPlaying = { current: 'D:\\TV\\Harbor Lights\\Season 1\\Harbor.Lights.S01E04.mkv', title: 'Harbor Lights · S01E04', time: 1100, length: 2900, paused: false };
    renderNowPlaying();
  });
  await shot(page, 'now-playing.jpg');
  await page.close();

  page = await open(browser, state({ lang: 'fr', resume: 'show' }));
  await shot(page, 'home-fr.jpg');

  await browser.close();
})();
