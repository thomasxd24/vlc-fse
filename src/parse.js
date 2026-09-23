'use strict';

// Pure filename/folder parsing helpers. Kept free of fs/electron so they can be unit tested.

const VIDEO_EXTENSIONS = new Set([
  '.mkv', '.mp4', '.m4v', '.avi', '.mov', '.wmv', '.mpg', '.mpeg', '.ts', '.m2ts',
  '.webm', '.flv', '.vob', '.ogv', '.3gp', '.divx', '.iso'
]);

// Release-name noise that marks the end of a title.
const JUNK_TOKENS = [
  '2160p', '1080p', '1080i', '720p', '576p', '480p', '4k', 'uhd', 'hdr', 'hdr10', 'dv', 'dolby',
  'bluray', 'blu-ray', 'bdrip', 'brrip', 'bdremux', 'remux', 'webrip', 'web-dl', 'webdl', 'web',
  'hdtv', 'dvdrip', 'dvdscr', 'dvd', 'hdrip', 'x264', 'x265', 'h264', 'h265', 'hevc', 'avc', 'xvid',
  'divx', 'aac', 'ac3', 'dts', 'ddp5', 'dd5', 'atmos', 'truehd', 'flac', '10bit', '8bit',
  'proper', 'repack', 'extended', 'unrated', 'remastered', 'directors', 'imax', 'multi',
  'subbed', 'dubbed', 'internal', 'limited', 'vff', 'vfq', 'vfi', 'vf2', 'vostfr', 'truefrench', 'french',
  'complete', 'integrale', 'intégrale'
];
const JUNK_RE = new RegExp(`(^|[\\s._\\-\\[(])(${JUNK_TOKENS.map(escapeRe).join('|')})(?=$|[\\s._\\-\\])])`, 'i');

const EPISODE_PATTERNS = [
  // S01E02, S01E02E03, S01E02-E03, s1e2
  /(?:^|[\s._\-\[(])s(\d{1,2})[\s._\-]?e(\d{1,3})(?:-?e(\d{1,3})|-(\d{1,3})(?=$|[\s._\-\])]))?/i,
  // 1x02
  /(?:^|[\s._\-\[(])(\d{1,2})x(\d{2,3})(?=$|[\s._\-\])])/i,
  // Season 1 Episode 2
  /season[\s._\-]*(\d{1,2})[\s._\-]*episode[\s._\-]*(\d{1,3})/i
];

function escapeRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function isVideoFile(name) {
  const dot = name.lastIndexOf('.');
  if (dot < 0) return false;
  return VIDEO_EXTENSIONS.has(name.slice(dot).toLowerCase());
}

function stripExtension(name) {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(0, dot) : name;
}

function tidy(s) {
  return s
    .replace(/[._]+/g, ' ')
    .replace(/\s*[\[(]\s*[\])]\s*/g, ' ')
    .replace(/[\s\-\[(]+$/, '')
    .replace(/^[\s\-]+|[\s\-]+$/g, '')
    .replace(/\s{2,}/g, ' ')
    .trim();
}

function titleCase(s) {
  // Only fix titles that are entirely lower case (common for scene releases).
  if (s !== s.toLowerCase()) return s;
  return s.replace(/\b([a-z])/g, (m) => m.toUpperCase());
}

/**
 * Parse a movie file or folder name.
 * "The.Matrix.1999.1080p.BluRay.x264" -> { title: "The Matrix", year: 1999 }
 * "Heat (1995)" -> { title: "Heat", year: 1995 }
 */
function parseMovieName(raw) {
  let name = stripExtension(raw);
  // Drop bracketed release-group tags at the start, e.g. "[YTS] Movie".
  name = name.replace(/^\s*\[[^\]]*\]\s*/, '');

  let year = null;
  let title = name;

  // Pick the last plausible year that is not the very first token (so "2001 A Space Odyssey 1968" works).
  const yearRe = /(?:^|[\s._\-\[(])((?:19|20)\d{2})(?=$|[\s._\-\])])/g;
  let match;
  let chosen = null;
  while ((match = yearRe.exec(name)) !== null) {
    const start = match.index + match[0].indexOf(match[1]);
    if (start === 0) continue;
    chosen = { year: Number(match[1]), start };
  }
  if (chosen) {
    year = chosen.year;
    title = name.slice(0, chosen.start);
  } else {
    const junk = JUNK_RE.exec(name);
    if (junk && junk.index > 0) title = name.slice(0, junk.index);
  }

  title = titleCase(tidy(title)) || tidy(name);
  return { title, year };
}

/**
 * Parse an episode filename. Returns null if no season/episode marker is found.
 * "Breaking.Bad.S02E05.720p.mkv" -> { show: "Breaking Bad", season: 2, episode: 5, title: null }
 */
function parseEpisodeName(raw) {
  const name = stripExtension(raw);
  for (const re of EPISODE_PATTERNS) {
    const m = re.exec(name);
    if (!m) continue;
    const season = Number(m[1]);
    const episode = Number(m[2]);
    const episodeEnd = m[3] || m[4] ? Number(m[3] || m[4]) : null;
    const before = name.slice(0, m.index);
    let after = name.slice(m.index + m[0].length);
    const junk = JUNK_RE.exec(after);
    if (junk) after = after.slice(0, junk.index);
    const show = titleCase(tidy(before.replace(/^\s*\[[^\]]*\]\s*/, ''))) || null;
    const epTitle = tidy(after.replace(/^[\s._\-]+/, '')) || null;
    return {
      show,
      season,
      episode,
      episodeEnd: episodeEnd && episodeEnd > episode ? episodeEnd : null,
      title: epTitle
    };
  }
  return null;
}

/** "Season 01", "S2", "Series 3", "Specials" -> season number (0 for specials) or null. */
function parseSeasonFolder(raw) {
  if (/^specials?$/i.test(raw.trim())) return 0;
  const m = /^(?:season|series|saison|staffel|temporada|s)[\s._\-]*(\d{1,3})\b/i.exec(raw.trim());
  return m ? Number(m[1]) : null;
}

/** Parse a TV show folder name, e.g. "The Office (US) (2005)". */
// "S02", "Season 2", "Saison 2" etc. inside a release-style folder name, e.g. "Fallout.S02.1080p.WEBrip-GRP".
const SEASON_IN_NAME = /(?:^|[\s._\-\[(])(?:s(\d{1,2})(?:e\d{1,3})?|(?:season|saison|series|staffel|temporada)[\s._\-]*(\d{1,2}))(?=$|[\s._\-\])])/i;

/**
 * Parse a TV show folder name: "The Office (US) (2005)", "Dark", or a season-pack release name like
 * "Fallout S02 MULTi VFF 1080p WEBrip x265-GRP" (-> title "Fallout", season 2).
 */
function parseShowFolder(raw) {
  let name = raw.replace(/^\s*\[[^\]]*\]\s*/, '');
  let season = null;
  const sm = SEASON_IN_NAME.exec(name);
  if (sm && sm.index > 0) {
    season = Number(sm[1] ?? sm[2]);
    name = name.slice(0, sm.index);
  }
  const junk = JUNK_RE.exec(name);
  if (junk && junk.index > 0) name = name.slice(0, junk.index);

  let year = null;
  const m = /^(.*?)[\s._]*[(\[]?((?:19|20)\d{2})[)\]]?\s*$/.exec(name);
  if (m && tidy(m[1])) {
    year = Number(m[2]);
    name = m[1];
  }
  return { title: titleCase(tidy(name)) || tidy(raw), year, season };
}

/** Case/punctuation-insensitive key used to group episodes into shows. */
function normalizeKey(s) {
  return String(s || '')
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .replace(/&/g, 'and')
    .replace(/[^a-z0-9]+/g, ' ')
    .trim();
}

module.exports = {
  VIDEO_EXTENSIONS,
  isVideoFile,
  stripExtension,
  parseMovieName,
  parseEpisodeName,
  parseSeasonFolder,
  parseShowFolder,
  normalizeKey
};
