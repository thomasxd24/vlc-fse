'use strict';

/*
 * Gamepad helpers: naming a pad from the browser's id string (and telling the Legion Go's own controllers
 * apart from others), and stick measurements for the tester (drift at rest, outer-circle error). Pure, so
 * it's unit tested; loaded by the renderer as a plain script and by tests through module.exports.
 */
(function (root) {
  // USB ids of the Legion Go family's built-in controllers: Legion Go (xinput, dinput, dual dinput and FPS
  // modes, then the later firmware's ids) and Legion Go S.
  const LEGION_IDS = {
    '17ef': ['6182', '6183', '6184', '6185', '61eb', '61ec', '61ed', '61ee'],
    '1a86': ['e310', 'e311']
  };

  /**
   * Chromium ids look like "Legion Controller for Windows (STANDARD GAMEPAD Vendor: 17ef Product: 6182)";
   * Firefox's like "17ef-6182-Legion Controller for Windows".
   */
  function describePad(id) {
    const raw = String(id || '');
    let vendor = null;
    let product = null;
    let name = raw;
    const chromium = /\(?\s*(?:STANDARD GAMEPAD\s*)?Vendor:\s*([0-9a-f]{4})\s*Product:\s*([0-9a-f]{4})\s*\)?\s*$/i.exec(raw);
    const firefox = /^([0-9a-f]{4})-([0-9a-f]{4})-(.*)$/i.exec(raw);
    if (chromium) {
      vendor = chromium[1].toLowerCase();
      product = chromium[2].toLowerCase();
      name = raw.slice(0, chromium.index);
    } else if (firefox) {
      vendor = firefox[1].toLowerCase();
      product = firefox[2].toLowerCase();
      name = firefox[3];
    }
    name = name.replace(/\(\s*STANDARD GAMEPAD\s*\)/i, '').replace(/\s{2,}/g, ' ').trim() || 'Controller';
    const legion = Boolean((vendor && LEGION_IDS[vendor] && LEGION_IDS[vendor].includes(product)) || /\blegion\b/i.test(name));
    return { name, vendor, product, legion };
  }

  /** Distance from centre (0–1+) and angle (radians) of a stick. */
  function stick(x, y) {
    return { mag: Math.hypot(x || 0, y || 0), angle: Math.atan2(y || 0, x || 0) };
  }

  const BINS = 36;

  /**
   * How round a stick's outer range is, from positions sampled while it was rolled around the edge: for each
   * of 36 directions the furthest reach, then the average distance of those from the unit circle (%). Only
   * reported once most directions have been covered.
   */
  function circularity(samples) {
    const reach = new Array(BINS).fill(0);
    for (const [x, y] of samples) {
      const s = stick(x, y);
      if (s.mag < 0.6) continue;
      const bin = Math.floor(((s.angle + Math.PI) / (2 * Math.PI)) * BINS) % BINS;
      reach[bin] = Math.max(reach[bin], s.mag);
    }
    const hit = reach.filter((r) => r > 0);
    const coverage = hit.length / BINS;
    if (coverage < 0.75) return { coverage, error: null };
    const error = hit.reduce((n, r) => n + Math.abs(1 - Math.min(r, 1.5)), 0) / hit.length;
    return { coverage, error: Math.round(error * 1000) / 10 };
  }

  /** Updates per second, from the times (ms) at which the pad reported new data. */
  function pollRate(times) {
    if (times.length < 2) return 0;
    const span = times[times.length - 1] - times[0];
    return span > 0 ? Math.round(((times.length - 1) * 1000) / span) : 0;
  }

  const api = { describePad, stick, circularity, pollRate, LEGION_IDS };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.Pads = api;
})(typeof window !== 'undefined' ? window : globalThis);
