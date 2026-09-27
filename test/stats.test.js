'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const Stats = require('../renderer/stats.js');

const at = (y, m, d, h = 20) => new Date(y, m - 1, d, h).getTime();
const NOW = at(2026, 9, 26, 21);

const data = {
  items: {
    g1: { type: 'game', title: 'Hades', playtime: 90 },
    g2: { type: 'game', title: 'Celeste', playtime: 0 },
    g3: { type: 'game', title: 'Portal 2', playtime: 1200 }, // Steam playtime only, never launched from Foyer
    s1: { type: 'show', title: 'Dark' },
    m1: { type: 'movie', title: 'Heat' }
  },
  sessions: [
    { kind: 'game', id: 'g1', start: at(2026, 9, 26), minutes: 60 },
    { kind: 'game', id: 'g1', start: at(2026, 9, 24), minutes: 30 },
    { kind: 'game', id: 'g2', start: at(2026, 9, 1), minutes: 45 },
    { kind: 'watch', id: 's1', start: at(2026, 9, 25), minutes: 50 },
    { kind: 'watch', id: 'm1', start: at(2026, 3, 2), minutes: 120 },
    { kind: 'watch', id: 'gone', start: at(2026, 9, 25), minutes: 10 } // removed from the library
  ]
};

test('week: seven daily buckets ending today, totals and rankings for the period', () => {
  const r = Stats.aggregate(data, 'week', NOW);
  assert.equal(r.buckets.length, 7);
  assert.equal(r.buckets[6].start, at(2026, 9, 26, 0));
  assert.equal(r.buckets[6].game, 60);
  assert.equal(r.buckets[5].watch, 60); // Dark + the removed item
  assert.equal(r.totals.game, 90);
  assert.equal(r.totals.watch, 60);
  assert.equal(r.totals.activeDays, 3);
  assert.deepEqual(r.games.map((g) => [g.title, g.minutes]), [['Hades', 90]]);
  assert.deepEqual(r.watched.map((w) => w.title), ['Dark']); // unknown ids are left out of the rankings
  assert.equal(r.max, 60);
});

test('month and year periods', () => {
  const month = Stats.aggregate(data, 'month', NOW);
  assert.equal(month.buckets.length, 30);
  assert.equal(month.totals.game, 135);
  const year = Stats.aggregate(data, 'year', NOW);
  assert.equal(year.buckets.length, 12);
  assert.equal(year.buckets[11].start, at(2026, 9, 1, 0));
  assert.equal(year.buckets[5].watch, 120); // March
  assert.deepEqual(year.watched.map((w) => w.title), ['Heat', 'Dark']);
});

test('all time adds playtime Foyer never saw (Steam totals)', () => {
  const r = Stats.aggregate(data, 'all', NOW);
  assert.deepEqual(r.games.map((g) => [g.title, g.minutes]), [['Portal 2', 1200], ['Hades', 90], ['Celeste', 45]]);
  assert.equal(r.totals.game, 1335);
  assert.equal(r.buckets[0].start, at(2026, 3, 1, 0)); // months from the first logged session
  assert.equal(r.buckets.at(-1).start, at(2026, 9, 1, 0));
});

test('empty log', () => {
  const r = Stats.aggregate({ sessions: [], items: {} }, 'all', NOW);
  assert.equal(r.hasLog, false);
  assert.equal(r.max, 0);
  assert.equal(r.buckets.length, 3);
});

test('gridlines are round and cover the tallest bar', () => {
  assert.deepEqual(Stats.gridSteps(100), [0, 30, 60, 90, 120]);
  assert.deepEqual(Stats.gridSteps(0), [0, 15]);
  const g = Stats.gridSteps(1000);
  assert.ok(g.at(-1) >= 1000 && g.length <= 6 && g[1] % 60 === 0);
});
