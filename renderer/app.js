'use strict';

/* global Nav */

const api = window.cinema;

// ============================================================================ State

const S = {
  settings: {},
  library: { movies: [], shows: [], continueWatching: [] },
  scanning: false,
  metaStatus: {},
  nowPlaying: null,
  platform: 'win32',
  version: ''
};
const idx = { movies: new Map(), shows: new Map(), episodes: new Map() };

const prefs = loadPrefs();
function loadPrefs() {
  try {
    return { movieSort: 'title', movieFilter: 'all', showSort: 'title', showFilter: 'all', ...JSON.parse(localStorage.getItem('prefs') || '{}') };
  } catch {
    return { movieSort: 'title', movieFilter: 'all', showSort: 'title', showFilter: 'all' };
  }
}
function savePrefs() {
  try {
    localStorage.setItem('prefs', JSON.stringify(prefs));
  } catch {}
}

function applyState(next) {
  Object.assign(S, next);
  idx.movies = new Map(S.library.movies.map((m) => [m.id, m]));
  idx.shows = new Map(S.library.shows.map((s) => [s.id, s]));
  idx.episodes = new Map();
  for (const s of S.library.shows) for (const e of s.episodes) idx.episodes.set(e.id, { ...e, show: s });
  document.documentElement.style.setProperty('--scale', S.settings.uiScale || 1);
  renderStatus();
}

// ============================================================================ Helpers

const $ = (sel, root = document) => root.querySelector(sel);

function h(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
}

function hue(s) {
  let x = 0;
  for (const c of String(s)) x = (x * 31 + c.charCodeAt(0)) >>> 0;
  return x % 360;
}

function placeholder(title, text = title) {
  const h1 = hue(title);
  return `<div class="placeholder" style="background:linear-gradient(135deg,hsl(${h1} 42% 30%),hsl(${(h1 + 40) % 360} 48% 12%))">${h(text)}</div>`;
}

function img(src, title, text) {
  return src
    ? `<img src="${h(src)}" alt="" loading="lazy" decoding="async" data-title="${h(title)}" data-text="${h(text ?? title)}">`
    : placeholder(title, text);
}

// Broken artwork: swap in a generated placeholder (inline onerror is blocked by the CSP).
document.addEventListener(
  'error',
  (e) => {
    const el = e.target;
    if (el.tagName === 'IMG' && el.dataset.title !== undefined) el.outerHTML = placeholder(el.dataset.title, el.dataset.text);
  },
  true
);

function fmtTime(sec) {
  sec = Math.max(0, Math.floor(sec || 0));
  const hh = Math.floor(sec / 3600);
  const mm = Math.floor((sec % 3600) / 60);
  const ss = String(sec % 60).padStart(2, '0');
  return hh ? `${hh}:${String(mm).padStart(2, '0')}:${ss}` : `${mm}:${ss}`;
}

function fmtRuntime(min) {
  if (!min) return '';
  const hh = Math.floor(min / 60);
  const mm = min % 60;
  return hh ? `${hh}h ${mm}m` : `${mm}m`;
}

function remaining(pr) {
  if (!pr.length) return '';
  const left = Math.max(0, Math.round((pr.length - pr.time) / 60));
  return left ? `${left} min left` : 'Almost done';
}

function epCode(e) {
  const end = e.episodeEnd ? `–${e.episodeEnd}` : '';
  return e.season === 0 ? `Special ${e.episode}${end}` : `S${e.season} E${e.episode}${end}`;
}

function seasonName(n) {
  return n === 0 ? 'Specials' : `Season ${n}`;
}

function pct(pr) {
  return pr.length ? Math.min(100, (pr.time / pr.length) * 100) : 0;
}

function norm(s) {
  return String(s || '')
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .replace(/[^a-z0-9]+/g, ' ')
    .trim();
}

const ICON = {
  play: '<svg class="ico" viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.5v15a1 1 0 0 0 1.5.86l12.5-7.5a1 1 0 0 0 0-1.72L8.5 3.64A1 1 0 0 0 7 4.5z"/></svg>',
  restart: '<svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/></svg>',
  check: '<svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 12.5l5 5L20 6.5"/></svg>',
  folder: '<svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linejoin="round"><path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H10l2 2.5h7.5A1.5 1.5 0 0 1 21 9v9.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18.5z"/></svg>',
  stop: '<svg class="ico" viewBox="0 0 24 24" fill="currentColor"><rect x="5" y="5" width="14" height="14" rx="2"/></svg>',
  plus: '<svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>',
  refresh: '<svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.6-6.4"/><path d="M21 3v6h-6"/></svg>'
};

// ============================================================================ Backdrop

const Backdrop = (() => {
  const layers = [...document.querySelectorAll('#backdrop .layer')];
  let active = 0;
  let currentUrl = null;
  let timer = null;
  function set(url, { immediate = false } = {}) {
    clearTimeout(timer);
    const apply = () => {
      if (url === currentUrl) return;
      currentUrl = url;
      const nextLayer = layers[1 - active];
      const prevLayer = layers[active];
      if (!url) {
        prevLayer.classList.remove('on');
        return;
      }
      const probe = new Image();
      probe.onload = () => {
        if (currentUrl !== url) return;
        nextLayer.style.backgroundImage = `url("${url}")`;
        nextLayer.classList.add('on');
        prevLayer.classList.remove('on');
        active = 1 - active;
      };
      probe.src = url;
    };
    if (immediate) apply();
    else timer = setTimeout(apply, 220);
  }
  return { set };
})();

// ============================================================================ Router

const TABS = [
  { name: 'home', label: 'Home' },
  { name: 'movies', label: 'Movies' },
  { name: 'shows', label: 'TV Shows' },
  { name: 'search', label: 'Search' },
  { name: 'settings', label: 'Settings' }
];

const stack = [{ name: 'home' }];
const route = () => stack[stack.length - 1];
const page = $('#page');

function go(r) {
  saveView();
  stack.push(r);
  render();
}

function switchTab(name) {
  saveView();
  stack.length = 0;
  stack.push({ name });
  render({ focusTab: true });
}

function back() {
  if (closeModal()) return;
  if (S.nowPlaying) return;
  if (stack.length > 1) {
    stack.pop();
    render({ restore: true });
    return;
  }
  const inTabs = document.activeElement && document.activeElement.closest('#tabs');
  if (route().name !== 'home') {
    if (!inTabs) {
      // First Back from inside a tab jumps up to the tab bar, the second goes Home.
      Nav.focus($(`#tabs [data-tab="${route().name}"]`));
      return;
    }
    switchTab('home');
    return;
  }
  confirmExit();
}

/** Remember focus and scroll positions so Back returns to exactly the same place. */
function saveView() {
  const r = route();
  const a = document.activeElement;
  r.focusKey = a && a.dataset ? a.dataset.key : null;
  r.scroll = {};
  page.querySelectorAll('[data-scroll]').forEach((el) => {
    r.scroll[el.dataset.scroll] = [el.scrollLeft, el.scrollTop];
  });
}

function restoreView(r) {
  if (r.scroll) {
    page.querySelectorAll('[data-scroll]').forEach((el) => {
      const s = r.scroll[el.dataset.scroll];
      if (s) [el.scrollLeft, el.scrollTop] = s;
    });
  }
  const el = r.focusKey && page.querySelector(`[data-key="${CSS.escape(r.focusKey)}"]`);
  return el || null;
}

function renderTabs() {
  const root = route().name;
  const topLevel = stack.length === 1;
  $('#tabs').innerHTML = TABS.map(
    (t) => `<button class="tab focusable ${t.name === root && topLevel ? 'active' : ''}" ${t.name === root ? 'data-nav-default' : ''} data-tab="${t.name}" data-key="tab-${t.name}">${t.label}</button>`
  ).join('');
  document.body.classList.toggle('hide-topbar', !topLevel);
}

const VIEWS = {};

function render({ restore = false, focusTab = false, keepFocus = false } = {}) {
  const r = route();
  const prevFocusKey = keepFocus && document.activeElement ? document.activeElement.dataset.key : null;
  const prevInTabs = keepFocus && document.activeElement && document.activeElement.closest('#tabs');
  if (keepFocus) saveView();
  renderTabs();
  document.body.classList.remove('dim-backdrop');
  const view = VIEWS[r.name];
  page.innerHTML = view.render(r);
  view.mount && view.mount(r);

  let target = null;
  if (keepFocus && prevInTabs) target = $(`#tabs [data-key="${CSS.escape(prevFocusKey)}"]`);
  else if (restore || keepFocus) target = restoreView(r);
  if (!target && focusTab) target = $(`#tabs [data-tab="${r.name}"]`);
  if (!target) target = page.querySelector('[data-autofocus]') || page.querySelector('.focusable');
  if (!target) target = $(`#tabs [data-tab="${stack[0].name}"]`);
  Nav.focus(target, { scroll: !(restore || keepFocus) });
}

page.addEventListener('focusin', (e) => {
  const view = VIEWS[route().name];
  if (view.onFocus && e.target.closest) view.onFocus(route(), e.target);
});

// ============================================================================ Cards

function posterCard(item, keyPrefix) {
  const pr = item.progress;
  let badge = '';
  if (item.type === 'movie') {
    if (pr.watched) badge = '<div class="badge-watched">✓</div>';
  } else {
    const left = item.episodes.length - item.watchedCount;
    if (left === 0 && item.episodes.length) badge = '<div class="badge-watched">✓</div>';
    else if (item.watchedCount > 0) badge = `<div class="badge-count">${left}</div>`;
  }
  const bar = item.type === 'movie' && pr.resumable ? `<div class="progress"><i style="width:${pct(pr)}%"></i></div>` : '';
  const sub = item.type === 'movie' ? item.year || '' : `${item.seasons.length} season${item.seasons.length === 1 ? '' : 's'}`;
  return `
    <button class="card focusable" data-key="${keyPrefix}-${item.id}" data-hero="${item.type}:${item.id}"
      data-act="${item.type === 'movie' ? 'open-movie' : 'open-show'}" data-id="${item.id}">
      <div class="art">${img(item.poster, item.title)}${badge}${bar}</div>
      <div class="label">${h(item.title)}</div>
      <div class="sublabel">${h(sub)}</div>
    </button>`;
}

function continueCard(c) {
  if (c.kind === 'movie') {
    const m = idx.movies.get(c.id);
    if (!m) return '';
    return `
      <button class="card wide focusable" data-key="cw-${m.id}" data-hero="movie:${m.id}" data-act="play-movie" data-id="${m.id}">
        <div class="art">${img(m.backdrop || m.poster, m.title)}<div class="progress"><i style="width:${pct(m.progress)}%"></i></div></div>
        <div class="label">${h(m.title)}</div>
        <div class="sublabel">${h(remaining(m.progress) || 'Resume')}</div>
      </button>`;
  }
  const e = idx.episodes.get(c.id);
  if (!e) return '';
  const pr = e.progress;
  const bar = pr.resumable ? `<div class="progress"><i style="width:${pct(pr)}%"></i></div>` : '';
  return `
    <button class="card wide focusable" data-key="cw-${e.id}" data-hero="episode:${e.id}" data-act="play-episode" data-show="${e.show.id}" data-id="${e.id}">
      <div class="art">${img(e.thumb || e.show.backdrop || e.show.poster, e.show.title, e.show.title)}${bar}</div>
      <div class="label">${h(e.show.title)}</div>
      <div class="sublabel">${h(epCode(e))}${e.title ? ' · ' + h(e.title) : ''}${pr.resumable ? ' · ' + h(remaining(pr)) : ''}</div>
    </button>`;
}

function episodeCard(e, show) {
  const pr = e.progress;
  const bar = pr.resumable ? `<div class="progress"><i style="width:${pct(pr)}%"></i></div>` : '';
  const watched = pr.watched ? '<div class="badge-watched">✓</div>' : '';
  const art = e.thumb ? img(e.thumb, show.title, `E${e.episode}`) : placeholder(show.title, `E${e.episode}`);
  const meta = [e.runtime ? fmtRuntime(e.runtime) : '', e.airDate ? e.airDate.slice(0, 4) : ''].filter(Boolean).join(' · ');
  return `
    <button class="card wide focusable ${pr.watched ? 'watched' : ''}" data-key="ep-${e.id}" data-act="play-episode" data-show="${show.id}" data-id="${e.id}" data-watch="episode">
      <div class="art">${art}${watched}${bar}</div>
      <div class="label"><span class="ep-num">${e.episode}${e.episodeEnd ? '–' + e.episodeEnd : ''}</span>${h(e.title || `Episode ${e.episode}`)}</div>
      <div class="sublabel">${h(meta || (pr.resumable ? remaining(pr) : ''))}</div>
      <div class="ep-overview">${h(e.overview)}</div>
    </button>`;
}

function row(title, cards, key) {
  if (!cards.length) return '';
  return `
    <section class="row" data-nav-group>
      <h2>${h(title)}</h2>
      <div class="track" data-scroll="${key}">${cards.join('')}</div>
    </section>`;
}

// ============================================================================ Views

function metaLine(item) {
  const bits = [];
  if (item.year) bits.push(`<span>${item.year}</span>`);
  if (item.type === 'movie' && item.runtime) bits.push(`<span>${fmtRuntime(item.runtime)}</span>`);
  if (item.type === 'show') bits.push(`<span>${item.seasons.length} season${item.seasons.length === 1 ? '' : 's'} · ${item.episodes.length} episodes</span>`);
  if (item.rating) bits.push(`<span class="rating">${item.rating}</span>`);
  if (item.type === 'show' && item.status) bits.push(`<span class="pill">${h(item.status)}</span>`);
  if (item.type === 'movie' && item.progress.watched) bits.push('<span class="pill">Watched</span>');
  return `<div class="meta">${bits.join('')}</div>`;
}

VIEWS.welcome = {
  render() {
    return `
      <div class="page welcome">
        <h1>Welcome to Marquee</h1>
        <p>Your films and TV, big-screen ready. Point Marquee at the folders where your movies and shows live and it will build a library you can browse from the sofa. Everything plays in VLC.</p>
        <div class="check ${S.vlcFound ? 'ok' : ''}"><span class="dot"></span>${S.vlcFound ? `VLC found at ${h(S.vlcFound)}` : 'VLC not found yet. Install it from videolan.org or set its location in Settings.'}</div>
        <div class="actions">
          <button class="btn primary focusable" data-act="add-library" data-type="movies" data-key="w-movies" data-autofocus>${ICON.plus}Add movies folder</button>
          <button class="btn focusable" data-act="add-library" data-type="tv" data-key="w-tv">${ICON.plus}Add TV shows folder</button>
          <button class="btn focusable" data-act="tab" data-tab="settings" data-key="w-settings">Settings</button>
        </div>
      </div>`;
  }
};

VIEWS.home = {
  render(r) {
    if (!S.settings.libraries || !S.settings.libraries.length) {
      r.welcome = true;
      return VIEWS.welcome.render();
    }
    r.welcome = false;
    const L = S.library;
    const recentMovies = [...L.movies].sort((a, b) => b.addedAt - a.addedAt).slice(0, 24);
    const recentShows = [...L.shows].sort((a, b) => b.addedAt - a.addedAt).slice(0, 24);
    const unwatched = L.movies.filter((m) => !m.progress.watched && !m.progress.resumable);
    const rows = [
      row('Continue Watching', L.continueWatching.map(continueCard).filter(Boolean), 'cw'),
      row('Recently Added Movies', recentMovies.map((m) => posterCard(m, 'rm')), 'rm'),
      row('Recently Updated Shows', recentShows.map((s) => posterCard(s, 'rs')), 'rs'),
      row('Unwatched Movies', unwatched.slice(0, 40).map((m) => posterCard(m, 'um')), 'um'),
      row('All Shows', L.shows.map((s) => posterCard(s, 'as')), 'as')
    ].join('');
    const empty =
      !L.movies.length && !L.shows.length
        ? `<div class="page-head"><div class="empty-hint">${S.scanning ? 'Scanning your folders…' : 'No videos found in your library folders yet. Check the folders in Settings.'}</div></div>`
        : '';
    return `
      <div class="page home">
        <div class="hero" id="hero"></div>
        <div class="rows" data-scroll="home">${rows}${empty}</div>
      </div>`;
  },
  mount() {
    if ($('#hero')) heroFromFocus();
  },
  onFocus: () => heroFromFocus()
};

function heroFromFocus() {
  const hero = $('#hero');
  const a = document.activeElement;
  if (!hero || !a) return;
  const key = a.dataset && a.dataset.hero;
  if (!key) {
    if (!hero.innerHTML) {
      const first = page.querySelector('[data-hero]');
      if (first) paintHero(first.dataset.hero);
    }
    return;
  }
  paintHero(key);
}

function paintHero(key) {
  const hero = $('#hero');
  if (!hero || hero.dataset.item === key) return;
  hero.dataset.item = key;
  const [type, id] = key.split(':');
  let kicker = '';
  let title = '';
  let meta = '';
  let overview = '';
  let bg = null;
  if (type === 'movie') {
    const m = idx.movies.get(id);
    if (!m) return;
    kicker = m.progress.resumable ? `Resume · ${remaining(m.progress)}` : 'Movie';
    title = m.title;
    meta = metaLine(m);
    overview = m.overview;
    bg = m.backdrop || m.poster;
  } else if (type === 'show') {
    const s = idx.shows.get(id);
    if (!s) return;
    kicker = 'TV Show';
    title = s.title;
    meta = metaLine(s);
    overview = s.overview;
    bg = s.backdrop || s.poster;
  } else if (type === 'episode') {
    const e = idx.episodes.get(id);
    if (!e) return;
    kicker = `${e.show.title} · ${epCode(e)}`;
    title = e.title || e.show.title;
    meta = `<div class="meta">${e.runtime ? `<span>${fmtRuntime(e.runtime)}</span>` : ''}${e.progress.resumable ? `<span>${remaining(e.progress)}</span>` : '<span>Up next</span>'}</div>`;
    overview = e.overview || e.show.overview;
    bg = e.show.backdrop || e.thumb || e.show.poster;
  }
  hero.innerHTML = `
    <div class="kicker">${h(kicker)}</div>
    <h1>${h(title)}</h1>
    ${meta}
    ${overview ? `<p class="overview">${h(overview)}</p>` : ''}`;
  Backdrop.set(bg);
}

function sortItems(items, sort) {
  const list = [...items];
  const byTitle = (a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: 'base', numeric: true });
  if (sort === 'added') list.sort((a, b) => b.addedAt - a.addedAt);
  else if (sort === 'year') list.sort((a, b) => (b.year || 0) - (a.year || 0) || byTitle(a, b));
  else if (sort === 'rating') list.sort((a, b) => (b.rating || 0) - (a.rating || 0) || byTitle(a, b));
  else list.sort(byTitle);
  return list;
}

function chips(group, current, options) {
  return options
    .map(([value, label]) => `<button class="chip focusable ${value === current ? 'on' : ''}" data-act="pref" data-pref="${group}" data-value="${value}" data-key="chip-${group}-${value}">${label}</button>`)
    .join('');
}

const SORTS = [
  ['title', 'A–Z'],
  ['added', 'Recently added'],
  ['year', 'Year'],
  ['rating', 'Rating']
];

VIEWS.movies = {
  render() {
    let items = S.library.movies;
    if (prefs.movieFilter === 'unwatched') items = items.filter((m) => !m.progress.watched);
    if (prefs.movieFilter === 'progress') items = items.filter((m) => m.progress.resumable);
    items = sortItems(items, prefs.movieSort);
    return `
      <div class="page" data-scroll="movies">
        <div class="page-head"><h1 class="page-title">Movies</h1><div class="page-count">${items.length} of ${S.library.movies.length}</div></div>
        <div class="toolbar" data-nav-group>
          ${chips('movieSort', prefs.movieSort, SORTS)}
          <span class="chip-sep"></span>
          ${chips('movieFilter', prefs.movieFilter, [['all', 'All'], ['unwatched', 'Unwatched'], ['progress', 'In progress']])}
        </div>
        <div class="grid" data-nav-group>${items.map((m) => posterCard(m, 'mg')).join('') || '<div class="empty-hint">Nothing here.</div>'}</div>
      </div>`;
  },
  onFocus: () => ambientFromFocus()
};

VIEWS.shows = {
  render() {
    let items = S.library.shows;
    if (prefs.showFilter === 'unwatched') items = items.filter((s) => s.watchedCount < s.episodes.length);
    if (prefs.showFilter === 'progress') items = items.filter((s) => s.watchedCount > 0 && s.watchedCount < s.episodes.length);
    items = sortItems(items, prefs.showSort);
    return `
      <div class="page" data-scroll="shows">
        <div class="page-head"><h1 class="page-title">TV Shows</h1><div class="page-count">${items.length} of ${S.library.shows.length}</div></div>
        <div class="toolbar" data-nav-group>
          ${chips('showSort', prefs.showSort, SORTS)}
          <span class="chip-sep"></span>
          ${chips('showFilter', prefs.showFilter, [['all', 'All'], ['unwatched', 'Unwatched'], ['progress', 'Watching']])}
        </div>
        <div class="grid" data-nav-group>${items.map((s) => posterCard(s, 'sg')).join('') || '<div class="empty-hint">Nothing here.</div>'}</div>
      </div>`;
  },
  onFocus: () => ambientFromFocus()
};

function ambientFromFocus() {
  const a = document.activeElement;
  const key = a && a.dataset && a.dataset.hero;
  if (!key) return;
  const [type, id] = key.split(':');
  const item = type === 'movie' ? idx.movies.get(id) : idx.shows.get(id);
  document.body.classList.add('dim-backdrop');
  if (item) Backdrop.set(item.backdrop);
}

VIEWS.movie = {
  render(r) {
    const m = idx.movies.get(r.id);
    if (!m) return `<div class="page"><div class="page-head"><h1 class="page-title">Movie not found</h1></div></div>`;
    const pr = m.progress;
    const actions = pr.resumable
      ? `<button class="btn primary focusable" data-act="play-movie" data-id="${m.id}" data-mode="resume" data-key="m-resume" data-autofocus>${ICON.play}Resume from ${fmtTime(pr.time)}</button>
         <button class="btn focusable" data-act="play-movie" data-id="${m.id}" data-mode="start" data-key="m-start">${ICON.restart}Play from start</button>`
      : `<button class="btn primary focusable" data-act="play-movie" data-id="${m.id}" data-mode="start" data-key="m-play" data-autofocus>${ICON.play}Play</button>`;
    return `
      <div class="page detail" data-scroll="movie">
        <div class="detail-main">
          <div class="detail-poster">${img(m.poster, m.title)}</div>
          <div class="detail-info">
            <h1>${h(m.title)}</h1>
            ${metaLine(m)}
            ${m.tagline ? `<p class="tagline">${h(m.tagline)}</p>` : ''}
            ${m.overview ? `<p class="overview">${h(m.overview)}</p>` : ''}
            ${m.genres.length ? `<div class="genres">${h(m.genres.join(' · '))}</div>` : ''}
            ${pr.resumable ? `<div class="resume-bar"><div class="bar"><i style="width:${pct(pr)}%"></i></div>${fmtTime(pr.time)} of ${fmtTime(pr.length)}</div>` : ''}
            <div class="actions" data-nav-group>
              ${actions}
              <button class="btn focusable" data-act="toggle-watched" data-kind="movie" data-id="${m.id}" data-watched="${pr.watched ? 1 : 0}" data-key="m-watched">${ICON.check}${pr.watched ? 'Mark as unwatched' : 'Mark as watched'}</button>
              <button class="btn focusable" data-act="show-file" data-path="${h(m.path)}" data-key="m-file">${ICON.folder}Show file</button>
            </div>
          </div>
        </div>
        <div class="file-path">${h(m.path)}</div>
      </div>`;
  },
  mount(r) {
    const m = idx.movies.get(r.id);
    Backdrop.set(m ? m.backdrop || m.poster : null, { immediate: true });
  }
};

VIEWS.show = {
  render(r) {
    const s = idx.shows.get(r.id);
    if (!s) return `<div class="page"><div class="page-head"><h1 class="page-title">Show not found</h1></div></div>`;
    const next = s.nextUp ? idx.episodes.get(s.nextUp) : null;
    if (r.season === undefined || !s.seasons.includes(r.season)) {
      r.season = next ? next.season : s.seasons.find((n) => n > 0) ?? s.seasons[0];
    }
    const eps = s.episodes.filter((e) => e.season === r.season);
    const allWatched = s.watchedCount === s.episodes.length;
    let playBtn = '';
    if (next) {
      const label = next.progress.resumable ? `Resume ${epCode(next)}` : s.watchedCount ? `Play next · ${epCode(next)}` : `Play ${epCode(next)}`;
      playBtn = `<button class="btn primary focusable" data-act="play-episode" data-show="${s.id}" data-id="${next.id}" data-key="s-play" data-autofocus>${ICON.play}${h(label)}</button>`;
    } else if (s.episodes.length) {
      playBtn = `<button class="btn primary focusable" data-act="play-episode" data-show="${s.id}" data-id="${s.episodes[0].id}" data-mode="start" data-key="s-play" data-autofocus>${ICON.restart}Watch again from ${epCode(s.episodes[0])}</button>`;
    }
    return `
      <div class="page detail" data-scroll="show">
        <div class="detail-main" style="min-height:58vh">
          <div class="detail-poster">${img(s.poster, s.title)}</div>
          <div class="detail-info">
            <h1>${h(s.title)}</h1>
            ${metaLine(s)}
            ${s.overview ? `<p class="overview">${h(s.overview)}</p>` : ''}
            ${s.genres.length ? `<div class="genres">${h(s.genres.join(' · '))}</div>` : ''}
            <div class="actions" data-nav-group>
              ${playBtn}
              <button class="btn focusable" data-act="toggle-watched" data-kind="show" data-id="${s.id}" data-watched="${allWatched ? 1 : 0}" data-key="s-watched">${ICON.check}${allWatched ? 'Mark show unwatched' : 'Mark show watched'}</button>
            </div>
          </div>
        </div>
        <div class="seasons" data-nav-group data-scroll="seasons">
          ${s.seasons
            .map((n) => {
              const list = s.episodes.filter((e) => e.season === n);
              const done = list.every((e) => e.progress.watched);
              return `<button class="chip focusable ${n === r.season ? 'on' : ''}" ${n === r.season ? 'data-nav-default' : ''} data-act="season" data-season="${n}" data-show="${s.id}" data-watch="season" data-key="season-${n}">${seasonName(n)}${done ? ' ✓' : ''}</button>`;
            })
            .join('')}
        </div>
        <section class="row" data-nav-group>
          <div class="track episode-track" data-scroll="eps-${r.season}">${eps.map((e) => episodeCard(e, s)).join('')}</div>
        </section>
        <div class="file-path">Press <span class="kbd">W</span> or <span class="kbd">X</span> on a controller to mark the selected episode or season as watched.</div>
      </div>`;
  },
  mount(r) {
    const s = idx.shows.get(r.id);
    Backdrop.set(s ? s.backdrop || s.poster : null, { immediate: true });
  },
  // Moving along the season chips switches the episode row, like tabs.
  onFocus(r, target) {
    const chip = target.closest('[data-act="season"]');
    if (chip && Number(chip.dataset.season) !== r.season) selectSeason(r, Number(chip.dataset.season));
  }
};

let seasonTimer = null;
function selectSeason(r, n) {
  clearTimeout(seasonTimer);
  seasonTimer = setTimeout(() => {
    if (route() !== r) return;
    r.season = n;
    const key = document.activeElement && document.activeElement.dataset.key;
    render({ keepFocus: true });
    const el = key && page.querySelector(`[data-key="${CSS.escape(key)}"]`);
    if (el) Nav.focus(el, { scroll: false });
  }, 180);
}

const KEYS = 'abcdefghijklmnopqrstuvwxyz1234567890'.split('');

VIEWS.search = {
  render(r) {
    r.query = r.query || '';
    return `
      <div class="page search">
        <div class="search-side">
          <div class="page-head" style="padding-left:0"><h1 class="page-title">Search</h1></div>
          <input class="search-box focusable" id="q" data-key="q" value="${h(r.query)}" placeholder="Title…" spellcheck="false" autocomplete="off">
          <div class="keyboard" data-nav-group>
            ${KEYS.map((k) => `<button class="key focusable" data-act="key" data-k="${k}" data-key="k-${k}" ${k === 'a' ? 'data-autofocus' : ''}>${k}</button>`).join('')}
            <button class="key wide focusable" data-act="key" data-k=" " data-key="k-space">Space</button>
            <button class="key wide focusable" data-act="key" data-k="back" data-key="k-back">Delete</button>
            <button class="key wide focusable" data-act="key" data-k="clear" data-key="k-clear" style="grid-column:span 6">Clear</button>
          </div>
        </div>
        <div class="search-results" id="results" data-scroll="search"></div>
      </div>`;
  },
  mount(r) {
    Backdrop.set(null);
    const q = $('#q');
    q.addEventListener('input', () => {
      r.query = q.value;
      renderResults(r);
    });
    renderResults(r);
  }
};

function renderResults(r) {
  const q = norm(r.query);
  const box = $('#results');
  if (!box) return;
  if (!q) {
    box.innerHTML = `<div class="empty-hint" style="padding-top:9rem">Type with the on-screen keys or your keyboard.</div>`;
    return;
  }
  const words = q.split(' ');
  const match = (t) => {
    const n = norm(t);
    return words.every((w) => n.includes(w));
  };
  const movies = S.library.movies.filter((m) => match(m.title));
  const shows = S.library.shows.filter((s) => match(s.title));
  const rank = (a, b) => Number(norm(b.title).startsWith(q)) - Number(norm(a.title).startsWith(q)) || a.title.localeCompare(b.title);
  movies.sort(rank);
  shows.sort(rank);
  box.innerHTML = `
    <div class="page-head" style="padding-left:0"><div class="page-count">${movies.length + shows.length} results</div></div>
    ${shows.length ? `<h2>TV Shows</h2><div class="grid" data-nav-group>${shows.slice(0, 60).map((s) => posterCard(s, 'rs')).join('')}</div>` : ''}
    ${movies.length ? `<h2>Movies</h2><div class="grid" data-nav-group>${movies.slice(0, 120).map((m) => posterCard(m, 'rm')).join('')}</div>` : ''}
    ${!movies.length && !shows.length ? '<div class="empty-hint">No matches.</div>' : ''}`;
}

function searchKey(k) {
  const r = route();
  if (r.name !== 'search') return;
  if (k === 'back') r.query = r.query.slice(0, -1);
  else if (k === 'clear') r.query = '';
  else r.query += k;
  $('#q').value = r.query;
  renderResults(r);
}

function toggleRow(name, desc, on, act, key) {
  return `
    <button class="setting focusable" data-act="${act}" data-key="${key}">
      <div class="s-label"><div class="s-name">${h(name)}</div>${desc ? `<div class="s-desc">${h(desc)}</div>` : ''}</div>
      <div class="toggle ${on ? 'on' : ''}"></div>
    </button>`;
}

function valueRow(name, desc, value, act, key, extra = '') {
  return `
    <button class="setting focusable" data-act="${act}" data-key="${key}" ${extra}>
      <div class="s-label"><div class="s-name">${h(name)}</div>${desc ? `<div class="s-desc">${h(desc)}</div>` : ''}</div>
      <div class="s-value">${h(value)}</div>
    </button>`;
}

function inputRow(name, desc, value, setting, key, placeholderText = '') {
  return `
    <label class="setting">
      <div class="s-label"><div class="s-name">${h(name)}</div>${desc ? `<div class="s-desc">${h(desc)}</div>` : ''}</div>
      <input class="s-input focusable" data-setting="${setting}" data-key="${key}" value="${h(value)}" placeholder="${h(placeholderText)}" spellcheck="false" autocomplete="off">
    </label>`;
}

VIEWS.settings = {
  render() {
    const st = S.settings;
    const libs = st.libraries || [];
    const ms = S.metaStatus || {};
    const metaState = !st.tmdbKey ? 'Off' : ms.running ? 'Fetching…' : ms.error ? ms.error : 'On';
    return `
      <div class="page settings" data-scroll="settings">
        <div class="page-head" style="padding-left:0"><h1 class="page-title">Settings</h1></div>
        <div class="settings-grid" data-nav-group>
          <div class="section-title">Library</div>
          ${libs
            .map(
              (l, i) => `
            <button class="setting focusable" data-act="library-menu" data-index="${i}" data-key="lib-${i}">
              <span class="lib-type">${l.type === 'tv' ? 'TV' : 'Movies'}</span>
              <div class="s-label"><div class="s-name">${h(l.path)}</div></div>
            </button>`
            )
            .join('')}
          ${libs.length ? '' : '<div class="empty-hint" style="padding:0.4rem 0 0.8rem">No folders yet.</div>'}
          <div class="settings-actions" data-nav-group>
            <button class="btn small focusable" data-act="add-library" data-type="movies" data-key="add-movies">${ICON.plus}Add movies folder</button>
            <button class="btn small focusable" data-act="add-library" data-type="tv" data-key="add-tv">${ICON.plus}Add TV shows folder</button>
            <button class="btn small focusable" data-act="rescan" data-key="rescan">${ICON.refresh}${S.scanning ? 'Scanning…' : 'Rescan now'}</button>
          </div>
          <div class="s-desc" style="color:var(--faint);margin-top:.4rem">${S.library.movies.length} movies · ${S.library.shows.length} shows · ${S.library.shows.reduce((n, s) => n + s.episodes.length, 0)} episodes${S.library.scannedAt ? ' · scanned ' + new Date(S.library.scannedAt).toLocaleString() : ''}</div>

          <div class="section-title">Playback</div>
          ${valueRow('VLC location', 'Used to play everything. Select to detect automatically or browse.', st.vlcPath || (S.vlcFound ? `Auto: ${S.vlcFound}` : 'Not found'), 'vlc-menu', 'vlc')}
          ${toggleRow('Play in fullscreen', 'Start VLC in fullscreen mode.', st.vlcFullscreen, 'toggle-vlcFullscreen', 't-vlcfs')}
          ${toggleRow('Autoplay next episode', 'Queue the rest of the season after the episode you pick.', st.autoplayNext, 'toggle-autoplayNext', 't-autoplay')}
          ${inputRow('Extra VLC options', 'Passed to VLC as-is, e.g. --sub-language=eng --audio-language=eng', st.vlcExtraArgs || '', 'vlcExtraArgs', 'in-args', '--sub-language=eng')}

          <div class="section-title">Artwork &amp; info</div>
          ${inputRow('TMDB API key', 'Optional. Adds posters, backdrops, synopses and episode names from themoviedb.org.', st.tmdbKey || '', 'tmdbKey', 'in-tmdb', 'API key or read access token')}
          ${inputRow('Metadata language', 'Language for titles and synopses, e.g. en-US, fr-FR, de-DE.', st.language || 'en-US', 'language', 'in-lang', 'en-US')}
          ${valueRow('Refresh artwork & info', 'Forget downloaded metadata and fetch it again.', metaState, 'clear-metadata', 'meta-refresh')}

          <div class="section-title">Interface</div>
          ${toggleRow('Fullscreen', 'Run Marquee fullscreen. F11 toggles it any time.', st.startFullscreen, 'toggle-startFullscreen', 't-fs')}
          ${toggleRow('Start with Windows', 'Open Marquee when you sign in — ideal for a living-room PC.', st.launchAtLogin, 'toggle-launchAtLogin', 't-login')}
          ${valueRow('Text size', 'Make everything larger for viewing from across the room.', `${Math.round((st.uiScale || 1) * 100)}%`, 'cycle-scale', 'scale')}

          <div class="section-title">System</div>
          <div class="settings-actions" data-nav-group>
            <button class="btn small focusable" data-act="minimize" data-key="min">Minimize</button>
            <button class="btn small focusable" data-act="fullscreen" data-key="fs">Toggle fullscreen</button>
            <button class="btn small danger focusable" data-act="quit" data-key="quit">Exit Marquee</button>
          </div>
          <div class="about">
            Marquee ${h(S.version)} · Navigate with arrow keys, a remote or a game controller.
            <span class="kbd">Enter</span> select · <span class="kbd">Esc</span>/<span class="kbd">Backspace</span> back ·
            <span class="kbd">/</span> search · <span class="kbd">W</span> watched · <span class="kbd">F5</span> rescan · <span class="kbd">F11</span> fullscreen
            ${st.tmdbKey ? '<br>This product uses the TMDB API but is not endorsed or certified by TMDB.' : ''}
          </div>
        </div>
      </div>`;
  },
  mount() {
    Backdrop.set(null);
    page.querySelectorAll('input[data-setting]').forEach((input) => {
      input.addEventListener('change', () => {
        const key = input.dataset.setting;
        const value = input.value.trim();
        if ((S.settings[key] || '') === value) return;
        save({ [key]: value });
        toast('Saved');
      });
    });
  }
};

// ============================================================================ Modals & toasts

let modalState = null;

/**
 * Show a list of choices and resolve with the chosen value (or null on Back).
 * @param {{title: string, text?: string, choices: {label: string, value: any, primary?: boolean, danger?: boolean}[]}} opts
 */
function choose({ title, text, choices }) {
  closeModal();
  return new Promise((resolve) => {
    const root = $('#modal-root');
    const returnFocus = document.activeElement;
    root.innerHTML = `
      <div class="modal" data-nav-trap>
        <div class="dialog" role="dialog" aria-modal="true">
          <h2>${h(title)}</h2>
          ${text ? `<p>${h(text)}</p>` : ''}
          <div class="choices">
            ${choices.map((c, i) => `<button class="btn focusable ${c.primary ? 'primary' : ''} ${c.danger ? 'danger' : ''}" data-choice="${i}" ${i === 0 ? 'data-autofocus' : ''}>${c.icon || ''}${h(c.label)}</button>`).join('')}
          </div>
        </div>
      </div>`;
    const done = (v) => {
      root.innerHTML = '';
      modalState = null;
      if (returnFocus && returnFocus.isConnected) Nav.focus(returnFocus, { scroll: false });
      resolve(v);
    };
    modalState = { close: () => done(null) };
    root.querySelector('.modal').addEventListener('click', (e) => {
      const b = e.target.closest('[data-choice]');
      if (b) done(choices[Number(b.dataset.choice)].value);
      else if (e.target.classList.contains('modal')) done(null);
    });
    Nav.focusFirst();
  });
}

function closeModal() {
  if (!modalState) return false;
  modalState.close();
  return true;
}

function toast(text, kind = 'info') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.textContent = text;
  $('#toasts').appendChild(el);
  setTimeout(() => el.remove(), kind === 'error' ? 6000 : 2600);
}

async function confirmExit() {
  const v = await choose({
    title: 'Leave Marquee?',
    choices: [
      { label: 'Stay', value: null },
      { label: 'Minimize', value: 'min' },
      { label: 'Exit', value: 'quit', danger: true }
    ]
  });
  if (v === 'min') api.minimize();
  if (v === 'quit') api.quit();
}

// ============================================================================ Now playing

function renderNowPlaying() {
  const box = $('#now-playing');
  const np = S.nowPlaying;
  if (!np) {
    if (!box.hidden) {
      box.hidden = true;
      box.removeAttribute('data-nav-trap');
      box.innerHTML = '';
      Nav.focus(Nav.current() || page.querySelector('[data-autofocus]') || page.querySelector('.focusable'), { scroll: false });
    }
    return;
  }
  const file = np.current ? np.current.split(/[\\/]/).pop() : '';
  const p = np.length ? (np.time / np.length) * 100 : 0;
  if (box.hidden) {
    box.hidden = false;
    box.setAttribute('data-nav-trap', '');
    box.innerHTML = `
      <div class="np">
        <div class="kicker">Playing in VLC</div>
        <h1 id="np-title"></h1>
        <div class="np-file" id="np-file"></div>
        <div class="bar"><i id="np-bar"></i></div>
        <div class="time" id="np-time"></div>
        <div class="actions">
          <button class="btn danger focusable" data-np="stop" data-autofocus>${ICON.stop}Stop playback</button>
        </div>
      </div>`;
    box.querySelector('[data-np="stop"]').addEventListener('click', () => api.stop());
    Nav.focusFirst();
  }
  $('#np-title').textContent = np.title;
  $('#np-file').textContent = file;
  $('#np-bar').style.width = `${p}%`;
  $('#np-time').textContent = np.length ? `${fmtTime(np.time)} / ${fmtTime(np.length)}` : 'Starting…';
}

// ============================================================================ Actions

async function playMovie(id, mode) {
  const m = idx.movies.get(id);
  if (!m) return;
  let resume = mode === 'resume';
  if (!mode && m.progress.resumable) {
    const v = await choose({
      title: m.title,
      choices: [
        { label: `Resume from ${fmtTime(m.progress.time)}`, value: 'resume', primary: true, icon: ICON.play },
        { label: 'Play from start', value: 'start', icon: ICON.restart }
      ]
    });
    if (!v) return;
    resume = v === 'resume';
  }
  start({ kind: 'movie', id, resume });
}

async function playEpisode(showId, id, mode) {
  const e = idx.episodes.get(id);
  if (!e) return;
  let resume = mode !== 'start';
  if (!mode && e.progress.resumable) {
    const v = await choose({
      title: `${e.show.title} · ${epCode(e)}`,
      text: e.title || '',
      choices: [
        { label: `Resume from ${fmtTime(e.progress.time)}`, value: 'resume', primary: true, icon: ICON.play },
        { label: 'Play from start', value: 'start', icon: ICON.restart }
      ]
    });
    if (!v) return;
    resume = v === 'resume';
  }
  start({ kind: 'episode', showId, id, resume });
}

async function start(req) {
  const r = await api.play(req);
  if (!r.ok) toast(r.error, 'error');
}

async function save(patch) {
  S.settings = await api.saveSettings(patch);
  document.documentElement.style.setProperty('--scale', S.settings.uiScale || 1);
}

async function addLibrary(type) {
  const dir = await api.pickFolder();
  if (!dir) return;
  const libs = [...(S.settings.libraries || [])];
  if (libs.some((l) => l.path === dir)) return toast('That folder is already in your library.');
  libs.push({ path: dir, type });
  await save({ libraries: libs });
  toast(`Added ${dir}. Scanning…`);
  if (route().name === 'home') render();
  else render({ keepFocus: true });
}

async function toggleWatchedFocused() {
  const a = document.activeElement;
  if (!a || !a.dataset) return;
  const d = a.dataset;
  if (d.watch === 'episode') {
    const e = idx.episodes.get(d.id);
    if (e) await api.setWatched({ kind: 'episode', showId: d.show, id: d.id, watched: !e.progress.watched });
  } else if (d.watch === 'season') {
    const s = idx.shows.get(d.show);
    const n = Number(d.season);
    if (s) {
      const done = s.episodes.filter((e) => e.season === n).every((e) => e.progress.watched);
      await api.setWatched({ kind: 'season', showId: s.id, id: n, watched: !done });
    }
  } else if (d.hero) {
    const [type, id] = d.hero.split(':');
    if (type === 'movie') await api.setWatched({ kind: 'movie', id, watched: !idx.movies.get(id).progress.watched });
    else if (type === 'show') {
      const s = idx.shows.get(id);
      await api.setWatched({ kind: 'show', id, watched: s.watchedCount < s.episodes.length });
    }
  }
}

const ACTIONS = {
  tab: (d) => switchTab(d.tab),
  'open-movie': (d) => go({ name: 'movie', id: d.id }),
  'open-show': (d) => go({ name: 'show', id: d.id }),
  'play-movie': (d) => playMovie(d.id, d.mode),
  'play-episode': (d) => playEpisode(d.show, d.id, d.mode),
  season: (d) => {
    const r = route();
    if (Number(d.season) !== r.season) selectSeason(r, Number(d.season));
    else {
      const first = page.querySelector('.episode-track .focusable');
      if (first) Nav.focusGroup(first.closest('[data-nav-group]'));
    }
  },
  'toggle-watched': async (d) => {
    const watched = d.watched !== '1';
    await api.setWatched({ kind: d.kind, id: d.id, watched });
    toast(watched ? 'Marked as watched' : 'Marked as unwatched');
  },
  'show-file': (d) => api.showInFolder(d.path),
  pref: (d) => {
    prefs[d.pref] = d.value;
    savePrefs();
    render({ keepFocus: true });
  },
  key: (d) => searchKey(d.k),
  'add-library': (d) => addLibrary(d.type),
  rescan: () => api.rescan(),
  'library-menu': async (d) => {
    const libs = [...S.settings.libraries];
    const lib = libs[Number(d.index)];
    const v = await choose({
      title: lib.path,
      text: lib.type === 'tv' ? 'TV shows folder' : 'Movies folder',
      choices: [
        { label: lib.type === 'tv' ? 'Treat as movies folder' : 'Treat as TV shows folder', value: 'type' },
        { label: 'Remove from library', value: 'remove', danger: true },
        { label: 'Cancel', value: null }
      ]
    });
    if (v === 'remove') libs.splice(Number(d.index), 1);
    else if (v === 'type') libs[Number(d.index)] = { ...lib, type: lib.type === 'tv' ? 'movies' : 'tv' };
    else return;
    await save({ libraries: libs });
    render({ keepFocus: true });
  },
  'vlc-menu': async () => {
    const v = await choose({
      title: 'VLC location',
      text: S.settings.vlcPath || (S.vlcFound ? `Detected: ${S.vlcFound}` : 'VLC could not be found automatically.'),
      choices: [
        { label: 'Browse for vlc.exe…', value: 'browse', primary: true },
        { label: 'Detect automatically', value: 'auto' },
        { label: 'Cancel', value: null }
      ]
    });
    if (v === 'browse') {
      const p = await api.pickVlc();
      if (p) await save({ vlcPath: p });
    } else if (v === 'auto') {
      await save({ vlcPath: '' });
      S.vlcFound = await api.detectVlc();
      toast(S.vlcFound ? `Found VLC at ${S.vlcFound}` : 'VLC not found', S.vlcFound ? 'info' : 'error');
    }
    render({ keepFocus: true });
  },
  'toggle-vlcFullscreen': () => toggleSetting('vlcFullscreen'),
  'toggle-autoplayNext': () => toggleSetting('autoplayNext'),
  'toggle-startFullscreen': () => toggleSetting('startFullscreen'),
  'toggle-launchAtLogin': () => toggleSetting('launchAtLogin'),
  'cycle-scale': async () => {
    const steps = [0.85, 1, 1.15, 1.3, 1.5];
    const cur = S.settings.uiScale || 1;
    const next = steps[(steps.findIndex((s) => Math.abs(s - cur) < 0.01) + 1) % steps.length];
    await save({ uiScale: next });
    render({ keepFocus: true });
  },
  'clear-metadata': async () => {
    if (!S.settings.tmdbKey) return toast('Add a TMDB API key first.');
    await api.clearMetadata();
    toast('Refreshing artwork & info…');
  },
  minimize: () => api.minimize(),
  fullscreen: () => api.toggleFullscreen(),
  quit: () => api.quit()
};

async function toggleSetting(key) {
  await save({ [key]: !S.settings[key] });
  render({ keepFocus: true });
}

document.addEventListener('click', (e) => {
  const tab = e.target.closest('[data-tab]');
  if (tab && tab.closest('#tabs')) {
    if (tab.dataset.tab === route().name && stack.length === 1) {
      // Selecting the current tab drops into its content.
      Nav.focus(page.querySelector('[data-autofocus]') || page.querySelector('.focusable'));
      return;
    }
    switchTab(tab.dataset.tab);
    return;
  }
  const el = e.target.closest('[data-act]');
  if (!el || !page.contains(el)) return;
  const fn = ACTIONS[el.dataset.act];
  if (fn) fn(el.dataset);
});

// Global shortcuts.
document.addEventListener('keydown', (e) => {
  const typing = document.activeElement && document.activeElement.tagName === 'INPUT';
  if (e.key === 'F11') {
    e.preventDefault();
    api.toggleFullscreen();
  } else if (e.key === 'F5') {
    e.preventDefault();
    api.rescan();
  } else if (e.key === 'Tab') {
    e.preventDefault();
  } else if (!typing && !modalState && !S.nowPlaying) {
    if (e.key === '/' || (e.ctrlKey && e.key.toLowerCase() === 'f')) {
      e.preventDefault();
      if (route().name !== 'search') switchTab('search');
      Nav.focus($('#q'));
    } else if (e.key.toLowerCase() === 'w' && !e.ctrlKey && route().name !== 'search') {
      toggleWatchedFocused();
    } else if (route().name === 'search' && e.key.length === 1 && /[\w ]/.test(e.key) && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      searchKey(e.key.toLowerCase());
    } else if (route().name === 'search' && e.key === 'Backspace' && $('#q').value) {
      e.preventDefault();
      e.stopImmediatePropagation();
      searchKey('back');
    }
  }
}, true);

Nav.onBack(() => {
  back();
});

Nav.onAction((btn) => {
  if (modalState || S.nowPlaying) return;
  if (btn === 'x') toggleWatchedFocused();
  else if (btn === 'y') switchTab('search');
  else if (btn === 'start') switchTab('settings');
  else if (btn === 'lb' || btn === 'rb') {
    const i = TABS.findIndex((t) => t.name === stack[0].name);
    const next = TABS[(i + (btn === 'rb' ? 1 : TABS.length - 1)) % TABS.length];
    switchTab(next.name);
  }
});

// ============================================================================ Status & clock

function renderStatus() {
  const el = $('#status');
  const ms = S.metaStatus || {};
  let text = '';
  if (S.scanning) text = 'Scanning library…';
  else if (ms.running) text = 'Fetching artwork…';
  el.hidden = !text;
  el.innerHTML = text ? `<span class="spinner"></span>${h(text)}` : '';
}

function tick() {
  $('#clock').textContent = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}
setInterval(tick, 5000);
tick();

// ============================================================================ Boot

let lastSig = '';
function librarySignature() {
  // Cheap fingerprint of what the current view shows, to avoid re-rendering (and jolting focus) needlessly.
  const L = S.library;
  return JSON.stringify([
    L.scannedAt,
    L.movies.length,
    L.shows.length,
    L.continueWatching,
    S.settings.libraries,
    S.scanning,
    S.metaStatus,
    L.movies.map((m) => [m.poster, m.progress.watched, m.progress.resumable, m.overview.length]),
    L.shows.map((s) => [s.poster, s.watchedCount, s.nextUp, s.overview.length, s.episodes.map((e) => [e.progress.watched, e.progress.resumable, e.thumb, e.title])])
  ]);
}

function onState(next) {
  applyState(next);
  const sig = librarySignature();
  if (sig === lastSig) return;
  lastSig = sig;
  const r = route();
  // Don't pull the rug out from under someone typing in search or settings.
  if (r.name === 'search' || (r.name === 'settings' && document.activeElement && document.activeElement.tagName === 'INPUT')) return;
  if (r.name === 'home' && r.welcome && S.settings.libraries.length) return render();
  const hero = $('#hero');
  if (hero) delete hero.dataset.item;
  // If the page had nothing to focus (e.g. first scan still running), land on the new content instead.
  render(page.querySelector('.focusable') ? { keepFocus: true } : {});
}

(async function boot() {
  applyState(await api.getState());
  lastSig = librarySignature();
  S.vlcFound = await api.detectVlc();
  render();
  api.onState(onState);
  api.onNowPlaying((np) => {
    S.nowPlaying = np;
    renderNowPlaying();
  });
  api.onToast((t) => toast(t.text, t.kind));
  if (S.nowPlaying) renderNowPlaying();
})();
