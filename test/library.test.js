'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { scanLibraries } = require('../src/library');
const { buildArgs } = require('../src/vlc');

function tree(root, files) {
  for (const f of files) {
    const p = path.join(root, f);
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(p, '');
  }
}

test('scans movies and shows with local artwork', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'marquee-'));
  const movies = path.join(root, 'Movies');
  const tv = path.join(root, 'TV');
  tree(movies, [
    'Heat (1995)/Heat.1995.1080p.mkv',
    'Heat (1995)/poster.jpg',
    'Heat (1995)/fanart.jpg',
    'Heat (1995)/Sample/sample.mkv',
    'Loose/The.Matrix.1999.mkv',
    'Loose/The.Matrix.1999-poster.jpg',
    'Loose/Alien.1979.mkv',
    'Loose/poster.jpg'
  ]);
  tree(tv, [
    'Dark (2017)/poster.jpg',
    'Dark (2017)/Season 1/Dark.S01E02.mkv',
    'Dark (2017)/Season 1/Dark.S01E01.mkv',
    'Dark (2017)/Season 2/Dark.S02E01.mkv',
    'Dark (2017)/Season 2/Dark.S02E01.srt',
    'Planet Earth/Season 1/01 From Pole to Pole.mkv',
    'Planet Earth/Season 1/02 Mountains.mkv',
    'Planet Earth/Specials/Making Of.mkv',
    'Fallout S01 MULTi 1080p WEB x265-GRP/Fallout.S01E01.MULTi.1080p.WEB.x265-GRP.mkv',
    'Fallout S02 MULTi VFF 1080p WEBrip 10 bits x265-Tyrell/Fallout.S02E01.MULTi.VFF.1080p.mkv',
    'Fallout S02 MULTi VFF 1080p WEBrip 10 bits x265-Tyrell/Fallout.S02E02.MULTi.VFF.1080p.mkv',
    'Fallout S02 MULTi VFF 1080p WEBrip 10 bits x265-Tyrell/poster.jpg'
  ]);

  const lib = await scanLibraries([
    { path: movies, type: 'movies' },
    { path: tv, type: 'tv' },
    { path: path.join(root, 'missing'), type: 'movies' }
  ]);

  assert.deepEqual(lib.movies.map((m) => [m.title, m.year]), [['Alien', 1979], ['Heat', 1995], ['The Matrix', 1999]]);
  const heat = lib.movies.find((m) => m.title === 'Heat');
  assert.equal(path.basename(heat.poster), 'poster.jpg');
  assert.equal(path.basename(heat.backdrop), 'fanart.jpg');
  const matrix = lib.movies.find((m) => m.title === 'The Matrix');
  assert.equal(path.basename(matrix.poster), 'The.Matrix.1999-poster.jpg');
  // A generic poster.jpg in a shared folder must not be attributed to an arbitrary film.
  assert.equal(lib.movies.find((m) => m.title === 'Alien').poster, null);

  assert.deepEqual(lib.shows.map((s) => s.title), ['Dark', 'Fallout', 'Planet Earth']);
  const dark = lib.shows[0];
  // Two season-pack folders merge into one show, and art from either folder is picked up.
  const fallout = lib.shows[1];
  assert.deepEqual(fallout.episodes.map((e) => [e.season, e.episode]), [[1, 1], [2, 1], [2, 2]]);
  assert.ok(fallout.poster.endsWith('poster.jpg'));
  assert.equal(dark.year, 2017);
  assert.ok(dark.poster.endsWith('poster.jpg'));
  assert.deepEqual(dark.episodes.map((e) => [e.season, e.episode]), [[1, 1], [1, 2], [2, 1]]);

  const earth = lib.shows[2];
  assert.deepEqual(earth.episodes.map((e) => [e.season, e.episode]), [[0, 1], [1, 1], [1, 2]]);
  fs.rmSync(root, { recursive: true, force: true });
});

test('builds VLC arguments with per-item resume', () => {
  const args = buildArgs([{ path: 'C:\\a.mkv', startTime: 125.7 }, { path: 'C:\\b.mkv' }], { port: 1234, password: 'pw' });
  assert.ok(args.includes('--fullscreen'));
  assert.ok(args.includes('--http-port=1234'));
  const i = args.indexOf('C:\\a.mkv');
  assert.equal(args[i + 1], ':start-time=125');
  assert.equal(args[i + 2], 'C:\\b.mkv');
  assert.equal(args[args.length - 1], 'C:\\b.mkv');
});
