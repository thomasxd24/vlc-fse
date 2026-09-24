'use strict';

const fs = require('fs/promises');
const path = require('path');
const crypto = require('crypto');
const { normalizeKey } = require('./parse');

const STORE = 'https://store.steampowered.com';
const CDNS = ['https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps', 'https://cdn.cloudflare.steamstatic.com/steam/apps'];
const SGDB = 'https://www.steamgriddb.com/api/v2';
const STORE_SPACING_MS = 1600; // the store API allows ~200 requests per 5 minutes
const RETRY_MS = 7 * 24 * 3600 * 1000;

const CDN_FILES = {
  poster: 'library_600x900_2x.jpg',
  posterSmall: 'library_600x900.jpg',
  hero: 'library_hero.jpg',
  logo: 'logo.png',
  header: 'header.jpg'
};

function stripHtml(s) {
  return String(s || '')
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<[^>]+>/g, '')
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, '&')
    .replace(/&#39;|&apos;/g, "'")
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&nbsp;/g, ' ')
    .trim();
}

/**
 * Game details and artwork. Steam store data needs no key; SteamGridDB (covers, backgrounds and logos
 * for anything, including non-Steam games) needs a free API key. Everything is cached in `store` and
 * images are saved under `imageDir`.
 */
class GameInfo {
  constructor({ store, imageDir, sgdbKey, language }) {
    this.store = store;
    this.imageDir = imageDir;
    this.sgdbKey = (sgdbKey || '').trim();
    this.lang = language === 'fr' ? 'french' : 'english';
    this.lastStoreCall = 0;
    this.running = false;
  }

  entries() {
    if (!this.store.get('games')) this.store.set('games', {});
    return this.store.get('games');
  }

  lookup(id) {
    const e = this.entries()[id];
    return e && !e.miss ? e : null;
  }

  // ------------------------------------------------------------------ HTTP

  async storeApi(pathname, params) {
    const wait = this.lastStoreCall + STORE_SPACING_MS - Date.now();
    if (wait > 0) await new Promise((r) => setTimeout(r, wait));
    this.lastStoreCall = Date.now();
    const url = new URL(STORE + pathname);
    for (const [k, v] of Object.entries(params)) url.searchParams.set(k, v);
    const res = await fetch(url, { signal: AbortSignal.timeout(12000) });
    if (res.status === 429) throw Object.assign(new Error('Steam store rate limit'), { retry: true });
    if (!res.ok) throw new Error(`Steam store ${res.status}`);
    return res.json();
  }

  async sgdb(pathname) {
    if (!this.sgdbKey) return null;
    const res = await fetch(SGDB + pathname, { headers: { Authorization: `Bearer ${this.sgdbKey}` }, signal: AbortSignal.timeout(12000) });
    if (res.status === 401 || res.status === 403) throw Object.assign(new Error('SteamGridDB rejected the API key'), { fatal: true });
    if (res.status === 404) return null;
    if (!res.ok) throw new Error(`SteamGridDB ${res.status}`);
    const body = await res.json();
    return body && body.success ? body.data : null;
  }

  /** Download `url` once into the image cache and return the local path (or null). */
  async download(url) {
    if (!url) return null;
    const ext = (path.extname(new URL(url).pathname) || '.jpg').slice(0, 5);
    const dest = path.join(this.imageDir, crypto.createHash('sha1').update(url).digest('hex').slice(0, 20) + ext);
    try {
      await fs.access(dest);
      return dest;
    } catch {}
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(20000) });
      if (!res.ok) return null;
      const buf = Buffer.from(await res.arrayBuffer());
      if (buf.length < 200) return null; // error placeholders
      await fs.mkdir(this.imageDir, { recursive: true });
      await fs.writeFile(dest, buf);
      return dest;
    } catch {
      return null;
    }
  }

  async cdn(appid, file) {
    for (const base of CDNS) {
      const p = await this.download(`${base}/${appid}/${file}`);
      if (p) return p;
    }
    return null;
  }

  // ------------------------------------------------------------------ Steam store

  async storeDetails(appid) {
    const r = await this.storeApi('/api/appdetails', { appids: appid, l: this.lang });
    const d = r && r[appid] && r[appid].success ? r[appid].data : null;
    if (!d) return null;
    const shots = (d.screenshots || []).slice(0, 8);
    return {
      steamAppId: String(appid),
      title: d.name,
      overview: stripHtml(d.short_description),
      about: stripHtml(d.about_the_game).slice(0, 3000),
      genres: (d.genres || []).map((g) => g.description),
      developers: d.developers || [],
      publishers: d.publishers || [],
      releaseDate: d.release_date && d.release_date.date ? d.release_date.date : null,
      metacritic: d.metacritic ? d.metacritic.score : null,
      controller: d.controller_support || null, // "full" | "partial"
      screenshots: await Promise.all(shots.map(async (s) => ({ thumb: await this.download(s.path_thumbnail), full: s.path_full }))).then((x) => x.filter((s) => s.thumb))
    };
  }

  /** Steam store search, e.g. to find a manually added game's Steam page. */
  async storeSearch(term) {
    const r = await this.storeApi('/api/storesearch/', { term, l: this.lang, cc: 'US' });
    return (r.items || []).filter((i) => i.type === 'app' || !i.type).map((i) => ({ steamAppId: String(i.id), title: i.name, thumb: i.tiny_image || null }));
  }

  // ------------------------------------------------------------------ SteamGridDB

  async sgdbSearch(term) {
    const data = await this.sgdb(`/search/autocomplete/${encodeURIComponent(term)}`);
    return (data || []).map((g) => ({
      sgdbId: g.id,
      title: g.name,
      year: g.release_date ? new Date(g.release_date * 1000).getFullYear() : null
    }));
  }

  /** Candidate images of one kind ('grids' | 'heroes' | 'logos') for a game, as {url, thumb}. */
  async sgdbImages(kind, { sgdbId, steamAppId }) {
    if (!this.sgdbKey) return [];
    const query = kind === 'grids' ? '?dimensions=600x900,342x482,660x930&types=static' : '?types=static';
    const target = sgdbId ? `game/${sgdbId}` : steamAppId ? `steam/${steamAppId}` : null;
    if (!target) return [];
    const data = await this.sgdb(`/${kind}/${target}${query}`);
    return (data || []).slice(0, 24).map((i) => ({ url: i.url, thumb: i.thumb || i.url }));
  }

  // ------------------------------------------------------------------ Enrichment

  needs(id) {
    const e = this.entries()[id];
    return Boolean(!e || e.stale || (e.miss && Date.now() - e.at > RETRY_MS));
  }

  /**
   * Fill in details and missing artwork for one game. `game` has {id, source, appid?, title, art, override}.
   * The override (from "Edit info") can pin a Steam app id or a SteamGridDB id.
   */
  async fetchGame(game) {
    const o = game.override || {};
    let steamAppId = o.steamAppId || (game.source === 'steam' ? game.appid : null);

    if (!steamAppId && game.source !== 'steam' && !o.noSteamMatch) {
      // Manual game: take the store's top hit only if its name really matches.
      const hits = await this.storeSearch(game.title).catch(() => []);
      const key = normalizeKey(game.title);
      const hit = hits.find((h) => normalizeKey(h.title) === key) || hits.find((h) => normalizeKey(h.title).startsWith(key) && key.length > 4);
      if (hit) steamAppId = hit.steamAppId;
    }

    const info = steamAppId ? await this.storeDetails(steamAppId).catch((e) => (e.retry ? Promise.reject(e) : null)) : null;
    const out = { ...(info || {}), steamAppId: steamAppId || null, art: {} };
    const have = game.art || {};

    // Artwork the game doesn't have locally: SteamGridDB first (if set up), then Steam's CDN.
    let sgdbId = o.sgdbId || null;
    if (this.sgdbKey && !sgdbId && !steamAppId) {
      const hits = await this.sgdbSearch(game.title).catch((e) => (e.fatal ? Promise.reject(e) : []));
      if (hits[0]) sgdbId = hits[0].sgdbId;
    }
    const kinds = [
      ['poster', 'grids', CDN_FILES.poster],
      ['hero', 'heroes', CDN_FILES.hero],
      ['logo', 'logos', CDN_FILES.logo]
    ];
    for (const [key, sgdbKind, cdnFile] of kinds) {
      if (have[key]) continue;
      let p = null;
      if (this.sgdbKey && (sgdbId || steamAppId)) {
        const imgs = await this.sgdbImages(sgdbKind, { sgdbId, steamAppId }).catch((e) => (e.fatal ? Promise.reject(e) : []));
        if (imgs[0]) p = await this.download(imgs[0].url);
      }
      if (!p && steamAppId) p = (await this.cdn(steamAppId, cdnFile)) || (key === 'poster' ? await this.cdn(steamAppId, CDN_FILES.posterSmall) : null);
      if (p) out.art[key] = p;
    }
    if (!have.header && steamAppId) {
      const p = await this.cdn(steamAppId, CDN_FILES.header);
      if (p) out.art.header = p;
    }
    out.sgdbId = sgdbId;
    return out;
  }

  /** Enrich every game that isn't cached yet, one at a time. `shouldPause()` lets playback stop the work. */
  async enrich(games, onUpdate, shouldPause = () => false) {
    if (this.running) return;
    this.running = true;
    try {
      for (const g of games) {
        while (shouldPause()) await new Promise((r) => setTimeout(r, 5000));
        if (!this.needs(g.id)) continue;
        try {
          const r = await this.fetchGame(g);
          const prev = this.entries()[g.id];
          if (r && (r.title || r.overview || Object.keys(r.art || {}).length)) this.entries()[g.id] = { ...r, at: Date.now() };
          else if (prev && !prev.miss) this.entries()[g.id] = { ...prev, stale: false, at: Date.now() }; // keep what we had
          else this.entries()[g.id] = { miss: true, at: Date.now() };
          this.store.save();
          onUpdate();
        } catch (err) {
          if (err.fatal) throw err;
          if (err.retry) await new Promise((r) => setTimeout(r, 60000));
        }
      }
    } finally {
      this.running = false;
    }
  }

  /** Forget cached info for one game (after "Edit info") so the next enrich refetches it. */
  forget(id) {
    delete this.entries()[id];
    this.store.save();
  }
}

module.exports = { GameInfo, stripHtml };
