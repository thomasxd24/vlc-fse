'use strict';

const { contextBridge, ipcRenderer } = require('electron');

const on = (channel) => (cb) => {
  const handler = (_e, payload) => cb(payload);
  ipcRenderer.on(channel, handler);
  return () => ipcRenderer.removeListener(channel, handler);
};
const call = (channel) => (arg) => ipcRenderer.invoke(channel, arg);

contextBridge.exposeInMainWorld('foyer', {
  getState: call('get-state'),
  saveUiState: call('save-ui-state'),
  rescan: call('rescan'),
  // Media
  play: call('play'),
  stop: call('stop'),
  setWatched: call('set-watched'),
  setPref: call('set-pref'),
  showInFolder: call('show-in-folder'),
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
  // System
  systemGet: call('system-get'),
  systemSet: call('system-set'),
  wifi: call('wifi'),
  power: call('power'),
  openExternal: call('open-external'),
  toggleFullscreen: call('toggle-fullscreen'),
  minimize: call('minimize'),
  quit: call('quit'),
  // Events
  onState: on('state'),
  onNowPlaying: on('now-playing'),
  onGame: on('game'),
  onToast: on('toast')
});
