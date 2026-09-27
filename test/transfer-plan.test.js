'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('path');
const { planTransfer, safeName } = require('../src/transfer-plan');

const M = path.join('/lib', 'Movies');
const T = path.join('/lib', 'TV');
const files = (list, base = '/remote') => list.map((rel) => ({ remote: `${base}/${rel}`, rel, size: 100 }));
const dests = (plan) => plan.items.map((i) => path.relative('/lib', i.dest).split(path.sep).join('/'));

test('a film folder goes to Movies/Title (Year) with its subtitles, without samples', () => {
  const plan = planTransfer(
    { name: 'Heat.1995.1080p.BluRay.x264-GRP', isDir: true },
    files(['Heat.1995.1080p.BluRay.x264-GRP.mkv', 'Heat.1995.1080p.BluRay.x264-GRP.en.srt', 'Subs/French.srt', 'Sample/sample.mkv', 'heat.nfo']),
    { movieRoot: M, tvRoot: T }
  );
  assert.equal(plan.kind, 'movie');
  assert.deepEqual(dests(plan), ['Movies/Heat (1995)/French.srt', 'Movies/Heat (1995)/Heat.1995.1080p.BluRay.x264-GRP.en.srt', 'Movies/Heat (1995)/Heat.1995.1080p.BluRay.x264-GRP.mkv']);
  assert.equal(plan.totalSize, 300);
});

test('a single film file, and a folder of several films', () => {
  const one = planTransfer({ name: 'The.Matrix.1999.mkv', isDir: false }, files(['The.Matrix.1999.mkv']), { movieRoot: M, tvRoot: T });
  assert.deepEqual(dests(one), ['Movies/The Matrix (1999)/The.Matrix.1999.mkv']);
  const many = planTransfer({ name: 'Films', isDir: true }, files(['Alien.1979.mkv', 'Aliens.1986.mkv', 'Aliens.1986.srt']), { movieRoot: M, tvRoot: T });
  assert.deepEqual(dests(many), ['Movies/Alien (1979)/Alien.1979.mkv', 'Movies/Aliens (1986)/Aliens.1986.mkv', 'Movies/Aliens (1986)/Aliens.1986.srt']);
});

test('a season pack goes to TV/Show/Season NN and merges into an existing show folder', () => {
  const plan = planTransfer(
    { name: 'Fallout S02 MULTi VFF 1080p WEBrip x265-GRP', isDir: true },
    files(['Fallout.S02E01.MULTi.1080p.mkv', 'Fallout.S02E02.MULTi.1080p.mkv', 'Fallout.S02E02.MULTi.1080p.fr.srt']),
    { movieRoot: M, tvRoot: T, existingShowDirs: ['Dark (2017)', 'Fallout (2024)'] }
  );
  assert.equal(plan.kind, 'tv');
  assert.deepEqual(dests(plan), [
    'TV/Fallout (2024)/Season 02/Fallout.S02E01.MULTi.1080p.mkv',
    'TV/Fallout (2024)/Season 02/Fallout.S02E02.MULTi.1080p.fr.srt',
    'TV/Fallout (2024)/Season 02/Fallout.S02E02.MULTi.1080p.mkv'
  ]);
  assert.deepEqual(plan.folders, [path.join(T, 'Fallout (2024)', 'Season 02')]);
});

test('a whole show with season folders, a lone season folder, and a single episode', () => {
  const show = planTransfer(
    { name: 'Planet Earth (2006)', isDir: true },
    files(['Season 1/01 From Pole to Pole.mkv', 'Season 2/01 Mountains.mkv', 'Specials/Making Of.mkv']),
    { movieRoot: M, tvRoot: T }
  );
  assert.equal(show.kind, 'tv');
  assert.deepEqual(dests(show), [
    'TV/Planet Earth (2006)/Season 01/01 From Pole to Pole.mkv',
    'TV/Planet Earth (2006)/Season 02/01 Mountains.mkv',
    'TV/Planet Earth (2006)/Specials/Making Of.mkv'
  ]);
  const season = planTransfer({ name: 'Season 3', isDir: true, parent: 'Dark (2017)' }, files(['Dark.S03E01.mkv']), { movieRoot: M, tvRoot: T });
  assert.deepEqual(dests(season), ['TV/Dark (2017)/Season 03/Dark.S03E01.mkv']);
  const ep = planTransfer({ name: 'breaking.bad.s01e02.720p.mkv', isDir: false }, files(['breaking.bad.s01e02.720p.mkv']), { movieRoot: M, tvRoot: T });
  assert.deepEqual(dests(ep), ['TV/Breaking Bad/Season 01/breaking.bad.s01e02.720p.mkv']);
});

test('the kind can be forced, and a missing library folder yields no plan', () => {
  const forced = planTransfer({ name: 'Home videos', isDir: true }, files(['Holiday.mkv']), { kind: 'tv', movieRoot: M, tvRoot: T });
  assert.deepEqual(dests(forced), ['TV/Home videos/Season 01/Holiday.mkv']);
  const none = planTransfer({ name: 'Dark.S01E01.mkv', isDir: false }, files(['Dark.S01E01.mkv']), { movieRoot: M });
  assert.equal(none.root, null);
  assert.equal(none.items.length, 0);
});

test('remote names can never escape the library or break Windows paths', () => {
  assert.equal(safeName('..'), '_');
  assert.equal(safeName('a/../../b'), 'a .. .. b');
  assert.equal(safeName('What? Why: "Now" '), 'What Why Now');
  assert.equal(safeName('CON'), '_CON');
  assert.equal(safeName('Title.'), 'Title');
  const plan = planTransfer({ name: 'x', isDir: true }, [{ remote: '/r/..\\..\\evil.mkv', rel: '..\\..\\evil.mkv', size: 1 }], { movieRoot: M, tvRoot: T });
  for (const i of plan.items) assert.ok(path.resolve(i.dest).startsWith(path.resolve(M) + path.sep));
});
