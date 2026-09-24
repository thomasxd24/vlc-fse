'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const vdf = require('../src/vdf');
const { scanSteam } = require('../src/steam');
const { titleFromExe, splitArgs } = require('../src/games');
const { GameInfo, stripHtml } = require('../src/gameinfo');

function write(root, rel, content = '') {
  const p = path.join(root, rel);
  fs.mkdirSync(path.dirname(p), { recursive: true });
  fs.writeFileSync(p, content);
  return p;
}

test('vdf parser handles nesting, escapes, comments and case', () => {
  const data = vdf.parse(`
    // comment
    "UserLocalConfigStore"
    {
      "Software" { "Valve" { "Steam" { "Apps" {
        "620" { "LastPlayed" "1700000000" "Playtime" "95" }
      } } } }
      "path"  "D:\\\\Steam Library"
      "quote" "say \\"hi\\""
    }`);
  assert.equal(vdf.get(data, 'userlocalconfigstore', 'software', 'valve', 'steam', 'apps', '620', 'playtime'), '95');
  assert.equal(vdf.get(data, 'UserLocalConfigStore', 'path'), 'D:\\Steam Library');
  assert.equal(vdf.get(data, 'UserLocalConfigStore', 'quote'), 'say "hi"');
  assert.equal(vdf.get(data, 'nope', 'x'), undefined);
});

test('scans installed Steam games with playtime and local artwork', async () => {
  const steam = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-steam-'));
  const lib2 = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-lib-'));
  write(steam, 'steamapps/libraryfolders.vdf', `"libraryfolders" { "0" { "path" "${steam.replace(/\\/g, '\\\\')}" } "1" { "path" "${lib2.replace(/\\/g, '\\\\')}" } }`);
  write(steam, 'steamapps/appmanifest_620.acf', '"AppState" { "appid" "620" "name" "Portal 2" "installdir" "Portal 2" "StateFlags" "4" "SizeOnDisk" "123" }');
  write(steam, 'steamapps/appmanifest_228980.acf', '"AppState" { "appid" "228980" "name" "Steamworks Common Redistributables" "StateFlags" "4" }');
  write(steam, 'steamapps/appmanifest_999.acf', '"AppState" { "appid" "999" "name" "Downloading Game" "StateFlags" "1026" }');
  write(lib2, 'steamapps/appmanifest_1145360.acf', '"AppState" { "appid" "1145360" "name" "Hades" "installdir" "Hades" "StateFlags" "4" }');
  write(steam, 'config/loginusers.vdf', '"users" { "76561197960287930" { "AccountName" "gabe" "PersonaName" "Gabe" "MostRecent" "1" "Timestamp" "1" } }');
  // 76561197960287930 & 0xffffffff = 22202
  write(steam, 'userdata/22202/config/localconfig.vdf', '"UserLocalConfigStore" { "Software" { "Valve" { "Steam" { "apps" { "620" { "LastPlayed" "1700000000" "Playtime" "95" } } } } } }');
  write(steam, 'userdata/22202/config/grid/620p.png', 'x'); // custom cover set in Steam
  write(steam, 'appcache/librarycache/620_library_hero.jpg', 'x'); // old flat layout
  write(steam, 'appcache/librarycache/1145360/abcdef/library_600x900.jpg', 'x'); // new nested layout
  write(steam, 'appcache/librarycache/1145360/logo.png', 'x');

  const r = await scanSteam(steam);
  assert.equal(r.steamPath, steam);
  assert.equal(r.user.accountId, '22202');
  const byId = Object.fromEntries(r.games.map((g) => [g.appid, g]));
  assert.deepEqual(Object.keys(byId).sort(), ['1145360', '620'], 'tools and half-installed apps are skipped');
  assert.equal(byId['620'].title, 'Portal 2');
  assert.equal(byId['620'].playtime, 95);
  assert.equal(byId['620'].lastPlayed, 1700000000 * 1000);
  assert.ok(byId['620'].art.poster.endsWith('620p.png'));
  assert.ok(byId['620'].art.hero.endsWith('620_library_hero.jpg'));
  assert.ok(byId['1145360'].art.poster.endsWith(path.join('abcdef', 'library_600x900.jpg')));
  assert.ok(byId['1145360'].art.logo.endsWith('logo.png'));
  assert.ok(byId['1145360'].installDir.startsWith(lib2));
  fs.rmSync(steam, { recursive: true, force: true });
  fs.rmSync(lib2, { recursive: true, force: true });
});

test('scanSteam returns null without Steam', async () => {
  const empty = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-nosteam-'));
  const prev = process.env.FOYER_STEAM_PATH;
  delete process.env.FOYER_STEAM_PATH;
  const home = process.env.HOME;
  process.env.HOME = empty;
  try {
    assert.equal(await scanSteam(path.join(empty, 'nope')), null);
  } finally {
    process.env.HOME = home;
    if (prev) process.env.FOYER_STEAM_PATH = prev;
  }
});

test('manual game titles and launch options', () => {
  assert.equal(titleFromExe('C:\\Games\\Hades II\\Hades2.exe'.replace(/\\/g, path.sep)), 'Hades II');
  assert.equal(titleFromExe(['C:', 'Games', 'Celeste', 'bin', 'x64', 'Celeste.exe'].join(path.sep)), 'Celeste');
  assert.equal(titleFromExe(['C:', 'Games', 'Tetris.exe'].join(path.sep)), 'Tetris');
  assert.equal(titleFromExe(['D:', 'Emu', 'Dolphin', 'Binaries', 'Dolphin.exe'].join(path.sep)), 'Dolphin');
  assert.deepEqual(splitArgs('-windowed --profile "My Profile"'), ['-windowed', '--profile', 'My Profile']);
  assert.deepEqual(splitArgs(''), []);
});

test('game info: Steam store details, CDN art and manual-game matching', async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-gi-'));
  const seen = [];
  t.mock.method(globalThis, 'fetch', async (url) => {
    const u = new URL(url);
    seen.push(u.hostname + u.pathname);
    const json = (body) => ({ ok: true, status: 200, json: async () => body });
    const image = () => ({ ok: true, arrayBuffer: async () => new Uint8Array(500).buffer });
    if (u.hostname.includes('steamstatic')) return u.pathname.endsWith('library_600x900_2x.jpg') ? { ok: false, status: 404 } : image();
    if (u.pathname === '/api/storesearch/') return json({ items: [{ id: 1145360, name: 'Hades', type: 'app' }, { id: 1, name: 'Hades Star' }] });
    if (u.pathname === '/api/appdetails') {
      assert.equal(u.searchParams.get('l'), 'french');
      return json({
        1145360: {
          success: true,
          data: {
            name: 'Hades',
            short_description: 'Défiez le dieu des morts &amp; frappez&nbsp;fort.',
            genres: [{ description: 'Action' }, { description: 'Indépendant' }],
            developers: ['Supergiant Games'],
            publishers: ['Supergiant Games'],
            release_date: { date: '17 sept. 2020' },
            metacritic: { score: 93 },
            controller_support: 'full',
            screenshots: [{ path_thumbnail: 'https://cdn.akamai.steamstatic.com/s1_600.jpg', path_full: 'https://cdn.akamai.steamstatic.com/s1_1920.jpg' }]
          }
        }
      });
    }
    throw new Error('unexpected ' + url);
  });
  const store = { data: { games: {} }, get(k) { return this.data[k]; }, set(k, v) { this.data[k] = v; }, save() {} };
  const gi = new GameInfo({ store, imageDir: dir, language: 'fr' });
  gi.lastStoreCall = 0;
  const STORE_WAIT = gi.storeApi.bind(gi);
  gi.storeApi = (p, q) => ((gi.lastStoreCall = 0), STORE_WAIT(p, q)); // skip rate-limit waits in tests

  const r = await gi.fetchGame({ id: 'game-1', source: 'manual', title: 'Hades', art: {}, override: {} });
  assert.equal(r.steamAppId, '1145360');
  assert.equal(r.overview, 'Défiez le dieu des morts & frappez fort.');
  assert.deepEqual(r.genres, ['Action', 'Indépendant']);
  assert.equal(r.metacritic, 93);
  assert.equal(r.controller, 'full');
  assert.equal(r.screenshots.length, 1);
  assert.ok(fs.existsSync(r.art.poster), 'falls back to the 1x poster when the 2x one is missing');
  assert.ok(r.art.hero && r.art.logo && r.art.header);

  // A title that doesn't really match a store result isn't matched.
  const miss = await gi.fetchGame({ id: 'game-2', source: 'manual', title: 'Hadés Fan Remake Deluxe', art: {}, override: {} });
  assert.equal(miss.steamAppId, null);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('stripHtml', () => {
  assert.equal(stripHtml('<p>A &quot;b&quot;<br>c</p>'), 'A "b"\nc');
});

test('a refetch that finds nothing keeps the info we already had', async (t) => {
  const store = { data: { games: { 'steam-1': { title: 'Old', overview: 'Kept', art: {}, stale: true, at: 1 } } }, get(k) { return this.data[k]; }, set(k, v) { this.data[k] = v; }, save() {} };
  const gi = new GameInfo({ store, imageDir: os.tmpdir() });
  gi.fetchGame = async () => ({ steamAppId: '1', art: {} }); // store page gone
  await gi.enrich([{ id: 'steam-1', source: 'steam', appid: '1', title: 'Old', art: {}, override: {} }], () => {});
  assert.equal(gi.lookup('steam-1').overview, 'Kept');
  assert.equal(gi.needs('steam-1'), false);
});
