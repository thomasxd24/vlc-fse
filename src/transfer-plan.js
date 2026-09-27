'use strict';

// Where downloaded files go: decides whether a remote file or folder is a film or a show, and lays it out the
// way the library scanner reads best ("Movies/Title (Year)/…", "TV/Show/Season 02/…"). Pure, so it's tested.

const path = require('path');
const { isVideoFile, stripExtension, parseMovieName, parseEpisodeName, parseSeasonFolder, parseShowFolder, normalizeKey } = require('./parse');

const SUB_EXTENSIONS = new Set(['.srt', '.ass', '.ssa', '.sub', '.idx', '.vtt', '.sup']);
const SAMPLE = /(^|[\s._\-])(sample|trailer)([\s._\-]|$)/i;
const IGNORED_DIR = /^(sample|samples|extras?|featurettes?|trailers?)$/i;

const ext = (name) => path.posix.extname(name).toLowerCase();
const isSub = (name) => SUB_EXTENSIONS.has(ext(name));

/** Make a name safe as a Windows file or folder name (and never a path). */
function safeName(name) {
  const s = String(name)
    .replace(/[<>:"/\\|?*\x00-\x1f]/g, ' ')
    .replace(/\s{2,}/g, ' ')
    .trim()
    .replace(/[. ]+$/, '');
  if (!s || /^\.+$/.test(s)) return '_';
  return /^(con|prn|aux|nul|com\d|lpt\d)(\.|$)/i.test(s) ? `_${s}` : s;
}

const pad2 = (n) => String(n).padStart(2, '0');

/** Keep the video files and subtitles worth copying; drop samples, trailers and extras. */
function relevant(files) {
  return files.filter((f) => {
    const parts = f.rel.split('/');
    if (parts.slice(0, -1).some((d) => IGNORED_DIR.test(d))) return false;
    const name = parts[parts.length - 1];
    if (isVideoFile(name)) return !SAMPLE.test(stripExtension(name));
    return isSub(name);
  });
}

/** Film or show? Episode markers in the files, or a season in the folder name, mean a show. */
function detectKind(selectionName, files) {
  const videos = files.filter((f) => isVideoFile(f.rel));
  if (videos.some((f) => parseEpisodeName(path.posix.basename(f.rel)))) return 'tv';
  if (parseShowFolder(selectionName).season !== null) return 'tv';
  if (files.some((f) => f.rel.split('/').slice(0, -1).some((d) => parseSeasonFolder(d) !== null))) return 'tv';
  return 'movie';
}

/** The video a subtitle belongs to: the one whose name it starts with ("Film.mkv" ← "Film.en.srt"), if any. */
function ownerOf(sub, videos) {
  const base = stripExtension(path.posix.basename(sub.rel)).toLowerCase();
  const dir = path.posix.dirname(sub.rel);
  let best = null;
  for (const v of videos) {
    const vb = stripExtension(path.posix.basename(v.rel)).toLowerCase();
    if (base.startsWith(vb) && (!best || vb.length > best.len)) best = { v, len: vb.length };
  }
  if (best) return best.v;
  // "Subs/English.srt" next to a single film belongs to that film.
  const near = videos.filter((v) => dir === path.posix.dirname(v.rel) || dir.startsWith(`${path.posix.dirname(v.rel)}/`) || path.posix.dirname(v.rel) === '.');
  return near.length === 1 ? near[0] : videos.length === 1 ? videos[0] : null;
}

function planMovies(selection, files, root) {
  const videos = files.filter((f) => isVideoFile(f.rel));
  const subs = files.filter((f) => isSub(f.rel));
  const folderInfo = selection.isDir ? parseMovieName(selection.name) : null;
  const items = [];
  const folderFor = new Map();
  for (const v of videos) {
    const fromFile = parseMovieName(path.posix.basename(v.rel));
    // A folder holding one film usually has the cleaner name ("Heat (1995)"), unless the file has the year.
    const info = videos.length === 1 && folderInfo && (folderInfo.year || !fromFile.year) ? folderInfo : fromFile;
    const folder = safeName(info.year ? `${info.title} (${info.year})` : info.title);
    folderFor.set(v, folder);
    items.push({ ...v, dest: path.join(root, folder, safeName(path.posix.basename(v.rel))) });
  }
  for (const s of subs) {
    const owner = ownerOf(s, videos);
    if (owner) items.push({ ...s, dest: path.join(root, folderFor.get(owner), safeName(path.posix.basename(s.rel))) });
  }
  return items;
}

/** Name of the show's folder: an existing one with the same title if there is one, so episodes merge in. */
function showFolder(title, year, existingDirs) {
  const key = normalizeKey(title);
  const hit = existingDirs.find((d) => normalizeKey(parseShowFolder(d).title) === key);
  if (hit) return hit;
  return safeName(year ? `${title} (${year})` : title);
}

function planShows(selection, files, root, existingDirs) {
  const selInfo = parseShowFolder(selection.isDir ? selection.name : stripExtension(selection.name));
  const videos = files.filter((f) => isVideoFile(f.rel));
  const subs = files.filter((f) => isSub(f.rel));
  const items = [];
  const placeOf = new Map();

  const place = (f) => {
    const name = path.posix.basename(f.rel);
    const dirs = f.rel.split('/').slice(0, -1);
    const ep = parseEpisodeName(name);
    // Show name: from a folder that names it (the selected one, or the first below it), else from the file.
    let title = null;
    let year = null;
    if (selection.isDir && selInfo.title && parseSeasonFolder(selection.name) === null) {
      title = selInfo.title;
      year = selInfo.year;
    } else if (dirs.length && parseSeasonFolder(dirs[0]) === null) {
      const d = parseShowFolder(dirs[0]);
      title = d.title;
      year = d.year;
    }
    if (!title && selection.isDir && selection.parent && parseSeasonFolder(selection.name) !== null) {
      // A "Season 2" folder picked on its own: the show is the folder above it.
      const p = parseShowFolder(selection.parent);
      title = p.title;
      year = p.year;
    }
    if (!title) title = (ep && ep.show) || parseMovieName(name).title;
    // Season: from the file, a "Season N" folder, or a season-pack folder name; specials land in season 0.
    let season = ep ? ep.season : null;
    for (let i = dirs.length - 1; season === null && i >= 0; i--) {
      season = parseSeasonFolder(dirs[i]);
      if (season === null) season = parseShowFolder(dirs[i]).season;
    }
    if (season === null && selection.isDir) season = parseSeasonFolder(selection.name) ?? selInfo.season;
    if (season === null) season = 1;
    const seasonDir = season === 0 ? 'Specials' : `Season ${pad2(season)}`;
    return path.join(root, showFolder(title, year, existingDirs), seasonDir);
  };

  for (const v of videos) {
    const dir = place(v);
    placeOf.set(v, dir);
    items.push({ ...v, dest: path.join(dir, safeName(path.posix.basename(v.rel))) });
  }
  for (const s of subs) {
    const own = parseEpisodeName(path.posix.basename(s.rel)) ? place(s) : placeOf.get(ownerOf(s, videos));
    if (own) items.push({ ...s, dest: path.join(own, safeName(path.posix.basename(s.rel))) });
  }
  return items;
}

/**
 * Plan a download.
 * @param {{name: string, isDir: boolean, parent?: string}} selection  the remote file or folder picked
 * @param {{remote: string, rel: string, size: number}[]} files  every file in it (`rel` is relative, '/'-separated)
 * @param {{kind?: 'movie'|'tv', movieRoot?: string, tvRoot?: string, existingShowDirs?: string[]}} opts
 * @returns {{kind: 'movie'|'tv', root: string|null, items: {remote: string, rel: string, size: number, dest: string}[], totalSize: number, folders: string[]}}
 */
function planTransfer(selection, files, { kind, movieRoot, tvRoot, existingShowDirs = [] } = {}) {
  const keep = relevant(files);
  const k = kind || detectKind(selection.name, keep);
  const root = k === 'tv' ? tvRoot : movieRoot;
  if (!root) return { kind: k, root: null, items: [], totalSize: 0, folders: [] };
  const items = (k === 'tv' ? planShows(selection, keep, root, existingShowDirs) : planMovies(selection, keep, root)).sort((a, b) => a.dest.localeCompare(b.dest));
  return {
    kind: k,
    root,
    items,
    totalSize: items.reduce((n, i) => n + (i.size || 0), 0),
    folders: [...new Set(items.map((i) => path.dirname(i.dest)))]
  };
}

module.exports = { planTransfer, detectKind, safeName, SUB_EXTENSIONS };
