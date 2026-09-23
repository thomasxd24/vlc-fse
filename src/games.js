'use strict';

const { spawn } = require('child_process');
const { EventEmitter } = require('events');
const path = require('path');
const crypto = require('crypto');
const { runningAppId } = require('./steam');

const STEAM_START_TIMEOUT_MS = 3 * 60 * 1000; // first launches can sit on "installing prerequisites"
const STEAM_POLL_MS = 3000;
const LAUNCHER_STUB_MS = 15000;

function manualId(exe) {
  return 'game-' + crypto.createHash('sha1').update(exe.toLowerCase() + Date.now()).digest('hex').slice(0, 12);
}

/** Turn "C:\\Games\\Hades II\\Hades2.exe" into a readable default title ("Hades II"). */
function titleFromExe(exe) {
  const base = path.basename(exe, path.extname(exe));
  const generic = /^(bin|bin32|bin64|binaries|win64|win32|windows|x64|x86|game|release|shipping|retail)$/i;
  // Skip build-output folders ("Game\Binaries\Win64\Game.exe") to reach the game's own folder.
  let dir = path.dirname(exe);
  while (generic.test(path.basename(dir)) && path.dirname(dir) !== dir) dir = path.dirname(dir);
  let name = path.basename(dir);
  if (!name || /^[a-z]:$/i.test(name) || /^(games?|program files( \(x86\))?|jeux)$/i.test(name)) name = base;
  return name.replace(/[._]+/g, ' ').replace(/\s{2,}/g, ' ').trim();
}

/** Split a launch-options string, honouring double quotes. */
function splitArgs(s) {
  return (String(s || '').match(/"[^"]*"|\S+/g) || []).map((a) => a.replace(/^"|"$/g, ''));
}

/**
 * One play session. Emits 'running' once the game is actually up, then 'exit' {playedMs, reason}.
 * Steam games are tracked through Steam's RunningAppID registry value, so it works however the game is
 * launched (DRM wrappers, launchers, restarts). Manual games are tracked by their process.
 */
class GameSession extends EventEmitter {
  constructor(game, { openExternal, openPath }) {
    super();
    this.game = game;
    this.openExternal = openExternal;
    this.openPath = openPath;
    this.startedAt = 0;
    this.runningSince = 0;
    this.timer = null;
    this.child = null;
    this.done = false;
  }

  async start() {
    this.startedAt = Date.now();
    if (this.game.source === 'steam') return this.startSteam();
    return this.startManual();
  }

  async startSteam() {
    await this.openExternal(`steam://rungameid/${this.game.appid}`);
    const want = Number(this.game.appid);
    const tick = async () => {
      if (this.done) return;
      const running = await runningAppId();
      if (running === null) {
        // Can't observe Steam (not Windows): assume it started, and let the user say when they're done.
        if (!this.runningSince) this.markRunning();
        return;
      }
      if (running === want) {
        if (!this.runningSince) this.markRunning();
      } else if (this.runningSince) {
        this.finish('exited');
      } else if (Date.now() - this.startedAt > STEAM_START_TIMEOUT_MS) {
        this.finish('timeout');
      }
    };
    this.timer = setInterval(tick, STEAM_POLL_MS);
    tick();
  }

  async startManual() {
    const g = this.game;
    if (!/\.exe$/i.test(g.exe)) {
      // Shortcuts (.lnk/.url) and scripts are opened by the shell; there's no process of ours to watch.
      const err = await this.openPath(g.exe);
      if (err) return this.finish('error', new Error(err));
      this.markRunning();
      return this.finish('stub');
    }
    this.child = spawn(g.exe, splitArgs(g.args), {
      cwd: g.cwd || path.dirname(g.exe),
      detached: true,
      stdio: 'ignore',
      windowsHide: false
    });
    this.child.on('error', (err) => this.finish('error', err));
    this.child.on('exit', () => {
      // Some games are stubs that start the real game and quit at once; don't count that as the session.
      this.finish(Date.now() - this.startedAt < LAUNCHER_STUB_MS ? 'stub' : 'exited');
    });
    this.child.unref();
    this.markRunning();
  }

  markRunning() {
    this.runningSince = Date.now();
    this.emit('running');
  }

  /** The user says they're done (e.g. we couldn't track the game). */
  stopTracking() {
    this.finish('user');
  }

  finish(reason, error) {
    if (this.done) return;
    this.done = true;
    clearInterval(this.timer);
    const playedMs = this.runningSince && reason !== 'stub' ? Date.now() - this.runningSince : 0;
    this.emit('exit', { reason, playedMs, error });
  }
}

module.exports = { GameSession, titleFromExe, splitArgs, manualId };
