'use strict';

/* global S, idx, prefs, t, h, img, placeholder, fmtRuntime, fmtPlaytime, fmtAgo, remaining, epCode, seasonName, pct, norm,
   ICON, Backdrop, VIEWS, page, route, render, Nav, $, keyboardHtml */

// ============================================================================ Cards

const visibleItems = (list) => list.filter((x) => !x.hidden);

function posterCard(item, keyPrefix) {
  let badge = '';
  let sub = '';
  let act = '';
  if (item.type === 'movie') {
    if (item.progress.watched) badge = '<div class="badge-watched">✓</div>';
    sub = item.year || '';
    act = 'open-movie';
  } else if (item.type === 'show') {
    const left = item.episodes.length - item.watchedCount;
    if (left === 0 && item.episodes.length) badge = '<div class="badge-watched">✓</div>';
    else if (item.watchedCount > 0) badge = `<div class="badge-count">${left}</div>`;
    sub = t('n.seasons', { n: item.seasons.length });
    act = 'open-show';
  } else {
    sub = item.lastPlayed ? fmtAgo(item.lastPlayed) : item.source === 'steam' ? 'Steam' : t('game.notPlayed');
    act = 'open-game';
  }
  const fav = item.favorite ? `<div class="badge-fav">${ICON.starOn}</div>` : '';
  const bar = item.type === 'movie' && item.progress.resumable ? `<div class="progress"><i style="width:${pct(item.progress)}%"></i></div>` : '';
  const art = item.type === 'game' && !item.poster ? gamePlaceholder(item) : img(item.poster, item.title);
  return `
    <button class="card focusable" data-key="${keyPrefix}-${item.id}" data-hero="${item.type}:${item.id}" data-opts="${item.type}:${item.id}"
      data-act="${act}" data-id="${item.id}">
      <div class="art">${art}${badge}${fav}${bar}</div>
      <div class="label">${h(item.title)}</div>
      <div class="sublabel">${h(sub)}</div>
    </button>`;
}

/** A game without a cover: its icon (if any) over a generated gradient, with the title. */
function gamePlaceholder(g) {
  const icon = g.icon ? `<img class="ph-icon" src="${h(g.icon)}" alt="">` : `<span class="ph-icon">${ICON.gamepad}</span>`;
  return `<div class="placeholder game-ph" style="--h:${hueOf(g.title)}">${icon}<span>${h(g.title)}</span></div>`;
}

function hueOf(s) {
  let x = 0;
  for (const c of String(s)) x = (x * 31 + c.charCodeAt(0)) >>> 0;
  return x % 360;
}

/** Wide card for "Jump back in": background art with the game's logo on top. */
function gameWideCard(g, keyPrefix) {
  const bg = g.hero || g.header || g.poster;
  const logo = g.logo ? `<img class="card-logo logo-img" src="${h(g.logo)}" alt="">` : `<div class="card-logo-text">${h(g.title)}</div>`;
  return `
    <button class="card wide focusable" data-key="${keyPrefix}-${g.id}" data-hero="game:${g.id}" data-opts="game:${g.id}" data-act="play-game" data-id="${g.id}" data-hint="hint.play">
      <div class="art">${bg ? img(bg, g.title, '') : placeholder(g.title, '')}<div class="art-shade"></div>${logo}</div>
      <div class="label">${h(g.title)}</div>
      <div class="sublabel">${h([fmtAgo(g.lastPlayed), fmtPlaytime(g.playtime)].filter(Boolean).join(' · '))}</div>
    </button>`;
}

function continueCard(c) {
  if (c.kind === 'movie') {
    const m = idx.movies.get(c.id);
    if (!m) return '';
    return `
      <button class="card wide focusable" data-key="cw-${m.id}" data-hero="movie:${m.id}" data-opts="movie:${m.id}" data-act="play-movie" data-id="${m.id}" data-hint="hint.play">
        <div class="art">${img(m.backdrop || m.poster, m.title)}<div class="progress"><i style="width:${pct(m.progress)}%"></i></div></div>
        <div class="label">${h(m.title)}</div>
        <div class="sublabel">${h(remaining(m.progress) || t('media.resume'))}</div>
      </button>`;
  }
  const e = idx.episodes.get(c.id);
  if (!e) return '';
  const pr = e.progress;
  const bar = pr.resumable ? `<div class="progress"><i style="width:${pct(pr)}%"></i></div>` : '';
  return `
    <button class="card wide focusable" data-key="cw-${e.id}" data-hero="episode:${e.id}" data-opts="episode:${e.show.id}:${e.id}" data-act="play-episode" data-show="${e.show.id}" data-id="${e.id}" data-hint="hint.play">
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
    <button class="card wide focusable ${pr.watched ? 'watched' : ''}" data-key="ep-${e.id}" data-act="play-episode" data-show="${show.id}" data-id="${e.id}" data-opts="episode:${show.id}:${e.id}" data-hint="hint.play">
      <div class="art">${art}${watched}${bar}</div>
      <div class="label"><span class="ep-num">${e.episode}${e.episodeEnd ? '–' + e.episodeEnd : ''}</span>${h(e.title || t('ep.n', { n: e.episode }))}</div>
      <div class="sublabel">${h(meta || (pr.resumable ? remaining(pr) : ''))}</div>
      <div class="ep-overview">${h(e.overview)}</div>
    </button>`;
}

/**
 * A horizontal row of cards. `opts.total` shows a count next to the title; `opts.seeAll` ({tab, pref, value})
 * ends the row with a card that opens that tab already sorted/filtered.
 */
function row(title, cards, key, opts = {}) {
  if (!cards.length) return '';
  const count = opts.total ? `<span class="row-count">${opts.total}</span>` : '';
  const kicker = opts.kicker ? `<span class="row-kicker">${h(opts.kicker)}</span>` : '';
  const seeAll = opts.seeAll
    ? `<button class="card see-all focusable ${opts.wide ? 'wide' : ''}" data-act="see-all" data-tab="${opts.seeAll.tab}" data-pref="${opts.seeAll.pref || ''}" data-value="${opts.seeAll.value || ''}" data-key="${key}-all">
         <div class="art"><span class="see-all-inner">${ICON.more}<span>${h(t('home.seeAll'))}</span></span></div>
       </button>`
    : '';
  return `
    <section class="row" data-nav-group data-row="${key}">
      <h2>${kicker}${h(title)}${count}</h2>
      <div class="track" data-scroll="${key}">${cards.join('')}${seeAll}</div>
    </section>`;
}

function metaLine(item) {
  const bits = [];
  if (item.year) bits.push(`<span>${item.year}</span>`);
  if (item.type === 'movie' && item.runtime) bits.push(`<span>${fmtRuntime(item.runtime)}</span>`);
  if (item.type === 'show') bits.push(`<span>${h(t('n.seasons', { n: item.seasons.length }))} · ${h(t('n.episodes', { n: item.episodes.length }))}</span>`);
  if (item.rating) bits.push(`<span class="rating">${item.rating}</span>`);
  if (item.type === 'show' && item.status) bits.push(`<span class="pill">${h(item.status)}</span>`);
  if (item.type === 'movie' && item.progress.watched) bits.push(`<span class="pill">${h(t('media.watched'))}</span>`);
  if (item.favorite) bits.push(`<span class="pill fav">${ICON.starOn}${h(t('opt.favorite'))}</span>`);
  return `<div class="meta">${bits.join('')}</div>`;
}

function gameMetaLine(g) {
  const bits = [];
  if (g.playtime) bits.push(`<span>${h(fmtPlaytime(g.playtime))}</span>`);
  if (g.lastPlayed) bits.push(`<span>${h(t('game.lastPlayed', { when: fmtAgo(g.lastPlayed) }))}</span>`);
  if (g.releaseDate) bits.push(`<span>${h(g.releaseDate)}</span>`);
  if (g.metacritic) bits.push(`<span class="pill score">${g.metacritic}</span>`);
  if (g.controller === 'full') bits.push(`<span class="pill">${ICON.gamepad}${h(t('game.controllerFull'))}</span>`);
  bits.push(`<span class="pill">${g.source === 'steam' ? ICON.steam + 'Steam' : h(t('game.manual'))}</span>`);
  if (g.favorite) bits.push(`<span class="pill fav">${ICON.starOn}${h(t('opt.favorite'))}</span>`);
  return `<div class="meta">${bits.join('')}</div>`;
}

// ============================================================================ Welcome & Home

VIEWS.welcome = {
  render() {
    const steam = S.library.steamFound;
    return `
      <div class="page welcome">
        <h1>${h(t('welcome.title'))}</h1>
        <p>${h(t('welcome.body'))}</p>
        <div class="check ${steam ? 'ok' : ''}"><span class="dot"></span>${h(steam ? t('welcome.steamFound') : t('welcome.steamMissing'))}</div>
        <div class="check ${S.vlcFound ? 'ok' : ''}"><span class="dot"></span>${h(S.vlcFound ? t('welcome.vlcFound', { path: S.vlcFound }) : t('welcome.vlcMissing'))}</div>
        <div class="actions" data-nav-group>
          <button class="btn primary focusable" data-act="add-game" data-key="w-game" data-autofocus>${ICON.plus}${h(t('games.add'))}</button>
          <button class="btn focusable" data-act="add-library" data-type="movies" data-key="w-movies">${ICON.plus}${h(t('lib.addMovies'))}</button>
          <button class="btn focusable" data-act="add-library" data-type="tv" data-key="w-tv">${ICON.plus}${h(t('lib.addTv'))}</button>
          <button class="btn focusable" data-act="tab" data-tab="settings" data-key="w-settings">${h(t('tab.settings'))}</button>
        </div>
      </div>`;
  }
};

function greeting() {
  const hr = new Date().getHours();
  const part = hr < 5 ? 'night' : hr < 12 ? 'morning' : hr < 18 ? 'afternoon' : hr < 22 ? 'evening' : 'night';
  const name = S.library.steamUser;
  return name ? t(`greet.${part}Name`, { name }) : t(`greet.${part}`);
}

VIEWS.home = {
  render(r) {
    const L = S.library;
    const games = visibleItems(L.games);
    const movies = visibleItems(L.movies);
    const shows = visibleItems(L.shows);
    if (!games.length && !movies.length && !shows.length && !S.scanning) {
      r.welcome = true;
      return VIEWS.welcome.render();
    }
    r.welcome = false;
    const played = games.filter((g) => g.lastPlayed).sort((a, b) => b.lastPlayed - a.lastPlayed);
    const recentGames = [...games].sort((a, b) => b.addedAt - a.addedAt);
    const favorites = [...games, ...movies, ...shows].filter((x) => x.favorite).sort((a, b) => a.title.localeCompare(b.title));
    const recentMovies = [...movies].sort((a, b) => b.addedAt - a.addedAt);
    const recentShows = [...shows].sort((a, b) => b.addedAt - a.addedAt);
    const unwatched = movies.filter((m) => !m.progress.watched && !m.progress.resumable);
    const cw = L.continueWatching.map(continueCard).filter(Boolean);

    // Whatever you did most recently leads: games or films/TV.
    const lastGame = played.length ? played[0].lastPlayed : 0;
    const lastWatch = L.continueWatching.length ? L.continueWatching[0].at : 0;
    const gameRow = { key: 'jb', html: (kicker) => row(t('home.jumpBackIn'), played.slice(0, 12).map((g) => gameWideCard(g, 'jb')), 'jb', { kicker, wide: true, seeAll: played.length > 12 ? { tab: 'games', pref: 'gameSort', value: 'recent' } : null }) };
    const watchRow = { key: 'cw', html: (kicker) => row(t('home.continueWatching'), cw, 'cw', { kicker, wide: true }) };
    const lead = lastWatch > lastGame ? [watchRow, gameRow] : [gameRow, watchRow];
    const leadHtml = [];
    for (const r0 of lead) {
      const html = r0.html(leadHtml.length ? '' : greeting());
      if (html) leadHtml.push(html);
    }
    const g1 = leadHtml.length ? '' : greeting();
    const rows = [
      ...leadHtml,
      row(t('home.favorites'), favorites.map((x) => posterCard(x, 'fv')), 'fv', { kicker: g1, total: favorites.length }),
      row(t('home.recentGames'), recentGames.slice(0, 16).map((g) => posterCard(g, 'rg')), 'rg', { kicker: leadHtml.length || favorites.length ? '' : g1, total: games.length, seeAll: games.length > 16 ? { tab: 'games', pref: 'gameSort', value: 'added' } : null }),
      row(t('home.recentMovies'), recentMovies.slice(0, 16).map((m) => posterCard(m, 'rm')), 'rm', { total: movies.length, seeAll: movies.length > 16 ? { tab: 'movies', pref: 'movieSort', value: 'added' } : null }),
      row(t('home.recentShows'), recentShows.slice(0, 16).map((x) => posterCard(x, 'rs')), 'rs', { total: shows.length, seeAll: shows.length > 16 ? { tab: 'shows', pref: 'showSort', value: 'added' } : null }),
      row(t('home.unwatchedMovies'), unwatched.slice(0, 16).map((m) => posterCard(m, 'um')), 'um', { total: unwatched.length, seeAll: unwatched.length > 16 ? { tab: 'movies', pref: 'movieFilter', value: 'unwatched' } : null })
    ].join('');
    const empty = !rows.trim() ? `<div class="page-head"><div class="empty-hint">${h(t('status.scanning'))}</div></div>` : '';
    return `
      <div class="page home">
        <section class="hero" id="hero" data-nav-group></section>
        <div class="rows" data-scroll="home">${rows}${empty}</div>
      </div>`;
  },
  mount() {
    // Start on the first card (the banner shows it); the banner's buttons are one press Up.
    const first = page.querySelector('.rows .card');
    if (first) first.setAttribute('data-autofocus', '');
    alignedRow = null;
    if ($('#hero')) heroFromFocus();
    Spotlight.arm();
  },
  onFocus(r, target) {
    heroFromFocus();
    alignRow(target);
  }
};

/** Keep the row you're on in the same place under the banner, so moving up/down never jumps around. */
let alignedRow = null;
function alignRow(target) {
  const rowEl = target.closest && target.closest('.row');
  const rows = rowEl && rowEl.closest('.rows');
  if (!rows || rowEl === alignedRow) return;
  alignedRow = rowEl;
  // After Nav's own scrollIntoView, so ours wins.
  requestAnimationFrame(() => {
    const top = rowEl.offsetTop;
    rows.scrollTo({ top: Math.max(0, top), behavior: reducedMotion() ? 'auto' : 'smooth' });
  });
}

function heroFromFocus() {
  const hero = $('#hero');
  const a = document.activeElement;
  if (!hero || !a) return;
  const key = a.dataset && a.dataset.hero;
  if (!key) {
    // Focus on the banner's own buttons keeps what it shows.
    if (!hero.dataset.item) {
      const first = page.querySelector('[data-hero]');
      if (first) paintHero(first.dataset.hero);
    }
    return;
  }
  Spotlight.stop();
  paintHero(key);
}

/** The banner's buttons for an item: the main action (play/resume) and its details page. */
function heroActions(type, id) {
  const btn = (act, attrs, icon, label, primary) =>
    `<button class="btn ${primary ? 'primary' : ''} focusable" data-act="${act}" ${attrs} ${primary ? 'data-nav-default' : ''} data-key="hero-${primary ? 'play' : 'info'}">${icon}${h(label)}</button>`;
  if (type === 'game') return btn('play-game', `data-id="${id}"`, ICON.play, t('game.play'), true) + btn('open-game', `data-id="${id}"`, ICON.more, t('opt.details'));
  if (type === 'movie') {
    const m = idx.movies.get(id);
    const label = m.progress.resumable ? t('media.resumeFrom', { time: fmtTime(m.progress.time) }) : t('media.play');
    return btn('play-movie', `data-id="${id}" data-mode="${m.progress.resumable ? 'resume' : 'start'}"`, ICON.play, label, true) + btn('open-movie', `data-id="${id}"`, ICON.more, t('opt.details'));
  }
  if (type === 'show') {
    const s = idx.shows.get(id);
    const next = s.nextUp ? idx.episodes.get(s.nextUp) : null;
    const main = next ? btn('play-episode', `data-show="${s.id}" data-id="${next.id}"`, ICON.play, next.progress.resumable ? t('show.resumeEp', { ep: epCode(next) }) : t('show.playEp', { ep: epCode(next) }), true) : '';
    return main + btn('open-show', `data-id="${id}"`, ICON.more, t('opt.details'), !main);
  }
  if (type === 'episode') {
    const e = idx.episodes.get(id);
    return (
      btn('play-episode', `data-show="${e.show.id}" data-id="${e.id}"`, ICON.play, e.progress.resumable ? t('media.resumeFrom', { time: fmtTime(e.progress.time) }) : t('media.play'), true) +
      btn('open-show', `data-id="${e.show.id}"`, ICON.more, t('opt.goToShow'))
    );
  }
  return '';
}

function paintHero(key, { spotlight = false } = {}) {
  const hero = $('#hero');
  if (!hero || hero.dataset.item === key) return;
  const [type, id] = key.split(':');
  let kicker = '';
  let title = '';
  let logo = null;
  let meta = '';
  let overview = '';
  let bg = null;
  let progress = null;
  if (type === 'movie') {
    const m = idx.movies.get(id);
    if (!m) return;
    kicker = m.progress.resumable ? `${t('media.resume')} · ${remaining(m.progress)}` : t('kind.movie');
    title = m.title;
    meta = metaLine(m);
    overview = m.overview;
    bg = m.backdrop || m.poster;
    if (m.progress.resumable) progress = m.progress;
  } else if (type === 'show') {
    const s = idx.shows.get(id);
    if (!s) return;
    kicker = t('kind.show');
    title = s.title;
    meta = metaLine(s);
    overview = s.overview;
    bg = s.backdrop || s.poster;
  } else if (type === 'episode') {
    const e = idx.episodes.get(id);
    if (!e) return;
    kicker = `${e.show.title} · ${epCode(e)}`;
    title = e.title || e.show.title;
    meta = `<div class="meta">${e.runtime ? `<span>${fmtRuntime(e.runtime)}</span>` : ''}<span>${h(e.progress.resumable ? remaining(e.progress) : t('media.upNext'))}</span></div>`;
    overview = e.overview || e.show.overview;
    bg = e.show.backdrop || e.thumb || e.show.poster;
    if (e.progress.resumable) progress = e.progress;
  } else if (type === 'game') {
    const g = idx.games.get(id);
    if (!g) return;
    kicker = g.genres.slice(0, 2).join(' · ') || t('kind.game');
    title = g.title;
    logo = g.logo;
    meta = gameMetaLine(g);
    overview = g.overview;
    bg = g.hero || g.header || g.poster;
  } else return;
  hero.dataset.item = key;
  hero.classList.remove('swap');
  void hero.offsetWidth; // restart the crossfade
  hero.classList.add('swap');
  hero.classList.toggle('spotlight', spotlight);
  if (!spotlight) parallaxFrom(document.activeElement);
  // Keep focus if it was on the banner's buttons (a spotlight change re-renders them).
  const refocus = hero.contains(document.activeElement) ? document.activeElement.dataset.key : null;
  hero.innerHTML = `
    <div class="kicker">${spotlight ? `<span class="spot-dot"></span>${h(t('home.spotlight'))} · ` : ''}${h(kicker)}</div>
    ${logo ? `<img class="hero-logo logo-img" src="${h(logo)}" alt="${h(title)}">` : `<h1>${h(title)}</h1>`}
    ${meta}
    ${overview ? `<p class="overview">${h(overview)}</p>` : ''}
    ${progress ? `<div class="hero-progress"><div class="bar"><i style="width:${pct(progress)}%"></i></div><span>${h(t('media.timeOf', { time: fmtTime(progress.time), total: fmtTime(progress.length) }))}</span></div>` : ''}
    <div class="actions hero-actions">${heroActions(type, id)}</div>`;
  if (refocus) {
    const el = hero.querySelector(`[data-key="${refocus}"]`) || hero.querySelector('.btn');
    if (el) Nav.focus(el, { scroll: false });
  }
  Backdrop.set(bg);
}

/**
 * Spotlight: after a while without input on Home, the banner slowly cycles through your recent games and
 * shows (the TV-launcher "attract" loop). Any input snaps it back to what's selected.
 */
const Spotlight = (() => {
  const IDLE_MS = 45000;
  const EVERY_MS = 11000;
  let idleTimer = null;
  let cycleTimer = null;
  let index = 0;
  let running = false;

  function items() {
    return [...page.querySelectorAll('.rows [data-row="jb"] [data-hero], .rows [data-row="cw"] [data-hero], .rows [data-row="fv"] [data-hero]')]
      .map((el) => el.dataset.hero)
      .filter((k, i, all) => all.indexOf(k) === i)
      .slice(0, 10);
  }

  function next() {
    const list = items();
    const hero = $('#hero');
    if (list.length < 2 || !hero || route().name !== 'home' || document.hidden || modals.length || S.game || S.nowPlaying) return;
    index = (index + 1) % list.length;
    if (list[index] === hero.dataset.item) index = (index + 1) % list.length;
    paintHero(list[index], { spotlight: true });
  }

  function start() {
    if (running || reducedMotion()) return;
    running = true;
    index = Math.max(0, items().indexOf(($('#hero') || {}).dataset?.item));
    next();
    cycleTimer = setInterval(next, EVERY_MS);
  }

  function stop() {
    clearInterval(cycleTimer);
    const was = running;
    running = false;
    if (was) {
      // Back to whatever is actually selected.
      const hero = $('#hero');
      const a = document.activeElement;
      if (hero) {
        delete hero.dataset.item;
        const key = a && a.dataset && a.dataset.hero;
        paintHero(key || (page.querySelector('[data-hero]') || {}).dataset?.hero || '');
      }
    }
  }

  function arm() {
    clearTimeout(idleTimer);
    if (route().name === 'home') idleTimer = setTimeout(start, IDLE_MS);
  }

  const wake = () => {
    if (running) stop();
    arm();
  };
  for (const ev of ['keydown', 'pointerdown', 'wheel', 'touchstart']) document.addEventListener(ev, wake, { passive: true, capture: true });
  Nav.onMove(wake);
  Nav.onAction(wake);
  return { arm, stop, isRunning: () => running };
})();

// ============================================================================ Grids

/** Nudge the backdrop a little against the focused card's position, for depth. */
function parallaxFrom(el) {
  const bd = $('#backdrop');
  if (!el || !el.getBoundingClientRect || !bd) return;
  const r = el.getBoundingClientRect();
  const x = (r.left + r.width / 2) / window.innerWidth - 0.5;
  bd.style.setProperty('--px', x.toFixed(3));
}

function sortItems(items, sort) {
  const list = [...items];
  const byTitle = (a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: 'base', numeric: true });
  if (sort === 'added') list.sort((a, b) => b.addedAt - a.addedAt);
  else if (sort === 'year') list.sort((a, b) => (b.year || 0) - (a.year || 0) || byTitle(a, b));
  else if (sort === 'rating') list.sort((a, b) => (b.rating || 0) - (a.rating || 0) || byTitle(a, b));
  else if (sort === 'recent') list.sort((a, b) => (b.lastPlayed || 0) - (a.lastPlayed || 0) || byTitle(a, b));
  else if (sort === 'playtime') list.sort((a, b) => (b.playtime || 0) - (a.playtime || 0) || byTitle(a, b));
  else list.sort(byTitle);
  return list;
}

function chips(group, current, options) {
  return options
    .map(
      ([value, labelKey]) =>
        `<button class="chip focusable ${value === current ? 'on' : ''}" ${value === current ? 'data-nav-default' : ''} data-act="pref" data-pref="${group}" data-value="${value}" data-key="chip-${group}-${value}">${h(t(labelKey))}</button>`
    )
    .join('');
}

const MEDIA_SORTS = [
  ['title', 'sort.az'],
  ['added', 'sort.added'],
  ['year', 'sort.year'],
  ['rating', 'sort.rating']
];

function applyFilter(items, filter, custom) {
  if (filter === 'hidden') return items.filter((x) => x.hidden);
  const shown = items.filter((x) => !x.hidden);
  if (filter === 'favorites') return shown.filter((x) => x.favorite);
  return custom ? custom(shown, filter) : shown;
}

function gridPage({ title, all, items, keyPrefix, toolbar, empty, scroll }) {
  return `
    <div class="page" data-scroll="${scroll}">
      <div class="page-head"><h1 class="page-title">${h(title)}</h1><div class="page-count">${h(t('grid.count', { n: items.length, total: all }))}</div></div>
      <div class="toolbar" data-nav-group>${toolbar}</div>
      <div class="grid" data-nav-group>${items.map((x) => posterCard(x, keyPrefix)).join('') || `<div class="empty-hint">${h(empty || t('grid.empty'))}</div>`}</div>
    </div>`;
}

VIEWS.games = {
  render() {
    const all = S.library.games;
    const items = sortItems(
      applyFilter(all, prefs.gameFilter, (list, f) => (f === 'steam' ? list.filter((g) => g.source === 'steam') : f === 'other' ? list.filter((g) => g.source !== 'steam') : list)),
      prefs.gameSort
    );
    const toolbar = `
      <button class="chip accent focusable" data-act="add-game" data-key="add-game">${ICON.plus}${h(t('games.add'))}</button>
      <span class="chip-sep"></span>
      ${chips('gameSort', prefs.gameSort, [['recent', 'sort.recent'], ['az', 'sort.az'], ['playtime', 'sort.playtime'], ['added', 'sort.added']])}
      <span class="chip-sep"></span>
      ${chips('gameFilter', prefs.gameFilter, [['all', 'filter.all'], ['steam', 'filter.steam'], ['other', 'filter.other'], ['favorites', 'filter.favorites'], ['hidden', 'filter.hidden']])}`;
    const empty = !all.length ? (S.library.steamFound ? t('games.emptySteam') : t('games.empty')) : null;
    return gridPage({ title: t('tab.games'), all: all.length, items, keyPrefix: 'gg', toolbar, empty, scroll: 'games' });
  },
  onFocus: () => ambientFromFocus()
};

VIEWS.movies = {
  render() {
    const all = S.library.movies;
    const items = sortItems(
      applyFilter(all, prefs.movieFilter, (list, f) => (f === 'unwatched' ? list.filter((m) => !m.progress.watched) : f === 'progress' ? list.filter((m) => m.progress.resumable) : list)),
      prefs.movieSort
    );
    const toolbar = `
      ${chips('movieSort', prefs.movieSort, MEDIA_SORTS)}
      <span class="chip-sep"></span>
      ${chips('movieFilter', prefs.movieFilter, [['all', 'filter.all'], ['unwatched', 'filter.unwatched'], ['progress', 'filter.inProgress'], ['favorites', 'filter.favorites'], ['hidden', 'filter.hidden']])}`;
    return gridPage({ title: t('tab.movies'), all: all.length, items, keyPrefix: 'mg', toolbar, empty: !all.length ? t('lib.noMovies') : null, scroll: 'movies' });
  },
  onFocus: () => ambientFromFocus()
};

VIEWS.shows = {
  render() {
    const all = S.library.shows;
    const items = sortItems(
      applyFilter(all, prefs.showFilter, (list, f) =>
        f === 'unwatched' ? list.filter((s) => s.watchedCount < s.episodes.length) : f === 'progress' ? list.filter((s) => s.watchedCount > 0 && s.watchedCount < s.episodes.length) : list
      ),
      prefs.showSort
    );
    const toolbar = `
      ${chips('showSort', prefs.showSort, MEDIA_SORTS)}
      <span class="chip-sep"></span>
      ${chips('showFilter', prefs.showFilter, [['all', 'filter.all'], ['unwatched', 'filter.unwatched'], ['progress', 'filter.watching'], ['favorites', 'filter.favorites'], ['hidden', 'filter.hidden']])}`;
    return gridPage({ title: t('tab.shows'), all: all.length, items, keyPrefix: 'sg', toolbar, empty: !all.length ? t('lib.noShows') : null, scroll: 'shows' });
  },
  onFocus: () => ambientFromFocus()
};

function ambientFromFocus() {
  const a = document.activeElement;
  const key = a && a.dataset && a.dataset.hero;
  if (!key) return;
  const [type, id] = key.split(':');
  const item = type === 'movie' ? idx.movies.get(id) : type === 'show' ? idx.shows.get(id) : idx.games.get(id);
  document.body.classList.add('dim-backdrop');
  parallaxFrom(a);
  if (item) Backdrop.set(item.backdrop || item.hero || item.header || null);
}

// ============================================================================ Game detail

VIEWS.game = {
  render(r) {
    const g = idx.games.get(r.id);
    if (!g) return `<div class="page"><div class="page-head"><h1 class="page-title">${h(t('err.notFound'))}</h1></div></div>`;
    const shots = g.screenshots.length
      ? `<section class="row" data-nav-group>
          <h2>${h(t('game.screenshots'))}</h2>
          <div class="track shots" data-scroll="shots">${g.screenshots
            .map((s, i) => `<button class="card shot focusable" data-act="screenshot" data-id="${g.id}" data-index="${i}" data-key="shot-${i}"><div class="art">${img(s.thumb, g.title, '')}</div></button>`)
            .join('')}</div>
        </section>`
      : '';
    const credits = [g.developers.join(', '), g.publishers.filter((p) => !g.developers.includes(p)).join(', ')].filter(Boolean).join(' · ');
    return `
      <div class="page detail" data-scroll="game">
        <div class="detail-main game-main">
          <div class="detail-poster">${g.poster ? img(g.poster, g.title) : gamePlaceholder(g)}</div>
          <div class="detail-info">
            ${g.logo ? `<img class="detail-logo logo-img" src="${h(g.logo)}" alt="${h(g.title)}">` : `<h1>${h(g.title)}</h1>`}
            ${gameMetaLine(g)}
            ${g.overview ? `<p class="overview">${h(g.overview)}</p>` : ''}
            ${g.genres.length || credits ? `<div class="genres">${h([g.genres.join(' · '), credits].filter(Boolean).join('  —  '))}</div>` : ''}
            <div class="actions" data-nav-group>
              <button class="btn primary focusable" data-act="play-game" data-id="${g.id}" data-key="g-play" data-autofocus>${ICON.play}${h(t('game.play'))}</button>
              <button class="btn focusable" data-act="toggle-fav" data-kind="game" data-id="${g.id}" data-key="g-fav">${g.favorite ? ICON.starOn : ICON.star}${h(g.favorite ? t('opt.unfavorite') : t('opt.favorite'))}</button>
              <button class="btn focusable" data-act="edit-game" data-id="${g.id}" data-key="g-edit">${ICON.edit}${h(t('game.edit'))}</button>
              <button class="btn focusable icon-only" data-act="options" data-opts="game:${g.id}" data-key="g-more" aria-label="${h(t('hint.options'))}">${ICON.more}</button>
            </div>
          </div>
        </div>
        ${shots}
      </div>`;
  },
  mount(r) {
    const g = idx.games.get(r.id);
    Backdrop.set(g ? g.hero || g.header || g.poster : null, { immediate: true });
  }
};

// ============================================================================ Movie & show detail

VIEWS.movie = {
  render(r) {
    const m = idx.movies.get(r.id);
    if (!m) return `<div class="page"><div class="page-head"><h1 class="page-title">${h(t('err.notFound'))}</h1></div></div>`;
    const pr = m.progress;
    const actions = pr.resumable
      ? `<button class="btn primary focusable" data-act="play-movie" data-id="${m.id}" data-mode="resume" data-key="m-resume" data-autofocus>${ICON.play}${h(t('media.resumeFrom', { time: fmtTime(pr.time) }))}</button>
         <button class="btn focusable" data-act="play-movie" data-id="${m.id}" data-mode="start" data-key="m-start">${ICON.restart}${h(t('media.playFromStart'))}</button>`
      : `<button class="btn primary focusable" data-act="play-movie" data-id="${m.id}" data-mode="start" data-key="m-play" data-autofocus>${ICON.play}${h(t('media.play'))}</button>`;
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
            ${pr.resumable ? `<div class="resume-bar"><div class="bar"><i style="width:${pct(pr)}%"></i></div>${h(t('media.timeOf', { time: fmtTime(pr.time), total: fmtTime(pr.length) }))}</div>` : ''}
            <div class="actions" data-nav-group>
              ${actions}
              <button class="btn focusable" data-act="toggle-watched" data-kind="movie" data-id="${m.id}" data-watched="${pr.watched ? 1 : 0}" data-key="m-watched">${ICON.check}${h(pr.watched ? t('opt.markUnwatched') : t('opt.markWatched'))}</button>
              <button class="btn focusable icon-only" data-act="options" data-opts="movie:${m.id}" data-key="m-more" aria-label="${h(t('hint.options'))}">${ICON.more}</button>
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
    if (!s) return `<div class="page"><div class="page-head"><h1 class="page-title">${h(t('err.notFound'))}</h1></div></div>`;
    const next = s.nextUp ? idx.episodes.get(s.nextUp) : null;
    if (r.season === undefined || !s.seasons.includes(r.season)) {
      r.season = next ? next.season : s.seasons.find((n) => n > 0) ?? s.seasons[0];
    }
    const eps = s.episodes.filter((e) => e.season === r.season);
    const allWatched = s.watchedCount === s.episodes.length;
    let playBtn = '';
    if (next) {
      const label = next.progress.resumable ? t('show.resumeEp', { ep: epCode(next) }) : s.watchedCount ? t('show.playNext', { ep: epCode(next) }) : t('show.playEp', { ep: epCode(next) });
      playBtn = `<button class="btn primary focusable" data-act="play-episode" data-show="${s.id}" data-id="${next.id}" data-key="s-play" data-autofocus>${ICON.play}${h(label)}</button>`;
    } else if (s.episodes.length) {
      playBtn = `<button class="btn primary focusable" data-act="play-episode" data-show="${s.id}" data-id="${s.episodes[0].id}" data-mode="start" data-key="s-play" data-autofocus>${ICON.restart}${h(t('show.watchAgain', { ep: epCode(s.episodes[0]) }))}</button>`;
    }
    return `
      <div class="page detail" data-scroll="show">
        <div class="detail-main" style="min-height:56vh">
          <div class="detail-poster">${img(s.poster, s.title)}</div>
          <div class="detail-info">
            <h1>${h(s.title)}</h1>
            ${metaLine(s)}
            ${s.overview ? `<p class="overview">${h(s.overview)}</p>` : ''}
            ${s.genres.length ? `<div class="genres">${h(s.genres.join(' · '))}</div>` : ''}
            <div class="actions" data-nav-group>
              ${playBtn}
              <button class="btn focusable" data-act="toggle-watched" data-kind="show" data-id="${s.id}" data-watched="${allWatched ? 1 : 0}" data-key="s-watched">${ICON.check}${h(allWatched ? t('opt.showUnwatched') : t('opt.showWatched'))}</button>
              <button class="btn focusable icon-only" data-act="options" data-opts="show:${s.id}" data-key="s-more" aria-label="${h(t('hint.options'))}">${ICON.more}</button>
            </div>
          </div>
        </div>
        <div class="seasons" data-nav-group data-scroll="seasons">
          ${s.seasons
            .map((n) => {
              const list = s.episodes.filter((e) => e.season === n);
              const done = list.every((e) => e.progress.watched);
              return `<button class="chip focusable ${n === r.season ? 'on' : ''}" ${n === r.season ? 'data-nav-default' : ''} data-act="season" data-season="${n}" data-show="${s.id}" data-opts="season:${s.id}:${n}" data-key="season-${n}">${h(seasonName(n))}${done ? ' ✓' : ''}</button>`;
            })
            .join('')}
        </div>
        <section class="row" data-nav-group>
          <div class="track episode-track" data-scroll="eps-${r.season}">${eps.map((e) => episodeCard(e, s)).join('')}</div>
        </section>
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

// ============================================================================ Search

VIEWS.search = {
  render(r) {
    r.query = r.query || '';
    return `
      <div class="page search">
        <div class="search-side">
          <div class="page-head" style="padding-left:0"><h1 class="page-title">${h(t('tab.search'))}</h1></div>
          <input class="search-box focusable" id="q" data-key="q" value="${h(r.query)}" placeholder="${h(t('search.placeholder'))}" spellcheck="false" autocomplete="off">
          ${keyboardHtml()}
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
    box.innerHTML = `<div class="empty-hint" style="padding-top:9rem">${h(t('search.hint'))}</div>`;
    return;
  }
  const words = q.split(' ');
  const match = (x) => {
    const n = norm(x.title);
    return words.every((w) => n.includes(w));
  };
  const rank = (a, b) => Number(norm(b.title).startsWith(q)) - Number(norm(a.title).startsWith(q)) || a.title.localeCompare(b.title);
  const games = visibleItems(S.library.games).filter(match).sort(rank);
  const movies = visibleItems(S.library.movies).filter(match).sort(rank);
  const shows = visibleItems(S.library.shows).filter(match).sort(rank);
  const total = games.length + movies.length + shows.length;
  const section = (title, list, prefix) => (list.length ? `<h2>${h(title)}</h2><div class="grid" data-nav-group>${list.slice(0, 60).map((x) => posterCard(x, prefix)).join('')}</div>` : '');
  box.innerHTML = `
    <div class="page-head" style="padding-left:0"><div class="page-count">${h(t('search.results', { n: total }))}</div></div>
    ${section(t('tab.games'), games, 'qg')}
    ${section(t('tab.shows'), shows, 'qs')}
    ${section(t('tab.movies'), movies, 'qm')}
    ${!total ? `<div class="empty-hint">${h(t('search.none'))}</div>` : ''}`;
}

// ============================================================================ Settings

function toggleRow(name, desc, on, setting, key) {
  return `
    <button class="setting focusable" data-act="toggle-setting" data-setting="${setting}" data-key="${key}">
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

const mask = (v) => (v ? `••••${String(v).slice(-4)}` : t('set.notSet'));

VIEWS.settings = {
  render() {
    const st = S.settings;
    const libs = st.libraries || [];
    const langLabel = { auto: t('set.langAuto'), en: 'English', fr: 'Français' }[st.uiLanguage || 'auto'];
    const ms = S.metaStatus || {};
    const gs = S.gameInfoStatus || {};
    const status = (on, s) => (!on ? t('set.off') : s.running ? t('set.fetching') : s.error ? s.error : t('set.on'));
    const games = S.library.games;
    return `
      <div class="page settings" data-scroll="settings">
        <div class="page-head" style="padding-left:0"><h1 class="page-title">${h(t('tab.settings'))}</h1></div>
        <div class="settings-grid" data-nav-group>
          <div class="section-title">${h(t('set.general'))}</div>
          ${valueRow(t('set.language'), t('set.languageDesc'), langLabel, 'choose-language', 'lang')}

          <div class="section-title">${h(t('tab.games'))}</div>
          ${toggleRow(t('set.steam'), S.library.steamFound ? t('set.steamFound', { n: games.filter((g) => g.source === 'steam').length }) : t('set.steamMissing'), st.steamEnabled, 'steamEnabled', 't-steam')}
          ${valueRow(t('set.steamPath'), t('set.steamPathDesc'), st.steamPath || t('set.auto'), 'pick-steam', 'steam-path')}
          ${valueRow(t('set.sgdb'), t('set.sgdbDesc'), mask(st.sgdbKey), 'edit-key', 'sgdb', 'data-setting="sgdbKey"')}
          ${toggleRow(t('set.freeWhilePlaying'), t('set.freeWhilePlayingDesc'), st.freeWhilePlaying, 'freeWhilePlaying', 't-free')}
          <div class="settings-actions" data-nav-group>
            <button class="btn small focusable" data-act="add-game" data-key="s-add-game">${ICON.plus}${h(t('games.add'))}</button>
            <button class="btn small focusable" data-act="clear-metadata" data-key="s-refresh-games">${ICON.refresh}${h(t('set.refreshInfo'))}</button>
            <span class="s-desc">${h(status(true, gs))}</span>
          </div>

          <div class="section-title">${h(t('set.library'))}</div>
          ${libs
            .map(
              (l, i) => `
            <button class="setting focusable" data-act="library-menu" data-index="${i}" data-key="lib-${i}">
              <span class="lib-type">${h(l.type === 'tv' ? t('lib.tv') : t('tab.movies'))}</span>
              <div class="s-label"><div class="s-name">${h(l.path)}</div></div>
            </button>`
            )
            .join('')}
          ${libs.length ? '' : `<div class="empty-hint" style="padding:0.4rem 0 0.8rem">${h(t('lib.noFolders'))}</div>`}
          <div class="settings-actions" data-nav-group>
            <button class="btn small focusable" data-act="add-library" data-type="movies" data-key="add-movies">${ICON.plus}${h(t('lib.addMovies'))}</button>
            <button class="btn small focusable" data-act="add-library" data-type="tv" data-key="add-tv">${ICON.plus}${h(t('lib.addTv'))}</button>
            <button class="btn small focusable" data-act="rescan" data-key="rescan">${ICON.refresh}${h(S.scanning ? t('status.scanning') : t('lib.rescan'))}</button>
          </div>
          <div class="s-desc lib-stats">${h(t('lib.stats', { movies: S.library.movies.length, shows: S.library.shows.length, episodes: S.library.shows.reduce((n, s) => n + s.episodes.length, 0), games: games.length }))}</div>

          <div class="section-title">${h(t('set.playback'))}</div>
          ${valueRow(t('set.vlc'), t('set.vlcDesc'), st.vlcPath || (S.vlcFound ? t('set.autoPath', { path: S.vlcFound }) : t('set.notFound')), 'vlc-menu', 'vlc')}
          ${toggleRow(t('set.vlcFullscreen'), t('set.vlcFullscreenDesc'), st.vlcFullscreen, 'vlcFullscreen', 't-vlcfs')}
          ${toggleRow(t('set.autoplay'), t('set.autoplayDesc'), st.autoplayNext, 'autoplayNext', 't-autoplay')}
          ${valueRow(t('set.vlcArgs'), t('set.vlcArgsDesc'), st.vlcExtraArgs || t('set.none'), 'edit-text', 'vlc-args', 'data-setting="vlcExtraArgs"')}
          ${valueRow(t('set.tmdb'), t('set.tmdbDesc'), mask(st.tmdbKey), 'edit-key', 'tmdb', 'data-setting="tmdbKey"')}
          ${valueRow(t('set.refreshMedia'), t('set.refreshMediaDesc'), status(Boolean(st.tmdbKey), ms), 'clear-metadata', 'meta-refresh')}

          <div class="section-title">${h(t('set.controls'))}</div>
          ${toggleRow(t('set.haptics'), t('set.hapticsDesc'), st.haptics, 'haptics', 't-haptics')}
          ${toggleRow(t('set.sounds'), t('set.soundsDesc'), st.sounds, 'sounds', 't-sounds')}
          ${valueRow(t('set.textSize'), t('set.textSizeDesc'), `${Math.round((st.uiScale || 1) * 100)}%`, 'cycle-scale', 'scale')}
          ${valueRow(t('set.animations'), t('set.animationsDesc'), st.animations === 'reduced' ? t('set.animReduced') : t('set.animFull'), 'cycle-animations', 'anim')}
          <div class="controls-help">${controlsHelp()}</div>

          <div class="section-title">${h(t('set.fse'))}</div>
          <div class="fse-card">
            <div class="check ${S.fsePackage ? 'ok' : ''}"><span class="dot"></span>${h(S.fsePackage ? t('set.fseInstalled') : t('set.fseNotInstalled'))}</div>
            <p>${h(S.fsePackage ? t('set.fseHowTo') : t('set.fseInstallHowTo'))}</p>
            <div class="settings-actions" data-nav-group>
              ${S.platform === 'win32' ? `<button class="btn small focusable" data-act="open-gaming-settings" data-key="fse-settings">${ICON.gamepad}${h(t('set.fseOpenSettings'))}</button>` : ''}
              ${S.fsePackage ? '' : `<button class="btn small focusable" data-act="open-releases" data-key="fse-releases">${ICON.link}${h(t('set.fseGetPackage'))}</button>`}
            </div>
          </div>
          ${toggleRow(t('set.fullscreen'), t('set.fullscreenDesc'), st.startFullscreen, 'startFullscreen', 't-fs')}
          ${S.fsePackage ? '' : toggleRow(t('set.login'), t('set.loginDesc'), st.launchAtLogin, 'launchAtLogin', 't-login')}

          <div class="section-title">${h(t('upd.section'))}</div>
          ${updateRows(st)}

          <div class="section-title">${h(t('set.system'))}</div>
          <div class="settings-actions" data-nav-group>
            <button class="btn small focusable" data-act="minimize" data-key="min">${ICON.desktop}${h(t('qm.desktop'))}</button>
            <button class="btn small focusable" data-act="fullscreen" data-key="fs">${h(t('set.toggleFullscreen'))}</button>
            <button class="btn small danger focusable" data-act="quit" data-key="quit">${ICON.exit}${h(t('qm.quit'))}</button>
          </div>
          <div class="about">
            Foyer ${h(S.version)}
            ${st.tmdbKey ? `<br>${h(t('about.tmdb'))}` : ''}
            <br>${h(t('about.steam'))}${st.sgdbKey ? ' ' + h(t('about.sgdb')) : ''}
          </div>
        </div>
      </div>`;
  },
  mount() {
    Backdrop.set(null);
  }
};

function updateRows(st) {
  const up = S.update;
  if (!up || up.status === 'unsupported') {
    return `<div class="fse-card"><p>${h(t('upd.unsupported', { version: S.version }))}</p></div>`;
  }
  const statusText = {
    idle: t('upd.idle'),
    checking: t('upd.checking'),
    uptodate: t('upd.upToDate'),
    available: t('upd.availableShort', { version: up.version }),
    downloading: t('upd.downloading', { n: up.progress || 0 }),
    ready: t('upd.installing'),
    installing: t('upd.installing'),
    error: t('err.update', { message: up.error || '' })
  }[up.status];
  const busy = ['checking', 'downloading', 'installing', 'ready'].includes(up.status);
  return `
    ${valueRow(t('upd.version'), statusText, `Foyer ${S.version}`, busy ? 'noop' : up.status === 'available' ? 'install-update' : 'check-update', 'upd-row')}
    <div class="settings-actions" data-nav-group>
      ${up.status === 'available' ? `<button class="btn small primary focusable" data-act="install-update" data-key="upd-install">${ICON.refresh}${h(t('upd.now'))}</button>` : ''}
      <button class="btn small focusable" data-act="check-update" data-key="upd-check" ${busy ? 'disabled' : ''}>${ICON.search}${h(t('upd.checkNow'))}</button>
    </div>
    ${toggleRow(t('upd.auto'), t('upd.autoDesc'), st.autoCheckUpdates, 'autoCheckUpdates', 't-upd')}`;
}

function controlsHelp() {
  const g = (b, label) => `<span class="hint"><span class="glyph g-${b}">${{ a: 'A', b: 'B', x: 'X', y: 'Y', lb: 'LB', rb: 'RB', lt: 'LT', rt: 'RT', menu: '☰' }[b]}</span><span>${h(label)}</span></span>`;
  return [
    g('a', t('hint.select')),
    g('b', t('hint.back')),
    g('x', t('hint.options')),
    g('y', t('hint.search')),
    g('lb', t('help.prevTab')),
    g('rb', t('help.nextTab')),
    g('lt', t('help.pageUp')),
    g('rt', t('help.pageDown')),
    g('menu', t('hint.menu'))
  ].join('');
}
