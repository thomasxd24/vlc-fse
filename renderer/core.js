'use strict';

/* global Nav, I18N */

const api = window.foyer;
const t = (key, vars) => I18N.t(key, vars);

// ============================================================================ State

const S = {
  settings: {},
  lang: 'en',
  library: { movies: [], shows: [], games: [], continueWatching: [] },
  scanning: false,
  metaStatus: {},
  gameInfoStatus: {},
  nowPlaying: null,
  game: null,
  platform: 'win32',
  systemControls: false,
  fsePackage: false,
  version: ''
};
const idx = { movies: new Map(), shows: new Map(), episodes: new Map(), games: new Map() };

const VIEW_PREFS_DEFAULT = {
  movieSort: 'title',
  movieFilter: 'all',
  showSort: 'title',
  showFilter: 'all',
  gameSort: 'recent',
  gameFilter: 'all'
};
const prefs = loadPrefs();
function loadPrefs() {
  try {
    return { ...VIEW_PREFS_DEFAULT, ...JSON.parse(localStorage.getItem('prefs') || '{}') };
  } catch {
    return { ...VIEW_PREFS_DEFAULT };
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
  idx.games = new Map(S.library.games.map((g) => [g.id, g]));
  idx.episodes = new Map();
  for (const s of S.library.shows) for (const e of s.episodes) idx.episodes.set(e.id, { ...e, show: s });
  document.documentElement.style.setProperty('--scale', S.settings.uiScale || 1);
  document.documentElement.lang = S.lang;
  I18N.setLang(S.lang);
  Sound.enable(S.settings.sounds !== false);
  Nav.setRumble(S.settings.haptics !== false);
  applyMotion();
  renderStatus();
}

// ============================================================================ Motion

const prefersReduced = matchMedia('(prefers-reduced-motion: reduce)');
function reducedMotion() {
  return S.settings.animations === 'reduced' || prefersReduced.matches;
}
function applyMotion() {
  document.body.classList.toggle('reduce-motion', reducedMotion());
}
prefersReduced.addEventListener('change', applyMotion);

/**
 * Run `update` (which re-renders) as an animated view transition. When `from` is given, that element
 * morphs into whatever `to()` returns in the new view (e.g. a card's artwork into the detail poster).
 */
function viewTransition(update, { from = null, to = null } = {}) {
  if (!document.startViewTransition || reducedMotion()) return update();
  if (from) from.style.viewTransitionName = 'morph';
  let target = null;
  const vt = document.startViewTransition(() => {
    if (from) from.style.viewTransitionName = '';
    update();
    target = to ? to() : null;
    if (target) target.style.viewTransitionName = 'morph';
  });
  vt.finished.finally(() => {
    if (target) target.style.viewTransitionName = '';
  });
}

// Fade artwork in once it has loaded (the .art box shows a shimmer until then).
document.addEventListener(
  'load',
  (e) => {
    if (e.target.tagName === 'IMG') e.target.classList.add('loaded');
  },
  true
);
function markLoadedImages(root) {
  root.querySelectorAll('img').forEach((i) => {
    if (i.complete && i.naturalWidth) i.classList.add('loaded');
  });
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
    else if (el.tagName === 'IMG' && el.classList.contains('logo-img')) el.remove();
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
  return hh ? t('fmt.hm', { h: hh, m: mm }) : t('fmt.m', { m: mm });
}

function fmtPlaytime(min) {
  if (!min) return '';
  if (min < 60) return t('game.minutesPlayed', { n: min });
  const hours = min / 60;
  return t('game.hoursPlayed', { n: hours < 10 ? Math.round(hours * 10) / 10 : Math.round(hours) });
}

function fmtAgo(ms) {
  if (!ms) return '';
  const diff = (ms - Date.now()) / 1000;
  const rtf = new Intl.RelativeTimeFormat(S.lang, { numeric: 'auto' });
  const abs = Math.abs(diff);
  if (abs < 90) return t('time.justNow');
  if (abs < 3600) return rtf.format(Math.round(diff / 60) || 0, 'minute');
  if (abs < 86400) return rtf.format(Math.round(diff / 3600), 'hour');
  if (abs < 86400 * 30) return rtf.format(Math.round(diff / 86400), 'day');
  if (abs < 86400 * 365) return rtf.format(Math.round(diff / (86400 * 30)), 'month');
  return rtf.format(Math.round(diff / (86400 * 365)), 'year');
}

function remaining(pr) {
  if (!pr.length) return '';
  const left = Math.max(0, Math.round((pr.length - pr.time) / 60));
  return left ? t('media.minLeft', { n: left }) : t('media.almostDone');
}

function epCode(e) {
  const end = e.episodeEnd ? `–${e.episodeEnd}` : '';
  return e.season === 0 ? t('ep.special', { n: `${e.episode}${end}` }) : t('ep.code', { s: e.season, e: `${e.episode}${end}` });
}

function seasonName(n) {
  return n === 0 ? t('season.specials') : t('season.n', { n });
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

const svg = (body, fill = false) =>
  `<svg class="ico" viewBox="0 0 24 24" ${fill ? 'fill="currentColor"' : 'fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round"'}>${body}</svg>`;
const ICON = {
  play: svg('<path d="M7 4.5v15a1 1 0 0 0 1.5.86l12.5-7.5a1 1 0 0 0 0-1.72L8.5 3.64A1 1 0 0 0 7 4.5z"/>', true),
  restart: svg('<path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/>'),
  check: svg('<path d="M4 12.5l5 5L20 6.5"/>'),
  folder: svg('<path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H10l2 2.5h7.5A1.5 1.5 0 0 1 21 9v9.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18.5z"/>'),
  stop: svg('<rect x="5" y="5" width="14" height="14" rx="2"/>', true),
  plus: svg('<path d="M12 5v14M5 12h14"/>'),
  refresh: svg('<path d="M21 12a9 9 0 1 1-2.6-6.4"/><path d="M21 3v6h-6"/>'),
  star: svg('<path d="M12 3.5l2.6 5.3 5.9.9-4.3 4.1 1 5.8L12 16.9l-5.2 2.7 1-5.8L3.5 9.7l5.9-.9z"/>'),
  starOn: svg('<path d="M12 3.5l2.6 5.3 5.9.9-4.3 4.1 1 5.8L12 16.9l-5.2 2.7 1-5.8L3.5 9.7l5.9-.9z"/>', true),
  edit: svg('<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M13.5 6.5l4 4"/>'),
  hide: svg('<path d="M3 3l18 18"/><path d="M10.6 5.1A10 10 0 0 1 12 5c5 0 9 4.5 10 7-.4 1-1.3 2.4-2.6 3.7M6.6 6.6C4.4 8 2.9 10.1 2 12c1 2.5 5 7 10 7 1.8 0 3.4-.6 4.8-1.4"/><path d="M9.9 9.9a3 3 0 0 0 4.2 4.2"/>'),
  show: svg('<path d="M2 12c1-2.5 5-7 10-7s9 4.5 10 7c-1 2.5-5 7-10 7S3 14.5 2 12z"/><circle cx="12" cy="12" r="3"/>'),
  trash: svg('<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>'),
  more: svg('<circle cx="5" cy="12" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="19" cy="12" r="1.8"/>', true),
  image: svg('<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="8.5" cy="9.5" r="1.8"/><path d="M21 16l-5-5-9 9"/>'),
  link: svg('<path d="M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1"/><path d="M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1"/>'),
  back: svg('<path d="M15 5l-7 7 7 7"/>'),
  menu: svg('<path d="M4 7h16M4 12h16M4 17h16"/>'),
  moon: svg('<path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"/>'),
  power: svg('<path d="M12 3v9"/><path d="M6.3 6.3a8 8 0 1 0 11.4 0"/>'),
  desktop: svg('<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/>'),
  exit: svg('<path d="M14 4h4a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-4"/><path d="M9 8l-4 4 4 4M5 12h11"/>'),
  volume: svg('<path d="M4 9v6h4l5 4V5L8 9z"/><path d="M16.5 8.5a5 5 0 0 1 0 7"/><path d="M19 6a8.5 8.5 0 0 1 0 12"/>'),
  mute: svg('<path d="M4 9v6h4l5 4V5L8 9z"/><path d="M17 9l5 6M22 9l-5 6"/>'),
  sun: svg('<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>'),
  gamepad: svg('<path d="M6.5 7h11a4.5 4.5 0 0 1 4.4 5.5l-1.1 4.6a2.5 2.5 0 0 1-4.3 1.1L14.4 16H9.6l-2.1 2.2a2.5 2.5 0 0 1-4.3-1.1l-1.1-4.6A4.5 4.5 0 0 1 6.5 7z"/><path d="M8 10v3M6.5 11.5h3"/><circle cx="15.5" cy="11" r=".6" fill="currentColor"/><circle cx="17" cy="12.8" r=".6" fill="currentColor"/>'),
  steam: svg('<circle cx="15.5" cy="8.5" r="3.5"/><circle cx="8" cy="16" r="2.5"/><path d="M10.3 15l2.8-3.8M2 11.5l4.1 1.8"/>'),
  search: svg('<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.3-4.3"/>')
};

// ============================================================================ Sound

/** Short synthesized UI sounds (no audio files), quiet enough not to be annoying. */
const Sound = (() => {
  let ctx = null;
  let enabled = true;
  function tone(freq, dur, vol, type = 'sine', slideTo) {
    if (!enabled) return;
    try {
      ctx = ctx || new AudioContext();
      const t0 = ctx.currentTime;
      const o = ctx.createOscillator();
      const g = ctx.createGain();
      o.type = type;
      o.frequency.setValueAtTime(freq, t0);
      if (slideTo) o.frequency.exponentialRampToValueAtTime(slideTo, t0 + dur);
      g.gain.setValueAtTime(vol, t0);
      g.gain.exponentialRampToValueAtTime(0.0001, t0 + dur);
      o.connect(g).connect(ctx.destination);
      o.start(t0);
      o.stop(t0 + dur + 0.02);
    } catch {}
  }
  return {
    enable: (v) => (enabled = v),
    move: () => tone(1500, 0.028, 0.018),
    select: () => tone(700, 0.07, 0.04, 'sine', 1050),
    back: () => tone(560, 0.07, 0.03, 'sine', 360),
    edge: () => tone(170, 0.05, 0.025, 'triangle'),
    open: () => tone(440, 0.09, 0.03, 'sine', 880)
  };
})();

Nav.onMove((kind) => {
  if (kind === 'move') {
    Sound.move();
    Nav.haptic(0.18, 10);
  } else if (kind === 'select') {
    Sound.select();
    Nav.haptic(0.35, 16);
  } else if (kind === 'edge') {
    Sound.edge();
    Nav.haptic(0.5, 22);
  }
});

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
  { name: 'home', key: 'tab.home' },
  { name: 'games', key: 'tab.games' },
  { name: 'movies', key: 'tab.movies' },
  { name: 'shows', key: 'tab.shows' },
  { name: 'search', key: 'tab.search' },
  { name: 'settings', key: 'tab.settings' }
];

const stack = [{ name: 'home' }];
const route = () => stack[stack.length - 1];
const page = $('#page');
const VIEWS = {};

const artOf = (el) => (el && el.closest ? (el.closest('.card') || el).querySelector('.art') : null);

function go(r) {
  saveView();
  Sound.open();
  const from = artOf(document.activeElement);
  document.body.dataset.nav = 'forward';
  viewTransition(
    () => {
      stack.push(r);
      render();
    },
    { from, to: () => page.querySelector('.detail-poster') }
  );
}

function switchTab(name) {
  if (stack.length === 1 && route().name === name) return;
  saveView();
  const before = TABS.findIndex((tab) => tab.name === stack[0].name);
  const after = TABS.findIndex((tab) => tab.name === name);
  document.body.dataset.nav = stack.length > 1 ? 'back' : after > before ? 'right' : 'left';
  stack.length = 0;
  stack.push({ name });
  render({ focusTab: Nav.mode() !== 'touch' });
}

function back() {
  if (closeModal()) return Sound.back();
  if (QuickMenu.isOpen()) return QuickMenu.close();
  if (S.nowPlaying || S.game) return;
  if (stack.length > 1) {
    Sound.back();
    const from = page.querySelector('.detail-poster');
    document.body.dataset.nav = 'back';
    viewTransition(
      () => {
        stack.pop();
        render({ restore: true, animate: true });
      },
      { from, to: () => artOf(document.activeElement) }
    );
    return;
  }
  const inTabs = document.activeElement && document.activeElement.closest('#tabs');
  if (route().name !== 'home') {
    if (!inTabs && Nav.mode() !== 'touch') {
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
  r.focusKey = a && a.dataset && page.contains(a) ? a.dataset.key : r.focusKey;
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

/** Mirror where we are to the main process, so the UI can come back to the same spot after a game. */
let uiStateTimer = null;
function persistUiState() {
  clearTimeout(uiStateTimer);
  uiStateTimer = setTimeout(() => {
    saveView();
    const clean = stack.map(({ name, id, season, query, focusKey, scroll }) => ({ name, id, season, query, focusKey, scroll }));
    api.saveUiState({ stack: clean });
  }, 300);
}

function renderTabs() {
  const root = stack[0].name;
  const topLevel = stack.length === 1;
  $('#tabs').innerHTML = TABS.map(
    (tab) =>
      `<button class="tab focusable ${tab.name === root && topLevel ? 'active' : ''}" ${tab.name === root ? 'data-nav-default' : ''} data-tab="${tab.name}" data-key="tab-${tab.name}">${h(t(tab.key))}</button>`
  ).join('');
  document.body.classList.toggle('detail', !topLevel);
  requestAnimationFrame(moveTabIndicator);
}

/** The pill behind the active tab slides from tab to tab. */
function moveTabIndicator() {
  const ind = $('#tab-ind');
  const active = $('#tabs .tab.active');
  if (!ind) return;
  if (!active) {
    ind.style.opacity = '0';
    return;
  }
  const wrap = $('#tabs').getBoundingClientRect();
  const r = active.getBoundingClientRect();
  ind.style.opacity = '1';
  ind.style.width = `${r.width}px`;
  ind.style.transform = `translateX(${r.left - wrap.left}px)`;
}
window.addEventListener('resize', () => requestAnimationFrame(moveTabIndicator));

function render({ restore = false, focusTab = false, keepFocus = false, animate = !(restore || keepFocus) } = {}) {
  const r = route();
  const active = document.activeElement;
  const prevFocusKey = keepFocus && active ? active.dataset.key : null;
  const prevInTop = keepFocus && active && active.closest('#topbar');
  if (keepFocus) saveView();
  renderTabs();
  document.body.classList.remove('dim-backdrop');
  const view = VIEWS[r.name];
  page.innerHTML = view.render(r);
  // Staggered entrance when arriving on a page; none when refreshing it in place (e.g. new artwork).
  page.dataset.enter = animate ? 'yes' : 'no';
  markLoadedImages(page);
  if (view.mount) view.mount(r);

  let target = null;
  if (keepFocus && prevInTop) target = $(`#topbar [data-key="${CSS.escape(prevFocusKey || '')}"]`);
  else if (restore || keepFocus) target = restoreView(r);
  if (!target && focusTab) target = $(`#tabs [data-tab="${r.name}"]`);
  if (!target) target = page.querySelector('[data-autofocus]') || page.querySelector('.focusable');
  if (!target) target = $(`#tabs [data-tab="${stack[0].name}"]`);
  Nav.focus(target, { scroll: !(restore || keepFocus) });
  Hints.update();
  persistUiState();
}

page.addEventListener('focusin', (e) => {
  const view = VIEWS[route().name];
  if (view.onFocus && e.target.closest) view.onFocus(route(), e.target);
});
document.addEventListener('focusin', () => Hints.update());

// ============================================================================ Dialogs & toasts

const modals = [];

/** Open a dialog. `build(root, close)` fills it; returns a promise resolved with close(value). */
function openModal(html, { wide = false, onMount } = {}) {
  return new Promise((resolve) => {
    const root = document.createElement('div');
    root.className = 'modal';
    root.setAttribute('data-nav-trap', '');
    root.innerHTML = `<div class="dialog ${wide ? 'wide' : ''}" role="dialog" aria-modal="true">${html}</div>`;
    $('#modal-root').appendChild(root);
    const returnFocus = document.activeElement;
    const entry = {
      root,
      close(value = null) {
        root.removeAttribute('data-nav-trap');
        root.classList.add('closing');
        setTimeout(() => root.remove(), reducedMotion() ? 0 : 170);
        modals.splice(modals.indexOf(entry), 1);
        if (returnFocus && returnFocus.isConnected) Nav.focus(returnFocus, { scroll: false });
        else Nav.focusFirst();
        Hints.update();
        resolve(value);
      }
    };
    modals.push(entry);
    root.addEventListener('click', (e) => {
      if (e.target === root) entry.close(null);
    });
    if (onMount) onMount(root, entry.close);
    Nav.focusFirst();
    Hints.update();
  });
}

function closeModal() {
  const top = modals[modals.length - 1];
  if (!top) return false;
  top.close(null);
  return true;
}

/**
 * A list of choices; resolves with the chosen value (or null on Back).
 * @param {{title: string, text?: string, choices: {label: string, value: any, icon?: string, primary?: boolean, danger?: boolean}[]}} opts
 */
function choose({ title, text, choices }) {
  const html = `
    <h2>${h(title)}</h2>
    ${text ? `<p>${h(text)}</p>` : ''}
    <div class="choices" data-nav-group>
      ${choices
        .map(
          (c, i) =>
            `<button class="btn focusable ${c.primary ? 'primary' : ''} ${c.danger ? 'danger' : ''}" data-choice="${i}" ${i === 0 ? 'data-autofocus' : ''}>${c.icon || ''}<span>${h(c.label)}</span></button>`
        )
        .join('')}
    </div>`;
  return openModal(html, {
    onMount(root, close) {
      root.addEventListener('click', (e) => {
        const b = e.target.closest('[data-choice]');
        if (b) close(choices[Number(b.dataset.choice)].value);
      });
    }
  });
}

const KEYS = 'abcdefghijklmnopqrstuvwxyz1234567890'.split('');

/** On-screen keyboard markup (letters, digits, space, delete, clear) for controller typing. */
function keyboardHtml(extraKeys = '') {
  return `
    <div class="keyboard" data-nav-group>
      ${KEYS.map((k) => `<button class="key focusable" data-k="${k}" data-key="k-${k}" ${k === 'a' ? 'data-autofocus' : ''}>${k}</button>`).join('')}
      <button class="key wide focusable" data-k=" " data-key="k-space">${h(t('kb.space'))}</button>
      <button class="key wide focusable" data-k="back" data-key="k-back">${h(t('kb.delete'))}</button>
      ${extraKeys || `<button class="key wide full focusable" data-k="clear" data-key="k-clear">${h(t('kb.clear'))}</button>`}
    </div>`;
}

/** Ask for a line of text with an on-screen keyboard (physical and touch keyboards work too). */
function promptText({ title, value = '', placeholder: ph = '', ok }) {
  const html = `
    <h2>${h(title)}</h2>
    <input class="text-input focusable" id="prompt-input" value="${h(value)}" placeholder="${h(ph)}" spellcheck="false" autocomplete="off">
    ${keyboardHtml(`<button class="key wide focusable" data-k="clear" data-key="k-clear">${h(t('kb.clear'))}</button><button class="key wide ok focusable" data-k="ok" data-key="k-ok">${h(ok || t('common.ok'))}</button>`)}`;
  return openModal(html, {
    onMount(root, close) {
      const input = root.querySelector('#prompt-input');
      root.addEventListener('click', (e) => {
        const k = e.target.closest('[data-k]');
        if (!k) return;
        const key = k.dataset.k;
        if (key === 'ok') return close(input.value);
        if (key === 'back') input.value = input.value.slice(0, -1);
        else if (key === 'clear') input.value = '';
        else input.value += key;
      });
      input.addEventListener('change', () => close(input.value));
      // Typing on a physical keyboard while a key is focused goes straight into the field.
      root.addEventListener('keydown', (e) => {
        if (document.activeElement === input) return;
        if (e.key.length === 1 && !e.ctrlKey && !e.altKey) {
          e.preventDefault();
          e.stopPropagation();
          input.value += e.key;
        } else if (e.key === 'Backspace' && input.value) {
          e.preventDefault();
          e.stopPropagation();
          input.value = input.value.slice(0, -1);
        }
      }, true);
    }
  });
}

function toast(text, kind = 'info') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.textContent = text;
  $('#toasts').appendChild(el);
  setTimeout(() => {
    el.classList.add('out');
    setTimeout(() => el.remove(), 260);
  }, kind === 'error' ? 6500 : 2600);
}

async function confirmExit() {
  const v = await choose({
    title: t('exit.title'),
    choices: [
      { label: t('exit.stay'), value: null },
      { label: t('qm.desktop'), value: 'min', icon: ICON.desktop },
      { label: t('exit.quit'), value: 'quit', danger: true, icon: ICON.exit }
    ]
  });
  if (v === 'min') api.minimize();
  if (v === 'quit') api.quit();
}

/** On a detail page, X / right-click / long-press anywhere opens that item's options. */
function pageOpts() {
  const r = route();
  if (stack.length < 2 || !r.id) return null;
  return { game: `game:${r.id}`, movie: `movie:${r.id}`, show: `show:${r.id}` }[r.name] || null;
}

// ============================================================================ Button hints

/** Bottom bar showing what the controller buttons do right now (hidden for touch and mouse). */
const Hints = (() => {
  const el = $('#hints');
  const glyph = (b) => `<span class="glyph g-${b}">${{ a: 'A', b: 'B', x: 'X', y: 'Y', menu: '☰', lb: 'LB', rb: 'RB', lt: 'LT', rt: 'RT' }[b]}</span>`;
  const key = (k) => `<span class="kcap">${h(k)}</span>`;
  let last = '';
  function update() {
    const m = Nav.mode();
    if (m !== 'pad' && m !== 'keyboard') {
      el.hidden = true;
      return;
    }
    const focused = Nav.current();
    const inModal = modals.length > 0 || QuickMenu.isOpen() || S.nowPlaying || S.game;
    const items = [];
    const pad = m === 'pad';
    const sel = focused && focused.dataset.hint ? t(focused.dataset.hint) : t('hint.select');
    items.push([pad ? glyph('a') : key(t('key.enter')), sel]);
    items.push([pad ? glyph('b') : key(t('key.esc')), t('hint.back')]);
    if (!inModal && ((focused && focused.dataset.opts) || pageOpts())) items.push([pad ? glyph('x') : key(t('key.menu')), t('hint.options')]);
    if (!inModal && pad) items.push([glyph('y'), t('hint.search')]);
    if (!inModal) items.push([pad ? glyph('menu') : key('F1'), t('hint.menu')]);
    const html = items.map(([g, label]) => `<span class="hint">${g}<span>${h(label)}</span></span>`).join('');
    if (html !== last) {
      el.innerHTML = html;
      last = html;
    }
    el.hidden = false;
  }
  Nav.onMode(() => {
    update();
    renderTabHints();
  });
  return { update };
})();

function renderTabHints() {
  document.body.classList.toggle('show-tab-glyphs', Nav.mode() === 'pad');
}

// ============================================================================ Status bar (clock, battery, Wi-Fi)

function renderStatus() {
  const el = $('#status');
  let text = '';
  const up = S.update;
  if (up && up.status === 'downloading' && !$('#update-layer').innerHTML) text = t('upd.downloadingShort', { n: up.progress || 0 });
  else if (S.scanning) text = t('status.scanning');
  else if (S.metaStatus && S.metaStatus.running) text = t('status.artwork');
  else if (S.gameInfoStatus && S.gameInfoStatus.running) text = t('status.gameInfo');
  el.hidden = !text;
  el.title = text;
  el.innerHTML = text ? `<span class="spinner"></span><span class="status-text">${h(text)}</span>` : '';
}

const Status = (() => {
  const clock = $('#clock');
  const batteryEl = $('#battery');
  const wifiEl = $('#wifi');
  let battery = null;

  function tick() {
    clock.textContent = new Date().toLocaleTimeString(S.lang, { hour: '2-digit', minute: '2-digit' });
  }

  function renderBattery() {
    // Desktops report a permanently full "battery"; main tells us whether a real one exists.
    if (!battery || S.hasBattery === false) {
      batteryEl.hidden = true;
      return;
    }
    const pct = Math.round(battery.level * 100);
    const low = pct <= 15 && !battery.charging;
    batteryEl.hidden = false;
    batteryEl.className = `battery ${low ? 'low' : ''} ${battery.charging ? 'charging' : ''}`;
    batteryEl.innerHTML = `<span class="bat"><i style="width:${pct}%"></i></span><span>${pct}%</span>`;
    batteryEl.title = battery.charging ? t('status.charging') : '';
  }

  function renderWifi(w) {
    if (!w) {
      wifiEl.hidden = !navigator.onLine;
      wifiEl.className = 'wifi';
      wifiEl.innerHTML = navigator.onLine ? wifiSvg(4) : '';
      return;
    }
    wifiEl.hidden = false;
    wifiEl.className = `wifi ${w.connected ? '' : 'off'}`;
    const bars = w.connected ? Math.max(1, Math.ceil(w.signal / 25)) : 0;
    wifiEl.innerHTML = wifiSvg(bars);
    wifiEl.title = w.connected ? `${w.ssid} · ${w.signal}%` : t('status.offline');
    Status.wifi = w;
  }

  function wifiSvg(bars) {
    const arc = (r, on) => `<path d="M${12 - r} ${18 - r * 0.55}a${r} ${r} 0 0 1 ${r * 2} 0" opacity="${on ? 1 : 0.28}"/>`;
    return `<svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round">${arc(3, bars >= 2)}${arc(6.5, bars >= 3)}${arc(10, bars >= 4)}<circle cx="12" cy="18.5" r="1.3" fill="currentColor" stroke="none" opacity="${bars >= 1 ? 1 : 0.28}"/></svg>`;
  }

  async function pollWifi() {
    if (document.hidden) return;
    renderWifi(S.platform === 'win32' ? await api.wifi().catch(() => null) : null);
  }

  function start() {
    tick();
    setInterval(tick, 5000);
    if (navigator.getBattery) {
      navigator.getBattery().then((b) => {
        battery = b;
        for (const ev of ['levelchange', 'chargingchange', 'dischargingtimechange']) b.addEventListener(ev, renderBattery);
        renderBattery();
      });
    }
    pollWifi();
    setInterval(pollWifi, 30000);
    window.addEventListener('online', pollWifi);
    window.addEventListener('offline', pollWifi);
  }
  return { start, tick, renderBattery, get battery() { return battery; }, wifi: null };
})();

// ============================================================================ Quick menu (☰)

const QuickMenu = (() => {
  const root = $('#quick-menu');
  let open = false;
  let returnFocus = null;
  let sys = { supported: false };
  const sendTimers = {};

  function sliderRow(key, icon, value, label) {
    const v = value == null ? 0 : value;
    return `
      <div class="qm-slider focusable" data-slider data-sys="${key}" data-key="qm-${key}" tabindex="0" role="slider" aria-valuenow="${v}" aria-label="${h(label)}">
        <span class="qm-ico">${icon}</span>
        <span class="qm-label">${h(label)}</span>
        <span class="qm-bar"><i style="width:${v}%"></i></span>
        <span class="qm-val">${value == null ? '–' : v}</span>
      </div>`;
  }

  function html() {
    const now = new Date();
    const b = Status.battery;
    const w = Status.wifi;
    const showPower = S.platform === 'win32';
    return `
      <div class="qm-panel">
        <div class="qm-head">
          <div class="qm-time">${h(now.toLocaleTimeString(S.lang, { hour: '2-digit', minute: '2-digit' }))}</div>
          <div class="qm-date">${h(now.toLocaleDateString(S.lang, { weekday: 'long', day: 'numeric', month: 'long' }))}</div>
          <div class="qm-facts">
            ${b ? `<span>${Math.round(b.level * 100)}%${b.charging ? ' · ' + h(t('status.charging')) : ''}</span>` : ''}
            ${w && w.connected ? `<span>${h(w.ssid)}</span>` : ''}
          </div>
        </div>
        ${
          sys.supported
            ? `<div class="qm-sliders" data-nav-group>
                ${sliderRow('volume', sys.muted ? ICON.mute : ICON.volume, sys.volume, t('qm.volume'))}
                ${sys.brightness != null ? sliderRow('brightness', ICON.sun, sys.brightness, t('qm.brightness')) : ''}
              </div>`
            : ''
        }
        <div class="qm-grid" data-nav-group>
          <button class="qm-btn focusable" data-qm="desktop" data-key="qm-desktop" data-autofocus>${ICON.desktop}<span>${h(t('qm.desktop'))}</span></button>
          ${showPower ? `<button class="qm-btn focusable" data-qm="sleep" data-key="qm-sleep">${ICON.moon}<span>${h(t('qm.sleep'))}</span></button>` : ''}
          ${showPower ? `<button class="qm-btn focusable" data-qm="restart" data-key="qm-restart">${ICON.restart}<span>${h(t('qm.restart'))}</span></button>` : ''}
          ${showPower ? `<button class="qm-btn focusable" data-qm="shutdown" data-key="qm-shutdown">${ICON.power}<span>${h(t('qm.shutdown'))}</span></button>` : ''}
          <button class="qm-btn focusable" data-qm="settings" data-key="qm-settings">${ICON.gamepad}<span>${h(t('tab.settings'))}</span></button>
          <button class="qm-btn danger focusable" data-qm="quit" data-key="qm-quit">${ICON.exit}<span>${h(t('qm.quit'))}</span></button>
        </div>
      </div>`;
  }

  function paint() {
    const key = document.activeElement && document.activeElement.dataset.key;
    root.innerHTML = html();
    const el = key && root.querySelector(`[data-key="${CSS.escape(key)}"]`);
    Nav.focus(el || root.querySelector('[data-autofocus]'), { scroll: false });
  }

  function setValue(key, v) {
    v = Math.max(key === 'brightness' ? 1 : 0, Math.min(100, Math.round(v)));
    sys[key] = v;
    if (key === 'volume' && v > 0) sys.muted = false;
    const row = root.querySelector(`[data-sys="${key}"]`);
    if (row) {
      row.querySelector('.qm-bar i').style.width = `${v}%`;
      row.querySelector('.qm-val').textContent = v;
      row.setAttribute('aria-valuenow', v);
      if (key === 'volume') row.querySelector('.qm-ico').innerHTML = ICON.volume;
    }
    // Coalesce rapid changes (holding the stick) into one call every 120ms.
    clearTimeout(sendTimers[key]);
    sendTimers[key] = setTimeout(() => api.systemSet({ key, value: v }), 120);
  }

  root.addEventListener('nudge', (e) => {
    const row = e.target.closest('[data-sys]');
    if (!row) return;
    const key = row.dataset.sys;
    setValue(key, (sys[key] || 0) + e.detail * (key === 'volume' ? 4 : 10));
    Nav.haptic(0.15, 8);
    Sound.move();
  });
  // Touch / mouse: tap or drag along a bar.
  root.addEventListener('pointerdown', (e) => {
    const bar = e.target.closest('.qm-bar');
    if (!bar) return;
    const key = bar.closest('[data-sys]').dataset.sys;
    const set = (ev) => {
      const r = bar.getBoundingClientRect();
      setValue(key, ((ev.clientX - r.left) / r.width) * 100);
    };
    set(e);
    bar.setPointerCapture(e.pointerId);
    bar.onpointermove = set;
    bar.onpointerup = () => (bar.onpointermove = null);
  });
  root.addEventListener('click', async (e) => {
    const ico = e.target.closest('.qm-ico');
    if (ico && ico.closest('[data-sys="volume"]')) {
      sys.muted = !sys.muted;
      api.systemSet({ key: 'muted', value: sys.muted });
      ico.innerHTML = sys.muted ? ICON.mute : ICON.volume;
      return;
    }
    const b = e.target.closest('[data-qm]');
    if (!b) return;
    const action = b.dataset.qm;
    if (action === 'settings') {
      close();
      switchTab('settings');
      return;
    }
    if (action === 'desktop') {
      close();
      return api.power('desktop');
    }
    if (action === 'quit') {
      close();
      return api.quit();
    }
    const labels = { sleep: t('qm.sleep'), restart: t('qm.restart'), shutdown: t('qm.shutdown') };
    const ok = await choose({
      title: t('qm.confirm', { action: labels[action] }),
      choices: [
        { label: labels[action], value: true, primary: true },
        { label: t('common.cancel'), value: false }
      ]
    });
    if (ok) {
      close();
      api.power(action);
    }
  });

  async function show() {
    if (open) return close();
    open = true;
    returnFocus = document.activeElement;
    root.hidden = false;
    root.setAttribute('data-nav-trap', '');
    Sound.open();
    paint();
    Hints.update();
    if (S.systemControls) {
      sys = await api.systemGet().catch(() => ({ supported: false }));
      if (open) paint();
    }
  }

  function close() {
    if (!open) return;
    open = false;
    root.removeAttribute('data-nav-trap');
    const finish = () => {
      if (open) return;
      root.hidden = true;
      root.classList.remove('closing');
      root.innerHTML = '';
    };
    if (reducedMotion()) finish();
    else {
      root.classList.add('closing');
      setTimeout(finish, 200);
    }
    if (returnFocus && returnFocus.isConnected) Nav.focus(returnFocus, { scroll: false });
    else Nav.focusFirst();
    Hints.update();
  }

  root.addEventListener('click', (e) => {
    if (e.target === root) close();
  });

  return { show, close, isOpen: () => open };
})();

// ============================================================================ Overlays: VLC playback & running game

function renderNowPlaying() {
  const box = $('#now-playing');
  const np = S.nowPlaying;
  if (!np) {
    if (!box.hidden) {
      box.hidden = true;
      box.removeAttribute('data-nav-trap');
      box.innerHTML = '';
      Nav.focus(page.querySelector('[data-autofocus]') || page.querySelector('.focusable'), { scroll: false });
      Hints.update();
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
        <div class="kicker">${h(t('np.playingInVlc'))}</div>
        <h1 id="np-title"></h1>
        <div class="np-file" id="np-file"></div>
        <div class="bar"><i id="np-bar"></i></div>
        <div class="time" id="np-time"></div>
        <div class="actions">
          <button class="btn danger focusable" data-np="stop" data-autofocus>${ICON.stop}${h(t('np.stop'))}</button>
        </div>
      </div>`;
    box.querySelector('[data-np="stop"]').addEventListener('click', () => api.stop());
    Nav.focusFirst();
    Hints.update();
  }
  $('#np-title').textContent = np.title;
  $('#np-file').textContent = file;
  $('#np-bar').style.width = `${p}%`;
  $('#np-time').textContent = np.length ? `${fmtTime(np.time)} / ${fmtTime(np.length)}` : t('np.starting');
}

function renderGameLayer() {
  const box = $('#game-layer');
  const g = S.game;
  if (!g) {
    if (!box.hidden) {
      box.hidden = true;
      box.removeAttribute('data-nav-trap');
      box.innerHTML = '';
      Nav.focus(page.querySelector('[data-autofocus]') || page.querySelector('.focusable'), { scroll: false });
      Hints.update();
    }
    return;
  }
  const game = idx.games.get(g.id);
  const art = game && (game.hero || game.header || game.poster);
  const logo = game && game.logo;
  const phaseText = g.phase === 'launching' ? t('gl.launching') : g.phase === 'untracked' ? t('gl.untracked') : t('gl.running');
  box.hidden = false;
  box.setAttribute('data-nav-trap', '');
  box.innerHTML = `
    <div class="gl-bg" ${art ? `style="background-image:url('${h(art)}')"` : ''}></div>
    <div class="gl">
      ${logo ? `<img class="gl-logo logo-img" src="${h(logo)}" alt="">` : `<h1>${h(g.title)}</h1>`}
      <div class="gl-phase">${g.phase === 'launching' ? '<span class="spinner"></span>' : ''}${h(phaseText)}</div>
      <div class="actions" data-nav-group>
        ${g.phase !== 'launching' ? `<button class="btn primary focusable" data-gl="back" data-autofocus>${ICON.play}${h(t('gl.backToGame'))}</button>` : ''}
        <button class="btn focusable" data-gl="done" ${g.phase === 'launching' ? 'data-autofocus' : ''}>${ICON.check}${h(g.phase === 'launching' ? t('gl.cancel') : t('gl.done'))}</button>
      </div>
      <p class="gl-note">${h(t('gl.note'))}</p>
    </div>`;
  box.querySelector('[data-gl="done"]').addEventListener('click', () => api.endGame());
  const backBtn = box.querySelector('[data-gl="back"]');
  if (backBtn) backBtn.addEventListener('click', () => api.backToGame());
  Nav.focusFirst();
  Hints.update();
}
