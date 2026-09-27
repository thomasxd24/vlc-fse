'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('crypto');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { Updater, compareVersions, detectInstallType } = require('../src/updater');

test('version comparison', () => {
  assert.ok(compareVersions('2.1.0', '2.0.9') > 0);
  assert.ok(compareVersions('v2.0.10', '2.0.9') > 0);
  assert.ok(compareVersions('2.0.0', '2.0.0') === 0);
  assert.ok(compareVersions('2.0.0', '2.0.0-beta.1') > 0);
  assert.ok(compareVersions('1.9.9', '2.0.0') < 0);
  assert.ok(compareVersions('2.0.0', '0.0') > 0);
  assert.equal(compareVersions('2', '2.0.0'), 0);
  assert.equal(compareVersions('garbage', '1.0.0'), 0);
});

test('install type detection', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lounge-inst-'));
  const exePath = path.join(dir, 'Lounge.exe');
  assert.equal(detectInstallType({ packaged: false, exePath, platform: 'win32' }), null, 'dev builds never update');
  assert.equal(detectInstallType({ packaged: true, exePath, platform: 'linux' }), null);
  assert.equal(detectInstallType({ packaged: true, windowsStore: true, exePath, platform: 'win32' }), 'fse');
  assert.equal(detectInstallType({ packaged: true, exePath, platform: 'win32' }), 'zip');
  fs.writeFileSync(path.join(dir, 'Uninstall Lounge.exe'), '');
  assert.equal(detectInstallType({ packaged: true, exePath, platform: 'win32' }), 'nsis');
  fs.rmSync(dir, { recursive: true, force: true });
  // Installed back when the app was called Foyer.
  const old = fs.mkdtempSync(path.join(os.tmpdir(), 'lounge-inst-'));
  fs.writeFileSync(path.join(old, 'Uninstall Foyer.exe'), '');
  assert.equal(detectInstallType({ packaged: true, exePath: path.join(old, 'Foyer.exe'), platform: 'win32' }), 'nsis');
  fs.rmSync(old, { recursive: true, force: true });
});

function release(payload, digestOf = payload, prefix = 'Lounge') {
  const sha = crypto.createHash('sha256').update(digestOf).digest('hex');
  const assets = [`${prefix}-Setup-2.1.0.exe`, `${prefix}-2.1.0-win-x64.zip`, `${prefix}-FSE-2.1.0.zip`, `${prefix}-Setup-2.1.0.exe.blockmap`].map((name) => ({
    name,
    size: payload.length,
    digest: `sha256:${sha}`,
    browser_download_url: `https://github.com/o/r/releases/download/v2.1.0/${name}`
  }));
  return { tag_name: 'v2.1.0', body: '## What\'s Changed\n* Faster things', assets };
}

function mockFetch(t, rel, payload) {
  const calls = [];
  t.mock.method(globalThis, 'fetch', async (url) => {
    calls.push(String(url));
    if (String(url).includes('/releases/latest')) return { ok: true, status: 200, json: async () => rel };
    return new Response(payload, { status: 200, headers: { 'content-length': String(payload.length) } });
  });
  return calls;
}

test('finds the right asset for each install type, downloads and verifies it', async (t) => {
  const payload = Buffer.alloc(300000, 7);
  for (const [type, name] of [['nsis', 'Lounge-Setup-2.1.0.exe'], ['zip', 'Lounge-2.1.0-win-x64.zip'], ['fse', 'Lounge-FSE-2.1.0.zip']]) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lounge-upd-'));
    const calls = mockFetch(t, release(payload), payload);
    const u = new Updater({ repo: 'o/r', version: '2.0.0', installType: type, dir });
    const progress = [];
    u.on('state', (s) => progress.push(s.status));
    const st = await u.check();
    assert.equal(st.status, 'available', type);
    assert.equal(st.version, '2.1.0');
    assert.equal(u.asset.name, name);
    const file = await u.download();
    assert.equal(path.basename(file), name);
    assert.equal(fs.statSync(file).size, payload.length);
    assert.equal(u.state.status, 'ready');
    assert.ok(calls.some((c) => c.endsWith(name)));
    const cmd = await u.installCommand({ pid: 123, exePath: path.join(dir, 'Lounge.exe') });
    if (type === 'nsis') assert.deepEqual(cmd.args, ['/S', '--updated', '--force-run']);
    else if (type === 'fse') {
      // Started through WMI, outside the package, with a status file to follow.
      assert.equal(u.state.status, 'elevating');
      const launcher = cmd.args[cmd.args.indexOf('-Command') + 1];
      assert.match(launcher, /Invoke-CimMethod -ClassName Win32_Process -MethodName Create/);
      assert.ok(launcher.includes(`-Zip "${file}"`) && launcher.includes(`-Status "${cmd.status}"`));
      assert.ok(fs.readFileSync(path.join(dir, 'apply-fse.ps1'), 'utf8').startsWith('﻿param('));
    } else {
      assert.equal(cmd.command, 'powershell.exe');
      const script = cmd.args[cmd.args.indexOf('-File') + 1];
      assert.ok(fs.readFileSync(script, 'utf8').startsWith('﻿param('), 'script written with a BOM for Windows PowerShell');
      assert.ok(cmd.args.includes('123'));
    }
    t.mock.restoreAll();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('rejects a corrupted download', async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lounge-upd-'));
  const payload = Buffer.alloc(1000, 1);
  mockFetch(t, release(payload, Buffer.from('something else')), payload);
  const u = new Updater({ repo: 'o/r', version: '2.0.0', installType: 'zip', dir });
  await u.check();
  await assert.rejects(u.download(), /checksum/);
  assert.equal(u.state.status, 'error');
  assert.equal(fs.readdirSync(dir).length, 0, 'the bad file is removed');
  fs.rmSync(dir, { recursive: true, force: true });
});

test('up to date, no release yet, and unsupported installs', async (t) => {
  const dir = os.tmpdir();
  mockFetch(t, { tag_name: 'v2.0.0', assets: [] }, Buffer.alloc(0));
  const u = new Updater({ repo: 'o/r', version: '2.0.0', installType: 'zip', dir });
  assert.equal((await u.check()).status, 'uptodate');
  t.mock.restoreAll();
  t.mock.method(globalThis, 'fetch', async () => ({ ok: false, status: 404 }));
  assert.equal((await u.check()).status, 'uptodate');
  const none = new Updater({ repo: 'o/r', version: '2.0.0', installType: null, dir });
  assert.equal(none.state.status, 'unsupported');
  assert.equal((await none.check()).status, 'unsupported');
});

test('releases published under the old name (Foyer) are still found', async (t) => {
  const payload = Buffer.alloc(1000, 3);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lounge-upd-'));
  mockFetch(t, release(payload, payload, 'Foyer'), payload);
  const u = new Updater({ repo: 'o/r', version: '2.0.0', installType: 'fse', dir });
  const st = await u.check();
  assert.equal(st.status, 'available');
  assert.equal(u.asset.name, 'Foyer-FSE-2.1.0.zip');
});

test('with both names in a release, the Lounge files are preferred', async (t) => {
  const payload = Buffer.alloc(1000, 5);
  const both = release(payload, payload, 'Foyer');
  both.assets.push(...release(payload).assets);
  for (const [type, name] of [['nsis', 'Lounge-Setup-2.1.0.exe'], ['zip', 'Lounge-2.1.0-win-x64.zip'], ['fse', 'Lounge-FSE-2.1.0.zip']]) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lounge-upd-'));
    mockFetch(t, both, payload);
    const u = new Updater({ repo: 'o/r', version: '2.0.0', installType: type, dir });
    await u.check();
    assert.equal(u.asset.name, name);
    t.mock.restoreAll();
  }
});

test('rate-limited API: falls back to github.com and verifies against SHA256SUMS.txt', async (t) => {
  const { parseChecksums } = require('../src/updater');
  const payload = Buffer.alloc(5000, 9);
  const sha = crypto.createHash('sha256').update(payload).digest('hex');
  assert.deepEqual(parseChecksums(`${sha}  Lounge-FSE-2.2.0.zip\r\n${'a'.repeat(64)} *Lounge-Setup-2.2.0.exe\n`), {
    'Lounge-FSE-2.2.0.zip': `sha256:${sha}`,
    'Lounge-Setup-2.2.0.exe': `sha256:${'a'.repeat(64)}`
  });
  const calls = [];
  const serve = (sums) =>
    t.mock.method(globalThis, 'fetch', async (url, opts = {}) => {
      url = String(url);
      calls.push(url);
      if (url.startsWith('https://api.github.com/')) return new Response('{"message":"API rate limit exceeded"}', { status: 403 });
      if (url === 'https://github.com/o/r/releases/latest') {
        assert.equal(opts.redirect, 'manual');
        return new Response(null, { status: 302, headers: { location: 'https://github.com/o/r/releases/tag/v2.2.0' } });
      }
      if (url.endsWith('/SHA256SUMS.txt')) return sums ? new Response(sums) : new Response('Not Found', { status: 404 });
      if (url === 'https://github.com/o/r/releases/download/v2.2.0/Lounge-FSE-2.2.0.zip') return new Response(payload, { headers: { 'content-length': String(payload.length) } });
      return new Response('?', { status: 500 });
    });

  serve(`${sha}  Lounge-FSE-2.2.0.zip\n`);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lounge-upd-'));
  const u = new Updater({ repo: 'o/r', version: '2.1.2', installType: 'fse', dir });
  const st = await u.check();
  assert.equal(st.status, 'available');
  assert.equal(st.version, '2.2.0');
  await u.download();
  assert.equal(u.state.status, 'ready');
  assert.ok(fs.readFileSync(u.file).equals(payload));
  t.mock.restoreAll();

  // Already up to date: no downloads at all.
  serve(null);
  const current = new Updater({ repo: 'o/r', version: '2.2.0', installType: 'fse', dir });
  assert.equal((await current.check()).status, 'uptodate');
  t.mock.restoreAll();

  // A newer release without a checksum file: nothing to verify against, so no update is offered.
  serve(null);
  const older = new Updater({ repo: 'o/r', version: '2.1.2', installType: 'fse', dir });
  const err = await older.check();
  assert.equal(err.status, 'error');
  assert.match(err.error, /rate limited/);
});

test('FSE install: follows the script until the installer has admin rights', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lounge-upd-'));
  const status = path.join(dir, 'fse-status.txt');
  const u = new Updater({ repo: 'o/r', version: '2.1.2', installType: 'fse', dir });
  const fast = { unpackMs: 200, promptMs: 150, everyMs: 10 };
  const after = (ms, text) => setTimeout(() => fs.writeFileSync(status, text + '\r\n'), ms);
  after(20, 'unpacking');
  after(60, 'asking');
  after(100, 'elevated');
  assert.equal(await u.waitForFse(status, fast), 'elevated');
  fs.writeFileSync(status, 'declined');
  assert.equal(await u.waitForFse(status, fast), 'declined');
  fs.writeFileSync(status, 'asking'); // the prompt never answered (the full screen experience hides it)
  assert.equal(await u.waitForFse(status, fast), 'noprompt');
  fs.writeFileSync(status, "failed: installer not found in the update");
  assert.equal(await u.waitForFse(status, fast), 'failed: installer not found in the update');
  fs.rmSync(status);
  assert.match(await u.waitForFse(status, fast), /^failed: .*didn't start/);
  assert.equal(require('../src/updater.js').wmiLaunch("a 'b'"), "$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = 'a ''b''' }; exit [int]$r.ReturnValue");
  fs.rmSync(dir, { recursive: true, force: true });
});
