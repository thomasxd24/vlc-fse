'use strict';

// Runs in <head>, before the first paint: decides whether the startup intro plays, so it never flashes on
// screen when it shouldn't. No intro when coming back from a game (main reloads the page with ?resume=1),
// or when Lounge's own Animations setting is Reduced (remembered from last time). With Windows' "animation
// effects" off it still plays, as a plain fade (html.intro-calm).
//
// The intro only starts once the window is actually on screen (a moment after it becomes visible): Lounge's
// window appears after the page has loaded, and in the full screen experience after Windows' own transition,
// which would otherwise swallow the whole animation.
(function () {
  let reduced = false;
  try {
    reduced = localStorage.getItem('animations') === 'reduced';
  } catch {}
  const root = document.documentElement;
  if (reduced || /[?&]resume=1\b/.test(location.search)) {
    root.classList.add('no-intro');
    return;
  }
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) root.classList.add('intro-calm');
  const play = () => {
    if (root.classList.contains('intro-play')) return;
    window.__introAt = performance.now();
    root.classList.add('intro-play');
  };
  const whenVisible = () => {
    if (document.visibilityState !== 'visible') return;
    document.removeEventListener('visibilitychange', whenVisible);
    requestAnimationFrame(() => setTimeout(play, 250));
  };
  document.addEventListener('visibilitychange', whenVisible);
  document.addEventListener('DOMContentLoaded', whenVisible);
  setTimeout(play, 4000); // never wait for ever
})();
