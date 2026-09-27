'use strict';

/*
 * Playtime stats: turns the session log ({ kind: 'game' | 'watch', id, start, minutes }) into what the stats
 * page shows for a period: time buckets for the chart, totals, and the most played games and most watched
 * films/shows. Pure (no DOM), so it's unit tested; loaded by the renderer as a plain script and by tests
 * through module.exports.
 */
(function (root) {
  const DAY = 86400000;

  function startOfDay(ms) {
    const d = new Date(ms);
    d.setHours(0, 0, 0, 0);
    return d.getTime();
  }

  function startOfMonth(ms, offset = 0) {
    const d = new Date(ms);
    return new Date(d.getFullYear(), d.getMonth() + offset, 1).getTime();
  }

  function startOfYear(ms, offset = 0) {
    return new Date(new Date(ms).getFullYear() + offset, 0, 1).getTime();
  }

  /** Days ending today; `n` of them. Built from calendar dates so daylight-saving days stay whole. */
  function dayBuckets(now, n) {
    const out = [];
    const today = new Date(startOfDay(now));
    for (let i = n - 1; i >= 0; i--) {
      const start = new Date(today.getFullYear(), today.getMonth(), today.getDate() - i).getTime();
      const end = new Date(today.getFullYear(), today.getMonth(), today.getDate() - i + 1).getTime();
      out.push({ start, end, unit: 'day' });
    }
    return out;
  }

  function monthBuckets(from, now) {
    const out = [];
    for (let m = startOfMonth(from); m <= now; m = startOfMonth(m, 1)) out.push({ start: m, end: startOfMonth(m, 1), unit: 'month' });
    return out;
  }

  function yearBuckets(from, now) {
    const out = [];
    for (let y = startOfYear(from); y <= now; y = startOfYear(y, 1)) out.push({ start: y, end: startOfYear(y, 1), unit: 'year' });
    return out;
  }

  /** The chart's buckets for a period. 'all' uses months, or years once the log spans more than three. */
  function bucketsFor(period, now, firstStart) {
    if (period === 'week') return dayBuckets(now, 7);
    if (period === 'month') return dayBuckets(now, 30);
    if (period === 'year') return monthBuckets(startOfMonth(now, -11), now);
    const from = Math.min(firstStart || now, now);
    const months = monthBuckets(from, now);
    if (months.length <= 36) return months.length >= 3 ? months : monthBuckets(startOfMonth(now, -2), now);
    return yearBuckets(from, now);
  }

  /**
   * @param {{sessions: {kind: string, id: string, start: number, minutes: number}[], items: Object<string, {type: string, title: string, playtime?: number}>}} data
   * @param {'week'|'month'|'year'|'all'} period
   * @param {number} now
   */
  function aggregate(data, period, now) {
    const sessions = data.sessions || [];
    const items = data.items || {};
    const firstStart = sessions.reduce((min, x) => Math.min(min, x.start), Infinity);
    const buckets = bucketsFor(period, now, Number.isFinite(firstStart) ? firstStart : null).map((b) => ({ ...b, game: 0, watch: 0 }));
    const from = period === 'all' ? -Infinity : buckets[0].start;

    const perGame = new Map();
    const perWatch = new Map();
    const days = new Set();
    let count = 0;
    for (const x of sessions) {
      if (x.start < from || x.start > now) continue;
      const b = buckets.find((k) => x.start >= k.start && x.start < k.end);
      if (b) b[x.kind === 'game' ? 'game' : 'watch'] += x.minutes;
      const map = x.kind === 'game' ? perGame : perWatch;
      map.set(x.id, (map.get(x.id) || 0) + x.minutes);
      days.add(startOfDay(x.start));
      count++;
    }

    // All time also counts playtime Lounge didn't see (Steam's own total, time from before the log existed).
    if (period === 'all') {
      for (const [id, it] of Object.entries(items)) {
        if (it.type === 'game' && it.playtime > (perGame.get(id) || 0)) perGame.set(id, it.playtime);
      }
    }

    const top = (map) =>
      [...map]
        .filter(([id]) => items[id])
        .map(([id, minutes]) => ({ id, minutes, ...items[id] }))
        .sort((a, b) => b.minutes - a.minutes || a.title.localeCompare(b.title));

    const games = top(perGame);
    const watched = top(perWatch);
    const sum = (list) => list.reduce((n, x) => n + x.minutes, 0);
    // Totals: the chart's own sums for a period; for all time, the per-title totals (which include Steam's).
    const gameTotal = period === 'all' ? sum(games) : buckets.reduce((n, b) => n + b.game, 0);
    const watchTotal = period === 'all' ? sum(watched) : buckets.reduce((n, b) => n + b.watch, 0);
    const spanDays = period === 'all' ? Math.max(1, Math.round((startOfDay(now) - startOfDay(Number.isFinite(firstStart) ? firstStart : now)) / DAY) + 1) : Math.round((buckets[buckets.length - 1].end - buckets[0].start) / DAY);

    return {
      period,
      buckets,
      max: buckets.reduce((m, b) => Math.max(m, b.game + b.watch), 0),
      totals: { game: gameTotal, watch: watchTotal, sessions: count, activeDays: days.size, dailyAverage: Math.round((gameTotal + watchTotal) / spanDays) },
      games,
      watched,
      hasLog: sessions.length > 0
    };
  }

  /** Round hour gridlines for a chart whose tallest bar is `max` minutes: [0, step, 2·step, …] covering max. */
  function gridSteps(max) {
    const steps = [15, 30, 60, 120, 180, 240, 360, 600, 1200, 1800, 3000, 6000, 12000, 30000, 60000];
    const step = steps.find((s) => max / s <= 4) || Math.ceil(max / 4);
    const top = Math.max(step, Math.ceil(max / step) * step);
    const out = [];
    for (let v = 0; v <= top; v += step) out.push(v);
    return out;
  }

  const api = { aggregate, gridSteps, bucketsFor };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.Stats = api;
})(typeof window !== 'undefined' ? window : globalThis);
