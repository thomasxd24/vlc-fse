'use strict';

/*
 * Input & spatial navigation for a couch/handheld UI.
 *
 * Anything with the `focusable` class takes part. Arrow keys or a gamepad d-pad/stick move focus to the
 * nearest element in that direction (nearest row first, then the best aligned in it). Elements inside a
 * `[data-nav-group]` remember the last focused child; `[data-nav-default]` marks where to enter a group the
 * first time. A container with `data-nav-trap` (dialogs, menus) confines focus to itself.
 *
 * The current input method (pad / keyboard / mouse / touch) is tracked and exposed as a body class, so the
 * UI can show button hints for controllers and hide them for touch.
 */
const Nav = (() => {
  const memory = new WeakMap();
  const listeners = { back: [], action: [], move: [], mode: [] };
  let mode = 'keyboard';
  let active = true;
  let rumble = true;

  function setMode(m) {
    if (m === mode) return;
    mode = m;
    document.body.classList.remove('input-pad', 'input-keyboard', 'input-mouse', 'input-touch');
    document.body.classList.add(`input-${m}`);
    for (const cb of listeners.mode) cb(m);
  }
  document.body.classList.add('input-keyboard');

  function scope() {
    const traps = document.querySelectorAll('[data-nav-trap]');
    return traps.length ? traps[traps.length - 1] : document.body;
  }

  function visible(el) {
    if (el.disabled || el.closest('[hidden], [inert]')) return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  }

  function candidates() {
    return [...scope().querySelectorAll('.focusable')].filter(visible);
  }

  function groupOf(el) {
    return el ? el.closest('[data-nav-group]') : null;
  }

  function remember(el) {
    let g = groupOf(el);
    while (g) {
      memory.set(g, el);
      g = groupOf(g.parentElement);
    }
  }

  function focus(el, { scroll = true } = {}) {
    if (!el) return;
    el.focus({ preventScroll: true });
    remember(el);
    if (scroll) reveal(el);
  }

  /** Scroll every scrollable ancestor just enough to show `el` (CSS scroll-padding supplies margins). */
  function reveal(el) {
    const smooth = !matchMedia('(prefers-reduced-motion: reduce)').matches;
    el.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: smooth ? 'smooth' : 'auto' });
  }

  function current() {
    const a = document.activeElement;
    return a && a.classList.contains('focusable') && scope().contains(a) ? a : null;
  }

  function first() {
    const s = scope();
    const pref = s.querySelector('[data-autofocus]');
    if (pref && visible(pref)) return pref;
    return candidates()[0] || null;
  }

  function score(from, to, dir) {
    const a = from.getBoundingClientRect();
    const b = to.getBoundingClientRect();
    // A candidate must lie beyond the current element in `dir`, allowing a little overlap (focused items are
    // scaled up, so neighbours in the same row can poke out by a pixel or two).
    const tx = Math.min(a.width, b.width) / 2;
    const ty = Math.min(a.height, b.height) / 2;
    let primary;
    let secondary;
    switch (dir) {
      case 'left':
        if (b.right > a.left + tx) return null;
        primary = Math.max(0, a.left - b.right);
        secondary = overlapGap(a.top, a.bottom, b.top, b.bottom);
        break;
      case 'right':
        if (b.left < a.right - tx) return null;
        primary = Math.max(0, b.left - a.right);
        secondary = overlapGap(a.top, a.bottom, b.top, b.bottom);
        break;
      case 'up':
        if (b.bottom > a.top + ty) return null;
        primary = Math.max(0, a.top - b.bottom);
        secondary = overlapGap(a.left, a.right, b.left, b.right);
        break;
      default:
        if (b.top < a.bottom - ty) return null;
        primary = Math.max(0, b.top - a.bottom);
        secondary = overlapGap(a.left, a.right, b.left, b.right);
    }
    return { primary, secondary };
  }

  function overlapGap(a1, a2, b1, b2) {
    if (b2 < a1) return a1 - b2;
    if (b1 > a2) return b1 - a2;
    return 0;
  }

  function move(dir) {
    const from = current();
    if (!from) return focus(first());
    // A focused slider takes left/right itself.
    if ((dir === 'left' || dir === 'right') && from.dataset.slider !== undefined) {
      from.dispatchEvent(new CustomEvent('nudge', { detail: dir === 'left' ? -1 : 1 }));
      return;
    }
    // Nearest row (or column) first, then whatever lines up best within it: this is what people expect on a TV,
    // e.g. Down from a button lands on the tabs just below it even if a bigger card further down is better aligned.
    // Left/right inside a row of cards stays in that row: at its end, you stay put rather than jumping
    // diagonally into the neighbouring row.
    const track = (dir === 'left' || dir === 'right') && from.closest('.track');
    const scored = [];
    for (const el of candidates()) {
      if (el === from || (track && !track.contains(el))) continue;
      const s = score(from, el, dir);
      if (s) scored.push({ el, ...s });
    }
    if (!scored.length) {
      for (const cb of listeners.move) cb('edge');
      return;
    }
    const r = from.getBoundingClientRect();
    const band = (dir === 'up' || dir === 'down' ? r.height : r.width) / 2;
    const nearest = Math.min(...scored.map((c) => c.primary));
    let best = null;
    let bestScore = Infinity;
    for (const c of scored) {
      if (c.primary > nearest + band) continue;
      const s = c.secondary * 4 + c.primary;
      if (s < bestScore) {
        bestScore = s;
        best = c.el;
      }
    }

    // Entering a different group: go back to what was focused there last time, or to its declared default
    // (e.g. the selected season or the active tab).
    let g = groupOf(best);
    while (g && !g.contains(from)) {
      const mem = memory.get(g);
      if (mem && mem.isConnected && g.contains(mem) && visible(mem)) {
        best = mem;
        break;
      }
      const def = g.querySelector('[data-nav-default]');
      if (def && visible(def) && groupOf(def) === g) {
        best = def;
        break;
      }
      g = groupOf(g.parentElement);
    }
    focus(best);
    for (const cb of listeners.move) cb('move');
  }

  /** Jump about a screen: along a horizontal row of cards, or up/down a grid or list. */
  function page(delta) {
    const from = current();
    if (!from) return;
    const track = from.closest('.track');
    const dir = track ? (delta > 0 ? 'right' : 'left') : delta > 0 ? 'down' : 'up';
    const container = track || from.closest('.rows, .page') || document.body;
    const span = track ? container.clientWidth : container.clientHeight;
    const start = from.getBoundingClientRect();
    const quiet = listeners.move.splice(0); // one sound for the whole jump, not one per step
    try {
      for (let i = 0; i < 80; i++) {
        const before = current();
        move(dir);
        const now = current();
        if (now === before) break;
        const r = now.getBoundingClientRect();
        const dist = track ? Math.abs(r.left - start.left) : Math.abs(r.top - start.top);
        if (dist >= span * 0.75) break;
      }
    } finally {
      listeners.move.push(...quiet);
    }
    for (const cb of listeners.move) cb('move');
  }

  const onBack = (cb) => listeners.back.push(cb);
  function emitBack() {
    for (const cb of [...listeners.back].reverse()) if (cb() !== false) return;
  }
  function emitAction(name) {
    for (const cb of listeners.action) cb(name);
  }

  function activate() {
    const el = current();
    if (el) {
      for (const cb of listeners.move) cb('select');
      el.classList.add('pressed');
      setTimeout(() => el.classList.remove('pressed'), 170);
      el.click();
    }
  }

  function isTextInput(el) {
    return el && (el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && !['checkbox', 'radio', 'button', 'range'].includes(el.type)));
  }

  // ---- Keyboard (also what TV remotes and many handheld "desktop modes" send) ----
  document.addEventListener('keydown', (e) => {
    if (!active) return;
    setMode('keyboard');
    const el = document.activeElement;
    const typing = isTextInput(el);
    const dirs = { ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'left', ArrowRight: 'right' };

    if (dirs[e.key]) {
      if (typing && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
        const atStart = el.selectionStart === 0 && el.selectionEnd === 0;
        const atEnd = el.selectionStart === el.value.length;
        if (!(e.key === 'ArrowLeft' ? atStart : atEnd)) return;
      }
      e.preventDefault();
      move(dirs[e.key]);
      return;
    }
    if (e.key === 'Enter' || (e.key === ' ' && !typing)) {
      if (typing && e.key === 'Enter') {
        el.dispatchEvent(new Event('change', { bubbles: true }));
        e.preventDefault();
        return;
      }
      e.preventDefault();
      activate();
      return;
    }
    if (e.key === 'Escape' || e.key === 'BrowserBack' || e.key === 'GoBack' || (e.key === 'Backspace' && !typing)) {
      e.preventDefault();
      if (typing && e.key === 'Escape') el.blur();
      emitBack();
      return;
    }
    if (e.key === 'PageDown' || e.key === 'PageUp') {
      e.preventDefault();
      page(e.key === 'PageDown' ? 1 : -1);
      return;
    }
    if (e.key === 'Tab') {
      e.preventDefault();
      emitAction(e.shiftKey ? 'lb' : 'rb');
      return;
    }
    if (e.key === 'ContextMenu') {
      e.preventDefault();
      emitAction('x');
    }
  });

  // ---- Pointer: mouse hover focuses; touch never focuses on hover ----
  document.addEventListener(
    'pointerdown',
    (e) => {
      setMode(e.pointerType === 'touch' || e.pointerType === 'pen' ? 'touch' : 'mouse');
      if (e.pointerType === 'mouse' && e.button === 3) emitBack(); // mouse "back" button
    },
    true
  );
  document.addEventListener('pointermove', (e) => {
    if (e.pointerType !== 'mouse') return;
    if (mode !== 'mouse' && (Math.abs(e.movementX) > 2 || Math.abs(e.movementY) > 2)) setMode('mouse');
    if (mode !== 'mouse') return;
    const el = e.target.closest && e.target.closest('.focusable');
    if (el && el !== document.activeElement && !isTextInput(document.activeElement) && scope().contains(el)) focus(el, { scroll: false });
  });
  document.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    if (mode === 'touch') return; // handled by long-press below
    const el = e.target.closest && e.target.closest('.focusable');
    if (el) focus(el, { scroll: false });
    emitAction('x');
  });

  // Swipe in from the left edge to go back, like on a phone.
  let edge = null;
  document.addEventListener(
    'touchstart',
    (e) => {
      const t = e.touches[0];
      edge = e.touches.length === 1 && t.clientX < 28 ? { x: t.clientX, y: t.clientY } : null;
    },
    { passive: true }
  );
  document.addEventListener(
    'touchmove',
    (e) => {
      if (!edge) return;
      const t = e.touches[0];
      if (Math.abs(t.clientY - edge.y) > 60) edge = null;
      else if (t.clientX - edge.x > 90) {
        edge = null;
        emitBack();
      }
    },
    { passive: true }
  );

  // Long-press opens the options menu on touch, like right-click with a mouse or X on a controller.
  let press = null;
  let pressFired = false;
  let pressStart = null;
  document.addEventListener(
    'touchstart',
    (e) => {
      clearTimeout(press);
      pressFired = false;
      const el = e.target.closest && e.target.closest('[data-opts]');
      if (!el || e.touches.length !== 1) return;
      pressStart = { x: e.touches[0].clientX, y: e.touches[0].clientY };
      press = setTimeout(() => {
        pressFired = true;
        focus(el, { scroll: false });
        if (navigator.vibrate) navigator.vibrate(12);
        emitAction('x');
      }, 550);
    },
    { passive: true }
  );
  document.addEventListener(
    'touchmove',
    (e) => {
      if (!pressStart) return;
      const t = e.touches[0];
      if (Math.hypot(t.clientX - pressStart.x, t.clientY - pressStart.y) > 12) clearTimeout(press);
    },
    { passive: true }
  );
  document.addEventListener('touchend', () => {
    clearTimeout(press);
    pressStart = null;
  });
  document.addEventListener(
    'click',
    (e) => {
      // Swallow the tap that ends a long-press, so it doesn't also open the item.
      if (pressFired) {
        pressFired = false;
        e.stopPropagation();
        e.preventDefault();
      }
    },
    true
  );

  // Keep something focused at all times (but never pull focus while someone's typing).
  document.addEventListener('focusout', () => {
    setTimeout(() => {
      if (active && !current() && !isTextInput(document.activeElement)) focus(first(), { scroll: false });
    }, 0);
  });

  // ---- Gamepad (Xbox layout: A=0 B=1 X=2 Y=3 LB=4 RB=5 LT=6 RT=7 View=8 Menu=9, d-pad 12-15) ----
  const held = {};
  const next = {};
  const REPEAT_DELAY = 360;
  const REPEAT_RATE = 95;
  let lastPad = null;
  let rafId = null;

  function padButton(name, pressed, fire, repeat) {
    const now = performance.now();
    if (pressed) {
      if (!held[name]) {
        held[name] = true;
        next[name] = now + REPEAT_DELAY;
        fire();
      } else if (repeat && now >= next[name]) {
        next[name] = now + REPEAT_RATE;
        fire();
      }
    } else {
      held[name] = false;
    }
  }

  function pollGamepads() {
    rafId = null;
    if (!active || document.hidden) return; // resumes on visibilitychange / setActive
    const pads = navigator.getGamepads ? [...navigator.getGamepads()].filter(Boolean) : [];
    // Use the pad with something pressed (handhelds can expose both built-in and Bluetooth pads).
    const gp = pads.find((p) => p.buttons.some((x) => x.pressed) || p.axes.some((a) => Math.abs(a) > 0.55)) || lastPad && pads.find((p) => p.index === lastPad.index);
    if (gp) {
      const b = (i) => Boolean(gp.buttons[i] && (gp.buttons[i].pressed || gp.buttons[i].value > 0.5));
      const ax = gp.axes[0] || 0;
      const ay = gp.axes[1] || 0;
      const T = 0.55;
      const run = (fn) => () => {
        lastPad = gp;
        setMode('pad');
        fn();
      };
      padButton('up', b(12) || ay < -T, run(() => move('up')), true);
      padButton('down', b(13) || ay > T, run(() => move('down')), true);
      padButton('left', b(14) || ax < -T, run(() => move('left')), true);
      padButton('right', b(15) || ax > T, run(() => move('right')), true);
      padButton('a', b(0), run(activate));
      padButton('b', b(1), run(emitBack));
      padButton('lt', b(6), run(() => page(-1)), true);
      padButton('rt', b(7), run(() => page(1)), true);
      for (const [i, name] of [[2, 'x'], [3, 'y'], [4, 'lb'], [5, 'rb'], [8, 'view'], [9, 'menu']]) {
        padButton(name, b(i), run(() => emitAction(name)), name === 'lb' || name === 'rb');
      }
    }
    rafId = requestAnimationFrame(pollGamepads);
  }

  function startPolling() {
    if (rafId === null && active && !document.hidden) rafId = requestAnimationFrame(pollGamepads);
  }
  window.addEventListener('gamepadconnected', () => startPolling());
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) startPolling();
  });
  startPolling();

  /** A tiny tick of vibration on the pad that was used last. */
  function haptic(strength = 0.25, ms = 14) {
    if (!rumble || mode !== 'pad' || !lastPad || !lastPad.vibrationActuator) return;
    try {
      lastPad.vibrationActuator.playEffect('dual-rumble', { duration: ms, strongMagnitude: 0, weakMagnitude: strength });
    } catch {}
  }

  return {
    focus,
    move,
    page,
    first,
    current,
    onBack,
    onAction: (cb) => listeners.action.push(cb),
    onMove: (cb) => listeners.move.push(cb),
    onMode: (cb) => listeners.mode.push(cb),
    mode: () => mode,
    back: emitBack,
    remember,
    haptic,
    setRumble: (on) => (rumble = on),
    setActive(on) {
      active = on;
      if (on) startPolling();
    },
    focusFirst: () => focus(first()),
    /** Focus the remembered element of a group, or its first focusable. */
    focusGroup(group) {
      const mem = memory.get(group);
      focus(mem && mem.isConnected ? mem : group.querySelector('.focusable'));
    }
  };
})();
