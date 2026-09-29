'use strict';

// Boots the real renderer in jsdom against a stubbed `window.lounge` and clicks the top bar, so a
// handler that silently ignores a button (as the icon buttons once were) fails here.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const vm = require('vm');
const path = require('path');
const { JSDOM } = require('jsdom');

const root = path.join(__dirname, '..', 'renderer');

function state() {
  return {
    settings: { uiScale: 1, sounds: false, haptics: false, animations: 'reduced', libraries: [] },
    lang: 'en',
    library: { movies: [], shows: [], games: [], continueWatching: [], nextUp: [], stats: {} },
    scanning: false,
    metaStatus: {},
    gameInfoStatus: {},
    uiState: null,
    toasts: [],
    platform: 'linux',
    servers: [],
    apps: [],
    transfers: [],
    tailscaleTried: [],
    version: '0.0.0'
  };
}

async function boot() {
  const html = fs.readFileSync(path.join(root, 'index.html'), 'utf8').replace(/<script src="[^"]+"><\/script>/g, '');
  const dom = new JSDOM(html, { runScripts: 'outside-only', pretendToBeVisual: true, url: 'http://localhost/' });
  const { window } = dom;
  window.__ctx = dom.getInternalVMContext();
  window.matchMedia = window.matchMedia || (() => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  window.HTMLElement.prototype.scrollIntoView = () => {};
  const noop = () => () => {};
  window.lounge = new Proxy(
    { kind: 'test', getState: async () => state(), detectVlc: async () => null },
    { get: (t, k) => (k in t ? t[k] : String(k).startsWith('on') ? noop : async () => null) }
  );
  for (const f of ['boot.js', 'i18n.js', 'stats.js', 'gamepad.js', 'nav.js', 'core.js', 'views.js', 'app.js']) {
    // Real scripts (not eval) so top-level const/let are shared between files, as in a browser.
    new vm.Script(fs.readFileSync(path.join(root, f), 'utf8'), { filename: f }).runInContext(dom.getInternalVMContext());
  }
  await new Promise((r) => setTimeout(r, 300));
  return window;
}

test('every top-bar tab and icon button navigates when clicked', async () => {
  const window = await boot();
  try {
    await clickThrough(window);
  } finally {
    window.close();
  }
});

// The renderer's own intervals would keep the test process alive after a failure.
test.after(() => setImmediate(() => process.exit(process.exitCode || 0)));

async function clickThrough(window) {
  const buttons = [...window.document.querySelectorAll('#tabs [data-tab], #tools [data-tab]')];
  assert.ok(buttons.length >= 6, `expected tabs and tool icons, found ${buttons.length}`);
  for (const name of ['search', 'settings', 'games', 'home']) {
    const btn = window.document.querySelector(`[data-tab="${name}"]`);
    assert.ok(btn, `no button for ${name}`);
    btn.dispatchEvent(new window.MouseEvent('click', { bubbles: true }));
    assert.equal(vm.runInContext('route().name', window.__ctx), name, `clicking ${name} did not open it`);
  }
}
