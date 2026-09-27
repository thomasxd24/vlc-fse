'use strict';

// Runs in <head>, before the first paint: decides whether the startup intro plays, so it never flashes on
// screen when it shouldn't. No intro when coming back from a game (main reloads the page with ?resume=1),
// or when animations are reduced (Lounge's setting, remembered from last time, or Windows' own).
(function () {
  let reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  try {
    reduced = reduced || localStorage.getItem('animations') === 'reduced';
  } catch {}
  if (reduced || /[?&]resume=1\b/.test(location.search)) document.documentElement.classList.add('no-intro');
})();
