'use strict';

/* global api, S, idx, prefs, savePrefs, applyState, t, h, img, fmtTime, ICON, Nav, Sound, Backdrop, $, page, stack, route, go,
   switchTab, back, render, TABS, choose, openModal, closeModal, modals, promptText, toast, Hints, Status, QuickMenu,
   renderStatus, renderNowPlaying, renderGameLayer, renderResults, selectSeason, paintHero */

// ============================================================================ Playback

async function playMovie(id, mode) {
  const m = idx.movies.get(id);
  if (!m) return;
  let resume = mode === 'resume';
  if (!mode && m.progress.resumable) {
    const v = await choose({
      title: m.title,
      choices: [
        { label: t('media.resumeFrom', { time: fmtTime(m.progress.time) }), value: 'resume', primary: true, icon: ICON.play },
        { label: t('media.playFromStart'), value: 'start', icon: ICON.restart }
      ]
    });
    if (!v) return;
    resume = v === 'resume';
  }
  startMedia({ kind: 'movie', id, resume });
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
        { label: t('media.resumeFrom', { time: fmtTime(e.progress.time) }), value: 'resume', primary: true, icon: ICON.play },
        { label: t('media.playFromStart'), value: 'start', icon: ICON.restart }
      ]
    });
    if (!v) return;
    resume = v === 'resume';
  }
  startMedia({ kind: 'episode', showId, id, resume });
}

async function startMedia(req) {
  const r = await api.play(req);
  if (!r.ok && r.errorKey) toast(t(r.errorKey, r.vars), 'error');
}

async function playGame(id) {
  const r = await api.playGame(id);
  if (!r.ok && r.errorKey) toast(t(r.errorKey, r.vars), 'error');
}

// ============================================================================ Options menu (X / long-press / right-click)

async function openOptions(spec) {
  if (!spec) return;
  const [kind, a, b] = spec.split(':');
  const choices = [];
  let title = '';
  const fav = (item) => ({ label: item.favorite ? t('opt.unfavorite') : t('opt.favorite'), value: 'fav', icon: item.favorite ? ICON.starOn : ICON.star });
  const hide = (item) => ({ label: item.hidden ? t('opt.unhide') : t('opt.hide'), value: 'hide', icon: item.hidden ? ICON.show : ICON.hide });

  if (kind === 'game') {
    const g = idx.games.get(a);
    if (!g) return;
    title = g.title;
    choices.push({ label: t('game.play'), value: 'play', icon: ICON.play, primary: true }, fav(g), { label: t('game.edit'), value: 'edit', icon: ICON.edit });
    if (route().name !== 'game') choices.push({ label: t('opt.details'), value: 'open', icon: ICON.more });
    choices.push({ label: t('opt.showFolder'), value: 'folder', icon: ICON.folder }, hide(g));
    if (g.source === 'manual') choices.push({ label: t('opt.remove'), value: 'remove', icon: ICON.trash, danger: true });
    const v = await choose({ title, choices });
    if (v === 'play') playGame(g.id);
    else if (v === 'fav') setPref(g.id, 'favorites', !g.favorite);
    else if (v === 'edit') editGame(g.id);
    else if (v === 'open') go({ name: 'game', id: g.id });
    else if (v === 'folder') api.showGameFolder(g.id);
    else if (v === 'hide') setPref(g.id, 'hidden', !g.hidden);
    else if (v === 'remove') removeGame(g);
  } else if (kind === 'movie') {
    const m = idx.movies.get(a);
    if (!m) return;
    choices.push(
      { label: m.progress.resumable ? t('media.resumeFrom', { time: fmtTime(m.progress.time) }) : t('media.play'), value: 'play', icon: ICON.play, primary: true },
      { label: m.progress.watched ? t('opt.markUnwatched') : t('opt.markWatched'), value: 'watched', icon: ICON.check },
      fav(m),
      hide(m),
      { label: t('opt.showFile'), value: 'file', icon: ICON.folder }
    );
    const v = await choose({ title: m.title, choices });
    if (v === 'play') playMovie(m.id, m.progress.resumable ? 'resume' : 'start');
    else if (v === 'watched') api.setWatched({ kind: 'movie', id: m.id, watched: !m.progress.watched });
    else if (v === 'fav') setPref(m.id, 'favorites', !m.favorite);
    else if (v === 'hide') setPref(m.id, 'hidden', !m.hidden);
    else if (v === 'file') api.showInFolder(m.path);
  } else if (kind === 'show') {
    const s = idx.shows.get(a);
    if (!s) return;
    const all = s.watchedCount === s.episodes.length;
    choices.push({ label: all ? t('opt.showUnwatched') : t('opt.showWatched'), value: 'watched', icon: ICON.check }, fav(s), hide(s));
    if (route().name !== 'show') choices.unshift({ label: t('opt.details'), value: 'open', icon: ICON.more, primary: true });
    const v = await choose({ title: s.title, choices });
    if (v === 'open') go({ name: 'show', id: s.id });
    else if (v === 'watched') api.setWatched({ kind: 'show', id: s.id, watched: !all });
    else if (v === 'fav') setPref(s.id, 'favorites', !s.favorite);
    else if (v === 'hide') setPref(s.id, 'hidden', !s.hidden);
  } else if (kind === 'episode') {
    const e = idx.episodes.get(b);
    if (!e) return;
    choices.push(
      { label: e.progress.resumable ? t('media.resumeFrom', { time: fmtTime(e.progress.time) }) : t('media.play'), value: 'play', icon: ICON.play, primary: true },
      { label: e.progress.watched ? t('opt.markUnwatched') : t('opt.markWatched'), value: 'watched', icon: ICON.check }
    );
    if (route().name !== 'show') choices.push({ label: t('opt.goToShow'), value: 'show', icon: ICON.more });
    const v = await choose({ title: `${e.show.title} · ${epCode(e)}`, text: e.title || '', choices });
    if (v === 'play') playEpisode(a, e.id, e.progress.resumable ? 'resume' : 'start');
    else if (v === 'watched') api.setWatched({ kind: 'episode', showId: a, id: e.id, watched: !e.progress.watched });
    else if (v === 'show') go({ name: 'show', id: a, season: e.season });
  } else if (kind === 'season') {
    const s = idx.shows.get(a);
    const n = Number(b);
    if (!s) return;
    const done = s.episodes.filter((e) => e.season === n).every((e) => e.progress.watched);
    const v = await choose({ title: `${s.title} · ${seasonName(n)}`, choices: [{ label: done ? t('opt.markUnwatched') : t('opt.markWatched'), value: 'watched', icon: ICON.check }] });
    if (v === 'watched') api.setWatched({ kind: 'season', showId: s.id, id: n, watched: !done });
  }
}

function setPref(id, key, value) {
  api.setPref({ id, key, value });
  const msg = key === 'favorites' ? (value ? 'toast.favAdded' : 'toast.favRemoved') : value ? 'toast.hidden' : 'toast.unhidden';
  toast(t(msg));
}

// ============================================================================ Game editing

async function editGame(id) {
  for (;;) {
    const g = idx.games.get(id);
    if (!g) return;
    const choices = [
      { label: t('edit.rename'), value: 'rename', icon: ICON.edit },
      { label: t('edit.matchSteam'), value: 'match', icon: ICON.steam },
      { label: t('edit.cover'), value: 'grids', icon: ICON.image },
      { label: t('edit.background'), value: 'heroes', icon: ICON.image },
      { label: t('edit.logo'), value: 'logos', icon: ICON.image }
    ];
    if (g.source === 'manual') choices.splice(1, 0, { label: t('edit.args'), value: 'args', icon: ICON.gamepad });
    choices.push({ label: t('common.done'), value: null, primary: true });
    const v = await choose({ title: t('edit.title', { name: g.title }), text: g.steamAppId ? t('edit.matched', { id: g.steamAppId }) : t('edit.unmatched'), choices });
    if (!v) return;
    if (v === 'rename') {
      const name = await promptText({ title: t('edit.rename'), value: g.title });
      if (name !== null && name.trim()) await api.editGame({ id, patch: { title: name.trim() } });
    } else if (v === 'args') {
      const args = await promptText({ title: t('edit.args'), value: g.args || '', placeholder: '-fullscreen' });
      if (args !== null) await api.editGame({ id, patch: { args } });
    } else if (v === 'match') {
      await matchSteam(g);
    } else {
      await pickArtwork(g, v);
    }
    await refreshState();
  }
}

async function matchSteam(g) {
  const term = await promptText({ title: t('edit.searchSteam'), value: g.title, ok: t('common.search') });
  if (term === null || !term.trim()) return;
  toast(t('edit.searching'));
  const hits = await api.searchSteam(term.trim());
  const choices = hits.slice(0, 8).map((x) => ({ label: `${x.title}  ·  #${x.steamAppId}`, value: x.steamAppId }));
  choices.push({ label: t('edit.noMatch'), value: 'none' });
  const v = await choose({ title: t('edit.pickMatch'), text: hits.length ? '' : t('search.none'), choices });
  if (!v) return;
  await api.editGame({ id: g.id, patch: { steamAppId: v === 'none' ? null : v } });
  toast(t('edit.updating'));
}

async function pickArtwork(g, kind) {
  if (!S.settings.sgdbKey) {
    const v = await choose({
      title: t('edit.needSgdb'),
      text: t('edit.needSgdbText'),
      choices: [
        { label: t('edit.addKey'), value: 'key', primary: true },
        { label: t('edit.getKey'), value: 'site' },
        { label: t('common.cancel'), value: null }
      ]
    });
    if (v === 'key') await editKey('sgdbKey');
    if (v === 'site') api.openExternal('https://www.steamgriddb.com/profile/preferences/api');
    return;
  }
  toast(t('edit.searching'));
  const imgs = await api.sgdbImages({ kind, id: g.id });
  if (!imgs.length) return toast(t('edit.noImages'), 'error');
  const shape = kind === 'grids' ? 'tall' : kind === 'heroes' ? 'wide' : 'logo';
  const url = await openModal(
    `<h2>${h(t(kind === 'grids' ? 'edit.cover' : kind === 'heroes' ? 'edit.background' : 'edit.logo'))}</h2>
     <div class="picker ${shape}" data-nav-group>
       ${imgs.map((i, n) => `<button class="card pick focusable" data-url="${h(i.url)}" ${n === 0 ? 'data-autofocus' : ''}><div class="art"><img src="${h(i.thumb)}" alt=""></div></button>`).join('')}
     </div>`,
    {
      wide: true,
      onMount(root, close) {
        root.addEventListener('click', (e) => {
          const b = e.target.closest('[data-url]');
          if (b) close(b.dataset.url);
        });
      }
    }
  );
  if (!url) return;
  const key = { grids: 'poster', heroes: 'hero', logos: 'logo' }[kind];
  const r = await api.setGameArt({ id: g.id, kind: key, url });
  toast(r.ok ? t('edit.saved') : t('edit.downloadFailed'), r.ok ? 'info' : 'error');
}

async function removeGame(g) {
  const ok = await choose({
    title: t('opt.removeConfirm', { name: g.title }),
    text: t('opt.removeText'),
    choices: [
      { label: t('opt.remove'), value: true, danger: true, icon: ICON.trash },
      { label: t('common.cancel'), value: false }
    ]
  });
  if (!ok) return;
  await api.removeGame(g.id);
  if (route().name === 'game') back();
}

async function addGame() {
  const r = await api.addGame();
  if (!r.ok) {
    if (r.errorKey) toast(t(r.errorKey), 'error');
    return;
  }
  await refreshState();
  toast(t('games.added'));
  go({ name: 'game', id: r.id });
}

async function showScreenshot(gameId, index) {
  const g = idx.games.get(gameId);
  if (!g) return;
  let i = index;
  const paint = async (root) => {
    const s = g.screenshots[i];
    root.querySelector('.lightbox-img').src = s.thumb;
    root.querySelector('.lightbox-count').textContent = `${i + 1} / ${g.screenshots.length}`;
    const full = await api.screenshot(s.full);
    if (full && g.screenshots[i] === s) root.querySelector('.lightbox-img').src = full;
  };
  await openModal(
    `<div class="lightbox">
       <img class="lightbox-img" alt="">
       <div class="lightbox-bar" data-nav-group>
         <button class="btn small focusable" data-dir="-1">${ICON.back}</button>
         <span class="lightbox-count"></span>
         <button class="btn small focusable flip" data-dir="1" data-autofocus>${ICON.back}</button>
       </div>
     </div>`,
    {
      wide: true,
      onMount(root) {
        root.classList.add('lightbox-modal');
        paint(root);
        const step = (d) => {
          i = (i + d + g.screenshots.length) % g.screenshots.length;
          paint(root);
        };
        root.addEventListener('click', (e) => {
          const b = e.target.closest('[data-dir]');
          if (b) step(Number(b.dataset.dir));
        });
        // Swipe left/right on touch.
        let x0 = null;
        root.addEventListener('touchstart', (e) => (x0 = e.touches[0].clientX), { passive: true });
        root.addEventListener('touchend', (e) => {
          if (x0 === null) return;
          const dx = e.changedTouches[0].clientX - x0;
          if (Math.abs(dx) > 60) step(dx < 0 ? 1 : -1);
          x0 = null;
        });
      }
    }
  );
}

// ============================================================================ Settings actions

async function save(patch) {
  S.settings = await api.saveSettings(patch);
  document.documentElement.style.setProperty('--scale', S.settings.uiScale || 1);
  Sound.enable(S.settings.sounds !== false);
  Nav.setRumble(S.settings.haptics !== false);
}

async function editKey(setting) {
  const v = await promptText({ title: t(setting === 'sgdbKey' ? 'set.sgdb' : 'set.tmdb'), value: S.settings[setting] || '', placeholder: t('set.pasteKey') });
  if (v === null) return;
  await save({ [setting]: v.trim() });
  toast(t('toast.saved'));
}

async function addLibrary(type) {
  const dir = await api.pickFolder();
  if (!dir) return;
  const libs = [...(S.settings.libraries || [])];
  if (libs.some((l) => l.path === dir)) return toast(t('lib.already'));
  libs.push({ path: dir, type });
  await save({ libraries: libs });
  toast(t('lib.added', { path: dir }));
  render({ keepFocus: true });
}

async function refreshState() {
  applyState(await api.getState());
  render({ keepFocus: true });
}

const ACTIONS = {
  tab: (d) => switchTab(d.tab),
  'open-movie': (d) => go({ name: 'movie', id: d.id }),
  'open-show': (d) => go({ name: 'show', id: d.id }),
  'open-game': (d) => go({ name: 'game', id: d.id }),
  'play-movie': (d) => playMovie(d.id, d.mode),
  'play-episode': (d) => playEpisode(d.show, d.id, d.mode),
  'play-game': (d) => playGame(d.id),
  options: (d) => openOptions(d.opts),
  'edit-game': (d) => editGame(d.id),
  'add-game': () => addGame(),
  screenshot: (d) => showScreenshot(d.id, Number(d.index)),
  'see-all': (d) => {
    if (d.pref) {
      prefs[d.pref] = d.value;
      savePrefs();
    }
    switchTab(d.tab);
  },
  'toggle-fav': (d) => {
    const item = idx.games.get(d.id) || idx.movies.get(d.id) || idx.shows.get(d.id);
    if (item) setPref(d.id, 'favorites', !item.favorite);
  },
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
    toast(watched ? t('toast.markedWatched') : t('toast.markedUnwatched'));
  },
  pref: (d) => {
    prefs[d.pref] = d.value;
    savePrefs();
    render({ keepFocus: true });
  },
  'add-library': (d) => addLibrary(d.type),
  rescan: () => api.rescan(),
  'library-menu': async (d) => {
    const libs = [...S.settings.libraries];
    const lib = libs[Number(d.index)];
    const v = await choose({
      title: lib.path,
      text: lib.type === 'tv' ? t('lib.tvFolder') : t('lib.moviesFolder'),
      choices: [
        { label: lib.type === 'tv' ? t('lib.treatAsMovies') : t('lib.treatAsTv'), value: 'type' },
        { label: t('lib.remove'), value: 'remove', danger: true, icon: ICON.trash },
        { label: t('common.cancel'), value: null }
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
      title: t('set.vlc'),
      text: S.settings.vlcPath || (S.vlcFound ? t('set.autoPath', { path: S.vlcFound }) : t('welcome.vlcMissing')),
      choices: [
        { label: t('set.vlcBrowse'), value: 'browse', primary: true },
        { label: t('set.vlcDetect'), value: 'auto' },
        { label: t('common.cancel'), value: null }
      ]
    });
    if (v === 'browse') {
      const p = await api.pickVlc();
      if (p) await save({ vlcPath: p });
    } else if (v === 'auto') {
      await save({ vlcPath: '' });
      S.vlcFound = await api.detectVlc();
      toast(S.vlcFound ? t('set.autoPath', { path: S.vlcFound }) : t('set.notFound'), S.vlcFound ? 'info' : 'error');
    }
    render({ keepFocus: true });
  },
  'pick-steam': async () => {
    const v = await choose({
      title: t('set.steamPath'),
      text: S.settings.steamPath || t('set.auto'),
      choices: [
        { label: t('set.browse'), value: 'browse', primary: true },
        { label: t('set.auto'), value: 'auto' },
        { label: t('common.cancel'), value: null }
      ]
    });
    if (v === 'browse') {
      const p = await api.pickFolder();
      if (p) await save({ steamPath: p });
    } else if (v === 'auto') await save({ steamPath: '' });
    render({ keepFocus: true });
  },
  'toggle-setting': async (d) => {
    await save({ [d.setting]: !S.settings[d.setting] });
    render({ keepFocus: true });
  },
  'edit-key': async (d) => {
    await editKey(d.setting);
    render({ keepFocus: true });
  },
  'edit-text': async (d) => {
    const v = await promptText({ title: t('set.vlcArgs'), value: S.settings[d.setting] || '', placeholder: '--sub-language=fre' });
    if (v === null) return;
    await save({ [d.setting]: v.trim() });
    render({ keepFocus: true });
  },
  'choose-language': async () => {
    const v = await choose({
      title: t('set.language'),
      choices: [
        { label: t('set.langAuto'), value: 'auto' },
        { label: 'English', value: 'en' },
        { label: 'Français', value: 'fr' }
      ]
    });
    if (!v) return;
    await save({ uiLanguage: v });
    await refreshState();
  },
  'cycle-animations': async () => {
    await save({ animations: S.settings.animations === 'reduced' ? 'full' : 'reduced' });
    applyMotion();
    render({ keepFocus: true });
  },
  'cycle-scale': async () => {
    const steps = [0.9, 1, 1.15, 1.3, 1.5];
    const cur = S.settings.uiScale || 1;
    const next = steps[(steps.findIndex((s) => Math.abs(s - cur) < 0.01) + 1) % steps.length];
    await save({ uiScale: next });
    render({ keepFocus: true });
  },
  'clear-metadata': async () => {
    await api.clearMetadata();
    toast(t('set.refreshing'));
  },
  'check-update': async () => {
    toast(t('upd.checking'));
    const st = await api.checkUpdate();
    if (!st) return;
    S.update = st;
    if (st.status === 'available') maybePromptUpdate(true);
    else if (st.status === 'uptodate') toast(t('upd.upToDate'));
    else if (st.status === 'error') toast(t('err.update', { message: st.error }), 'error');
    render({ keepFocus: true });
  },
  'install-update': () => startUpdate(),
  'open-gaming-settings': () => api.openExternal('ms-settings:gaming-gamebar'),
  'open-releases': () => api.openExternal('https://github.com/thomasxd24/vlc-fse/releases/latest'),
  minimize: () => api.minimize(),
  fullscreen: () => api.toggleFullscreen(),
  quit: () => api.quit()
};

document.addEventListener('click', (e) => {
  const tab = e.target.closest('[data-tab]');
  if (tab && tab.closest('#tabs')) {
    if (tab.dataset.tab === stack[0].name && stack.length === 1) {
      // Selecting the current tab drops into its content.
      Nav.focus(page.querySelector('[data-autofocus]') || page.querySelector('.focusable'));
      return;
    }
    switchTab(tab.dataset.tab);
    return;
  }
  if (e.target.closest('#back-btn')) return back();
  if (e.target.closest('#menu-btn')) return QuickMenu.show();
  const el = e.target.closest('[data-act]');
  if (!el || !page.contains(el)) return;
  const fn = ACTIONS[el.dataset.act];
  if (fn) fn(el.dataset);
});

// ============================================================================ Global shortcuts

function busy() {
  return modals.length > 0 || S.nowPlaying || S.game;
}

document.addEventListener(
  'keydown',
  (e) => {
    const typing = document.activeElement && document.activeElement.tagName === 'INPUT';
    if (e.key === 'F11') {
      e.preventDefault();
      api.toggleFullscreen();
    } else if (e.key === 'F5') {
      e.preventDefault();
      api.rescan();
    } else if (e.key === 'F1') {
      e.preventDefault();
      if (!busy()) QuickMenu.show();
    } else if (!typing && !busy() && !QuickMenu.isOpen()) {
      if (e.key === '/' || (e.ctrlKey && e.key.toLowerCase() === 'f')) {
        e.preventDefault();
        if (route().name !== 'search') switchTab('search');
        Nav.focus($('#q'));
      } else if (route().name === 'search' && e.key.length === 1 && /[\p{L}\p{N} ]/u.test(e.key) && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        searchKey(e.key.toLowerCase());
      } else if (route().name === 'search' && e.key === 'Backspace' && $('#q').value) {
        e.preventDefault();
        e.stopImmediatePropagation();
        searchKey('back');
      }
    }
  },
  true
);

function searchKey(k) {
  const r = route();
  if (r.name !== 'search') return;
  if (k === 'back') r.query = r.query.slice(0, -1);
  else if (k === 'clear') r.query = '';
  else r.query += k;
  $('#q').value = r.query;
  renderResults(r);
}

// On-screen keyboard on the Search page.
page.addEventListener('click', (e) => {
  const k = e.target.closest('.search [data-k]');
  if (k) searchKey(k.dataset.k);
});

Nav.onBack(() => {
  back();
});

Nav.onAction((btn) => {
  if (btn === 'menu' || btn === 'view') {
    if (S.nowPlaying || S.game || modals.length) return;
    return QuickMenu.show();
  }
  if (modals.length || S.nowPlaying || S.game || QuickMenu.isOpen()) return;
  if (btn === 'x') {
    const el = Nav.current();
    const spec = (el && el.dataset.opts) || pageOpts();
    if (spec) openOptions(spec);
  } else if (btn === 'y') {
    switchTab('search');
  } else if (btn === 'lb' || btn === 'rb') {
    const i = TABS.findIndex((tab) => tab.name === stack[0].name);
    const next = TABS[(i + (btn === 'rb' ? 1 : TABS.length - 1)) % TABS.length];
    Sound.move();
    Nav.haptic(0.3, 14);
    switchTab(next.name);
  }
});

// ============================================================================ Updates

const promptedVersions = new Set();
let updateLayerOpen = false;

function updateNotes(md) {
  // First few meaningful lines of the release notes, without Markdown noise.
  return String(md || '')
    .split(/\r?\n/)
    .map((l) => l.replace(/^#+\s*/, '').replace(/^[-*]\s+/, '• ').replace(/\*\*|__|`/g, '').replace(/\[([^\]]+)\]\([^)]+\)/g, '$1').replace(/\s+by @[\w-]+( in https?:\/\/\S+)?/g, '').replace(/https?:\/\/\S+/g, '').trim())
    .filter((l) => l && !/^(full changelog|what's changed)/i.test(l))
    .slice(0, 6)
    .join('\n');
}

/** "Foyer x.y.z is available": asked once per version per session, only when nothing else is going on. */
async function maybePromptUpdate(force = false) {
  const up = S.update;
  if (!up || up.status !== 'available') return;
  if (!force && (promptedVersions.has(up.version) || up.version === S.settings.skippedVersion)) return;
  if (modals.length || S.game || S.nowPlaying || QuickMenu.isOpen()) {
    setTimeout(() => maybePromptUpdate(force), 5000);
    return;
  }
  promptedVersions.add(up.version);
  const size = up.size ? ` (${Math.round(up.size / 1048576)} MB)` : '';
  const v = await choose({
    title: t('upd.availableTitle', { version: up.version }),
    text: [t('upd.availableText', { current: up.current }) + size, updateNotes(up.notes)].filter(Boolean).join('\n\n'),
    choices: [
      { label: t('upd.now'), value: 'now', primary: true, icon: ICON.refresh },
      { label: t('upd.later'), value: 'later' },
      { label: t('upd.skip'), value: 'skip' }
    ]
  });
  if (v === 'now') startUpdate();
  else if (v === 'skip') api.skipUpdate(up.version).then(refreshState);
}

async function startUpdate() {
  updateLayerOpen = true;
  renderUpdateLayer();
  const r = await api.installUpdate();
  if (!r.ok) {
    updateLayerOpen = false;
    renderUpdateLayer();
    if (r.errorKey) toast(t(r.errorKey, r.vars), 'error');
  }
}

function renderUpdateLayer() {
  const box = $('#update-layer');
  const up = S.update || {};
  const show = updateLayerOpen && ['downloading', 'ready', 'installing', 'checking', 'available'].includes(up.status);
  if (!show) {
    if (!box.hidden) {
      box.hidden = true;
      box.removeAttribute('data-nav-trap');
      box.innerHTML = '';
      Nav.focus(page.querySelector('[data-autofocus]') || page.querySelector('.focusable'), { scroll: false });
    }
    renderStatus();
    return;
  }
  const installing = up.status === 'installing' || up.status === 'ready';
  const pctDone = installing ? 100 : up.progress || 0;
  if (box.hidden) {
    box.hidden = false;
    box.setAttribute('data-nav-trap', '');
    box.innerHTML = `
      <div class="upd">
        <div class="brand-mark big"></div>
        <h1 id="upd-title"></h1>
        <div class="bar"><i id="upd-bar"></i></div>
        <div class="upd-status" id="upd-status"></div>
        <div class="actions"><button class="btn focusable" data-upd="hide" data-autofocus>${h(t('upd.background'))}</button></div>
      </div>`;
    box.querySelector('[data-upd="hide"]').addEventListener('click', () => {
      updateLayerOpen = false;
      renderUpdateLayer();
    });
    Nav.focusFirst();
  }
  $('#upd-title').textContent = t('upd.updatingTo', { version: up.version || '' });
  $('#upd-bar').style.width = `${pctDone}%`;
  $('#upd-status').textContent = installing ? t('upd.installing') : t('upd.downloading', { n: pctDone });
  box.querySelector('[data-upd="hide"]').hidden = installing;
  Hints.update();
}

function onUpdateState(st) {
  const prev = S.update && S.update.status;
  S.update = st;
  // Once installing starts, show it even if the user sent the download to the background.
  if (st.status === 'installing') updateLayerOpen = true;
  renderUpdateLayer();
  if (st.status === 'available' && prev !== 'available') maybePromptUpdate();
  if (st.status === 'error' && prev === 'downloading') toast(t('err.update', { message: st.error }), 'error');
  if (route().name === 'settings' && !modals.length && prev !== st.status) render({ keepFocus: true });
}

// ============================================================================ Boot

let lastSig = '';
function librarySignature() {
  // Cheap fingerprint of what the current view shows, to avoid re-rendering (and jolting focus) needlessly.
  const L = S.library;
  return JSON.stringify([
    L.scannedAt,
    S.lang,
    L.movies.length,
    L.shows.length,
    L.games.length,
    L.continueWatching,
    S.settings.libraries,
    S.scanning,
    S.metaStatus,
    S.gameInfoStatus,
    L.movies.map((m) => [m.poster, m.progress.watched, m.progress.resumable, m.overview.length, m.favorite, m.hidden]),
    L.shows.map((s) => [s.poster, s.watchedCount, s.nextUp, s.overview.length, s.favorite, s.hidden, s.episodes.map((e) => [e.progress.watched, e.progress.resumable, e.thumb, e.title])]),
    L.games.map((g) => [g.title, g.poster, g.hero, g.logo, g.overview.length, g.playtime, g.lastPlayed, g.favorite, g.hidden, g.screenshots.length])
  ]);
}

function onState(next) {
  applyState(next);
  Status.renderBattery();
  const sig = librarySignature();
  if (sig === lastSig) return;
  lastSig = sig;
  const r = route();
  // Don't pull the rug out from under someone typing in search, or while a dialog is open.
  if (r.name === 'search' || modals.length) return;
  const hero = $('#hero');
  if (hero) delete hero.dataset.item;
  // If the page had nothing to focus (e.g. first scan still running), land on the new content instead.
  render(page.querySelector('.focusable') ? { keepFocus: true } : {});
}

(async function boot() {
  const initial = await api.getState();
  applyState(initial);
  $('#back-btn').innerHTML = ICON.back;
  $('#menu-btn').innerHTML = ICON.menu;
  $('#back-btn').setAttribute('aria-label', t('hint.back'));
  $('#menu-btn').setAttribute('aria-label', t('hint.menu'));
  lastSig = librarySignature();
  S.vlcFound = await api.detectVlc();
  Status.start();

  // Coming back from a game: restore exactly where we were.
  if (initial.uiState && initial.uiState.stack && initial.uiState.stack.length) {
    stack.length = 0;
    stack.push(...initial.uiState.stack.filter((r) => VIEWS[r.name]));
    if (!stack.length) stack.push({ name: 'home' });
    render({ restore: true });
  } else {
    render();
  }

  api.onState(onState);
  api.onNowPlaying((np) => {
    S.nowPlaying = np;
    renderNowPlaying();
  });
  api.onGame((g) => {
    S.game = g;
    renderGameLayer();
  });
  api.onUpdate(onUpdateState);
  if (S.update && S.update.status === 'available') maybePromptUpdate();
  api.onToast((m) => toast(m.key ? t(m.key, m.vars) : m.text, m.kind));
  for (const m of initial.toasts || []) toast(t(m.key, m.vars), m.kind);
  if (S.nowPlaying) renderNowPlaying();
  if (S.game) renderGameLayer();
})();
