'use strict';

const { contextBridge, ipcRenderer } = require('electron');

const on = (channel) => (cb) => {
  const handler = (_e, payload) => cb(payload);
  ipcRenderer.on(channel, handler);
  return () => ipcRenderer.removeListener(channel, handler);
};
const call = (channel) => (arg) => ipcRenderer.invoke(channel, arg);

contextBridge.exposeInMainWorld('lounge', {
  getState: call('get-state'),
  saveUiState: call('save-ui-state'),
  rescan: call('rescan'),
  // Media
  play: call('play'),
  stop: call('stop'),
  npCommand: call('np-command'),
  setLanguages: call('set-languages'),
  setWatched: call('set-watched'),
  setPref: call('set-pref'),
  showInFolder: call('show-in-folder'),
  getStats: call('get-stats'),
  // Games
  playGame: call('play-game'),
  endGame: call('end-game'),
  backToGame: call('back-to-game'),
  addGame: call('add-game'),
  editGame: call('edit-game'),
  removeGame: call('remove-game'),
  searchSteam: call('search-steam'),
  searchSgdb: call('search-sgdb'),
  sgdbImages: call('sgdb-images'),
  setGameArt: call('set-game-art'),
  screenshot: call('screenshot'),
  showGameFolder: call('show-game-folder'),
  // Settings
  saveSettings: call('save-settings'),
  pickFolder: call('pick-folder'),
  pickVlc: call('pick-vlc'),
  detectVlc: call('detect-vlc'),
  clearMetadata: call('clear-metadata'),
  // Servers & transfers
  saveServer: call('server-save'),
  removeServer: call('server-remove'),
  forgetHostKey: call('server-forget-key'),
  testServer: call('server-test'),
  pickKeyFile: call('pick-key-file'),
  remoteList: call('remote-list'),
  remotePlan: call('remote-plan'),
  remoteDownload: call('remote-download'),
  cancelTransfer: call('transfer-cancel'),
  clearTransfers: call('transfer-clear'),
  retryTransfer: call('transfer-retry'),
  // Apps & Tailscale
  rescanApps: call('apps-rescan'),
  launchApp: call('app-launch'),
  hideApp: call('app-hide'),
  tailscaleStatus: call('tailscale-status'),
  tailscaleAction: call('tailscale-action'),
  tailscaleLogin: call('tailscale-login'),
  tailscaleCancelLogin: call('tailscale-cancel-login'),
  tailscaleOpenApp: call('tailscale-open-app'),
  // System
  systemGet: call('system-get'),
  systemSet: call('system-set'),
  wifi: call('wifi'),
  power: call('power'),
  openExternal: call('open-external'),
  checkUpdate: call('update-check'),
  installUpdate: call('update-install'),
  skipUpdate: call('update-skip'),
  toggleFullscreen: call('toggle-fullscreen'),
  minimize: call('minimize'),
  quit: call('quit'),
  // Events
  onState: on('state'),
  onNowPlaying: on('now-playing'),
  onGame: on('game'),
  onToast: on('toast'),
  onUpdate: on('update'),
  onTransfers: on('transfers'),
  onTailscale: on('tailscale')
});
