'use strict';

const fs = require('fs/promises');
const path = require('path');
const crypto = require('crypto');
const { normalizeKey } = require('./parse');

const API = 'https://api.themoviedb.org/3';
const IMG = 'https://image.tmdb.org/t/p';
const MISS_RETRY_MS = 7 * 24 * 3600 * 1000;

/**
 * Optional TMDB lookups. Results (including misses) are cached in `store` under "entries",
 * and images are downloaded once into `imageDir` so the UI works offline afterwards.
 */
class Metadata {
  constructor({ store, imageDir, apiKey, language }) {
    this.store = store;
    this.imageDir = imageDir;
    this.apiKey = (apiKey || '').trim();
    this.language = language || 'en-US';
  }

  get enabled() {
    return Boolean(this.apiKey);
  }

  entries() {
    let e = this.store.get('entries');
    if (!e) {
      e = {};
      this.store.set('entries', e);
    }
    return e;
  }

  static movieKey(m) {
    return `movie:${normalizeKey(m.title)}:${m.year || ''}`;
  }

  static showKey(s) {
    return `tv:${normalizeKey(s.title)}:${s.year || ''}`;
  }

  lookup(key) {
    const hit = this.entries()[key];
    return hit && !hit.miss ? hit : null;
  }

  needs(key) {
    const hit = this.entries()[key];
    if (!hit) return true;
    return Boolean(hit.stale) || (hit.miss && Date.now() - hit.at > MISS_RETRY_MS);
  }

  async api(pathname, params = {}) {
    const url = new URL(API + pathname);
    const headers = { Accept: 'application/json' };
    // v4 "read access tokens" are JWTs; v3 keys are 32 hex chars.
    if (this.apiKey.startsWith('eyJ')) headers.Authorization = `Bearer ${this.apiKey}`;
    else url.searchParams.set('api_key', this.apiKey);
    url.searchParams.set('language', this.language);
    for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== '') url.searchParams.set(k, v);
    const res = await fetch(url, { headers, signal: AbortSignal.timeout(10000) });
    if (res.status === 401) throw Object.assign(new Error('TMDB rejected the API key'), { fatal: true });
    if (!res.ok) throw new Error(`TMDB ${res.status}`);
    return res.json();
  }

  async image(filePath, size) {
    if (!filePath) return null;
    const name = crypto.createHash('sha1').update(size + filePath).digest('hex').slice(0, 20) + path.extname(filePath);
    const dest = path.join(this.imageDir, name);
    try {
      await fs.access(dest);
      return dest;
    } catch {}
    const res = await fetch(`${IMG}/${size}${filePath}`, { signal: AbortSignal.timeout(20000) });
    if (!res.ok) return null;
    await fs.mkdir(this.imageDir, { recursive: true });
    await fs.writeFile(dest, Buffer.from(await res.arrayBuffer()));
    return dest;
  }

  async fetchMovie(movie) {
    let r = await this.api('/search/movie', { query: movie.title, year: movie.year });
    if (!r.results?.length && movie.year) r = await this.api('/search/movie', { query: movie.title });
    const best = r.results?.[0];
    if (!best) return null;
    const d = await this.api(`/movie/${best.id}`);
    return {
      tmdbId: d.id,
      title: d.title,
      overview: d.overview || '',
      tagline: d.tagline || '',
      rating: d.vote_average ? Math.round(d.vote_average * 10) / 10 : null,
      runtime: d.runtime || null,
      genres: (d.genres || []).map((g) => g.name),
      releaseDate: d.release_date || null,
      poster: await this.image(d.poster_path, 'w500'),
      backdrop: await this.image(d.backdrop_path, 'w1280')
    };
  }

  async fetchShow(show) {
    let r = await this.api('/search/tv', { query: show.title, first_air_date_year: show.year });
    if (!r.results?.length && show.year) r = await this.api('/search/tv', { query: show.title });
    const best = r.results?.[0];
    if (!best) return null;
    const d = await this.api(`/tv/${best.id}`);
    const seasons = [...new Set(show.episodes.map((e) => e.season))];
    const episodes = {};
    for (const n of seasons) {
      try {
        const s = await this.api(`/tv/${d.id}/season/${n}`);
        for (const ep of s.episodes || []) {
          episodes[`${n}x${ep.episode_number}`] = {
            title: ep.name || null,
            overview: ep.overview || '',
            airDate: ep.air_date || null,
            runtime: ep.runtime || null,
            still: ep.still_path || null
          };
        }
      } catch (err) {
        if (err.fatal) throw err;
      }
    }
    // Only download stills for episodes we actually have.
    for (const e of show.episodes) {
      const info = episodes[`${e.season}x${e.episode}`];
      if (info && info.still && !e.thumb) info.still = await this.image(info.still, 'w300');
      else if (info) info.still = null;
    }
    return {
      tmdbId: d.id,
      title: d.name,
      overview: d.overview || '',
      rating: d.vote_average ? Math.round(d.vote_average * 10) / 10 : null,
      genres: (d.genres || []).map((g) => g.name),
      firstAirDate: d.first_air_date || null,
      status: d.status || null,
      poster: await this.image(d.poster_path, 'w500'),
      backdrop: await this.image(d.backdrop_path, 'w1280'),
      episodes
    };
  }

  /**
   * Fetch anything missing from the cache. Calls `onUpdate()` after each new result so the UI can refresh.
   * Throws only for a rejected API key.
   */
  async enrich(library, onUpdate) {
    if (!this.enabled) return;
    const jobs = [
      ...library.movies.map((m) => [Metadata.movieKey(m), () => this.fetchMovie(m)]),
      ...library.shows.map((s) => [Metadata.showKey(s), () => this.fetchShow(s)])
    ].filter(([key]) => this.needs(key));

    let i = 0;
    const worker = async () => {
      while (i < jobs.length) {
        const [key, run] = jobs[i++];
        try {
          const result = await run();
          this.entries()[key] = result ? { ...result, at: Date.now() } : { miss: true, at: Date.now() };
          this.store.save();
          onUpdate();
        } catch (err) {
          if (err.fatal) {
            i = jobs.length; // stop the other workers too
            throw err;
          }
          // Network hiccup: leave uncached so the next scan retries.
        }
      }
    };
    await Promise.all([worker(), worker(), worker()]);
  }
}

module.exports = { Metadata };
