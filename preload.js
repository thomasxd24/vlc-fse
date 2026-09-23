'use strict';

const { contextBridge, ipcRenderer } = require('electron');

const on = (channel) => (cb) => {
  const handler = (_e, payload) => cb(payload);
  ipcRenderer.on(channel, handler);
  return () => ipcRenderer.removeListener(channel, handler);
};

contextBridge.exposeInMainWorld('cinema', {
  getState: () => ipcRenderer.invoke('get-state'),
  rescan: () => ipcRenderer.invoke('rescan'),
  play: (req) => ipcRenderer.invoke('play', req),
  stop: () => ipcRenderer.invoke('stop'),
  setWatched: (req) => ipcRenderer.invoke('set-watched', req),
  saveSettings: (patch) => ipcRenderer.invoke('save-settings', patch),
  pickFolder: () => ipcRenderer.invoke('pick-folder'),
  pickVlc: () => ipcRenderer.invoke('pick-vlc'),
  detectVlc: () => ipcRenderer.invoke('detect-vlc'),
  clearMetadata: () => ipcRenderer.invoke('clear-metadata'),
  showInFolder: (p) => ipcRenderer.invoke('show-in-folder', p),
  toggleFullscreen: () => ipcRenderer.invoke('toggle-fullscreen'),
  minimize: () => ipcRenderer.invoke('minimize'),
  quit: () => ipcRenderer.invoke('quit'),
  onState: on('state'),
  onNowPlaying: on('now-playing'),
  onToast: on('toast')
});
