'use strict';

/*
 * Spatial navigation for a 10-foot UI.
 *
 * Anything with the `focusable` class takes part. Arrow keys (or a gamepad d-pad/stick) move focus to the
 * geometrically closest element in that direction. Elements inside a `[data-nav-group]` container remember
 * the last focused child, so moving back into a row returns to where you were rather than to whatever happens
 * to be closest. A container with `data-nav-trap` (modals) confines focus to itself.
 */
const Nav = (() => {
  const memory = new WeakMap();
  const listeners = { back: [], action: [] };
  let lastInput = 'keyboard';

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
    // Nearest row (or column) first, then whatever lines up best within it: this is what people expect on a TV,
    // e.g. Down from a button lands on the tabs just below it even if a bigger card further down is better aligned.
    const scored = [];
    for (const el of candidates()) {
      if (el === from) continue;
      const s = score(from, el, dir);
      if (s) scored.push({ el, ...s });
    }
    if (!scored.length) return;
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
  }

  const onBack = (cb) => listeners.back.push(cb);
  const emitBack = () => {
    for (const cb of [...listeners.back].reverse()) if (cb() !== false) return;
  };

  function activate() {
    const el = current();
    if (el) el.click();
  }

  function isTextInput(el) {
    return el && (el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && !['checkbox', 'radio', 'button', 'range'].includes(el.type)));
  }

  document.addEventListener('keydown', (e) => {
    lastInput = 'keyboard';
    document.body.classList.remove('using-mouse');
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
      if (el && el.tagName === 'BUTTON' && e.key === 'Enter') return; // native click fires
      e.preventDefault();
      activate();
      return;
    }
    if (e.key === 'Escape' || e.key === 'BrowserBack' || e.key === 'GoBack' || (e.key === 'Backspace' && !typing)) {
      e.preventDefault();
      if (typing && e.key === 'Escape') el.blur();
      emitBack();
    }
  });

  // Mouse: hovering focuses, so mouse and remote never disagree about what is selected.
  document.addEventListener('mousemove', (e) => {
    if (lastInput !== 'mouse') {
      lastInput = 'mouse';
      document.body.classList.add('using-mouse');
    }
    const el = e.target.closest && e.target.closest('.focusable');
    if (el && el !== document.activeElement && !isTextInput(document.activeElement) && scope().contains(el)) focus(el, { scroll: false });
  });
  document.addEventListener('mouseup', (e) => {
    if (e.button === 3) emitBack(); // mouse "back" button
  });

  // Keep something focused at all times.
  document.addEventListener('focusout', () => {
    setTimeout(() => {
      if (!current() && !isTextInput(document.activeElement)) focus(first(), { scroll: false });
    }, 0);
  });

  // ---- Gamepad (Xbox layout: A=0 B=1 X=2 Y=3, d-pad 12-15, Start=9, View=8) ----
  const pad = { held: {}, next: {} };
  const REPEAT_DELAY = 380;
  const REPEAT_RATE = 110;

  function padButton(name, pressed, fire, repeat) {
    const now = performance.now();
    if (pressed) {
      if (!pad.held[name]) {
        pad.held[name] = true;
        pad.next[name] = now + REPEAT_DELAY;
        fire();
      } else if (repeat && now >= pad.next[name]) {
        pad.next[name] = now + REPEAT_RATE;
        fire();
      }
    } else {
      pad.held[name] = false;
    }
  }

  function pollGamepads() {
    const pads = navigator.getGamepads ? [...navigator.getGamepads()].filter(Boolean) : [];
    for (const gp of pads) {
      const b = (i) => Boolean(gp.buttons[i] && gp.buttons[i].pressed);
      const ax = gp.axes[0] || 0;
      const ay = gp.axes[1] || 0;
      const T = 0.55;
      const input = () => {
        lastInput = 'gamepad';
        document.body.classList.remove('using-mouse');
      };
      padButton('up', b(12) || ay < -T, () => (input(), move('up')), true);
      padButton('down', b(13) || ay > T, () => (input(), move('down')), true);
      padButton('left', b(14) || ax < -T, () => (input(), move('left')), true);
      padButton('right', b(15) || ax > T, () => (input(), move('right')), true);
      padButton('a', b(0), () => (input(), activate()));
      padButton('b', b(1), () => (input(), emitBack()));
      for (const [i, name] of [[2, 'x'], [3, 'y'], [9, 'start'], [8, 'view'], [4, 'lb'], [5, 'rb']]) {
        padButton(name, b(i), () => {
          input();
          for (const cb of listeners.action) cb(name);
        });
      }
      break; // first connected pad only
    }
    requestAnimationFrame(pollGamepads);
  }
  requestAnimationFrame(pollGamepads);

  return {
    focus,
    move,
    first,
    current,
    onBack,
    onAction: (cb) => listeners.action.push(cb),
    back: emitBack,
    remember,
    focusFirst: () => focus(first()),
    /** Focus the remembered element of a group, or its first focusable. */
    focusGroup(group) {
      const mem = memory.get(group);
      focus(mem && mem.isConnected ? mem : group.querySelector('.focusable'));
    }
  };
})();
