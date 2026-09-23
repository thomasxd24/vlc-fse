'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const path = require('path');
const I18N = require('../renderer/i18n.js');

const { en, fr } = I18N.dicts;

test('English and French have the same keys', () => {
  const missingFr = Object.keys(en).filter((k) => !(k in fr));
  const extraFr = Object.keys(fr).filter((k) => !(k in en));
  assert.deepEqual(missingFr, [], 'missing in French');
  assert.deepEqual(extraFr, [], 'only in French');
});

test('every translation key used in the code exists', () => {
  const files = ['renderer/core.js', 'renderer/views.js', 'renderer/app.js', 'main.js'].map((f) => fs.readFileSync(path.join(__dirname, '..', f), 'utf8'));
  const used = new Set();
  for (const src of files) {
    for (const m of src.matchAll(/\bt\('([\w.]+)'/g)) used.add(m[1]);
    for (const m of src.matchAll(/'((?:err|hint|tab|sort|filter|toast)\.\w+)'/g)) used.add(m[1]);
    for (const m of src.matchAll(/data-hint="([\w.]+)"/g)) used.add(m[1]);
    for (const m of src.matchAll(/(?:toast|errorKey:?)\(?\s*'([a-z]+\.\w+)'/g)) used.add(m[1]);
  }
  const missing = [...used].filter((k) => !(k in en));
  assert.deepEqual(missing, []);
});

test('placeholders match between languages', () => {
  const vars = (s) => (typeof s === 'string' ? s : `${s.one} ${s.other}`).match(/\{\w+\}/g)?.sort().join() || '';
  const bad = Object.keys(en).filter((k) => vars(en[k]) !== vars(fr[k]));
  assert.deepEqual(bad, []);
});

test('plurals and interpolation', () => {
  I18N.setLang('fr');
  assert.equal(I18N.t('n.seasons', { n: 1 }), '1 saison');
  assert.equal(I18N.t('n.seasons', { n: 3 }), '3 saisons');
  assert.equal(I18N.t('game.hoursPlayed', { n: 1.5 }), '1,5 heure de jeu');
  I18N.setLang('en');
  assert.equal(I18N.t('n.seasons', { n: 1 }), '1 season');
  assert.equal(I18N.t('game.hoursPlayed', { n: 1.5 }), '1.5 hours played');
  assert.equal(I18N.t('err.vlcExited', { code: 1234 }), I18N.t('err.vlcExited', { code: 1234 }).replace('1,234', '1234'));
  assert.equal(I18N.t('no.such.key'), 'no.such.key');
});
