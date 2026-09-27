'use strict';

// Tailscale without its tray icon: the full screen experience has no taskbar or notification area, so Lounge
// drives Tailscale's own CLI (tailscale.exe, installed next to the tray app) for status, connecting,
// disconnecting, exit nodes and signing in.

const fs = require('fs');
const path = require('path');
const { execFile, spawn } = require('child_process');

const AUTH_URL = /https:\/\/login\.tailscale\.com\/a\/[\w-]+|https:\/\/[\w.-]+\/a\/[\w-]{6,}/;

const fileExists = (p) => {
  try {
    return fs.statSync(p).isFile();
  } catch {
    return false;
  }
};

function regQuery(key, value) {
  return new Promise((resolve) => {
    if (process.platform !== 'win32') return resolve(null);
    execFile('reg', ['query', key, '/v', value], { windowsHide: true, timeout: 5000 }, (err, stdout) => {
      if (err) return resolve(null);
      const m = new RegExp(`${value}\\s+REG_\\w+\\s+(.*)`, 'i').exec(stdout);
      resolve(m ? m[1].trim() : null);
    });
  });
}

/** The executable in a service ImagePath or command line: '"C:\\x y\\a.exe" -arg' or 'C:\\x\\a.exe -arg'. */
function exeOfCommand(cmd) {
  const m = /^\s*"([^"]+)"|^\s*(\S+)/.exec(String(cmd || ''));
  return m ? (m[1] || m[2]).replace(/^\\\?\?\\/, '') : null;
}

/**
 * Find Tailscale's CLI (tailscale.exe, installed next to the tray app and the service). Tries, in order: the
 * standard install folders, the folder of the Tailscale Windows service (its registered ImagePath, readable
 * without admin rights), folders given as hints (e.g. where the Start menu's Tailscale app lives), and PATH.
 * Returns { cli, tried: [{path, source, found}] } so Settings can show where it looked.
 */
async function locateCli({ env = process.env, hints = [], exists = fileExists, query = regQuery, platform = process.platform } = {}) {
  const tried = [];
  const check = (p, source) => {
    if (!p || tried.some((t) => t.path.toLowerCase() === p.toLowerCase())) return null;
    const found = exists(p);
    tried.push({ path: p, source, found });
    return found ? p : null;
  };
  if (platform !== 'win32') {
    for (const p of ['/usr/bin/tailscale', '/usr/local/bin/tailscale', '/Applications/Tailscale.app/Contents/MacOS/Tailscale']) {
      const hit = check(p, 'default');
      if (hit) return { cli: hit, tried };
    }
    return { cli: null, tried };
  }
  const win = path.win32;
  for (const base of [env.ProgramW6432, env.ProgramFiles, env['ProgramFiles(x86)']].filter(Boolean)) {
    const hit = check(win.join(base, 'Tailscale', 'tailscale.exe'), 'default');
    if (hit) return { cli: hit, tried };
  }
  const image = exeOfCommand(await query('HKLM\\SYSTEM\\CurrentControlSet\\Services\\Tailscale', 'ImagePath'));
  if (image) {
    const hit = check(win.join(win.dirname(image), 'tailscale.exe'), 'service');
    if (hit) return { cli: hit, tried };
  }
  for (const dir of hints.filter(Boolean)) {
    const hit = check(win.join(dir, 'tailscale.exe'), 'startMenu');
    if (hit) return { cli: hit, tried };
  }
  for (const dir of String(env.PATH || env.Path || '').split(';').filter(Boolean)) {
    if (!exists(win.join(dir, 'tailscale.exe'))) continue;
    return { cli: check(win.join(dir, 'tailscale.exe'), 'path'), tried };
  }
  return { cli: null, tried };
}

/** Quick synchronous guess at startup (the standard folders); locateCli() does the full search. */
function findCli(env = process.env) {
  const candidates =
    process.platform === 'win32'
      ? [env.ProgramW6432, env.ProgramFiles, env['ProgramFiles(x86)']].filter(Boolean).map((p) => path.join(p, 'Tailscale', 'tailscale.exe'))
      : ['/usr/bin/tailscale', '/usr/local/bin/tailscale', '/Applications/Tailscale.app/Contents/MacOS/Tailscale'];
  return candidates.find(fileExists) || null;
}

function run(cli, args, timeout = 15000) {
  return new Promise((resolve, reject) => {
    execFile(cli, args, { windowsHide: true, timeout, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 }, (err, stdout, stderr) => {
      if (err) {
        err.message = String(stderr || stdout || err.message).trim().split(/\r?\n/)[0] || err.message;
        return reject(err);
      }
      resolve(stdout);
    });
  });
}

const shortName = (p) => (p.DNSName ? p.DNSName.split('.')[0] : p.HostName) || p.HostName || '';

/**
 * Shape `tailscale status --json` for the UI.
 * state: 'connected' | 'stopped' | 'needsLogin' | 'starting' | 'unknown'
 */
function parseStatus(json) {
  const s = typeof json === 'string' ? JSON.parse(json) : json;
  const backend = s.BackendState || '';
  const state = { Running: 'connected', Stopped: 'stopped', NeedsLogin: 'needsLogin', NeedsMachineAuth: 'needsLogin', Starting: 'starting', NoState: 'starting' }[backend] || 'unknown';
  const peers = Object.values(s.Peer || {});
  const exitNode = peers.find((p) => p.ExitNode) || null;
  const self = s.Self || {};
  return {
    state,
    tailnet: (s.CurrentTailnet && s.CurrentTailnet.Name) || null,
    user: (s.User && self.UserID !== undefined && s.User[self.UserID] && s.User[self.UserID].LoginName) || null,
    hostName: shortName(self) || null,
    ip: (self.TailscaleIPs || []).find((ip) => ip.includes('.')) || (self.TailscaleIPs || [])[0] || null,
    authUrl: s.AuthURL || null,
    peersOnline: peers.filter((p) => p.Online).length,
    peersTotal: peers.length,
    exitNode: exitNode ? { name: shortName(exitNode), ip: (exitNode.TailscaleIPs || [])[0] || null } : null,
    exitNodes: peers
      .filter((p) => p.ExitNodeOption)
      .map((p) => ({ name: shortName(p), ip: (p.TailscaleIPs || []).find((ip) => ip.includes('.')) || (p.TailscaleIPs || [])[0], online: Boolean(p.Online), active: Boolean(p.ExitNode) }))
      .sort((a, b) => Number(b.online) - Number(a.online) || a.name.localeCompare(b.name))
  };
}

class Tailscale {
  constructor(env = process.env) {
    this.env = env;
    this.cli = findCli(env);
    this.tried = [];
    this.login = null; // the running `tailscale up` waiting for sign-in
  }

  get installed() {
    return Boolean(this.cli);
  }

  /** Search everywhere Tailscale might be (see locateCli); `hints` are extra folders to try. */
  async locate(hints = []) {
    const r = await locateCli({ env: this.env, hints });
    this.cli = r.cli;
    this.tried = r.tried;
    return r;
  }

  async status() {
    if (!this.installed) return { installed: false };
    try {
      return { installed: true, ...parseStatus(await run(this.cli, ['status', '--json'])) };
    } catch (err) {
      // "failed to connect to local tailscaled" when the service isn't running.
      return { installed: true, state: 'unknown', error: err.message };
    }
  }

  async up() {
    await run(this.cli, ['up'], 30000);
  }

  async down() {
    await run(this.cli, ['down']);
  }

  /** Route all traffic through a peer (its IP or name), or stop using an exit node (null). */
  async setExitNode(node) {
    await run(this.cli, ['set', `--exit-node=${node || ''}`]);
  }

  /**
   * Start signing in: runs `tailscale up`, which prints a login URL and waits until the sign-in completes in a
   * browser (on this PC or a phone). Resolves with the URL; `onDone(ok)` is called when the process ends.
   */
  startLogin(onDone) {
    if (this.login) {
      return this.login.url ? Promise.resolve(this.login.url) : this.login.urlPromise;
    }
    // Plain `up`: any settings flag would make Tailscale insist on restating every non-default setting.
    const child = spawn(this.cli, ['up'], { windowsHide: true });
    const job = { child, url: null };
    this.login = job;
    job.urlPromise = new Promise((resolve, reject) => {
      let buf = '';
      const scan = (chunk) => {
        buf += chunk.toString();
        const m = AUTH_URL.exec(buf);
        if (m && !job.url) {
          job.url = m[0];
          resolve(job.url);
        }
      };
      child.stdout.on('data', scan);
      child.stderr.on('data', scan);
      child.once('error', (err) => {
        this.login = null;
        reject(err);
      });
      child.once('exit', (code) => {
        this.login = null;
        if (!job.url) {
          // Already signed in: `up` just connects and exits without a URL.
          if (code === 0) resolve(null);
          else reject(new Error(buf.trim().split(/\r?\n/).pop() || `tailscale exited with code ${code}`));
        }
        if (onDone) onDone(code === 0);
      });
    });
    return job.urlPromise;
  }

  cancelLogin() {
    if (this.login) this.login.child.kill();
    this.login = null;
  }
}

module.exports = { Tailscale, parseStatus, findCli, locateCli, exeOfCommand, AUTH_URL };
