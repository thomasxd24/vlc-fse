'use strict';

const { spawn, execFile } = require('child_process');
const { EventEmitter } = require('events');
const fs = require('fs');
const net = require('net');
const path = require('path');
const crypto = require('crypto');

const POLL_MS = 1500;

function exists(p) {
  try {
    return fs.statSync(p).isFile();
  } catch {
    return false;
  }
}

function regQuery(key) {
  return new Promise((resolve) => {
    execFile('reg', ['query', key, '/ve'], { windowsHide: true }, (err, stdout) => {
      if (err) return resolve(null);
      const m = /REG_SZ\s+(.+)/.exec(stdout);
      resolve(m ? m[1].trim() : null);
    });
  });
}

function which(cmd) {
  const dirs = (process.env.PATH || '').split(path.delimiter);
  const exts = process.platform === 'win32' ? ['.exe', '.cmd', '.bat', ''] : [''];
  for (const d of dirs) {
    for (const e of exts) {
      const p = path.join(d, cmd + e);
      if (exists(p)) return p;
    }
  }
  return null;
}

/** Locate the VLC executable. Returns an absolute path or null. */
async function findVlc(preferred) {
  if (preferred && exists(preferred)) return preferred;

  if (process.platform === 'win32') {
    for (const key of [
      'HKLM\\SOFTWARE\\VideoLAN\\VLC',
      'HKLM\\SOFTWARE\\WOW6432Node\\VideoLAN\\VLC',
      'HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\vlc.exe',
      'HKCU\\SOFTWARE\\VideoLAN\\VLC'
    ]) {
      const v = await regQuery(key);
      if (v && exists(v)) return v;
    }
    const roots = [process.env.ProgramFiles, process.env['ProgramFiles(x86)'], process.env.ProgramW6432, process.env.LOCALAPPDATA && path.join(process.env.LOCALAPPDATA, 'Programs')];
    for (const r of roots.filter(Boolean)) {
      const p = path.join(r, 'VideoLAN', 'VLC', 'vlc.exe');
      if (exists(p)) return p;
    }
  } else if (process.platform === 'darwin') {
    const p = '/Applications/VLC.app/Contents/MacOS/VLC';
    if (exists(p)) return p;
  }
  return which('vlc');
}

function freePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.on('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

/**
 * Build VLC's command line. Each queue item may carry a start offset, applied as a per-item ":start-time" option.
 * @param {{path: string, startTime?: number}[]} queue
 */
function buildArgs(queue, { port, password, fullscreen = true, extraArgs = [] }) {
  const args = [
    '--extraintf=http',
    '--http-host=127.0.0.1',
    `--http-port=${port}`,
    `--http-password=${password}`,
    '--play-and-exit',
    '--no-random',
    '--no-loop',
    '--no-repeat',
    '--qt-continue=0', // this launcher owns resume; don't let VLC ask as well
    '--no-qt-privacy-ask', // VLC's first-run network-policy dialog would otherwise block fullscreen playback
    '--no-one-instance-when-started-from-file', // on/off options take --no-, never =0 (VLC refuses to start)
    '--no-one-instance'
  ];
  if (fullscreen) args.push('--fullscreen');
  args.push(...extraArgs);
  for (const item of queue) {
    args.push(item.path);
    if (item.startTime && item.startTime > 5) args.push(`:start-time=${Math.floor(item.startTime)}`);
  }
  return args;
}

/**
 * A single VLC playback session. Emits:
 *  - 'progress' {path, time, length}  roughly every POLL_MS while playing
 *  - 'exit'     {code, last}          when VLC closes
 */
class VlcSession extends EventEmitter {
  constructor(vlcPath, queue, opts = {}) {
    super();
    this.vlcPath = vlcPath;
    this.queue = queue;
    this.opts = opts;
    this.password = crypto.randomBytes(12).toString('hex');
    this.last = null;
    this.timer = null;
    this.child = null;
  }

  async start() {
    this.port = await freePort();
    const args = buildArgs(this.queue, { ...this.opts, port: this.port, password: this.password });
    this.child = spawn(this.vlcPath, args, { detached: false, stdio: 'ignore', windowsHide: false });
    this.child.on('error', (err) => {
      this.stop();
      this.emit('error', err);
    });
    this.child.on('exit', (code) => {
      this.stop();
      this.emit('exit', { code, last: this.last });
    });
    this.timer = setInterval(() => this.poll().catch(() => {}), POLL_MS);
  }

  stop() {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  kill() {
    this.stop();
    if (this.child && this.child.exitCode === null) this.child.kill();
  }

  async request(file) {
    const auth = Buffer.from(`:${this.password}`).toString('base64');
    const res = await fetch(`http://127.0.0.1:${this.port}/requests/${file}`, {
      headers: { Authorization: `Basic ${auth}` },
      signal: AbortSignal.timeout(1200)
    });
    if (!res.ok) throw new Error(`VLC HTTP ${res.status}`);
    return res.json();
  }

  /** Map VLC's idea of the current item back to one of our queue entries. */
  resolveCurrent(status) {
    const meta = status?.information?.category?.meta || {};
    const name = meta.filename;
    if (name) {
      const hit = this.queue.find((q) => path.basename(q.path) === name || path.basename(q.path, path.extname(q.path)) === name);
      if (hit) return hit;
    }
    if (this.queue.length === 1) return this.queue[0];
    return null;
  }

  async poll() {
    const status = await this.request('status.json');
    if (!status || status.state === 'stopped') return;
    let item = this.resolveCurrent(status);
    if (!item && status.currentplid >= 0) {
      // Fall back to the playlist: items appear in queue order.
      const pl = await this.request('playlist.json');
      const leaves = [];
      const collect = (n) => (n.children ? n.children.forEach(collect) : n.type === 'leaf' && leaves.push(n));
      collect(pl);
      const idx = leaves.findIndex((l) => String(l.id) === String(status.currentplid));
      if (idx >= 0 && idx < this.queue.length) item = this.queue[idx];
    }
    if (!item || !status.length) return;
    if (this.last && this.last.path !== item.path && this.queue.indexOf(item) > this.queue.findIndex((q) => q.path === this.last.path)) {
      // VLC moved on to a later item, so the previous one played to the end (or was skipped on purpose).
      this.emit('progress', { path: this.last.path, time: this.last.length, length: this.last.length });
    }
    this.last = { path: item.path, time: status.time, length: status.length };
    this.emit('progress', this.last);
  }
}

module.exports = { findVlc, buildArgs, VlcSession };
