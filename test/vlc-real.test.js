'use strict';

// Launches the real VLC with exactly the arguments Marquee uses, to catch options VLC rejects
// ("VLC media player could not start. Either the command line options were invalid...").
// Skipped when VLC isn't installed. CI installs VLC so this always runs there.

const test = require('node:test');
const assert = require('node:assert/strict');
const { findVlc, VlcSession } = require('../src/vlc');

test('real VLC accepts our command line and serves playback status', async (t) => {
  const vlc = await findVlc(process.env.VLC_PATH);
  if (!vlc) return t.skip('VLC not installed');
  if (process.platform !== 'win32' && process.getuid && process.getuid() === 0) return t.skip('VLC refuses to run as root');

  // "vlc://pause:N" is a built-in item that just waits, so no media file or display is needed.
  const session = new VlcSession(vlc, [{ path: 'vlc://pause:20' }], { fullscreen: false, extraArgs: ['-I', 'dummy'] });
  let exited = null;
  session.on('exit', (e) => (exited = e));
  await session.start();
  t.after(() => session.kill());

  let status = null;
  for (let i = 0; i < 60 && !status && !exited; i++) {
    await new Promise((r) => setTimeout(r, 250));
    status = await session.request('status.json').catch(() => null);
  }
  assert.equal(exited, null, `VLC exited early (code ${exited && exited.code}): it rejected an option`);
  assert.ok(status && typeof status.state === 'string', 'VLC HTTP interface answered with a status');
});
