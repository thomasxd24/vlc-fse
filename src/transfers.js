'use strict';

// The download queue: one job (a planned file or folder) at a time, each file written to "<name>.part" and
// renamed once complete, so the library scan never picks up half a film.

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { EventEmitter } = require('events');

const EMIT_MS = 500;
const KEEP_FINISHED = 20;

class TransferQueue extends EventEmitter {
  /** @param {{connect: (serverId: string) => Promise<object>}} opts  opens a client (see remote.js) for a server */
  constructor({ connect }) {
    super();
    this.connect = connect;
    this.jobs = [];
    this.running = null;
    this.emitTimer = null;
  }

  /** What the UI shows. */
  get state() {
    return this.jobs.map((j) => ({
      id: j.id,
      title: j.title,
      kind: j.kind,
      serverId: j.serverId,
      status: j.status,
      files: j.items.length,
      fileIndex: j.fileIndex,
      bytesDone: j.bytesDone,
      bytesTotal: j.bytesTotal,
      rate: j.rate,
      error: j.error,
      folders: j.folders,
      skipped: j.skipped
    }));
  }

  get active() {
    return this.jobs.some((j) => j.status === 'queued' || j.status === 'running');
  }

  changed(now = false) {
    if (now) {
      clearTimeout(this.emitTimer);
      this.emitTimer = null;
      this.emit('update', this.state);
      return;
    }
    if (this.emitTimer) return;
    this.emitTimer = setTimeout(() => {
      this.emitTimer = null;
      this.emit('update', this.state);
    }, EMIT_MS);
  }

  /** Queue a planned download (see transfer-plan.js). Returns the job id. */
  add({ serverId, title, plan }) {
    const job = {
      id: crypto.randomBytes(6).toString('hex'),
      serverId,
      title,
      kind: plan.kind,
      folders: plan.folders,
      items: plan.items,
      status: 'queued',
      fileIndex: 0,
      bytesDone: 0,
      bytesTotal: plan.totalSize,
      rate: 0,
      skipped: 0,
      error: null,
      client: null
    };
    this.jobs.push(job);
    this.changed(true);
    this.next();
    return job.id;
  }

  cancel(id) {
    const job = this.jobs.find((j) => j.id === id);
    if (!job) return;
    if (job.status === 'queued') job.status = 'cancelled';
    else if (job.status === 'running') {
      job.status = 'cancelled';
      if (job.client) job.client.close(); // aborts the file being downloaded
    }
    this.changed(true);
  }

  /** Run a failed or cancelled job again, as a new job (files already downloaded are skipped). */
  retry(id) {
    const job = this.jobs.find((j) => j.id === id);
    if (!job || job.status === 'queued' || job.status === 'running') return null;
    this.jobs = this.jobs.filter((j) => j !== job);
    return this.add({ serverId: job.serverId, title: job.title, plan: { kind: job.kind, folders: job.folders, items: job.items, totalSize: job.bytesTotal } });
  }

  /** Forget finished, failed and cancelled jobs (or just one of them). */
  clear(id) {
    this.jobs = this.jobs.filter((j) => (id ? j.id !== id : false) || j.status === 'queued' || j.status === 'running');
    this.changed(true);
  }

  async next() {
    if (this.running) return;
    const job = this.jobs.find((j) => j.status === 'queued');
    if (!job) return;
    this.running = job;
    try {
      await this.run(job);
      if (job.status === 'running') job.status = 'done';
    } catch (err) {
      if (job.status === 'running') {
        job.status = 'error';
        job.error = err.code && typeof err.code === 'string' ? err.code : err.message;
      }
    } finally {
      if (job.client) job.client.close();
      job.client = null;
      this.running = null;
      // Keep the list short: old finished jobs drop off.
      const finished = this.jobs.filter((j) => !['queued', 'running'].includes(j.status));
      if (finished.length > KEEP_FINISHED) this.jobs = this.jobs.filter((j) => !finished.slice(0, finished.length - KEEP_FINISHED).includes(j));
      this.changed(true);
      this.emit('finished', job);
    }
    this.next();
  }

  async run(job) {
    job.status = 'running';
    this.changed(true);
    job.client = await this.connect(job.serverId);
    let before = 0;
    let lastTick = { t: Date.now(), bytes: 0 };
    for (let i = 0; i < job.items.length; i++) {
      if (job.status !== 'running') return;
      const item = job.items[i];
      job.fileIndex = i;
      const existing = statSafe(item.dest);
      if (existing && existing.size === item.size) {
        // Already there (an earlier download, or the same file from another source).
        job.skipped++;
        before += item.size;
        job.bytesDone = before;
        this.changed();
        continue;
      }
      fs.mkdirSync(path.dirname(item.dest), { recursive: true });
      const part = `${item.dest}.part`;
      await job.client.download(item.remote, part, (bytes) => {
        job.bytesDone = before + bytes;
        const now = Date.now();
        if (now - lastTick.t >= 1000) {
          job.rate = Math.round((job.bytesDone - lastTick.bytes) / ((now - lastTick.t) / 1000));
          lastTick = { t: now, bytes: job.bytesDone };
        }
        this.changed();
      });
      if (job.status !== 'running') return;
      fs.renameSync(part, item.dest);
      before += item.size;
      job.bytesDone = before;
      this.changed();
    }
  }
}

function statSafe(p) {
  try {
    return fs.statSync(p);
  } catch {
    return null;
  }
}

module.exports = { TransferQueue };
