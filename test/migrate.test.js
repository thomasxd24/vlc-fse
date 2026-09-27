'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { migrateUserData } = require('../src/migrate');

function setup() {
  const appData = fs.mkdtempSync(path.join(os.tmpdir(), 'lounge-appdata-'));
  const put = (rel, content) => {
    const p = path.join(appData, rel);
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(p, content);
  };
  return { appData, put, userData: path.join(appData, 'Lounge') };
}

test('carries Foyer data over to Lounge, pointing artwork paths at the new folder', () => {
  const { appData, put, userData } = setup();
  const oldArt = path.join(appData, 'Foyer', 'artwork', 'p.jpg');
  put('Foyer/settings.json', JSON.stringify({ uiLanguage: 'fr' }));
  put('Foyer/metadata.json', JSON.stringify({ entries: { x: { poster: oldArt } } }));
  put('Foyer/stats.json', JSON.stringify({ sessions: [{ id: 'g', minutes: 5 }] }));
  put('Foyer/artwork/p.jpg', 'img');
  put('Foyer/Local Storage/leveldb/000003.log', 'prefs');
  put('Foyer/Cache/data_0', 'browser cache, not copied');
  put('Marquee/settings.json', JSON.stringify({ uiLanguage: 'en' }));

  assert.equal(migrateUserData({ appData, userData }), path.join(appData, 'Foyer'));
  assert.equal(JSON.parse(fs.readFileSync(path.join(userData, 'settings.json'), 'utf8')).uiLanguage, 'fr');
  const poster = JSON.parse(fs.readFileSync(path.join(userData, 'metadata.json'), 'utf8')).entries.x.poster;
  assert.equal(poster, path.join(userData, 'artwork', 'p.jpg'));
  assert.ok(fs.existsSync(poster));
  assert.ok(fs.existsSync(path.join(userData, 'stats.json')));
  assert.ok(fs.existsSync(path.join(userData, 'Local Storage', 'leveldb', '000003.log')));
  assert.ok(!fs.existsSync(path.join(userData, 'Cache')));

  // Never twice: Lounge now has its own settings.
  assert.equal(migrateUserData({ appData, userData }), null);
});

test('falls back to Marquee, and does nothing without earlier data', () => {
  const a = setup();
  a.put('Marquee/settings.json', '{}');
  a.put('Marquee/progress.json', '{"items":{}}');
  assert.equal(migrateUserData({ appData: a.appData, userData: a.userData }), path.join(a.appData, 'Marquee'));
  assert.ok(fs.existsSync(path.join(a.userData, 'progress.json')));

  const b = setup();
  assert.equal(migrateUserData({ appData: b.appData, userData: b.userData }), null);
  assert.ok(!fs.existsSync(b.userData));
});
