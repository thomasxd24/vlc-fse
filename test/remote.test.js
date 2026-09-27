'use strict';

// Browses and downloads from a real SFTP server (ssh2's own, in-process, serving a temp folder), then runs the
// transfer queue end to end: plan, download to .part, rename, skip what's already there.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');
const { Server, utils } = require('ssh2');
const { connect } = require('../src/remote');
const { TransferQueue } = require('../src/transfers');
const { planTransfer } = require('../src/transfer-plan');

const { STATUS_CODE, OPEN_MODE } = utils.sftp;

function tree(root, files) {
  for (const [rel, content] of Object.entries(files)) {
    const p = path.join(root, rel);
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(p, content);
  }
}

/** A minimal read-only SFTP server over `root` (enough for readdir, realpath and fastGet). */
function sftpServer(root, password) {
  const hostKey = utils.generateKeyPairSync('ed25519').private;
  const server = new Server({ hostKeys: [hostKey] }, (client) => {
    client.on('authentication', (ctx) => (ctx.method === 'password' && ctx.password === password ? ctx.accept() : ctx.reject(['password'])));
    client.on('error', () => {});
    client.on('ready', () => {
      client.on('session', (accept) => {
        accept().on('sftp', (acceptSftp) => {
          const sftp = acceptSftp();
          const handles = new Map();
          let next = 0;
          const local = (p) => path.join(root, path.posix.normalize(`/${p}`));
          const attrs = (st) => ({ mode: st.mode, uid: 0, gid: 0, size: st.size, atime: st.atime / 1000, mtime: st.mtime / 1000 });
          const handle = (v) => {
            const b = Buffer.alloc(4);
            b.writeUInt32BE(next);
            handles.set(next++, v);
            return b;
          };
          const get = (b) => handles.get(b.readUInt32BE(0));
          sftp.on('REALPATH', (id, p) => sftp.name(id, [{ filename: path.posix.normalize(`/${p === '.' ? '' : p}`), longname: '', attrs: {} }]));
          sftp.on('STAT', (id, p) => {
            try {
              sftp.attrs(id, attrs(fs.statSync(local(p))));
            } catch {
              sftp.status(id, STATUS_CODE.NO_SUCH_FILE);
            }
          });
          sftp.on('OPENDIR', (id, p) => {
            try {
              const names = fs.readdirSync(local(p));
              sftp.handle(id, handle({ dir: local(p), names, done: false }));
            } catch {
              sftp.status(id, STATUS_CODE.NO_SUCH_FILE);
            }
          });
          sftp.on('READDIR', (id, h) => {
            const d = get(h);
            if (!d || d.done) return sftp.status(id, STATUS_CODE.EOF);
            d.done = true;
            sftp.name(id, d.names.map((n) => ({ filename: n, longname: n, attrs: attrs(fs.statSync(path.join(d.dir, n))) })));
          });
          sftp.on('OPEN', (id, p, flags) => {
            if (!(flags & OPEN_MODE.READ)) return sftp.status(id, STATUS_CODE.PERMISSION_DENIED);
            try {
              sftp.handle(id, handle({ fd: fs.openSync(local(p), 'r') }));
            } catch {
              sftp.status(id, STATUS_CODE.NO_SUCH_FILE);
            }
          });
          sftp.on('FSTAT', (id, h) => sftp.attrs(id, attrs(fs.fstatSync(get(h).fd))));
          sftp.on('READ', (id, h, offset, len) => {
            const buf = Buffer.alloc(len);
            const n = fs.readSync(get(h).fd, buf, 0, len, offset);
            if (!n) return sftp.status(id, STATUS_CODE.EOF);
            sftp.data(id, buf.subarray(0, n));
          });
          sftp.on('CLOSE', (id, h) => {
            const v = get(h);
            if (v && v.fd !== undefined) fs.closeSync(v.fd);
            handles.delete(h.readUInt32BE(0));
            sftp.status(id, STATUS_CODE.OK);
          });
        });
      });
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, port: server.address().port })));
}

test('SFTP: browse, trust the host key on first use, refuse a wrong password or a changed key', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-sftp-'));
  tree(root, { 'Movies/Heat (1995)/Heat.1995.mkv': 'x', 'TV/readme.txt': 'y' });
  const { server, port } = await sftpServer(root, 'pw');
  t.after(() => server.close());
  const base = { protocol: 'sftp', host: '127.0.0.1', port, username: 'me', authType: 'password' };

  let fingerprint = null;
  const c = await connect(base, 'pw', { onHostKey: (k) => (fingerprint = k) });
  assert.match(fingerprint, /^[0-9a-f]{64}$/);
  assert.deepEqual(await c.list('/'), [
    { name: 'Movies', isDir: true, size: fs.statSync(path.join(root, 'Movies')).size },
    { name: 'TV', isDir: true, size: fs.statSync(path.join(root, 'TV')).size }
  ]);
  assert.deepEqual(await c.walk('/Movies'), [{ remote: '/Movies/Heat (1995)/Heat.1995.mkv', rel: 'Heat (1995)/Heat.1995.mkv', size: 1 }]);
  await assert.rejects(c.list('/nope'), { code: 'notFound' });
  c.close();

  const again = await connect({ ...base, hostKey: fingerprint }, 'pw');
  again.close();
  await assert.rejects(connect({ ...base, hostKey: fingerprint }, 'wrong'), { code: 'auth' });
  await assert.rejects(connect({ ...base, hostKey: 'f'.repeat(64) }, 'pw'), { code: 'hostKeyChanged' });
});

test('transfer queue: downloads a planned season pack into the library, then skips it next time', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-sftp-'));
  const big = crypto.randomBytes(3 * 1024 * 1024); // big enough for several parallel fastGet chunks
  tree(root, {
    'dl/Fallout S02 1080p WEB x265-GRP/Fallout.S02E01.1080p.mkv': big,
    'dl/Fallout S02 1080p WEB x265-GRP/Fallout.S02E01.1080p.en.srt': 'subs',
    'dl/Fallout S02 1080p WEB x265-GRP/Sample/sample.mkv': 'no'
  });
  const { server, port } = await sftpServer(root, 'pw');
  t.after(() => server.close());
  const srv = { protocol: 'sftp', host: '127.0.0.1', port, username: 'me', authType: 'password' };

  const lib = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-lib-'));
  const tv = path.join(lib, 'TV');
  fs.mkdirSync(path.join(tv, 'Fallout (2024)'), { recursive: true });

  const browse = await connect(srv, 'pw');
  const dir = '/dl/Fallout S02 1080p WEB x265-GRP';
  const plan = planTransfer({ name: path.posix.basename(dir), isDir: true }, await browse.walk(dir), { tvRoot: tv, movieRoot: path.join(lib, 'Movies'), existingShowDirs: fs.readdirSync(tv) });
  browse.close();
  assert.equal(plan.items.length, 2);

  const queue = new TransferQueue({ connect: () => connect(srv, 'pw') });
  const done = () => new Promise((resolve) => queue.once('finished', resolve));
  let finished = done();
  queue.add({ serverId: 's', title: 'Fallout S02', plan });
  let job = await finished;
  assert.equal(job.status, 'done', job.error);
  const season = path.join(tv, 'Fallout (2024)', 'Season 02');
  assert.deepEqual(fs.readdirSync(season).sort(), ['Fallout.S02E01.1080p.en.srt', 'Fallout.S02E01.1080p.mkv']);
  assert.ok(fs.readFileSync(path.join(season, 'Fallout.S02E01.1080p.mkv')).equals(big));
  assert.equal(queue.state[0].bytesDone, plan.totalSize);

  finished = done();
  queue.add({ serverId: 's', title: 'Fallout S02', plan });
  job = await finished;
  assert.equal(job.status, 'done');
  assert.equal(job.skipped, 2);
  queue.clear();
  assert.equal(queue.state.length, 0);
});

test('transfer queue: cancelling stops the download and leaves no finished file behind', async () => {
  const lib = fs.mkdtempSync(path.join(os.tmpdir(), 'foyer-lib-'));
  const dest = path.join(lib, 'Movies', 'Heat (1995)', 'Heat.mkv');
  let closed = false;
  let release;
  const client = {
    download: (remote, local, onBytes) =>
      new Promise((resolve, reject) => {
        fs.writeFileSync(local, 'part');
        onBytes(4);
        release = () => reject(new Error('connection closed'));
      }),
    close() {
      closed = true;
      if (release) release();
    }
  };
  const queue = new TransferQueue({ connect: async () => client });
  const finished = new Promise((resolve) => queue.once('finished', resolve));
  const id = queue.add({ serverId: 's', title: 'Heat', plan: { kind: 'movie', folders: [], totalSize: 10, items: [{ remote: '/Heat.mkv', rel: 'Heat.mkv', size: 10, dest }] } });
  await new Promise((r) => setTimeout(r, 20));
  queue.cancel(id);
  const job = await finished;
  assert.equal(job.status, 'cancelled');
  assert.ok(closed);
  assert.ok(!fs.existsSync(dest));
  assert.ok(fs.existsSync(`${dest}.part`)); // kept so FTP can resume it
});
