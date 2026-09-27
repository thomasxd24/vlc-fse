'use strict';

// Browsing and downloading from an SFTP, FTP or FTPS server, behind one small interface:
//   list(dir) -> [{name, isDir, size}]   walk(dir) -> [{remote, rel, size}]   download(remote, local, onBytes)   close()

const fs = require('fs');
const path = require('path');
const { Client: SshClient } = require('ssh2');
const ftp = require('basic-ftp');

const TIMEOUT_MS = 15000;
const WALK_MAX_DEPTH = 6;
const WALK_MAX_FILES = 5000;

const posix = path.posix;
const DEFAULT_PORTS = { sftp: 22, ftp: 21, ftps: 21 };

class RemoteError extends Error {
  constructor(code, message) {
    super(message || code);
    this.code = code;
  }
}

function sortEntries(list) {
  return list.sort((a, b) => Number(b.isDir) - Number(a.isDir) || a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }));
}

/** Everything below `dir`, depth-first, with paths relative to it. Hidden entries are skipped. */
async function walk(client, dir, depth = 0, rel = '', out = []) {
  if (depth > WALK_MAX_DEPTH || out.length >= WALK_MAX_FILES) return out;
  for (const e of await client.list(dir)) {
    if (e.name.startsWith('.')) continue;
    const r = rel ? `${rel}/${e.name}` : e.name;
    if (e.isDir) await walk(client, posix.join(dir, e.name), depth + 1, r, out);
    else out.push({ remote: posix.join(dir, e.name), rel: r, size: e.size });
  }
  return out;
}

// ---------------------------------------------------------------------------- SFTP

/**
 * Host keys are trusted on first use: `hostKey` is the SHA-256 fingerprint seen last time. A new server
 * reports its fingerprint through `onHostKey` so it can be remembered; a changed one refuses to connect.
 */
function connectSftp(server, secret, { onHostKey } = {}) {
  return new Promise((resolve, reject) => {
    const conn = new SshClient();
    let seenKey = null;
    const cfg = {
      host: server.host,
      port: Number(server.port) || 22,
      username: server.username,
      readyTimeout: TIMEOUT_MS,
      keepaliveInterval: 10000,
      hostHash: 'sha256',
      hostVerifier: (fingerprint) => {
        seenKey = fingerprint;
        return !server.hostKey || server.hostKey === fingerprint;
      }
    };
    if (server.authType === 'key') {
      try {
        cfg.privateKey = fs.readFileSync(server.keyPath);
      } catch {
        return reject(new RemoteError('keyUnreadable'));
      }
      if (secret) cfg.passphrase = secret;
    } else {
      cfg.password = secret || '';
    }
    conn.once('ready', () => {
      if (!server.hostKey && seenKey && onHostKey) onHostKey(seenKey);
      conn.sftp((err, sftp) => {
        if (err) {
          conn.end();
          return reject(err);
        }
        resolve(sftpClient(conn, sftp));
      });
    });
    conn.once('error', (err) => {
      if (server.hostKey && seenKey && seenKey !== server.hostKey) return reject(new RemoteError('hostKeyChanged'));
      if (err.level === 'client-authentication') return reject(new RemoteError('auth'));
      reject(err);
    });
    conn.connect(cfg);
  });
}

function sftpClient(conn, sftp) {
  const client = {
    list(dir) {
      return new Promise((resolve, reject) => {
        sftp.readdir(dir || '/', (err, list) => {
          if (err) return reject(err.code === 2 ? new RemoteError('notFound') : err);
          resolve(sortEntries(list.map((e) => ({ name: e.filename, isDir: e.attrs.isDirectory(), size: e.attrs.size || 0 }))));
        });
      });
    },
    walk: (dir) => walk(client, dir),
    download(remote, local, onBytes) {
      return new Promise((resolve, reject) => {
        sftp.fastGet(remote, local, { step: (done) => onBytes && onBytes(done) }, (err) => (err ? reject(err) : resolve()));
      });
    },
    home() {
      return new Promise((resolve) => sftp.realpath('.', (err, p) => resolve(err ? '/' : p)));
    },
    close() {
      conn.end();
    }
  };
  return client;
}

// ---------------------------------------------------------------------------- FTP / FTPS

async function connectFtp(server, secret) {
  const c = new ftp.Client(TIMEOUT_MS);
  try {
    await c.access({
      host: server.host,
      port: Number(server.port) || 21,
      user: server.username || 'anonymous',
      password: secret || '',
      secure: server.protocol === 'ftps',
      // Self-signed certificates (common on home NAS boxes) only when the user said so for this server.
      secureOptions: server.protocol === 'ftps' && server.insecureTls ? { rejectUnauthorized: false } : undefined
    });
  } catch (err) {
    c.close();
    if (err && err.code === 530) throw new RemoteError('auth');
    throw err;
  }
  const client = {
    async list(dir) {
      try {
        const list = await c.list(dir || '/');
        return sortEntries(list.filter((e) => e.name !== '.' && e.name !== '..').map((e) => ({ name: e.name, isDir: e.isDirectory, size: e.size || 0 })));
      } catch (err) {
        if (err && err.code === 550) throw new RemoteError('notFound');
        throw err;
      }
    },
    walk: (dir) => walk(client, dir),
    async download(remote, local, onBytes) {
      // Carry on from a partial download left by an earlier attempt.
      let start = 0;
      try {
        start = fs.statSync(local).size;
      } catch {}
      c.trackProgress((info) => onBytes && onBytes(start + info.bytes));
      try {
        await c.downloadTo(local, remote, start);
      } finally {
        c.trackProgress();
      }
    },
    async home() {
      return c.pwd().catch(() => '/');
    },
    close() {
      c.close();
    }
  };
  return client;
}

/**
 * Connect to a saved server.
 * @param {{protocol: 'sftp'|'ftp'|'ftps', host: string, port?: number, username?: string, authType?: 'password'|'key', keyPath?: string, hostKey?: string, insecureTls?: boolean}} server
 * @param {string} secret  password, or key passphrase
 */
function connect(server, secret, opts) {
  if (!server || !server.host) return Promise.reject(new RemoteError('noHost'));
  return server.protocol === 'sftp' ? connectSftp(server, secret, opts) : connectFtp(server, secret);
}

module.exports = { connect, RemoteError, DEFAULT_PORTS };
