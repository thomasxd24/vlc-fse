'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { Metadata } = require('../src/metadata');

function memoryStore() {
  return { data: { entries: {} }, get(k) { return this.data[k]; }, set(k, v) { this.data[k] = v; }, save() {} };
}

test('fetches, caches and reuses TMDB metadata', async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'marquee-art-'));
  const calls = [];
  t.mock.method(globalThis, 'fetch', async (url) => {
    const u = new URL(url);
    calls.push(u.pathname);
    const json = (body) => ({ ok: true, status: 200, json: async () => body });
    if (u.hostname === 'image.tmdb.org') return { ok: true, arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer };
    if (u.pathname === '/3/search/movie') {
      assert.equal(u.searchParams.get('query'), 'Heat');
      assert.equal(u.searchParams.get('api_key'), 'k');
      return json({ results: [{ id: 949 }] });
    }
    if (u.pathname === '/3/movie/949') return json({ id: 949, title: 'Heat', overview: 'Cops and robbers.', vote_average: 7.94, runtime: 170, genres: [{ name: 'Crime' }], poster_path: '/p.jpg', backdrop_path: '/b.jpg' });
    if (u.pathname === '/3/search/tv') return json({ results: [] });
    throw new Error('unexpected ' + url);
  });

  const store = memoryStore();
  const meta = new Metadata({ store, imageDir: dir, apiKey: 'k' });
  const library = {
    movies: [{ title: 'Heat', year: 1995 }],
    shows: [{ title: 'Nope', year: null, episodes: [] }]
  };
  let updates = 0;
  await meta.enrich(library, () => updates++);

  const hit = meta.lookup(Metadata.movieKey(library.movies[0]));
  assert.equal(hit.overview, 'Cops and robbers.');
  assert.equal(hit.rating, 7.9);
  assert.deepEqual(hit.genres, ['Crime']);
  assert.ok(fs.existsSync(hit.poster));
  assert.equal(meta.lookup(Metadata.showKey(library.shows[0])), null, 'misses are cached but not returned');
  assert.equal(updates, 2);

  // Second run: everything is cached, so no network at all.
  calls.length = 0;
  await meta.enrich(library, () => {});
  assert.deepEqual(calls, []);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('a rejected API key stops enrichment with an error', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => ({ ok: false, status: 401, json: async () => ({}) }));
  const meta = new Metadata({ store: memoryStore(), imageDir: os.tmpdir(), apiKey: 'bad' });
  await assert.rejects(meta.enrich({ movies: [{ title: 'A' }, { title: 'B' }], shows: [] }, () => {}), /rejected/);
});
