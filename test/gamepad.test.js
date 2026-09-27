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
