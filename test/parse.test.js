'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { parseMovieName, parseEpisodeName, parseSeasonFolder, parseShowFolder, isVideoFile } = require('../src/parse');

test('movie names', () => {
  const cases = [
    ['The.Matrix.1999.1080p.BluRay.x264-GROUP.mkv', 'The Matrix', 1999],
    ['Heat (1995).mkv', 'Heat', 1995],
    ['Blade Runner 2049 (2017) [2160p].mkv', 'Blade Runner 2049', 2017],
    ['2001.A.Space.Odyssey.1968.mkv', '2001 A Space Odyssey', 1968],
    ['1917 (2019).mp4', '1917', 2019],
    ['[YTS] Parasite 2019 720p.mp4', 'Parasite', 2019],
    ['amelie.mkv', 'Amelie', null],
    ['Some.Movie.1080p.WEB-DL.mkv', 'Some Movie', null],
    ['Spider-Man Into the Spider-Verse.mkv', 'Spider-Man Into the Spider-Verse', null]
  ];
  for (const [input, title, year] of cases) {
    assert.deepEqual(parseMovieName(input), { title, year }, input);
  }
});

test('episode names', () => {
  assert.deepEqual(parseEpisodeName('Breaking.Bad.S02E05.720p.HDTV.mkv'), {
    show: 'Breaking Bad', season: 2, episode: 5, episodeEnd: null, title: null
  });
  assert.deepEqual(parseEpisodeName('The Office (US) - s03e10 - A Benihana Christmas.mkv'), {
    show: 'The Office (US)', season: 3, episode: 10, episodeEnd: null, title: 'A Benihana Christmas'
  });
  assert.deepEqual(parseEpisodeName('Show.S01E01E02.mkv'), { show: 'Show', season: 1, episode: 1, episodeEnd: 2, title: null });
  assert.deepEqual(parseEpisodeName('Show - 4x07 - Title.avi'), { show: 'Show', season: 4, episode: 7, episodeEnd: null, title: 'Title' });
  assert.deepEqual(parseEpisodeName('S01E03.mkv'), { show: null, season: 1, episode: 3, episodeEnd: null, title: null });
  assert.equal(parseEpisodeName('The.Matrix.1999.mkv'), null);
  // 1080p must not be read as 10x80.
  assert.equal(parseEpisodeName('Movie.1080p.mkv'), null);
});

test('season and show folders', () => {
  assert.equal(parseSeasonFolder('Season 01'), 1);
  assert.equal(parseSeasonFolder('S2'), 2);
  assert.equal(parseSeasonFolder('Series 3'), 3);
  assert.equal(parseSeasonFolder('Specials'), 0);
  assert.equal(parseSeasonFolder('Extras'), null);
  assert.deepEqual(parseShowFolder('The Office (US) (2005)'), { title: 'The Office (US)', year: 2005, season: null });
  assert.deepEqual(parseShowFolder('Dark'), { title: 'Dark', year: null, season: null });
  // Season-pack release folders.
  assert.deepEqual(parseShowFolder('Fallout S02 MULTi VFF 1080p WEBrip 10 bits x265-Tyrell'), { title: 'Fallout', year: null, season: 2 });
  assert.deepEqual(parseShowFolder('Fallout.S01.MULTi.1080p.AMZN.WEB-DL'), { title: 'Fallout', year: null, season: 1 });
  assert.deepEqual(parseShowFolder('The.Last.of.Us.2023.S01.2160p.WEB'), { title: 'The Last of Us', year: 2023, season: 1 });
  assert.deepEqual(parseShowFolder('Shogun.2024.Saison.1.FRENCH.1080p'), { title: 'Shogun', year: 2024, season: 1 });
  assert.deepEqual(parseShowFolder('Dark.COMPLETE.1080p.NF.WEB-DL'), { title: 'Dark', year: null, season: null });
  assert.deepEqual(parseShowFolder('9-1-1 (2018)'), { title: '9-1-1', year: 2018, season: null });
});

test('video extensions', () => {
  assert.ok(isVideoFile('a.MKV'));
  assert.ok(!isVideoFile('a.srt'));
  assert.ok(!isVideoFile('mkv'));
});
