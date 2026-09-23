'use strict';

const fs = require('fs/promises');
const path = require('path');
const crypto = require('crypto');
const {
  isVideoFile,
  stripExtension,
  parseMovieName,
  parseEpisodeName,
  parseSeasonFolder,
  parseShowFolder,
  normalizeKey
} = require('./parse');

const MAX_DEPTH = 8;
const IGNORED_DIRS = /^(\$recycle\.bin|system volume information|\..*|@eadir|#recycle|extras?|featurettes?|behind the scenes|deleted scenes|interviews|scenes|shorts|trailers?|samples?|subs|subtitles)$/i;
const SAMPLE_FILE = /(^|[\s._\-])(sample|trailer)([\s._\-]|$)/i;
const IMAGE_EXTS = ['.jpg', '.jpeg', '.png', '.webp'];
const POSTER_NAMES = ['poster', 'folder', 'cover', 'movie', 'show', 'default'];
const BACKDROP_NAMES = ['fanart', 'backdrop', 'background', 'art'];

function makeId(prefix, p) {
  return prefix + crypto.createHash('sha1').update(p.toLowerCase()).digest('hex').slice(0, 16);
}

async function readDirSafe(dir) {
  try {
    return await fs.readdir(dir, { withFileTypes: true });
  } catch {
    return [];
  }
}

async function statSafe(p) {
  try {
    return await fs.stat(p);
  } catch {
    return null;
  }
}

/** Recursively collect video files below `root`. */
async function walk(root, depth = 0, out = []) {
  if (depth > MAX_DEPTH) return out;
  const entries = await readDirSafe(root);
  for (const e of entries) {
    const full = path.join(root, e.name);
    if (e.isDirectory()) {
      if (IGNORED_DIRS.test(e.name)) continue;
      await walk(full, depth + 1, out);
    } else if (e.isFile() && isVideoFile(e.name) && !SAMPLE_FILE.test(stripExtension(e.name))) {
      out.push(full);
    }
  }
  return out;
}

/** Return the first file in `files` whose name (minus image extension) matches a candidate, in priority order. */
function findImage(files, candidates) {
  const lower = new Map(files.map((f) => [f.toLowerCase(), f]));
  for (const c of candidates) {
    for (const ext of IMAGE_EXTS) {
      const hit = lower.get((c + ext).toLowerCase());
      if (hit) return hit;
    }
  }
  return null;
}

const dirCache = new Map();
async function listFiles(dir) {
  if (!dirCache.has(dir)) {
    dirCache.set(dir, readDirSafe(dir).then((es) => es.filter((e) => e.isFile()).map((e) => e.name)));
  }
  return dirCache.get(dir);
}

/** Art for a folder dedicated to one title: "<file>-poster.jpg", "<file>.jpg", then generic "poster.jpg" etc. */
async function localArt(dir, prefix) {
  const files = await listFiles(dir);
  const prefixed = (names) => (prefix ? names.map((n) => `${prefix}-${n}`) : []);
  const poster = findImage(files, [...prefixed(POSTER_NAMES), ...(prefix ? [prefix] : []), ...POSTER_NAMES]);
  const backdrop = findImage(files, [...prefixed(BACKDROP_NAMES), ...BACKDROP_NAMES]);
  return {
    poster: poster ? path.join(dir, poster) : null,
    backdrop: backdrop ? path.join(dir, backdrop) : null
  };
}

async function scanMovies(root) {
  const files = await walk(root);
  // Count videos per directory so a folder holding a single film can lend it its (usually cleaner) name.
  const perDir = new Map();
  for (const f of files) {
    const d = path.dirname(f);
    perDir.set(d, (perDir.get(d) || 0) + 1);
  }

  const movies = [];
  for (const file of files) {
    const dir = path.dirname(file);
    const base = stripExtension(path.basename(file));
    const ownFolder = dir !== root && perDir.get(dir) === 1;
    const fromFile = parseMovieName(path.basename(file));
    const fromFolder = ownFolder ? parseMovieName(path.basename(dir)) : null;
    const parsed = fromFolder && fromFolder.year ? fromFolder : fromFile.year || !fromFolder ? fromFile : fromFolder;

    let art;
    if (ownFolder) {
      art = await localArt(dir, base);
    } else {
      // In a shared folder, generic names like poster.jpg belong to nobody; only accept art named after this file.
      const dirFiles = await listFiles(dir);
      const poster = findImage(dirFiles, [`${base}-poster`, `${base}-cover`, base]);
      const backdrop = findImage(dirFiles, BACKDROP_NAMES.map((n) => `${base}-${n}`));
      art = {
        poster: poster ? path.join(dir, poster) : null,
        backdrop: backdrop ? path.join(dir, backdrop) : null
      };
    }

    const st = await statSafe(file);
    movies.push({
      id: makeId('m', file),
      type: 'movie',
      title: parsed.title,
      year: parsed.year,
      path: file,
      dir,
      poster: art.poster,
      backdrop: art.backdrop,
      size: st ? st.size : 0,
      addedAt: st ? Math.round(st.mtimeMs) : 0
    });
  }
  return movies;
}

async function scanShows(root) {
  const files = await walk(root);
  const shows = new Map();

  for (const file of files) {
    const rel = path.relative(root, file);
    const parts = rel.split(path.sep);
    const fileName = parts[parts.length - 1];
    const ep = parseEpisodeName(fileName);

    let showDir;
    let showInfo;
    if (parts.length > 1) {
      showDir = path.join(root, parts[0]);
      showInfo = parseShowFolder(parts[0]);
    } else {
      // Loose files in the library root: group by the show name in the filename.
      showDir = root;
      showInfo = { title: (ep && ep.show) || parseMovieName(fileName).title, year: null };
    }
    const key = parts.length > 1 ? showDir.toLowerCase() : `${root.toLowerCase()}::${normalizeKey(showInfo.title)}`;

    let show = shows.get(key);
    if (!show) {
      show = {
        id: makeId('s', key),
        type: 'show',
        title: showInfo.title,
        year: showInfo.year,
        dir: showDir,
        poster: null,
        backdrop: null,
        addedAt: 0,
        episodes: []
      };
      if (parts.length > 1) Object.assign(show, await localArt(showDir));
      shows.set(key, show);
    }

    // Season: filename marker wins, then the nearest "Season N" folder, then 1.
    let season = ep ? ep.season : null;
    if (season === null) {
      for (let i = parts.length - 2; i >= 1 && season === null; i--) season = parseSeasonFolder(parts[i]);
    }
    if (season === null) season = 1;

    const st = await statSafe(file);
    const addedAt = st ? Math.round(st.mtimeMs) : 0;
    show.addedAt = Math.max(show.addedAt, addedAt);

    const epDir = path.dirname(file);
    const thumbFiles = await listFiles(epDir);
    const epBase = stripExtension(fileName);
    const thumb = findImage(thumbFiles, [`${epBase}-thumb`, epBase]);

    show.episodes.push({
      id: makeId('e', file),
      season,
      episode: ep ? ep.episode : null,
      episodeEnd: ep ? ep.episodeEnd : null,
      title: ep && ep.title ? ep.title : ep ? null : stripExtension(fileName).replace(/[._]+/g, ' '),
      path: file,
      thumb: thumb ? path.join(epDir, thumb) : null,
      size: st ? st.size : 0,
      addedAt
    });
  }

  for (const show of shows.values()) {
    show.episodes.sort(
      (a, b) =>
        a.season - b.season ||
        (a.episode ?? Infinity) - (b.episode ?? Infinity) ||
        a.path.localeCompare(b.path, undefined, { numeric: true })
    );
    // Number unmarked episodes by their position within the season.
    const counters = new Map();
    for (const e of show.episodes) {
      const n = (counters.get(e.season) || 0) + 1;
      counters.set(e.season, e.episode ?? n);
      if (e.episode === null) e.episode = n;
    }
  }
  return [...shows.values()];
}

/**
 * Scan every configured library folder.
 * @param {{path: string, type: 'movies'|'tv'}[]} libraries
 */
async function scanLibraries(libraries) {
  dirCache.clear();
  const movies = [];
  const shows = [];
  for (const lib of libraries) {
    const st = await statSafe(lib.path);
    if (!st || !st.isDirectory()) continue;
    if (lib.type === 'tv') shows.push(...(await scanShows(lib.path)));
    else movies.push(...(await scanMovies(lib.path)));
  }
  dirCache.clear();
  const byTitle = (a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: 'base', numeric: true });
  movies.sort(byTitle);
  shows.sort(byTitle);
  return { movies, shows, scannedAt: Date.now() };
}

module.exports = { scanLibraries, scanMovies, scanShows };
