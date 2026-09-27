'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const Pads = require('../renderer/gamepad.js');

test('names pads and recognises the Legion Go controllers', () => {
  assert.deepEqual(Pads.describePad('Legion Controller for Windows (STANDARD GAMEPAD Vendor: 17ef Product: 6182)'), {
    name: 'Legion Controller for Windows',
    vendor: '17ef',
    product: '6182',
    legion: true
  });
  assert.equal(Pads.describePad('Controller (Vendor: 17ef Product: 6185)').legion, true); // FPS mode
  assert.equal(Pads.describePad('1a86-e310-Legion Go S Controller').legion, true);
  const xbox = Pads.describePad('Xbox 360 Controller (XInput STANDARD GAMEPAD)');
  assert.equal(xbox.name, 'Xbox 360 Controller (XInput STANDARD GAMEPAD)');
  assert.equal(xbox.legion, false);
  const dualsense = Pads.describePad('DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)');
  assert.equal(dualsense.name, 'DualSense Wireless Controller');
  assert.equal(dualsense.legion, false);
  assert.equal(Pads.describePad('').name, 'Controller');
});

test('stick circularity needs most directions, then reports the average error', () => {
  const circle = (r, n = 360) => Array.from({ length: n }, (_, i) => [r * Math.cos((i / n) * 2 * Math.PI), r * Math.sin((i / n) * 2 * Math.PI)]);
  assert.equal(Pads.circularity(circle(1).slice(0, 100)).error, null); // only a quarter covered
  assert.equal(Pads.circularity(circle(1)).error, 0);
  assert.equal(Pads.circularity(circle(0.9)).error, 10);
  // A square gate reaches past the circle on the diagonals.
  const square = circle(1).map(([x, y]) => {
    const m = Math.max(Math.abs(x), Math.abs(y));
    return [x / m, y / m];
  });
  assert.ok(Pads.circularity(square).error > 15);
  assert.ok(Pads.circularity([[0.1, 0.1], [0, 0]]).coverage === 0);
});

test('polling rate and stick helpers', () => {
  assert.equal(Pads.pollRate([0, 4, 8, 12, 16]), 250);
  assert.equal(Pads.pollRate([5]), 0);
  assert.equal(Pads.stick(0.6, 0.8).mag, 1);
});

// A status report as WebHID delivers it (no report id byte), with bytes placed at the documented offsets.
function report({ lb = 80, rb = 64, ls = 2, rs = 2 } = {}) {
  const buf = new Uint8Array(64); // hidraw layout: buf[0] would be the report id
  buf[5] = lb;
  buf[7] = rb;
  buf[12] = ls;
  buf[13] = rs;
  return buf.slice(1);
}

test('Legion Go status report: battery and attach state per half', () => {
  assert.deepEqual(Pads.parseLegionStatus(0x04, report()), { left: { state: 'attached', battery: 80 }, right: { state: 'attached', battery: 64 } });
  // Handheld Companion documents 3 (wireless) for a detached half; the GNOME helper saw 1 on newer firmware.
  assert.deepEqual(Pads.parseLegionStatus(0x04, report({ rs: 3 })).right, { state: 'detached', battery: 64 });
  assert.deepEqual(Pads.parseLegionStatus(0x04, report({ rs: 1 })).right, { state: 'detached', battery: 64 });
  assert.deepEqual(Pads.parseLegionStatus(0x04, report({ ls: 1, lb: 0 })).left, { state: 'off', battery: 0 });
  assert.equal(Pads.parseLegionStatus(0x04, report({ lb: 250 })).left.battery, 100);
  // Not a status report: other report ids, misaligned state bytes, short reports.
  assert.equal(Pads.parseLegionStatus(0x05, report()), null);
  assert.equal(Pads.parseLegionStatus(0x04, report({ ls: 0 })), null);
  assert.equal(Pads.parseLegionStatus(0x04, report({ rs: 0x92 })), null);
  assert.equal(Pads.parseLegionStatus(0x04, new Uint8Array(8)), null);
});

test('Legion Go notices: attach changes, and one low-battery warning per discharge', () => {
  const warned = { left: false, right: false };
  const s = (l, r) => ({ left: l, right: r });
  const att = (b) => ({ state: 'attached', battery: b });
  const det = (b) => ({ state: 'detached', battery: b });
  assert.deepEqual(Pads.legionChanges(null, s(att(80), att(64)), warned), []);
  assert.deepEqual(Pads.legionChanges(s(att(80), att(64)), s(att(80), det(64)), warned), [{ side: 'right', event: 'detached' }]);
  assert.deepEqual(Pads.legionChanges(s(att(80), det(16)), s(att(80), det(15)), warned), [{ side: 'right', event: 'low', battery: 15 }]);
  assert.deepEqual(Pads.legionChanges(s(att(80), det(15)), s(att(80), det(12)), warned), []); // warned once
  assert.deepEqual(Pads.legionChanges(s(att(80), det(12)), s(att(80), { state: 'off', battery: 0 }), warned), [{ side: 'right', event: 'off' }]);
  assert.deepEqual(Pads.legionChanges(s(att(80), { state: 'off', battery: 0 }), s(att(80), att(30)), warned), [{ side: 'right', event: 'attached' }]);
  assert.equal(warned.right, false); // reset once it's charging again
  assert.equal(Pads.lowestBattery(s(att(80), det(12))), 12);
  assert.equal(Pads.lowestBattery(s(att(80), { state: 'off', battery: 0 })), 80);
  assert.equal(Pads.lowestBattery(s({ state: 'off', battery: 0 }, { state: 'off', battery: 0 })), null);
});
