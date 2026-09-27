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

  // ---- The Legion Go controllers' own status, from the vendor HID interface (usage page 0xFFA0) ----
  // Input report 0x04 (64 bytes). Offsets below count the report id as byte 0, as hidraw/hidapi deliver it;
  // WebHID's event.data leaves the id out, so they're read one lower. Sources: Handheld Companion
  // (LegionController.cs: bytes 12/13 = controller state, 2 wired / 3 wireless) and the GNOME
  // peripheral-battery helper (bytes 5/7 = battery %, bit 0 of 12/13 set = undocked; 0x01 seen when detached).
  const LEGION_STATUS_REPORT = 0x04;
  const LEGION_USAGE_PAGE = 0xffa0;
  const OFFSETS = { leftBattery: 5, rightBattery: 7, leftState: 12, rightState: 13 };
  const LOW_BATTERY = 15;
  const LOW_BATTERY_RESET = 25;

  function half(state, battery) {
    if (state === 2) return { state: 'attached', battery };
    // Detached: 0 % means the half is switched off but its slot still reports.
    return battery === 0 ? { state: 'off', battery: 0 } : { state: 'detached', battery };
  }

  /**
   * Read one status report: { left: {state, battery}, right: {...} }, state being 'attached' | 'detached' |
   * 'off'. Null for anything that isn't a well-formed status report (other report ids, or the misaligned
   * reports the controllers sometimes send, whose state bytes are out of range).
   * @param {number} reportId
   * @param {Uint8Array|number[]} data  the report without its id byte (WebHID's event.data)
   */
  function parseLegionStatus(reportId, data) {
    if (reportId !== LEGION_STATUS_REPORT || !data || data.length < OFFSETS.rightState) return null;
    const at = (i) => data[i - 1];
    const ls = at(OFFSETS.leftState);
    const rs = at(OFFSETS.rightState);
    if (![1, 2, 3].includes(ls) || ![1, 2, 3].includes(rs)) return null;
    return {
      left: half(ls, Math.min(100, at(OFFSETS.leftBattery))),
      right: half(rs, Math.min(100, at(OFFSETS.rightBattery)))
    };
  }

  /**
   * What changed between two readings, as notices to show: [{side, event}] with event 'attached' |
   * 'detached' | 'off' | 'low'. `warned` ({left, right}) remembers low-battery warnings until the level
   * recovers, so each is shown once; it's updated in place.
   */
  function legionChanges(prev, next, warned) {
    const out = [];
    for (const side of ['left', 'right']) {
      const a = prev && prev[side];
      const b = next && next[side];
      if (!b) continue;
      if (a && a.state !== b.state) out.push({ side, event: b.state });
      // Attached halves charge from the tablet; only warn about ones running on their own battery.
      if (b.state === 'detached' && b.battery <= LOW_BATTERY && !warned[side]) {
        warned[side] = true;
        out.push({ side, event: 'low', battery: b.battery });
      } else if (warned[side] && (b.battery >= LOW_BATTERY_RESET || b.state === 'attached')) {
        warned[side] = false;
      }
    }
    return out;
  }

  /** The level to show next to the icon: the lower of the halves that are on. Null when none is. */
  function lowestBattery(status) {
    if (!status) return null;
    const on = ['left', 'right'].map((k) => status[k]).filter((h) => h && h.state !== 'off');
    return on.length ? Math.min(...on.map((h) => h.battery)) : null;
  }

  const api = { describePad, stick, circularity, pollRate, parseLegionStatus, legionChanges, lowestBattery, LEGION_IDS, LEGION_STATUS_REPORT, LEGION_USAGE_PAGE, OFFSETS };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.Pads = api;
})(typeof window !== 'undefined' ? window : globalThis);
