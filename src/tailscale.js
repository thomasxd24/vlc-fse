'use strict';

// Tailscale without its tray icon: the full screen experience has no taskbar or notification area, so Foyer
// drives Tailscale's own CLI (tailscale.exe, installed next to the tray app) for status, connecting,
// disconnecting, exit nodes and signing in.

const fs = require('fs');
const path = require('path');
const { execFile, spawn } = require('child_process');

const AUTH_URL = /https:\/\/login\.tailscale\.com\/a\/[\w-]+|https:\/\/[\w.-]+\/a\/[\w-]{6,}/;

function findCli(env = process.env) {
  const candidates =
    process.platform === 'win32'
      ? [env.ProgramW6432, env.ProgramFiles, env['ProgramFiles(x86)']].filter(Boolean).map((p) => path.join(p, 'Tailscale', 'tailscale.exe'))
      : ['/usr/bin/tailscale', '/usr/local/bin/tailscale', '/Applications/Tailscale.app/Contents/MacOS/Tailscale'];
  return candidates.find((p) => {
    try {
      return fs.statSync(p).isFile();
    } catch {
      return false;
    }
  }) || null;
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
    this.login = null; // the running `tailscale up` waiting for sign-in
  }

  get installed() {
    if (!this.cli) this.cli = findCli(this.env);
    return Boolean(this.cli);
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

module.exports = { Tailscale, parseStatus, findCli, AUTH_URL };
