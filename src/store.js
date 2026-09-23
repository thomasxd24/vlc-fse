'use strict';

const fs = require('fs');
const path = require('path');

/** Tiny JSON-file store with debounced, atomic writes. */
class JsonStore {
  constructor(file, defaults) {
    this.file = file;
    this.data = structuredClone(defaults);
    this.timer = null;
    try {
      const raw = JSON.parse(fs.readFileSync(file, 'utf8'));
      if (raw && typeof raw === 'object' && !Array.isArray(raw)) this.data = { ...this.data, ...raw };
    } catch {
      // Missing or corrupt: start from defaults.
    }
  }

  get(key) {
    return this.data[key];
  }

  set(key, value) {
    this.data[key] = value;
    this.save();
  }

  save() {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.flush(), 400);
  }

  flush() {
    clearTimeout(this.timer);
    this.timer = null;
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    const tmp = `${this.file}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(this.data));
    fs.renameSync(tmp, this.file);
  }
}

module.exports = { JsonStore };
