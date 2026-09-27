'use strict';

// The app was called Marquee, then Foyer; it's Lounge now. Electron keeps each name's data in its own
// %APPDATA% folder, so on the first launch under a new name, carry the newest earlier folder over: settings,
// library, progress, stats, servers, apps, artwork and the page's saved view preferences.

const fs = require('fs');
const path = require('path');

const LEGACY_NAMES = ['Foyer', 'Marquee']; // newest first
const COPY_DIRS = ['artwork', 'Local Storage'];

/**
 * Copy the newest legacy data folder into `userData`, unless it already has settings. Stores keep absolute
 * paths to cached artwork, so those are rewritten to point into the new folder.
 * @returns {string|null} the folder migrated from, or null
 */
function migrateUserData({ appData, userData, legacyNames = LEGACY_NAMES }) {
  if (fs.existsSync(path.join(userData, 'settings.json'))) return null;
  for (const name of legacyNames) {
    const old = path.join(appData, name);
    if (path.resolve(old) === path.resolve(userData) || !fs.existsSync(path.join(old, 'settings.json'))) continue;
    fs.mkdirSync(userData, { recursive: true });
    // JSON-escaped forms of the two folders, for rewriting paths inside the stores.
    const from = JSON.stringify(old).slice(1, -1);
    const to = JSON.stringify(userData).slice(1, -1);
    for (const f of fs.readdirSync(old)) {
      if (!f.endsWith('.json')) continue;
      const text = fs.readFileSync(path.join(old, f), 'utf8');
      fs.writeFileSync(path.join(userData, f), text.split(from).join(to));
    }
    for (const d of COPY_DIRS) {
      if (fs.existsSync(path.join(old, d))) fs.cpSync(path.join(old, d), path.join(userData, d), { recursive: true });
    }
    return old;
  }
  return null;
}

module.exports = { migrateUserData, LEGACY_NAMES };
