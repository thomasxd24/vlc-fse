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
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-inst-'));
  const exePath = path.join(dir, 'Foyer.exe');
  assert.equal(detectInstallType({ packaged: false, exePath, platform: 'win32' }), null, 'dev builds never update');
  assert.equal(detectInstallType({ packaged: true, exePath, platform: 'linux' }), null);
  assert.equal(detectInstallType({ packaged: true, windowsStore: true, exePath, platform: 'win32' }), 'fse');
  assert.equal(detectInstallType({ packaged: true, exePath, platform: 'win32' }), 'zip');
  fs.writeFileSync(path.join(dir, 'Uninstall Foyer.exe'), '');
  assert.equal(detectInstallType({ packaged: true, exePath, platform: 'win32' }), 'nsis');
  fs.rmSync(dir, { recursive: true, force: true });
});

function release(payload, digestOf = payload) {
  const sha = crypto.createHash('sha256').update(digestOf).digest('hex');
  const assets = ['Foyer-Setup-2.1.0.exe', 'Foyer-2.1.0-win-x64.zip', 'Foyer-FSE-2.1.0.zip', 'Foyer-Setup-2.1.0.exe.blockmap'].map((name) => ({
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
  for (const [type, name] of [['nsis', 'Foyer-Setup-2.1.0.exe'], ['zip', 'Foyer-2.1.0-win-x64.zip'], ['fse', 'Foyer-FSE-2.1.0.zip']]) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-upd-'));
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
    const cmd = await u.installCommand({ pid: 123, exePath: path.join(dir, 'Foyer.exe') });
    if (type === 'nsis') assert.deepEqual(cmd.args, ['/S', '--updated', '--force-run']);
    else {
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
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-upd-'));
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
